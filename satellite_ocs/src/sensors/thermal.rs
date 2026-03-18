use shared_protocol::{EmergencyData, Severity, SensorReading, ThermalSensor};
use tokio::time::{self, Duration, Instant};
use tracing::{info, warn};
use chrono::Utc;

use crate::faults::{self, FaultEvent};

pub fn spawn() {
    let sensor = ThermalSensor::new(1, "CPU");

    // BACK TO TOKIO: Fully concurrent async task
    tokio::spawn(async move {
        let mut seq = 0u64;
        let period = Duration::from_millis(sensor.sampling_interval_ms);
        
        let mut next_target = Instant::now() + period;
        let mut last_start = Instant::now();

        let mut faults_rx = faults::subscribe();
        let mut cur_fault_id: Option<String> = None;
        let mut extra_delay_ms: u64 = 0;
        let mut fault_until: Option<Instant> = None;
        let mut consecutive_misses: u32 = 0;

        loop {
            // Drain faults
            if let Some(rx) = faults_rx.as_mut() {
                loop {
                    match rx.try_recv() {
                        Ok(FaultEvent::ThermalDelay { fault_id, extra_ms, for_ms }) => {
                            cur_fault_id = Some(fault_id);
                            extra_delay_ms = extra_ms;
                            fault_until = Some(Instant::now() + Duration::from_millis(for_ms));
                            warn!(extra_ms, for_ms, "thermal: injected delay fault");
                        }
                        Ok(FaultEvent::Recover { fault_id }) => {
                            if cur_fault_id.as_deref() == Some(fault_id.as_str()) {
                                extra_delay_ms = 0;
                                fault_until = None;
                                faults::ack_recovered(&fault_id, "thermal").await;
                                info!("thermal: recovered");
                                cur_fault_id = None;
                            }
                        }
                        Ok(FaultEvent::Abort { reason }) => {
                            warn!(%reason, "thermal: mission abort received");
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(_) => break,
                    }
                }
            }

            // CONCURRENT PRECISION TIMING
            let now = Instant::now();
            if next_target > now {
                let wait_time = next_target - now;
                // Use Tokio's async sleep for the bulk of the wait
                if wait_time > Duration::from_millis(16) {
                    tokio::time::sleep(wait_time - Duration::from_millis(16)).await;
                }
                // Async Yield Loop for the final milliseconds
                // Checks the clock, but constantly yields to let Power/Attitude run
                while Instant::now() < next_target {
                    tokio::task::yield_now().await;
                }
            }

            let start = Instant::now();
            next_target = start + period;

            if let Some(until) = fault_until {
                if Instant::now() < until && extra_delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(extra_delay_ms)).await;
                }
            }

            let temp_c = 60.0 + ((seq % 40) as f64 * 0.2);
            let mut r: SensorReading = sensor.create_reading(temp_c, seq);

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
                event="sensor_sample",
                kind="thermal",
                seq=seq,
                actual_ms=format_args!("{:.3}", actual_ms),
                ideal_ms=format_args!("{:.3}", ideal_ms),
                jitter_ms=format_args!("{:.3}", r.jitter_ms),
                drift_ms=format_args!("{:.3}", r.drift_ms),
                temp_c=format_args!("{:.1}", temp_c),
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

            let current_jitter = r.jitter_ms;
            
            // ASYNC SEND: We are safely back in Tokio, so we can .await
            let send_res = tx.send(r).await;
            
            if send_res.is_err() || current_jitter > 1.0 {
                consecutive_misses += 1;
            } else {
                consecutive_misses = 0;
            }

            if consecutive_misses > 3 {
                warn!("SAFETY ALERT: thermal sensor missed >3 consecutive cycles");
                consecutive_misses = 0;

                if let Some(em_tx) = crate::telemetry::EMER_TX.get() {
                    let em = EmergencyData {
                        alert_id: format!("thermal-miss-{}", Utc::now().timestamp_millis()),
                        severity: Severity::High,
                        alert_type: "thermal".into(),
                        description: "Thermal sensor missed >3 consecutive cycles".into(),
                        affected_systems: vec!["thermal_management".into()],
                        recommended_actions: vec![
                            "increase_cooling".into(),
                            "enter_safe_mode_if_persistent".into(),
                        ],
                        auto_recovery_attempted: false,
                        timestamp: Utc::now(),
                    };
                    let _ = em_tx.try_send(em);
                }
            }

            last_start = start;
            seq = seq.wrapping_add(1);
        }
    });
}