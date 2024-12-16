use std::sync::Arc;

use clap::Parser;
use socks5_proto::Address;
use tokio::io::Result;
use tokio::net::{self, UdpSocket};

use rand::Rng;

#[derive(Parser)]
pub struct UdpConfig {
    /// The MTU of the UDP sockets.
    #[clap(long, value_parser, default_value = "9000")]
    pub mtu: usize,
    /// The minimum possible port of all UDP sockets.
    #[clap(long, value_parser, default_value = "40000")]
    pub min_port: u16,
    /// The maximum possible port of all UDP sockets.
    #[clap(long, value_parser, default_value = "65535")]
    pub max_port: u16,
}

pub async fn create_udp_socket(
    local: &str,
    min_port: u16,
    max_port: u16,
) -> Result<(Arc<UdpSocket>, Address)> {
    let mut addr = net::lookup_host(local).await?.next().unwrap();
    loop {
        addr.set_port(rand::thread_rng().gen_range(min_port..=max_port));
        if let Ok(sock) = UdpSocket::bind(addr).await {
            return Ok((Arc::new(sock), Address::SocketAddress(addr)));
        }
    }
}
