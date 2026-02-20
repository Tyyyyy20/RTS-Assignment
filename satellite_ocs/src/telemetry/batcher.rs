use crate::{config::Config, crypto::Crypto, logging};
use chrono::Utc;
use once_cell::sync::OnceCell;
use shared_protocol::{
    CommunicationPacket, EmergencyData, EncryptedFrame, Priority, SensorReading, Source,
};
use std::sync::Arc;
use tokio::{
    net::UdpSocket,
    sync::mpsc,
    time::{self, Duration},
};
use tracing::info;

use super::prio_buffer::{BufferHandle, InsertResult};

/// Sensors send readings here; an ingest task moves them into the priority buffer.
pub static CHANNEL: OnceCell<mpsc::Sender<SensorReading>> = OnceCell::new();

/// Emergency alerts (e.g., from thermal) go here; batcher sends immediately.
pub static EMER_TX: OnceCell<mpsc::Sender<EmergencyData>> = OnceCell::new();

/// The priority bounded buffer
pub static BUFFER: OnceCell<BufferHandle> = OnceCell::new();

/// Initialize the priority buffer (call once from main before spawning sensors)
pub fn init_priority_buffer(capacity: usize) {
    let _ = BUFFER.set(BufferHandle::new(capacity));
}

pub async fn spawn_batcher(cfg: Config, crypto: Crypto, tx_sock: Arc<UdpSocket>, framer: crate::net::framing::Framer) {
    // 1) sensor ingress channel
    let (tx, mut rx) = mpsc::channel::<SensorReading>(1024);
    let _ = CHANNEL.set(tx);

    // 1b) emergency channel
    let (em_tx, mut em_rx) = mpsc::channel::<EmergencyData>(32);
    let _ = EMER_TX.set(em_tx);

    // 2) bounded priority buffer
    if BUFFER.get().is_none() {
        init_priority_buffer(cfg.max_batch * 8);
    }
    let buf = BUFFER.get().unwrap().clone();

    // 3) Ingest: sensors → bounded buffer (with drop logging)
    tokio::spawn({
        let buf = buf.clone();
        async move {
            while let Some(mut r) = rx.recv().await {
                // compute read→ingest latency
                let now = chrono::Utc::now();
                let dt_ms = (now - r.timestamp)
                    .num_microseconds()
                    .map(|us| us as f64 / 1000.0)
                    .unwrap_or(0.0);
                r.processing_latency_ms = dt_ms;

                // Insert into bounded buffer; if dropped, log it
                match buf.push(r).await {
                    InsertResult::Accepted => {}
                    InsertResult::Dropped {
                        dropped_priority, ..
                    } => {
                        let prio = format!("{:?}", dropped_priority).to_lowercase();
                        logging::csv::log_drop(&prio, 1).await;
                    }
                }
            }
        }
    });

    // 3b) Emergency sender: send EmergencyData immediately
    {
        let crypto = crypto.clone();
        let tx_sock = tx_sock.clone();
        tokio::spawn(async move {
            while let Some(em) = em_rx.recv().await {
                let pkt = CommunicationPacket::new_emergency(em, Source::Satellite);
                if let Ok(bytes) = crypto.seal(&pkt) {
                    // peek header for pretty logs
                    log_frame_header(&bytes);
                    let _ = tx_sock.send(&bytes).await;
                }
            }
        });
    }

    // 4) Batcher: every batch_ms, pop by priority and send
    {
        let crypto = crypto.clone();
        let tx_sock = tx_sock.clone();
        let buf_for_send = buf.clone();
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(cfg.max_batch);
            let mut ticker = time::interval(Duration::from_millis(cfg.batch_ms));

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if !batch.is_empty() {
                            send(&cfg, &crypto, &tx_sock, &buf_for_send, &mut batch, &framer).await;
                        } else {
                            let pull = buf_for_send.pop_many(cfg.max_batch).await;
                            if !pull.is_empty() {
                                batch.extend(pull);
                                send(&cfg, &crypto, &tx_sock, &buf_for_send, &mut batch, &framer).await;
                            }
                        }
                    }
                    else => {
                        let pull = buf_for_send.pop_many(cfg.max_batch).await;
                        if !pull.is_empty() {
                            batch.extend(pull);
                            if batch.len() >= cfg.max_batch {
                                send(&cfg, &crypto, &tx_sock, &buf_for_send, &mut batch, &framer).await;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }
            }
        });
    }
}

async fn send(
    cfg: &Config,
    crypto: &Crypto,
    sock: &Arc<UdpSocket>,
    buf: &BufferHandle,
    batch: &mut Vec<SensorReading>,
    framer: &crate::net::framing::Framer,
) {
    // Compute queue latency (oldest sample age)
    let now = chrono::Utc::now();
    let oldest_ms = batch
        .iter()
        .map(|r| (now - r.timestamp).num_microseconds().unwrap_or(0) as f64 / 1000.0)
        .fold(0.0_f64, f64::max);

    // Buffer fill percent (for degraded mode)
    let fill_pct = buf.fill_pct().await;

    // Downlink gate: must be within window + init ≤ 5ms + prep ≤ 30ms
    let gate = if let Some(dl) = crate::downlink::DL.get() {
        dl.pre_send().await
    } else {
        crate::downlink::DownlinkEvent::Ready
    };

    match gate {
        crate::downlink::DownlinkEvent::MissedInit => {
            // treat as missed comms; don't send this batch
            logging::csv::log_tx_queue(oldest_ms, fill_pct).await;
            batch.clear();
            return;
        }
        crate::downlink::DownlinkEvent::ReadyPrepLate { prep_ms } => {
            // still send but note the lateness
            tracing::warn!(prep_ms = format_args!("{:.3}", prep_ms), "downlink: prep > 30ms");
        }
        crate::downlink::DownlinkEvent::ReadyDegraded => {
            tracing::warn!("downlink: degraded mode active");
        }
        crate::downlink::DownlinkEvent::NotInWindow => {
            // optional: you can buffer until next tick; here we just skip send
            logging::csv::log_tx_queue(oldest_ms, fill_pct).await;
            return;
        }
        crate::downlink::DownlinkEvent::Ready => {}
    }

    // Build telemetry packet + encrypt
    let pkt = CommunicationPacket::new_telemetry(batch.clone(), Source::Satellite);
    if let Ok(bytes) = crypto.seal(&pkt) {
        // log encrypted frame header
        log_frame_header(&bytes);

        // send
        let _ = sock.send(&bytes).await;

        // priority counts for logs
        let (mut c, mut i, mut n) = (0, 0, 0);
        for r in batch.iter() {
            match r.priority {
                Priority::Emergency | Priority::Critical => c += 1,
                Priority::Important => i += 1,
                Priority::Normal => n += 1,
            }
        }
        logging::csv::log_batch(batch.len(), c, i, n).await;
        logging::csv::log_tx_queue(oldest_ms, fill_pct).await;
        info!(
            "tx telemetry: total={} (critical={}, important={}, normal={}), queue_oldest_ms={:.3}, fill_pct={:.1}",
            batch.len(), c, i, n, oldest_ms, fill_pct
        );

        // Degraded mode trigger
        if fill_pct >= 80.0 {
            if let Some(dl) = crate::downlink::DL.get() {
                dl.set_degraded(true).await;
            }
        } else {
            if let Some(dl) = crate::downlink::DL.get() {
                dl.set_degraded(false).await;
            }
        }
    }

    batch.clear();
}

fn log_frame_header(bytes: &[u8]) {
    if bytes.len() < 4 {
        return;
    }
    let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() < 4 + len {
        return;
    }
    if let Ok(frame) = serde_json::from_slice::<EncryptedFrame>(&bytes[4..4 + len]) {
        info!(
            event = "tx_frame",
            pkt_type = ?frame.header.packet_type,
            seq      = frame.header.sequence_number,
            src      = ?frame.header.source,
            dst      = ?frame.header.destination,
            key_id   = frame.header.key_id,
            nonce    = %hex::encode(frame.header.nonce),
            bytes_total = bytes.len(),
            ct_len      = frame.ciphertext.len(),
        );
    }
}
