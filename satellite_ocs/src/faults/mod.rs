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

// Global broadcast bus used to send fault events to all sensors
static BUS: OnceCell<broadcast::Sender<FaultEvent>> = OnceCell::new();
// Channel used for sensors to send recovery acknowledgements back
static ACK_TX: OnceCell<mpsc::Sender<FaultAck>> = OnceCell::new();

/// Sensors call this to subscribe and listen for fault events.
/// Returns None if the fault injector has not started yet.
pub fn subscribe() -> Option<broadcast::Receiver<FaultEvent>> {
    BUS.get().map(|tx| tx.subscribe())
}

/// Sensors call this after they have successfully recovered from a fault.
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

/// Starts the fault injector task.
///
/// Every 60 seconds a fault is injected into the system.
/// The system then waits for components to recover and acknowledge the recovery.
///
/// If recovery time exceeds 200ms, the system will abort the mission.
pub fn init_and_spawn() {
    // Broadcast channel for publishing fault events
    let (bus_tx, _bus_rx) = broadcast::channel::<FaultEvent>(64);
    // Channel for receiving recovery acknowledgements
    let (ack_tx, mut ack_rx) = mpsc::channel::<FaultAck>(64);

    let _ = BUS.set(bus_tx.clone());
    let _ = ACK_TX.set(ack_tx);

    tokio::spawn(async move {
        // Counter used to rotate fault types
        let mut fault_counter = 0u64;
        // Trigger a fault every 60 seconds
        let mut ticker = time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            fault_counter = fault_counter.wrapping_add(1);
            let fault_id = Uuid::new_v4().to_string();

            // Rotate between three fault types
            // ThermalDelay → PowerCorrupt → AttitudePause
            let (target, fault_type, duration_ms) = match fault_counter % 3 {
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

            // Record the injected fault into CSV logs
            crate::logging::csv::log_fault_inject(
                &fault_id,
                target,
                fault_type,
                duration_ms
            ).await;

            // Allow the fault to remain active for the specified duration
            time::sleep(Duration::from_millis(duration_ms)).await;

            // Notify components that recovery should start
            let _ = bus_tx.send(FaultEvent::Recover {
                fault_id: fault_id.clone(),
            });

            // Start measuring recovery time
            let started = Instant::now();
            // Maximum waiting time for recovery acknowledgement
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
                            ).await;

                            if aborted {
                                let reason =
                                    format!("recovery {:.1}ms > 200ms", rec_ms);

                                warn!(
                                    %reason,
                                    fault_id,
                                    "mission aborted due to slow recovery"
                                );

                                let _ = bus_tx.send(FaultEvent::Abort { reason });
                            } else {
                                info!(
                                    recovery_ms = format_args!("{:.1}", rec_ms),
                                    component = %ack.component,
                                    fault_id = %fault_id,
                                    "fault recovered successfully"
                                );
                            }

                            recovered = true;
                            break;
                        }
                        // ACK for another fault → ignore and keep waiting
                    }
                    Ok(None) => {
                        // ACK channel closed unexpectedly
                        break;
                    }
                    Err(_) => {
                        // Timeout for this receive attempt
                        // Loop will exit if overall deadline is exceeded
                    }
                }
            }

            if !recovered {
                // No matching ACK received within the allowed time window
                crate::logging::csv::log_fault_recovery(
                    &fault_id,
                    "unknown",
                    1000.0,
                    true
                ).await;

                let _ = bus_tx.send(FaultEvent::Abort {
                    reason: "recovery timeout".into(),
                });
            }
        }
    });
}