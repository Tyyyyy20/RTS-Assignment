use shared_protocol::{PowerSensor, SensorReading};
use std::time::{Duration, Instant}; // Using std::time for microsecond precision
use tracing::{info, warn};

// Fault bus used to receive injected fault events
use crate::faults::{self, FaultEvent};

pub fn spawn() {
    // Create the simulated power sensor for the main power system
    let sensor = PowerSensor::new(2, "Power System");

    // 1. Move to a native OS thread to completely bypass Tokio's async overhead
    std::thread::spawn(move || {
        // We still need a local runtime to handle the channels and telemetry queue
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            // Sequence number for each sensor sample
            let mut seq = 0u64;

            // Sampling period of the power sensor
            let period = Duration::from_millis(sensor.sampling_interval_ms);

            // 2. Manual target tracking instead of tokio's ticker
            let mut next_target = Instant::now() + period;
            let mut last_start = Instant::now();

            // Fault state tracking
            let mut faults_rx = faults::subscribe();
            let mut cur_fault_id: Option<String> = None;
            let mut corrupt_until: Option<Instant> = None;

            loop {
                // Drain all pending fault events from the fault bus
                if let Some(rx) = faults_rx.as_mut() {
                    loop {
                        match rx.try_recv() {
                            // Inject a power data corruption fault
                            Ok(FaultEvent::PowerCorrupt { fault_id, for_ms }) => {
                                cur_fault_id = Some(fault_id);
                                corrupt_until = Some(Instant::now() + Duration::from_millis(for_ms));
                                warn!(for_ms, "power: injected corrupt fault");
                            }
                            // Recover the power sensor from a fault
                            Ok(FaultEvent::Recover { fault_id }) => {
                                if cur_fault_id.as_deref() == Some(fault_id.as_str()) {
                                    corrupt_until = None;
                                    faults::ack_recovered(&fault_id, "power").await;
                                    info!("power: recovered");
                                    cur_fault_id = None;
                                }
                            }
                            // Mission abort notification
                            Ok(FaultEvent::Abort { reason }) => {
                                warn!(%reason, "power: mission abort received");
                            }
                            Ok(_) => {}
                            // No more fault events in the queue
                            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                            // Channel closed or other error
                            Err(_) => break,
                        }
                    }
                }

                // 3. PURE SPIN LOOP: Do not let Windows OS suspend this thread!
                // This keeps the CPU awake and checking the clock with microsecond precision.
                let now = Instant::now();
                if next_target > now {
                    let wait_time = next_target - now;
                    // Buffer of 16ms to outsmart the slow Windows OS timer
                    if wait_time > Duration::from_millis(16) {
                        std::thread::sleep(wait_time - Duration::from_millis(16));
                    }
                    // Spin exactly to the microsecond
                    while Instant::now() < next_target {
                        std::hint::spin_loop();
                    }
                }

                // Record the start time of this sampling cycle
                let start = Instant::now();
                next_target = start + period; // strictly advance by period

                // Simulated nominal power values
                // let mut batt_pct = 95.0 - (seq as f64 * 0.05);
                let mut batt_pct = (95.0 - (seq as f64 * 0.05)).max(0.0);
                let mut voltage = 12.3;
                let mut current = 2.1;

                // If a corruption fault is active, inject invalid values
                if let Some(until) = corrupt_until {
                    if Instant::now() < until {
                        batt_pct = -5.0; // invalid value, should be detected as bad data
                        voltage = 0.0;
                        current = -10.0;
                    }
                }

                // Create a power sensor reading
                let mut r: SensorReading = sensor.create_reading(
                    batt_pct,
                    voltage,
                    current,
                    voltage * current,
                    seq,
                );

                // Timing measurements for the sensor sampling loop
                let actual_ms = start.duration_since(last_start).as_secs_f64() * 1000.0;
                let ideal_ms = period.as_secs_f64() * 1000.0;

                if seq == 0 {
                    r.jitter_ms = 0.0;
                    r.drift_ms = 0.0;
                } else {
                    r.jitter_ms = (actual_ms - ideal_ms).abs();
                    r.drift_ms = actual_ms - ideal_ms;
                }

                // Processing latency placeholder (not simulated)
                r.processing_latency_ms = 0.0;

                // Log the sensor sample and timing statistics
                info!(
                    event="sensor_sample",
                    kind="power",
                    seq=seq,
                    voltage=format_args!("{:.2}", voltage),
                    current=format_args!("{:.2}", current),
                    batt_pct=format_args!("{:.2}", batt_pct),
                    jitter_ms=format_args!("{:.3}", r.jitter_ms),
                    drift_ms=format_args!("{:.3}", r.drift_ms),
                    actual_ms=format_args!("{:.3}", actual_ms),
                    ideal_ms=format_args!("{:.3}", ideal_ms),
                );

                // Get the telemetry channel used to send the reading
                let tx = match crate::telemetry::CHANNEL.get() {
                    Some(tx) => tx.clone(),
                    None => {
                        warn!("telemetry channel not ready");
                        seq = seq.wrapping_add(1);
                        last_start = start;
                        continue;
                    }
                };

                // 4. SYNCHRONOUS SEND
                // Change `send().await` to `try_send()` so the thread doesn't pause 
                // to ask Tokio for permission.
                if let Err(e) = tx.try_send(r) {
                    warn!(?e, "power: failed to enqueue reading");
                }

                // Update timing state for the next iteration
                last_start = start;
                seq = seq.wrapping_add(1);
            }
        });
    });
}

