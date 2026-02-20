// health/heartbeat.rs
use crate::{config::Config, crypto::Crypto};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{self, Duration};
use tracing::warn;
use shared_protocol::{SystemHealth, CommunicationPacket, Source};
use chrono::Utc;

pub async fn spawn_heartbeat(_cfg: Config, crypto: Crypto, sock: Arc<UdpSocket>) {
    tokio::spawn(async move {
        let mut tick = time::interval(Duration::from_secs(1)); // tune as needed
        loop {
            tick.tick().await;

            let hb = SystemHealth {
                overall_status: "nominal".into(),
                cpu_usage_percent: 0.0,
                memory_usage_percent: 0.0,
                disk_usage_percent: 0.0,
                uptime_seconds: 0,
                active_tasks: 0,
                failed_tasks: 0,
                timestamp: Utc::now(),
            };

            let pkt = CommunicationPacket::new_heartbeat(hb, Source::Satellite);
            match crypto.seal(&pkt) {
                Ok(bytes) => {
                    if let Err(e) = sock.send(&bytes).await {
                        warn!(?e, "heartbeat send error");
                    }
                }
                Err(e) => warn!(%e, "heartbeat seal error"),
            }
        }
    });
}
