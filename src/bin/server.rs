use lazy_static::lazy_static;
use socks_uot::UdpConfig;
use std::{net::SocketAddr, sync::Arc};

use clap::Parser;
use socks5_proto::Address;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, Error, ErrorKind, Result},
    net::{
        self,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream, UdpSocket,
    },
};

#[derive(Parser)]
#[clap(name = "SOCKS-UoT server")]
#[clap(author = "Chengyuan Ma")]
#[clap(version = "0.1.0")]
#[clap(about = "A thin wrapper that supports UDP proxy over a TCP-only proxy system (server side).")]
struct Config {
    /// The listening address for client connections.
    #[clap(long, value_parser)]
    local: String,
    #[clap(flatten)]
    udp: UdpConfig,
}

async fn handle_incoming(stream: TcpStream, client_addr: SocketAddr) -> Result<()> {
    let (udp, _) =
        socks_uot::create_udp_socket(&CONFIG.local, CONFIG.udp.min_port, CONFIG.udp.max_port)
            .await?;
    let (client_read, client_write) = stream.into_split();

    if let Err(error) = tokio::select! {
        result = uot_client_to_server(udp.clone(), client_read, &client_addr) => result,
        result = uot_server_to_client(udp, client_write, &client_addr) => result,
    } {
        if error.kind() != ErrorKind::UnexpectedEof {
            log::error!("[{client_addr}] error when handling udp connection: {error:?}");
        }
    }
    Ok(())
}

async fn uot_client_to_server(
    udp: Arc<UdpSocket>,
    mut client: OwnedReadHalf,
    client_addr: &SocketAddr,
) -> Result<()> {
    loop {
        let address = Address::read_from(&mut client).await?;
        let mut buf_len = [0; 2];
        client.read_exact(&mut buf_len).await?;
        let len = u16::from_be_bytes(buf_len);
        let mut buf_dgram = vec![0; len as usize];
        client.read_exact(&mut buf_dgram).await?;

        log::debug!("[{client_addr}] UDP packet to {address}, length {len}");
        let address = match address {
            Address::SocketAddress(address) => address,
            Address::DomainAddress(domain, port) => {
                let joined = format!("{domain}:{port}");
                net::lookup_host(joined.clone())
                    .await?
                    .next()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::NotFound,
                            format!("cannot resolve domain name {joined}"),
                        )
                    })?
            }
        };
        udp.send_to(&buf_dgram, address).await?;
    }
}

async fn uot_server_to_client(
    udp: Arc<UdpSocket>,
    mut client: OwnedWriteHalf,
    client_addr: &SocketAddr,
) -> Result<()> {
    let mut buf = vec![0; CONFIG.udp.mtu];
    loop {
        let (len, from) = udp.recv_from(&mut buf).await?;
        let mut message = vec![];
        log::debug!("[{client_addr}] UDP packet from {from:?}, length {len}");
        Address::SocketAddress(from).write_to_buf(&mut message);
        message.extend_from_slice(&(len as u16).to_be_bytes());
        message.extend_from_slice(&buf[..len]);
        client.write_all(&message).await?;
    }
}

async fn listen() -> Result<()> {
    let listener = TcpListener::bind(&CONFIG.local).await?;
    loop {
        let (stream, client_addr) = listener.accept().await?;
        tokio::spawn(async move {
            log::debug!("[{client_addr}] incoming connection accepted");
            let result = handle_incoming(stream, client_addr).await;
            match result {
                Ok(()) => log::debug!("[{client_addr}] done handling, stream closed"),
                Err(err) => log::warn!("[{client_addr}] error handling: {err}"),
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
    listen().await?;
    Ok(())
}
