// src/faults/mod.rs
use once_cell::sync::OnceCell;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{self, Duration, Instant};
use tracing::{info, warn};
use uuid::Uuid;
use chrono::Utc;

#[derive(Debug, Clone)]
pub enum FaultEvent {
    ThermalDelay { fault_id: String, extra_ms: u64, for_ms: u64 },
    PowerCorrupt { fault_id: String, for_ms: u64 },
    AttitudePause { fault_id: String, for_ms: u64 },
    Recover { fault_id: String },
    Abort { reason: String },
}

#[derive(Debug, Clone)]
pub struct FaultAck {
    pub fault_id: String,
    pub component: String,
    pub recovered_ts_ms: i64,
}

// Global bus (publish faults; sensors subscribe)
static BUS: OnceCell<broadcast::Sender<FaultEvent>> = OnceCell::new();
// Acks from sensors back to injector
static ACK_TX: OnceCell<mpsc::Sender<FaultAck>> = OnceCell::new();

/// Sensors call this to listen for faults. Returns None if injector not started yet.
pub fn subscribe() -> Option<broadcast::Receiver<FaultEvent>> {
    BUS.get().map(|tx| tx.subscribe())
}

/// Sensors call this when they have cleared a fault after `Recover`.
pub async fn ack_recovered(fault_id: &str, component: &str) {
    if let Some(tx) = ACK_TX.get() {
        let _ = tx
            .send(FaultAck {
                fault_id: fault_id.to_string(),
                component: component.to_string(),
                recovered_ts_ms: Utc::now().timestamp_millis(),
            })
            .await;
    }
}

/// Start the injector: every 60s, inject one fault, then send Recover and measure recovery time.
/// If recovery > 200ms, broadcast Abort and log mission abort.
pub fn init_and_spawn() {
    let (bus_tx, _bus_rx) = broadcast::channel::<FaultEvent>(64);
    let (ack_tx, mut ack_rx) = mpsc::channel::<FaultAck>(64);
    let _ = BUS.set(bus_tx.clone());
    let _ = ACK_TX.set(ack_tx);

    tokio::spawn(async move {
        let mut which = 0u64;
        let mut ticker = time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            which = which.wrapping_add(1);
            let fault_id = Uuid::new_v4().to_string();

            // Round-robin: ThermalDelay (150ms), PowerCorrupt (200ms), AttitudePause (150ms)
            let (target, kind, duration_ms) = match which % 3 {
                0 => {
                    let _ = bus_tx.send(FaultEvent::ThermalDelay {
                        fault_id: fault_id.clone(),
                        extra_ms: 10,
                        for_ms: 150,
                    });
                    ("thermal", "delay", 150u64)
                }
                1 => {
                    let _ = bus_tx.send(FaultEvent::PowerCorrupt {
                        fault_id: fault_id.clone(),
                        for_ms: 200,
                    });
                    ("power", "corrupt", 200u64)
                }
                _ => {
                    let _ = bus_tx.send(FaultEvent::AttitudePause {
                        fault_id: fault_id.clone(),
                        for_ms: 150,
                    });
                    ("attitude", "pause", 150u64)
                }
            };

            // Log the injection
            crate::logging::csv::log_fault_inject(&fault_id, target, kind, duration_ms).await;

            // Let the fault persist
            time::sleep(Duration::from_millis(duration_ms)).await;

            // Tell components to recover; start measuring recovery time (deadline = 500ms)
            let _ = bus_tx.send(FaultEvent::Recover {
                fault_id: fault_id.clone(),
            });
            let started = Instant::now();
            let deadline = started + Duration::from_millis(500);
            let mut recovered = false;

            while Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match time::timeout(remaining, ack_rx.recv()).await {
                    Ok(Some(ack)) => {
                        if ack.fault_id == fault_id {
                            let rec_ms = started.elapsed().as_secs_f64() * 1000.0;
                            let aborted = rec_ms > 200.0;

                            crate::logging::csv::log_fault_recovery(
                                &fault_id,
                                &ack.component,
                                rec_ms,
                                aborted,
                            )
                            .await;

                            if aborted {
                                let reason =
                                    format!("recovery {:.1}ms > 200ms → mission abort", rec_ms);
                                warn!(%reason, fault_id, "faults: aborting mission");
                                let _ = bus_tx.send(FaultEvent::Abort { reason });
                            } else {
                                info!(
                                    recovery_ms = format_args!("{:.1}", rec_ms),
                                    component = %ack.component,
                                    fault_id = %fault_id,
                                    "faults: recovered"
                                );
                            }

                            recovered = true;
                            break;
                        }
                        // unrelated ACK → keep waiting
                    }
                    Ok(None) => {
                        // ACK channel closed
                        break;
                    }
                    Err(_elapsed) => {
                        // per-await timeout; loop condition will end if past deadline
                    }
                }
            }

            if !recovered {
                // No matching ACK within window → abort
                crate::logging::csv::log_fault_recovery(&fault_id, "unknown", 1000.0, true).await;
                let _ = bus_tx.send(FaultEvent::Abort {
                    reason: "recovery timeout".into(),
                });
            }
        }
    });
}
