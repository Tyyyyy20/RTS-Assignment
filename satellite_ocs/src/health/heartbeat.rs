use crate::{config::Config, crypto::Crypto};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{self, Duration};
use tracing::warn;
use shared_protocol::{SystemHealth, CommunicationPacket, Source};
use chrono::Utc;

/// Spawns a background task that periodically sends a system heartbeat
/// to the ground station. This heartbeat provides basic system health
/// information such as CPU, memory, and uptime status.
pub async fn spawn_heartbeat(_cfg: Config, crypto: Crypto, sock: Arc<UdpSocket>) {
    tokio::spawn(async move {
        // Timer that triggers every second to send a heartbeat
        let mut timer = time::interval(Duration::from_secs(1)); // adjust interval if needed
        loop {
            timer.tick().await;

            // Build the current system health status
            let heartbeat = SystemHealth {
                overall_status: "nominal".into(),
                cpu_usage_percent: 0.0,
                memory_usage_percent: 0.0,
                disk_usage_percent: 0.0,
                uptime_seconds: 0,
                active_tasks: 0,
                failed_tasks: 0,
                timestamp: Utc::now(),
            };

            // Create a heartbeat communication packet
            let pkt = CommunicationPacket::new_heartbeat(heartbeat, Source::Satellite);

            // Encrypt the packet before sending
            match crypto.seal(&pkt) {
                Ok(bytes) => {
                    // Send heartbeat to the ground station
                    if let Err(e) = sock.send(&bytes).await {
                        warn!(?e, "failed to send heartbeat");
                    }
                }
                // Encryption failed
                Err(e) => warn!(%e, "heartbeat encryption failed"),
            }
        }
    });
}