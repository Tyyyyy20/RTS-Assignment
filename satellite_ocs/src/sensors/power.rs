use shared_protocol::{PowerSensor, SensorReading};
use tokio::time::{self, Duration, Instant};
use tracing::{info, warn};

// fault bus
use crate::faults::{self, FaultEvent};

pub fn spawn() {
    let sensor = PowerSensor::new(2, "Main Bus");

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
        let mut corrupt_until: Option<Instant> = None;

        loop {
            // drain fault events
            if let Some(rx) = faults_rx.as_mut() {
                loop {
                    match rx.try_recv() {
                        Ok(FaultEvent::PowerCorrupt { fault_id, for_ms }) => {
                            cur_fault_id = Some(fault_id);
                            corrupt_until = Some(Instant::now() + Duration::from_millis(for_ms));
                            warn!(for_ms, "power: injected corrupt fault");
                        }
                        Ok(FaultEvent::Recover { fault_id }) => {
                            if cur_fault_id.as_deref() == Some(fault_id.as_str()) {
                                corrupt_until = None;
                                faults::ack_recovered(&fault_id, "power").await;
                                info!("power: recovered");
                                cur_fault_id = None;
                            }
                        }
                        Ok(FaultEvent::Abort { reason }) => {
                            warn!(%reason, "power: mission abort received");
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(_) => break,
                    }
                }
            }

            ticker.tick().await;
            let start = Instant::now();

            // simulated nominal values
            let mut batt_pct = 95.0 - (seq as f64 * 0.05);
            let mut voltage = 12.3;
            let mut current = 2.1;

            // if fault active, corrupt values
            if let Some(until) = corrupt_until {
                if Instant::now() < until {
                    batt_pct = -5.0;      // invalid → Quality::Invalid expected
                    voltage = 0.0;
                    current = -10.0;
                }
            }

            let mut r: SensorReading = sensor.create_reading(
                batt_pct,
                voltage,
                current,
                voltage * current,
                seq,
            );

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
                kind = "power",
                seq = seq,
                batt_pct = format_args!("{:.2}", batt_pct),
                voltage = format_args!("{:.2}", voltage),
                current = format_args!("{:.2}", current),
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
                warn!(?e, "power: failed to enqueue reading");
            }

            last_start = start;
            seq = seq.wrapping_add(1);
        }
    });
}
