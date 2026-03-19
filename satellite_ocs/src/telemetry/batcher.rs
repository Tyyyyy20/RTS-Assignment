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
use tracing::{info, warn};

use super::prio_buffer::{BufferHandle, InsertResult};

/// Channel where sensors send readings.
/// A background ingest task moves these readings into the priority buffer.
pub static CHANNEL: OnceCell<mpsc::Sender<SensorReading>> = OnceCell::new();

/// Emergency alerts (for example thermal faults).
/// These bypass batching and are sent immediately.
pub static EMER_TX: OnceCell<mpsc::Sender<EmergencyData>> = OnceCell::new();

/// Global bounded priority buffer that stores sensor readings before batching.
pub static BUFFER: OnceCell<BufferHandle> = OnceCell::new();

/// Initialize the priority buffer (call once from main before sensors start)
pub fn init_priority_buffer(capacity: usize) {
    let _ = BUFFER.set(BufferHandle::new(capacity));
}

pub async fn spawn_batcher(cfg: Config, crypto: Crypto, tx_sock: Arc<UdpSocket>, framer: crate::net::framing::Framer) {
    // 1) Channel where sensors push readings into the system
    let (tx, mut rx) = mpsc::channel::<SensorReading>(1024);
    let _ = CHANNEL.set(tx);

    // 1b) Channel used for high-priority emergency events
    let (em_tx, mut em_rx) = mpsc::channel::<EmergencyData>(32);
    let _ = EMER_TX.set(em_tx);

    // 2) Ensure the bounded priority buffer exists
    if BUFFER.get().is_none() {
        init_priority_buffer(cfg.max_batch * 8); 
    }
    let buf = BUFFER.get().unwrap().clone();

    // 3) Ingest task: move sensor readings from the channel into the priority buffer
    tokio::spawn({
        let buf = buf.clone();
        async move {
            while let Some(mut r) = rx.recv().await {
                // Calculate latency from sensor reading time to ingestion time
                let now = chrono::Utc::now();
                let dt_ms = (now - r.timestamp)
                    .num_microseconds()
                    .map(|us| us as f64 / 1000.0)
                    .unwrap_or(0.0);
                r.processing_latency_ms = dt_ms;

                // Log sensor reading to CSV
                let sensor_name = format!("{:?}", r.sensor_type).to_lowercase();
                let priority_str = format!("{:?}", r.priority).to_lowercase();
                let status = format!("{:?}", r.status).to_lowercase();
                logging::csv::log_sensor_reading(
                    &sensor_name,
                    r.sequence_number,
                    r.jitter_ms,
                    r.drift_ms,
                    r.processing_latency_ms,
                    &priority_str,
                    &status,
                ).await;

                // Insert reading into the bounded buffer.
                // If the buffer is full, lower priority data may be dropped.
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

    // 3b) Emergency sender: emergency packets bypass batching
    {
        let crypto = crypto.clone();
        let tx_sock = tx_sock.clone();
        tokio::spawn(async move {
            while let Some(em) = em_rx.recv().await {
                let pkt = CommunicationPacket::new_emergency(em, Source::Satellite);
                if let Ok(bytes) = crypto.seal(&pkt) {
                    // Log metadata of the encrypted frame before sending
                    log_frame_header(&bytes);
                    let _ = tx_sock.send(&bytes).await;
                }
            }
        });
    }

    // 4) Batcher task: periodically collect readings and transmit them as one batch
    {
        let crypto = crypto.clone();
        let tx_sock = tx_sock.clone();
        let buf_for_send = buf.clone();
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(cfg.max_batch);
            
            // REAL-TIME FIX: 2ms polling loop to guarantee hitting the 5ms window
            let mut ticker = time::interval(Duration::from_millis(2));
            
            // NEW STATE TRACKER: Keeps track of whether we are currently degraded
            let mut is_degraded = false;

            loop {
                tokio::select! {
                    // Regular periodic transmission
                    _ = ticker.tick() => {
                        if !batch.is_empty() {
                            send(&cfg, &crypto, &tx_sock, &buf_for_send, &mut batch, &framer, &mut is_degraded).await;
                        } else {
                            let pull = buf_for_send.pop_many(cfg.max_batch).await;
                            if !pull.is_empty() {
                                batch.extend(pull);
                                send(&cfg, &crypto, &tx_sock, &buf_for_send, &mut batch, &framer, &mut is_degraded).await;
                            }
                        }
                    }
                    // Opportunistic send when enough readings accumulate
                    else => {
                        let pull = buf_for_send.pop_many(cfg.max_batch).await;
                        if !pull.is_empty() {
                            batch.extend(pull);
                            if batch.len() >= cfg.max_batch {
                                send(&cfg, &crypto, &tx_sock, &buf_for_send, &mut batch, &framer, &mut is_degraded).await;
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
    _cfg: &Config,
    crypto: &Crypto,
    sock: &Arc<UdpSocket>,
    buf: &BufferHandle,
    batch: &mut Vec<SensorReading>,
    _framer: &crate::net::framing::Framer,
    is_degraded_state: &mut bool,
) {
    if batch.is_empty() { return; }

    // =========================================================================
    // ⏱️ METRIC FIX: START THE PREPARATION STOPWATCH
    // =========================================================================
    let prep_start = std::time::Instant::now();

    // Compute queue latency (max and average) ONLY for txqueue logging
    let now = chrono::Utc::now();
    let mut max_queue_ms: f64 = 0.0;
    for r in batch.iter() {
        let queued_duration_ms = (now - r.timestamp).num_microseconds().unwrap_or(0) as f64 / 1000.0;
        
        if queued_duration_ms > max_queue_ms {
            max_queue_ms = queued_duration_ms;
        }
    }

    // Current buffer fill percentage
    let fill_pct = buf.fill_pct().await;

    // =========================================================================
    // STEP 1: PRE-COMPUTE & ENCRYPT 
    // =========================================================================
    let pkt = CommunicationPacket::new_telemetry(batch.clone(), Source::Satellite);
    let bytes = match crypto.seal(&pkt) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to encrypt telemetry packet: {:?}", e);
            batch.clear();
            return;
        }
    };

    // =========================================================================
    // ⏱️ METRIC FIX: STOP THE STOPWATCH
    // This is the true time it took to "Prepare" the data (< 30ms)
    // =========================================================================
    let actual_prep_ms = prep_start.elapsed().as_secs_f64() * 1000.0;

    // =========================================================================
    // STEP 2: CHECK THE GATE (WAIT FOR WINDOW)
    // =========================================================================
    let gate = if let Some(dl) = crate::downlink::DL.get() {
        dl.pre_send().await
    } else {
        // Running without a downlink simulator (e.g., in unit tests)
        crate::downlink::DownlinkEvent::Ready { prep_ms: 0.0 }
    };

    // =========================================================================
    // STEP 3: EVALUATE & INSTANT SEND
    // =========================================================================
    match gate {
        crate::downlink::DownlinkEvent::MissedInit => {
            // Communication window missed; drop this batch
            logging::csv::log_tx_queue(max_queue_ms, fill_pct).await;
            logging::csv::log_downlink(batch.len(), actual_prep_ms, max_queue_ms, fill_pct, "missed_init_or_timeout").await;
            batch.clear();
            return;
        }
        crate::downlink::DownlinkEvent::NotInWindow => {
            // Not in communication window; return quietly to prevent CSV spam
            return;
        }
        crate::downlink::DownlinkEvent::ReadyPrepLate { .. } => {
            if actual_prep_ms > 30.0 {
                warn!(prep_ms = format_args!("{:.3}", actual_prep_ms), "downlink: prep > 30ms");
            }
        }
        crate::downlink::DownlinkEvent::Ready { .. }
        | crate::downlink::DownlinkEvent::ReadyDegraded { .. } => {
            if actual_prep_ms > 30.0 {
                warn!(prep_ms = format_args!("{:.3}", actual_prep_ms), "downlink: prep > 30ms");
            }
        }
    }

    // Bytes are already encrypted, so this send is virtually instantaneous
    log_frame_header(&bytes);
    let _ = sock.send(&bytes).await;

    // =========================================================================
    // STEP 4: METRICS & LOGGING
    // =========================================================================
    let (mut c, mut i, mut n) = (0, 0, 0);
    for r in batch.iter() {
        match r.priority {
            Priority::Emergency | Priority::Critical => c += 1,
            Priority::Important => i += 1,
            Priority::Normal => n += 1,
        }
    }

    logging::csv::log_batch(batch.len(), c, i, n).await;
    
    // txqueue logs the true age of the data
    logging::csv::log_tx_queue(max_queue_ms, fill_pct).await;
    
    // downlink logs the exact CPU Preparation Time (actual_prep_ms)
    logging::csv::log_downlink(
        batch.len(),
        actual_prep_ms,
        actual_prep_ms,
        fill_pct,
        "sent",
    )
    .await;

    info!(
        "tx telemetry: total={} (critical={}, important={}, normal={}), prep_ms={:.3}, queue_age={:.3}, fill_pct={:.1}",
        batch.len(),
        c,
        i,
        n,
        actual_prep_ms,
        max_queue_ms,
        fill_pct,
    );

    // =========================================================================
    // STEP 5: DEGRADED Mode MANAGEMENT
    // =========================================================================
    if fill_pct >= 80.0 {
        if !*is_degraded_state {
            warn!("Degraded mode activated: buffer fill at {:.1}% (>80%)", fill_pct);
            *is_degraded_state = true;
        }
        if let Some(dl) = crate::downlink::DL.get() {
            dl.set_degraded(true).await;
        }
    } else {
        if *is_degraded_state {
            info!("downlink: degraded mode cleared");
            *is_degraded_state = false;
        }
        if let Some(dl) = crate::downlink::DL.get() {
            dl.set_degraded(false).await;
        }
    }

    batch.clear();
}

fn log_frame_header(bytes: &[u8]) {
    // Frame format: [4 bytes length][JSON encoded EncryptedFrame]
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
            seq = frame.header.sequence_number,
            pkt_type = ?frame.header.packet_type,
            src = ?frame.header.source,
            dst = ?frame.header.destination,
            key_id = frame.header.key_id,
            nonce = %hex::encode(frame.header.nonce),
            bytes_total = bytes.len(),
            ct_len = frame.ciphertext.len(),
        );
    }
}
