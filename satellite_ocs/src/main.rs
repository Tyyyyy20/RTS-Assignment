// src/main.rs
mod config;
mod crypto;
mod errors;
mod net;
mod sensors;
mod scheduler;
mod telemetry;
mod commands;
mod health;
mod logging;
mod util;
mod downlink;
mod faults;

use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // -------- logging ----------
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("satellite_ocs=info".parse().unwrap())
                .add_directive("shared_protocol=info".parse().unwrap())
                .add_directive("tokio=warn".parse().unwrap()),
        )
        .compact()
        .init();

    // -------- config + crypto ----------
    let cfg = config::Cli::parse_and_build_config()?;
    let crypto = crypto::Crypto::from_config(&cfg)?;
    info!(?cfg, "Satellite OCS starting");

    // -------- sockets + framing ----------
    // Expect net::udp::connect(&cfg) to bind local socket and connect to GCS
    let (tx_sock_raw, rx_sock_raw) = net::udp::connect(&cfg).await?;
    // tokio::net::UdpSocket has no try_clone(); share via Arc
    let tx_sock = Arc::new(tx_sock_raw);
    let rx_sock = Arc::new(rx_sock_raw);

    // length-prefixed frame helper
    let framer = net::framing::Framer::default();

    // -------- telemetry buffer before producers ----------
    telemetry::init_priority_buffer(cfg.max_batch * 8); // e.g., 8 batches deep

    // -------- background services ----------
    // Downlink visibility window simulator (5ms init rule, 30ms prep check)
    downlink::init_and_spawn();

    // Fault injector (every 60s; recovery deadline 200ms)
    faults::init_and_spawn();

    // -------- spawn subsystems ----------
    // 1) Telemetry batcher (installs CHANNEL and EMER_TX)
    telemetry::spawn_batcher(cfg.clone(), crypto.clone(), tx_sock.clone(), framer.clone()).await;

    // 2) Sensors (thermal / power / attitude)
    sensors::spawn_all(cfg.clone()).await;

    // 3) RM scheduler (data compression, health monitor, antenna alignment)
    let _ = tokio::spawn(scheduler::rm::spawn_rm(cfg.clone()));

    // 4) Command receiver/handler (decrypts, ACKs)
    commands::spawn_receiver(
        cfg.clone(),
        crypto.clone(),
        rx_sock.clone(), // Arc<UdpSocket>
        tx_sock.clone(), // Arc<UdpSocket>
        framer,          // moved in
    ).await;

    // 5) Heartbeat sender (SystemHealth)
    health::spawn_heartbeat(cfg.clone(), crypto.clone(), tx_sock.clone()).await;

    info!("OCS running. Press Ctrl+C to stop…");

    // -------- graceful shutdown ----------
    if let Err(e) = tokio::signal::ctrl_c().await {
        warn!(?e, "failed to install Ctrl+C handler");
    }
    info!("shutdown signal received; exiting.");
    Ok(())
}
