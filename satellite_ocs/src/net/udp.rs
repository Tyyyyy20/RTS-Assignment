use crate::config::Config;
use anyhow::Result;
use tokio::net::UdpSocket;

/// Establish UDP sockets for communication with the ground control station.
/// tx_socket: used to send data to the ground station
/// rx_socket: used to receive data from the ground station
pub async fn connect(cfg: &Config) -> Result<(UdpSocket, UdpSocket)> {
    // Create an outgoing socket with an ephemeral local port
    let tx = UdpSocket::bind("0.0.0.0:0").await?;
    // Connect the socket to the ground control station address
    tx.connect(&cfg.gcs_addr).await?;
    // Bind a socket for receiving packets from the ground station
    let rx = UdpSocket::bind(&cfg.bind_addr).await?;
    // Return both sockets
    Ok((tx, rx))
}