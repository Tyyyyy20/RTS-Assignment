use crate::config::Config;
use anyhow::Result;
use tokio::net::UdpSocket;

pub async fn connect(cfg: &Config) -> Result<(UdpSocket, UdpSocket)> {
    let tx = UdpSocket::bind("0.0.0.0:0").await?;
    tx.connect(&cfg.gcs_addr).await?;
    let rx = UdpSocket::bind(&cfg.bind_addr).await?;
    Ok((tx, rx))
}
