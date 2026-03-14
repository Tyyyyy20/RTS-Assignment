use shared_protocol::{PowerSensor,SensorReading};
use tokio::time::{self,Duration,Instant};
use tracing::{info,warn};

// Fault bus used to receive injected fault events
use crate::faults::{self,FaultEvent};

pub fn spawn() {
    // Create the simulated power sensor for the main power system
    let sensor = PowerSensor::new(2,"Power System");

    tokio::spawn(async move {

        // Sequence number for each sensor sample
        let mut seq = 0u64;

        // Sampling period of the power sensor
        let period = Duration::from_millis(sensor.sampling_interval_ms);

        // Tokio interval used to trigger periodic sampling
        let mut ticker = time::interval(period);

        // If ticks are missed, delay instead of skipping them
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        // Prime the timer so the first tick starts correctly
        ticker.tick().await;
        let mut last_start = Instant::now();

        // Fault state tracking
        let mut faults_rx = faults::subscribe();
        let mut cur_fault_id:Option<String> = None;
        let mut corrupt_until:Option<Instant> = None;

        loop {

            // Drain all pending fault events from the fault bus
            if let Some(rx)=faults_rx.as_mut() {
                loop {
                    match rx.try_recv() {

                        // Inject a power data corruption fault
                        Ok(FaultEvent::PowerCorrupt{fault_id,for_ms})=>{
                            cur_fault_id = Some(fault_id);
                            corrupt_until = Some(Instant::now()+Duration::from_millis(for_ms));
                            warn!(for_ms,"power: injected corrupt fault");
                        }

                        // Recover the power sensor from a fault
                        Ok(FaultEvent::Recover{fault_id})=>{
                            if cur_fault_id.as_deref()==Some(fault_id.as_str()) {
                                corrupt_until = None;
                                faults::ack_recovered(&fault_id,"power").await;
                                info!("power: recovered");
                                cur_fault_id = None;
                            }
                        }

                        // Mission abort notification
                        Ok(FaultEvent::Abort{reason})=>{
                            warn!(%reason,"power: mission abort received");
                        }

                        Ok(_)=>{}

                        // No more fault events in the queue
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty)=>break,

                        // Channel closed or other error
                        Err(_)=>break,
                    }
                }
            }

            // Wait for the next sampling tick
            ticker.tick().await;

            // Record the start time of this sampling cycle
            let start = Instant::now();

            // Simulated nominal power values
            let mut batt_pct = 95.0-(seq as f64*0.05);
            let mut voltage = 12.3;
            let mut current = 2.1;

            // If a corruption fault is active, inject invalid values
            if let Some(until)=corrupt_until {
                if Instant::now()<until {
                    batt_pct = -5.0; // invalid value, should be detected as bad data
                    voltage = 0.0;
                    current = -10.0;
                }
            }

            // Create a power sensor reading
            let mut r:SensorReading = sensor.create_reading(
                batt_pct,
                voltage,
                current,
                voltage*current,
                seq,
            );

            // Timing measurements for the sensor sampling loop
            let actual_ms = start.duration_since(last_start).as_secs_f64()*1000.0;
            let ideal_ms = period.as_secs_f64()*1000.0;

            if seq==0 {
                r.jitter_ms = 0.0;
                r.drift_ms = 0.0;
            } else {
                r.jitter_ms = (actual_ms-ideal_ms).abs();
                r.drift_ms = actual_ms-ideal_ms;
            }

            // Processing latency placeholder (not simulated)
            r.processing_latency_ms = 0.0;

            // Log the sensor sample and timing statistics
            info!(
                event="sensor_sample",
                kind="power",
                seq=seq,
                voltage=format_args!("{:.2}",voltage),
                current=format_args!("{:.2}",current),
                batt_pct=format_args!("{:.2}",batt_pct),
                jitter_ms=format_args!("{:.3}",r.jitter_ms),
                drift_ms=format_args!("{:.3}",r.drift_ms),
                actual_ms=format_args!("{:.3}",actual_ms),
                ideal_ms=format_args!("{:.3}",ideal_ms),
            );

            // Get the telemetry channel used to send the reading
            let tx = match crate::telemetry::CHANNEL.get() {
                Some(tx)=>tx.clone(),
                None=>{
                    warn!("telemetry channel not ready");
                    seq = seq.wrapping_add(1);
                    last_start = start;
                    continue;
                }
            };

            // Send the power reading to the telemetry queue
            if let Err(e)=tx.send(r).await {
                warn!(?e,"power: failed to enqueue reading");
            }

            // Update timing state for the next iteration
            last_start = start;
            seq = seq.wrapping_add(1);
        }
    });
}