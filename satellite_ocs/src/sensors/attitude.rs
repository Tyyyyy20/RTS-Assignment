use shared_protocol::{AttitudeSensor,SensorReading};
use tokio::time::{self,Duration,Instant};
use tracing::{info,warn};

// Fault bus used to receive injected fault events
use crate::faults::{self,FaultEvent};

pub fn spawn() {
    // Create the simulated attitude sensor (IMU)
    let sensor = AttitudeSensor::new(3,"attitude_sensor");

    tokio::spawn(async move {
        // Sequence number for each sensor sample
        let mut seq = 0u64;

        // Sensor sampling period
        let period = Duration::from_millis(sensor.sampling_interval_ms);

        // Tokio interval timer used to trigger periodic sampling
        let mut ticker = time::interval(period);

        // If the system misses a tick, delay instead of skipping
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        // Prime the timer so the first tick starts correctly
        ticker.tick().await;
        let mut last_start = Instant::now();

        // Fault state management
        let mut faults_rx = faults::subscribe();
        let mut cur_fault_id:Option<String> = None;
        let mut pause_until:Option<Instant> = None;

        loop {

            // Drain all pending fault events from the fault bus
            if let Some(rx) = faults_rx.as_mut() {
                loop {
                    match rx.try_recv() {

                        // Inject a pause fault for the attitude sensor
                        Ok(FaultEvent::AttitudePause{fault_id,for_ms}) => {
                            cur_fault_id = Some(fault_id);
                            pause_until = Some(Instant::now()+Duration::from_millis(for_ms));
                            warn!(for_ms,"attitude: injected pause fault");
                        }

                        // Recover from a previously injected fault
                        Ok(FaultEvent::Recover{fault_id}) => {
                            if cur_fault_id.as_deref()==Some(fault_id.as_str()) {
                                pause_until = None;
                                faults::ack_recovered(&fault_id,"attitude").await;
                                info!("attitude: recovered");
                                cur_fault_id = None;
                            }
                        }

                        // Mission abort notification
                        Ok(FaultEvent::Abort{reason}) => {
                            warn!(%reason,"attitude: mission abort received");
                        }

                        Ok(_) => {}

                        // No more events in the queue
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty)=>break,

                        // Channel closed or other error
                        Err(_)=>break,
                    }
                }
            }

            // Wait until the next sampling tick
            ticker.tick().await;

            // Record the start time of this sampling cycle
            let start = Instant::now();

            // If the sensor is paused due to a fault, skip this cycle
            if let Some(until)=pause_until {
                if Instant::now()<until {

                    // Still record timing behaviour even if the cycle is skipped
                    let actual_ms = start.duration_since(last_start).as_secs_f64()*1000.0;
                    let ideal_ms = period.as_secs_f64()*1000.0;

                    info!(
                        event="sensor_sample",
                        kind="attitude",
                        seq=seq,
                        paused=true,
                        actual_ms=format_args!("{:.3}",actual_ms),
                        ideal_ms=format_args!("{:.3}",ideal_ms),
                    );

                    last_start = start;
                    seq = seq.wrapping_add(1);
                    continue;
                }
            }

            // Simulated Euler angles (centered around 0 degrees)
            let roll=((seq as f64*0.10)%6.0)-3.0;
            let pitch=((seq as f64*0.07)%6.0)-3.0;
            let yaw=((seq as f64*0.05)%6.0)-3.0;

            // Create a sensor reading packet
            let mut r:SensorReading = sensor.create_reading(roll,pitch,yaw,seq);

            // Timing measurements
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
                kind="attitude",
                seq=seq,
                actual_ms=format_args!("{:.3}",actual_ms),
                ideal_ms=format_args!("{:.3}",ideal_ms),
                jitter_ms=format_args!("{:.3}",r.jitter_ms),
                drift_ms=format_args!("{:.3}",r.drift_ms),
                roll=format_args!("{:.2}",roll),
                pitch=format_args!("{:.2}",pitch),
                yaw=format_args!("{:.2}",yaw),
            );

            // Get telemetry channel to send the reading
            let tx = match crate::telemetry::CHANNEL.get() {
                Some(tx)=>tx.clone(),
                None=>{
                    warn!("telemetry channel not ready");
                    seq = seq.wrapping_add(1);
                    last_start = start;
                    continue;
                }
            };

            // Send sensor reading to telemetry queue
            if let Err(e)=tx.send(r).await {
                warn!(?e,"attitude: failed to enqueue reading");
            }

            // Update timing state for next iteration
            last_start = start;
            seq = seq.wrapping_add(1);
        }
    });
}