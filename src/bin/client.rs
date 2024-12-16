use lazy_static::lazy_static;
use socks_uot::UdpConfig;
use std::{
    io::Cursor,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use clap::Parser;
use socks5_proto::{
    Address, Command, HandshakeMethod, HandshakeRequest, HandshakeResponse, Reply, Request,
    Response, UdpHeader,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, Error, ErrorKind, Result},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream, UdpSocket,
    },
    sync::Mutex,
};

#[derive(Parser)]
#[clap(name = "SOCKS-UoT client")]
#[clap(author = "Chengyuan Ma")]
#[clap(version = "0.1.0")]
#[clap(
    about = "A thin wrapper that supports UDP proxy over a TCP-only proxy system (client side)."
)]
struct Config {
    /// The SOCKS5 inbound address.
    #[clap(long, value_parser)]
    local: String,
    // /// The local IP returned by SOCKS5 UDP reply.
    // #[clap(long, value_parser)]
    // udp_local: String,
    /// The address and port of the UoT server.
    #[clap(long, value_parser)]
    server: String,
    /// The SOCKS5 inbound address of the downstream (TCP-only) proxy.
    #[clap(long, value_parser)]
    remote: String,
    #[clap(flatten)]
    udp: UdpConfig,
}

async fn handle_incoming(mut local: TcpStream, src_addr: SocketAddr) -> Result<()> {
    let hs_req = HandshakeRequest::read_from(&mut local).await?;
    if hs_req.methods.contains(&HandshakeMethod::None) {
        let hs_resp = HandshakeResponse::new(HandshakeMethod::None);
        hs_resp.write_to(&mut local).await?;
    } else {
        let hs_resp = HandshakeResponse::new(HandshakeMethod::Unacceptable);
        hs_resp.write_to(&mut local).await?;
        let _ = local.shutdown().await;
        return Err(Error::new(
            ErrorKind::Unsupported,
            "No available handshake method provided by client",
        ));
    }
    log::debug!("[{src_addr}] handshake completed");

    let req = match Request::read_from(&mut local).await {
        Ok(req) => req,
        Err(err) => {
            let resp = Response::new(Reply::GeneralFailure, Address::unspecified());
            resp.write_to(&mut local).await?;
            let _ = local.shutdown().await;
            return Err(err);
        }
    };

    match req.command {
        Command::Connect => {
            log::info!("[{src_addr}] CONNECT to {:?}", req.address);
            let (mut remote, addr) = connect_remote(req.address).await?;
            let resp = Response::new(Reply::Succeeded, addr);
            resp.write_to(&mut local).await?;
            tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
        }
        Command::Associate => {
            log::info!("[{src_addr}] ASSOCIATE to {:?}", req.address);
            let (remote, _addr) = connect_remote(string_to_address(&CONFIG.server)?).await?;

            let (local_udp, local_addr) = socks_uot::create_udp_socket(
                &CONFIG.local,
                CONFIG.udp.min_port,
                CONFIG.udp.max_port,
            )
            .await?;

            log::info!("[{src_addr}] local udp address {} {:?}", local_udp.local_addr()?, local_addr);
            let resp = Response::new(Reply::Succeeded, local_addr);
            let (remote_read, remote_write) = remote.into_split();
            let saddr = Arc::new(Mutex::new(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))));
            resp.write_to(&mut local).await?;
            if let Err(error) = tokio::select! {
                result = hang_on_control_connection(local) => result,
                result = uot_client_to_server(local_udp.clone(), remote_write, saddr.clone(), &src_addr) => result,
                result = uot_server_to_client(local_udp, remote_read, saddr, &src_addr) => result,
            } {
                if error.kind() != ErrorKind::UnexpectedEof {
                    log::error!("[{src_addr}] error when handling udp connection: {error:?}");
                }
                log::info!("[{src_addr}] UDP association stopped");
            }
        }
        Command::Bind => {
            let resp = Response::new(Reply::CommandNotSupported, Address::unspecified());
            resp.write_to(&mut local).await?;
        }
    }

    Ok(())
}

fn string_to_address(name: &str) -> Result<Address> {
    if let Ok(ip) = name.parse::<SocketAddr>() {
        Ok(Address::SocketAddress(ip))
    } else {
        let (domain, port) = name.rsplit_once(':').ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("cannot parse port from {name}"),
            )
        })?;
        Ok(Address::DomainAddress(
            String::from(domain),
            port.parse()
                .map_err(|err| Error::new(ErrorKind::InvalidInput, err))?,
        ))
    }
}

async fn connect_remote(address: Address) -> Result<(TcpStream, Address)> {
    let mut stream = TcpStream::connect(&CONFIG.remote).await?;
    let hs_req = HandshakeRequest::new(vec![HandshakeMethod::None]);
    hs_req.write_to(&mut stream).await?;
    let hs_res = HandshakeResponse::read_from(&mut stream).await?;
    if hs_res.method != HandshakeMethod::None {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "remote server does not support default authentication",
        ));
    }
    let req = Request::new(Command::Connect, address);
    req.write_to(&mut stream).await?;
    let resp = Response::read_from(&mut stream).await?;
    if resp.reply != Reply::Succeeded {
        return Err(Error::new(
            ErrorKind::ConnectionRefused,
            format!("CONNECT request to remote server failed: {:?}", resp.reply),
        ));
    }
    Ok((stream, resp.address))
}

async fn uot_client_to_server(
    socket: Arc<UdpSocket>,
    mut server: OwnedWriteHalf,
    src_udp_addr: Arc<Mutex<SocketAddr>>,
    src_addr: &SocketAddr,
) -> Result<()> {
    let mut buf = vec![0u8; CONFIG.udp.mtu + 262];
    loop {
        let (len, from_addr) = socket.recv_from(&mut buf).await?;
        {
            let mut src_addr = src_udp_addr.lock().await;
            *src_addr = from_addr;
        }
        let mut cursor = Cursor::new(&buf[..len]);
        let header = UdpHeader::read_from(&mut cursor).await?;
        let dgram = &buf[cursor.position() as usize..len];
        log::debug!(
            "[{src_addr}] UDP packet to {}, length {}, frag {}",
            header.address,
            dgram.len(),
            header.frag,
        );
        if header.frag != 0 {
            continue;
        }
        let mut data: Vec<u8> = vec![];
        header.address.write_to_buf(&mut data);
        data.extend_from_slice(&(dgram.len() as u16).to_be_bytes());
        data.extend_from_slice(dgram);
        server.write_all(&data).await?;
    }
}

async fn uot_server_to_client(
    socket: Arc<UdpSocket>,
    mut server: OwnedReadHalf,
    src_udp_addr: Arc<Mutex<SocketAddr>>,
    src_addr: &SocketAddr,
) -> Result<()> {
    loop {
        let address = Address::read_from(&mut server).await?;
        let mut buf_len = [0; 2];
        server.read_exact(&mut buf_len).await?;
        let len = u16::from_be_bytes(buf_len);
        let mut buf_dgram = vec![0; len as usize];
        server.read_exact(&mut buf_dgram).await?;
        log::debug!("[{src_addr}] UDP packet from {}, length {}", address, len);
        let header = UdpHeader::new(0, address);
        let mut final_dgram = vec![];
        header.write_to_buf(&mut final_dgram);
        final_dgram.extend_from_slice(&buf_dgram);
        {
            let src_addr = src_udp_addr.lock().await;
            socket.send_to(&final_dgram, *src_addr).await?;
        }
    }
}

async fn hang_on_control_connection(mut stream: TcpStream) -> Result<()> {
    let mut buf = [0; 1024];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(_) => continue,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

async fn listen() -> Result<()> {
    let listener = TcpListener::bind(&CONFIG.local).await?;
    loop {
        let (stream, src_addr) = listener.accept().await?;
        tokio::spawn(async move {
            log::debug!("[{src_addr}] incoming connection accepted");
            let result = handle_incoming(stream, src_addr).await;
            match result {
                Ok(()) => log::debug!("[{src_addr}] done handling, stream closed"),
                Err(err) => log::warn!("[{src_addr}] error handling: {err}"),
            }
        });
    }
}

lazy_static! {
    static ref CONFIG: Config = Config::parse();
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    listen().await
}
