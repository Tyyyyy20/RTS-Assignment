use shared_protocol::{AttitudeSensor, SensorReading};
use tokio::time::{self, Duration, Instant};
use tracing::{info, warn};

// fault bus
use crate::faults::{self, FaultEvent};

pub fn spawn() {
    let sensor = AttitudeSensor::new(3, "IMU");

    tokio::spawn(async move {
        let mut seq = 0u64;
        let period = Duration::from_millis(sensor.sampling_interval_ms);
        let mut ticker = time::interval(period);
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        // prime
        ticker.tick().await;
        let mut last_start = Instant::now();

        // fault state
        let mut faults_rx = faults::subscribe();
        let mut cur_fault_id: Option<String> = None;
        let mut pause_until: Option<Instant> = None;

        loop {
            // drain fault events
            if let Some(rx) = faults_rx.as_mut() {
                loop {
                    match rx.try_recv() {
                        Ok(FaultEvent::AttitudePause { fault_id, for_ms }) => {
                            cur_fault_id = Some(fault_id);
                            pause_until = Some(Instant::now() + Duration::from_millis(for_ms));
                            warn!(for_ms, "attitude: injected pause fault");
                        }
                        Ok(FaultEvent::Recover { fault_id }) => {
                            if cur_fault_id.as_deref() == Some(fault_id.as_str()) {
                                pause_until = None;
                                faults::ack_recovered(&fault_id, "attitude").await;
                                info!("attitude: recovered");
                                cur_fault_id = None;
                            }
                        }
                        Ok(FaultEvent::Abort { reason }) => {
                            warn!(%reason, "attitude: mission abort received");
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(_) => break,
                    }
                }
            }

            ticker.tick().await;
            let start = Instant::now();

            // if paused by a fault, skip producing/sending this cycle
            if let Some(until) = pause_until {
                if Instant::now() < until {
                    // still account for timing drift/jitter even when skipped
                    let actual_ms = start.duration_since(last_start).as_secs_f64() * 1000.0;
                    let ideal_ms = period.as_secs_f64() * 1000.0;
                    info!(
                        event = "sensor_sample",
                        kind = "attitude",
                        seq = seq,
                        paused = true,
                        actual_ms = format_args!("{:.3}", actual_ms),
                        ideal_ms = format_args!("{:.3}", ideal_ms),
                    );
                    last_start = start;
                    seq = seq.wrapping_add(1);
                    continue;
                }
            }

            // simulated euler angles (center around 0)
            let roll = ((seq as f64 * 0.10) % 6.0) - 3.0;
            let pitch = ((seq as f64 * 0.07) % 6.0) - 3.0;
            let yaw = ((seq as f64 * 0.05) % 6.0) - 3.0;

            let mut r: SensorReading = sensor.create_reading(roll, pitch, yaw, seq);

            // timing
            let actual_ms = start.duration_since(last_start).as_secs_f64() * 1000.0;
            let ideal_ms = period.as_secs_f64() * 1000.0;
            if seq == 0 {
                r.jitter_ms = 0.0;
                r.drift_ms = 0.0;
            } else {
                r.jitter_ms = (actual_ms - ideal_ms).abs();
                r.drift_ms = actual_ms - ideal_ms;
            }
            r.processing_latency_ms = 0.0;

            info!(
                event = "sensor_sample",
                kind = "attitude",
                seq = seq,
                roll = format_args!("{:.2}", roll),
                pitch = format_args!("{:.2}", pitch),
                yaw = format_args!("{:.2}", yaw),
                actual_ms = format_args!("{:.3}", actual_ms),
                ideal_ms = format_args!("{:.3}", ideal_ms),
                jitter_ms = format_args!("{:.3}", r.jitter_ms),
                drift_ms = format_args!("{:.3}", r.drift_ms),
            );

            let tx = match crate::telemetry::CHANNEL.get() {
                Some(tx) => tx.clone(),
                None => {
                    warn!("telemetry channel not ready");
                    seq = seq.wrapping_add(1);
                    last_start = start;
                    continue;
                }
            };
            if let Err(e) = tx.send(r).await {
                warn!(?e, "attitude: failed to enqueue reading");
            }

            last_start = start;
            seq = seq.wrapping_add(1);
        }
    });
}
