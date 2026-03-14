// src/main.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, interval};
use tracing::{info, warn, error, debug};
use tracing_subscriber::EnvFilter;
use chrono::{DateTime, Utc};
use anyhow::Result;

mod network_manager;
mod telemetry_processor;
mod fault_management;
mod performance_tracker;
mod command_scheduler;
mod system_monitor;

use network_manager::{NetworkManager, DriftSeverity};
use telemetry_processor::TelemetryProcessor;
use fault_management::{FaultManager, FaultSimulator};
use performance_tracker::{PerformanceTracker, PerformanceEvent, EventType};
use command_scheduler::{CommandScheduler, EnhancedCommandSchedulerStats, UnifiedDeadlineReport};

/// Handy: stringify any Debug-able enum for logs/metadata
fn format_debug_enum<T: std::fmt::Debug>(value: &T) -> String { format!("{:?}", value) }

/// Main Ground Control System state
pub struct GroundControlSystem {
    network_manager: Arc<NetworkManager>,
    telemetry_processor: Arc<Mutex<TelemetryProcessor>>,
    fault_manager: Arc<Mutex<FaultManager>>,
    performance_tracker: Arc<Mutex<PerformanceTracker>>,
    system_start_time: DateTime<Utc>,
    is_running: Arc<Mutex<bool>>,
    command_scheduler: Arc<Mutex<CommandScheduler>>,
}

#[allow(dead_code)]
fn parse_fault_severity_label(severity_label: &str) -> fault_management::Severity {
    match severity_label.to_ascii_lowercase().as_str() {
        "critical" | "emergency" => fault_management::Severity::Critical,
        "high" => fault_management::Severity::High,
        "medium" => fault_management::Severity::Medium,
        "low" => fault_management::Severity::Low,
        _ => fault_management::Severity::Low,
    }
}

impl GroundControlSystem {
    pub async fn new() -> Result<Self> {
        info!("Bootstrapping Ground Control System...");

        let system_start_time = Utc::now();

        // Components
        let network_manager = Arc::new(NetworkManager::new_default().await?);
        let telemetry_processor = Arc::new(Mutex::new(TelemetryProcessor::new()));
        let fault_manager = Arc::new(Mutex::new(FaultManager::new()));
        let performance_tracker = Arc::new(Mutex::new(PerformanceTracker::new()));
        let command_scheduler = Arc::new(Mutex::new(CommandScheduler::new()));

        Ok(Self {
            network_manager,
            telemetry_processor,
            fault_manager,
            performance_tracker,
            command_scheduler,
            system_start_time,
            is_running: Arc::new(Mutex::new(false)),
        })
    }

    pub fn get_performance_tracker_handle(&self) -> Arc<Mutex<PerformanceTracker>> {
        Arc::clone(&self.performance_tracker)
    }

    fn launch_fault_management_task(
        mut fault_rx: mpsc::Receiver<fault_management::FaultEvent>,
        fault_manager: Arc<Mutex<FaultManager>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Fault Management Task Online");
            while let Some(fault_event) = fault_rx.recv().await {
                let mut manager = fault_manager.lock().await;
                if let Err(err) = manager.handle_fault(fault_event).await {
                    error!("Fault Processing Failure: {err}");
                }
            }
        })
    }

    fn launch_performance_monitor_task(
        mut performance_rx: mpsc::Receiver<PerformanceEvent>,
        performance_tracker: Arc<Mutex<PerformanceTracker>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Performance Monitor Task Online");
            while let Some(event) = performance_rx.recv().await {
                let mut tracker = performance_tracker.lock().await;
                tracker.record_performance_event(event);
            }
        })
    }

    fn launch_fault_simulation_task(
        telemetry_tx: mpsc::Sender<(shared_protocol::CommunicationPacket, DateTime<Utc>)>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut fault_simulator = FaultSimulator::new();
            let mut interval = tokio::time::interval(Duration::from_secs(45));
            loop {
                interval.tick().await;
                let emergency = fault_simulator.create_random_fault();
                let packet = fault_simulator.create_fault_packet(emergency);
                let now = Utc::now();
                if let Err(err) = telemetry_tx.send((packet, now)).await {
                    error!("Simulated Fault Packet Injection Failed: {err}");
                } else {
                    info!("SIM: EmergencyAlert Routed Into Telemetry Pipeline");
                }
            }
        })
    }

    /// Main execution loop
    pub async fn run(&self) -> Result<()> {
        info!("Ground Control Runtime Started At {}", self.system_start_time);
        *self.is_running.lock().await = true;

        // Channels
        const TELEMETRY_Q_CAP: usize = 100;
        let (telemetry_tx, mut telemetry_rx) = mpsc::channel(TELEMETRY_Q_CAP);
        let (fault_tx, fault_rx) = mpsc::channel(50);
        let (performance_tx, performance_rx) = mpsc::channel(200);

        // Clones
        let network_manager = Arc::clone(&self.network_manager);
        let telemetry_processor = Arc::clone(&self.telemetry_processor);
        let fault_manager = Arc::clone(&self.fault_manager);
        let performance_tracker = Arc::clone(&self.performance_tracker);
        let is_running = Arc::clone(&self.is_running);
        let telemetry_backlog = Arc::new(AtomicUsize::new(0));
        let pending_command_issued_at = Arc::new(Mutex::new(std::collections::HashMap::<String, DateTime<Utc>>::new()));

        // --- Task 1: network receive ---
        let network_task = {
            let telemetry_tx = telemetry_tx.clone();
            let performance_tx = performance_tx.clone();
            let fault_manager = Arc::clone(&fault_manager);
            let is_running = Arc::clone(&is_running);
            let fault_tx_network = fault_tx.clone();
            let telemetry_backlog_counter = Arc::clone(&telemetry_backlog);
            let mut received_packet_count: u64 = 0;

            tokio::spawn(async move {
                info!("Network Reception Task Online");
                while *is_running.lock().await {
                    match network_manager.receive_packet_with_reception_timing().await {
                        Ok((packet, timing)) => {
                            if !Self::has_valid_packet_id(&packet) {
                                warn!("Rejected Packet Format: {}", packet.header.packet_id);
                                continue;
                            }
                            // success → reset consecutive failures
                            {
                                let mut fm = fault_manager.lock().await;
                                fm.record_successful_communication();
                            }

                            // enqueue
                            let queue_len_after_enqueue = telemetry_backlog_counter.fetch_add(1, Ordering::Relaxed) + 1;
                            if let Err(enqueue_error) = telemetry_tx.send((packet, timing.reception_time)).await {
                                telemetry_backlog_counter.fetch_sub(1, Ordering::Relaxed);
                                error!("Telemetry Enqueue Operation Failed: {enqueue_error}");
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::TelemetryDropped,
                                    duration_ms: 0.0,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("reason".into(), "mpsc_send_failed".into());
                                        m.insert("packet_id".into(), timing.packet_id.clone());
                                        m.insert("queue_len_after".into(), queue_len_after_enqueue.saturating_sub(1).to_string());
                                        m.insert("queue_capacity".into(), TELEMETRY_Q_CAP.to_string());
                                        m
                                    },
                                }).await;
                            } else {
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::TelemetryEnqueued,
                                    duration_ms: 0.0,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), timing.packet_id.clone());
                                        m.insert("queue_len".into(), queue_len_after_enqueue.to_string());
                                        m.insert("queue_capacity".into(), TELEMETRY_Q_CAP.to_string());
                                        m
                                    },
                                }).await;
                            }

                            if timing.decode_time_ms > 3.0 {
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: timing.reception_time,
                                    event_type: EventType::PacketDecodeViolation,
                                    duration_ms: timing.decode_time_ms,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), timing.packet_id.clone());
                                        m.insert("packet_type".into(), timing.packet_type.clone());
                                        m.insert("violation_amount_ms".into(),
                                                 format!("{:.3}", timing.decode_time_ms - 3.0));
                                        m
                                    },
                                }).await;
                            }

                            let _ = performance_tx.send(PerformanceEvent {
                                timestamp: timing.reception_time,
                                event_type: EventType::PacketReceived,
                                duration_ms: timing.end_to_end_latency_ms,
                                metadata: {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert("packet_id".into(), timing.packet_id.clone());
                                    m.insert("packet_type".into(), timing.packet_type.clone());
                                    m.insert("latency_ms".into(), format!("{:.3}", timing.end_to_end_latency_ms));
                                    m.insert("drift_ms".into(), format!("{:.3}", timing.reception_drift_ms));
                                    m.insert("jitter_ms".into(), format!("{:.3}", timing.jitter_ms));
                                    m.insert("decode_time_ms".into(), format!("{:.3}", timing.decode_time_ms));
                                    m.insert("drift_severity".into(), match timing.delay_severity {
                                        DriftSeverity::Normal => "normal",
                                        DriftSeverity::Minor => "minor",
                                        DriftSeverity::Moderate => "moderate",
                                        DriftSeverity::Severe => "severe",
                                    }.to_string());
                                    m
                                },
                            }).await;

                            if timing.is_delayed {
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: timing.reception_time,
                                    event_type: EventType::PacketDelayed,
                                    duration_ms: timing.jitter_ms,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), timing.packet_id.clone());
                                        m.insert("packet_type".into(), timing.packet_type.clone());
                                        m.insert("expected_time".into(),
                                                 (timing.reception_time
                                                  - chrono::Duration::milliseconds(timing.reception_drift_ms as i64))
                                                 .to_rfc3339());
                                        m.insert("actual_time".into(), timing.reception_time.to_rfc3339());
                                        m.insert("drift_amount_ms".into(), format!("{:.1}", timing.reception_drift_ms));
                                        m.insert("delay_severity".into(), match timing.delay_severity {
                                            DriftSeverity::Normal => "normal",
                                            DriftSeverity::Minor => "minor",
                                            DriftSeverity::Moderate => "moderate",
                                            DriftSeverity::Severe => "severe",
                                        }.to_string());
                                        m
                                    },
                                }).await;
                            }

                            received_packet_count += 1;
                            if received_packet_count % 50 == 0 {
                                info!(
                                    "Reception Drift Summary: Samples={}, Latency={:.1}ms, Drift={:.1}ms, Jitter={:.1}ms",
                                    received_packet_count, timing.end_to_end_latency_ms,
                                    timing.reception_drift_ms, timing.jitter_ms
                                );
                            }
                        }
                        Err(network_error) => {
                            let is_timeout = network_error.to_string().contains("timeout");
                            if is_timeout {
                                let (loss_detected, already_active) = {
                                    let mut fm = fault_manager.lock().await;
                                    fm.increment_consecutive_failures();
                                    (fm.has_loss_of_contact_condition(), fm.has_active_loss_of_contact())
                                };
                                if loss_detected && !already_active {
                                    warn!("Loss Of Contact Confirmed -> Activating Emergency Procedures");
                                    let res = {
                                        let mut fm = fault_manager.lock().await;
                                        fm.handle_loss_of_contact().await
                                    };
                                    if let Err(loss_of_contact_error) = res { error!("Loss-Of-Contact Handler Failed: {loss_of_contact_error}"); }
                                }
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                continue;
                            }

                            warn!("Network Reception Failure: {network_error}");
                            let _ = fault_tx_network.send(fault_management::FaultEvent {
                                timestamp: Utc::now(),
                                fault_type: fault_management::FaultType::NetworkError,
                                severity: fault_management::Severity::Medium,
                                description: format!("Network Receive Issue Detected: {network_error}"),
                                affected_systems: vec!["network".into()],
                            }).await;
                            tokio::time::sleep(Duration::from_millis(75)).await;
                        }
                    }
                }
            })
        };

        // --- Task 2: telemetry processing (≤3ms target) ---
        let telemetry_task = {
            let fault_tx_telemetry = fault_tx.clone();
            let performance_tx = performance_tx.clone();
            let telemetry_backlog_counter = Arc::clone(&telemetry_backlog);
            let pending_command_issued_at = Arc::clone(&pending_command_issued_at);

            tokio::spawn(async move {
                info!("Telemetry Processing Task Online");
                while let Some((packet, reception_time)) = telemetry_rx.recv().await {
                    let start = std::time::Instant::now();
                    let queue_len_after_dequeue = telemetry_backlog_counter.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
                    let queued_ms = (Utc::now() - reception_time).num_microseconds().unwrap_or(0) as f64 / 1000.0;
                    let packet_id_for_dequeue = packet.header.packet_id.clone();

                    let _ = performance_tx.send(PerformanceEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::TelemetryDequeued,
                        duration_ms: 0.0,
                        metadata: {
                            let mut m = std::collections::HashMap::new();
                            m.insert("packet_id".into(), packet_id_for_dequeue);
                            m.insert("queue_len".into(), queue_len_after_dequeue.to_string());
                            m.insert("queue_capacity".into(), TELEMETRY_Q_CAP.to_string());
                            m
                        },
                    }).await;

                    let mut tp = telemetry_processor.lock().await;
                    match tp.process_telemetry_packet(packet, reception_time).await {
                        Ok(result) => {
                            let t_ms = start.elapsed().as_secs_f64() * 1000.0;

                            for ack in &result.command_acknowledgments {
                                let issued_at = {
                                    pending_command_issued_at
                                        .lock().await
                                        .remove(&ack.command_id)
                                };

                                if let Some(issued_at) = issued_at {
                                    let rtt_ms = (ack.observed_at - issued_at)
                                        .num_microseconds()
                                        .unwrap_or(0) as f64 / 1000.0;

                                    info!(
                                        "COMMAND-TO-RESPONSE: {} | Issued={} Response={} RTT={:.3}ms Status={}",
                                        ack.command_id,
                                        issued_at.format("%H:%M:%S%.3f"),
                                        ack.observed_at.format("%H:%M:%S%.3f"),
                                        rtt_ms,
                                        ack.status
                                    );

                                    let _ = performance_tx.send(PerformanceEvent {
                                        timestamp: Utc::now(),
                                        event_type: EventType::CommandResponseRttSample,
                                        duration_ms: rtt_ms,
                                        metadata: {
                                            let mut m = std::collections::HashMap::new();
                                            m.insert("command_id".into(), ack.command_id.clone());
                                            m.insert("status".into(), ack.status.clone());
                                            if let Some(exec_ts) = ack.execution_timestamp {
                                                m.insert("execution_timestamp".into(), exec_ts.to_rfc3339());
                                            }
                                            if let Some(done_ts) = ack.completion_timestamp {
                                                m.insert("completion_timestamp".into(), done_ts.to_rfc3339());
                                            }
                                            m
                                        },
                                    }).await;
                                }
                            }

                            if t_ms > 3.0 {
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::TelemetryProcessingViolation,
                                    duration_ms: t_ms,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), result.packet_id.clone());
                                        m.insert("sensor_count".into(), result.sensor_count.to_string());
                                        m.insert("exceeded_by_ms".into(), format!("{:.3}", t_ms - 3.0));
                                        m
                                    },
                                }).await;
                                warn!("Telemetry Processing Overran 3ms Budget: {t_ms:.3}ms");
                            }

                            for f in result.detected_faults {
                                let _ = fault_tx_telemetry.send(f).await;
                            }
                            // NOTE: PacketDelayed events are already emitted by the network layer
                            // (task 1) for every packet where jitter > 25ms or latency > 200ms.
                            // We log severely delayed packets here as informational (not as
                            // PacketDelayed events) to avoid double-counting the delayed_packets
                            // metric.
                            for delayed in &result.delayed_packets_detected {
                                let severity = if delayed.delay_amount_ms > 500.0 { "severe" }
                                    else { "moderate" };
                                info!(
                                    "Severe Packet Delay Confirmed By Telemetry Stage: {} ({}) \
                                     Delay={:.1}ms [{}]",
                                    delayed.packet_id, delayed.packet_type,
                                    delayed.delay_amount_ms, severity
                                );
                            }

                            let _ = performance_tx.send(PerformanceEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::TelemetryProcessed,
                                duration_ms: t_ms,
                                metadata: {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert("packet_id".into(), result.packet_id);
                                    m.insert("sensor_count".into(), result.sensor_count.to_string());
                                    m.insert("delayed_packets".into(), result.delayed_packets_detected.len().to_string());
                                    m.insert("queue_wait_ms".into(), format!("{queued_ms:.3}"));
                                    m
                                },
                            }).await;
                        }
                        Err(e) => {
                            error!("Telemetry Processing Failure: {e}");
                            let _ = fault_tx_telemetry.send(fault_management::FaultEvent {
                                timestamp: Utc::now(),
                                fault_type: fault_management::FaultType::TelemetryError,
                                severity: fault_management::Severity::High,
                                description: format!("Telemetry Processing Pipeline Failed: {e}"),
                                affected_systems: vec!["telemetry".into()],
                            }).await;
                        }
                    }
                }
            })
        };

        // --- Task 3: missing packet rerequest ---
        let missing_packet_task = {
            let telemetry_processor = Arc::clone(&self.telemetry_processor);
            let network_manager = Arc::clone(&self.network_manager);
            let performance_tx = performance_tx.clone();
            let fault_tx_rerequest = fault_tx.clone();
            let is_running = Arc::clone(&is_running);

            tokio::spawn(async move {
                info!("Missing-Packet Re-Request Task Online");
                let mut itv = interval(Duration::from_millis(650));
                let mut rr_count = 0u64;

                while *is_running.lock().await {
                    itv.tick().await;

                    let missing = {
                        let mut p = telemetry_processor.lock().await;
                        let list = p.collect_missing_packet_uplink_candidates();
                        p.prune_stale_missing_packets();
                        p.prune_stale_delayed_packets();
                        list
                    };

                    for missing in missing {
                        rr_count += 1;
                        let send_started = std::time::Instant::now();
                        match network_manager.send_retransmission_request(&missing.packet_id).await {
                            Ok(_) => {
                                {
                                    let mut p = telemetry_processor.lock().await;
                                    p.mark_packet_as_rerequested(&missing.packet_id);
                                }

                                let uplink_send_ms = send_started.elapsed().as_secs_f64() * 1000.0;
                                let packet_to_uplink_ms = missing.wait_ms + uplink_send_ms;

                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::PacketRetransmissionRequested,
                                    duration_ms: 0.0,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), missing.packet_id.clone());
                                        m.insert("re_request_number".into(), rr_count.to_string());
                                        m
                                    },
                                }).await;

                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::PacketToUplinkLatencySample,
                                    duration_ms: packet_to_uplink_ms,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), missing.packet_id.clone());
                                        m.insert("wait_before_uplink_ms".into(), format!("{:.3}", missing.wait_ms));
                                        m.insert("uplink_send_ms".into(), format!("{:.3}", uplink_send_ms));
                                        m
                                    },
                                }).await;
                            }
                            Err(e) => {
                                error!("Retransmission Request Dispatch Failed For {}: {e}", missing.packet_id);
                                let _ = fault_tx_rerequest.send(fault_management::FaultEvent {
                                    timestamp: Utc::now(),
                                    fault_type: fault_management::FaultType::NetworkError,
                                    severity: fault_management::Severity::Medium,
                                    description: format!("Packet {} Failed During Retransmission Request; Error: {e}", missing.packet_id),
                                    affected_systems: vec!["network".into(), "telemetry".into()],
                                }).await;
                            }
                        }
                    }
                }
            })
        };

        // --- Task 4: fault management ---
        let fault_task = Self::launch_fault_management_task(fault_rx, Arc::clone(&fault_manager));

        // --- Task 5: performance monitor ---
        let performance_task = Self::launch_performance_monitor_task(
            performance_rx,
            Arc::clone(&performance_tracker),
        );

        // --- Task 6: health heartbeat ---
        let health_task = {
            let is_running = Arc::clone(&is_running);
            let performance_tracker = Arc::clone(&self.performance_tracker);
            let fault_manager = Arc::clone(&self.fault_manager);
            let command_scheduler = Arc::clone(&self.command_scheduler);
            let performance_tx = performance_tx.clone();
            let fault_tx_alert_sim = fault_tx.clone();

            tokio::spawn(async move {
                let mut itv = interval(Duration::from_secs(5));
                let mut count: u32 = 0;

                while *is_running.lock().await {
                    itv.tick().await;

                    let stats = { performance_tracker.lock().await.snapshot_current_stats() };
                    let fstats = { fault_manager.lock().await.get_stats() };

                    let _ = performance_tx.send(PerformanceEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::FaultRecoveryMetric,
                        duration_ms: fstats.mttr_avg_ms,
                        metadata: {
                            let mut m = std::collections::HashMap::new();
                            m.insert("mttr_avg_ms".into(), format!("{:.3}", fstats.mttr_avg_ms));
                            m.insert("mtbf_avg_ms".into(), format!("{:.3}", fstats.mtbf_avg_ms));
                            m.insert(
                                "avg_fault_response_time_ms".into(),
                                format!("{:.3}", fstats.avg_fault_response_time_ms),
                            );
                            m.insert(
                                "avg_fault_resolution_time_ms".into(),
                                format!("{:.3}", fstats.avg_fault_resolution_time_ms),
                            );
                            m
                        },
                    }).await;

                    info!(
                        "Health: pkts={} avg_proc={:.2}ms faults={} net_fail={}/{}",
                        stats.total_packets_received,
                        stats.avg_processing_time_ms,
                        stats.total_faults,
                        fstats.consecutive_network_failures,
                        3
                    );

                    if fstats.active_critical_faults > 0 {
                        error!("{} Critical Faults Currently Active!", fstats.active_critical_faults);
                    }

                    count += 1;

                    // Every 60 s (12 x 5 s ticks), inject one synthetic critical fault.
                    // FaultManager tags this marker and intentionally exceeds 100ms
                    // to exercise the critical ground-alert requirement path.
                    if count % 12 == 0 {
                        let _ = fault_tx_alert_sim.send(fault_management::FaultEvent {
                            timestamp: Utc::now(),
                            fault_type: fault_management::FaultType::SystemOverload,
                            severity: fault_management::Severity::Critical,
                            description: "SIM_CRITICAL_GROUND_ALERT_100MS".to_string(),
                            affected_systems: vec!["ground_control".to_string()],
                        }).await;
                        warn!("Injected synthetic critical-alert test fault (60s cadence)");

                        // Inject a deliberately unsafe command shortly after the fault so
                        // the newly activated interlock can reject it and emit block latency.
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        let unsafe_cmd = shared_protocol::Command::sensor_self_test(
                            99,
                            shared_protocol::SensorType::Thermal,
                        );

                        let mut sched = command_scheduler.lock().await;
                        match sched.schedule_command(unsafe_cmd) {
                            Ok(cmd_id) => {
                                warn!(
                                    "Injected synthetic unsafe command for interlock-block test: {}",
                                    cmd_id
                                );
                            }
                            Err(e) => {
                                warn!("Synthetic unsafe command injection skipped: {}", e);
                            }
                        }
                    }

                    // Every 30 s (count increments every 5 s → every 6 ticks) run
                    // the stale-fault sweep so long-lived faults eventually resolve.
                    if count % 6 == 0 {
                        let resolved = fault_manager.lock().await.auto_resolve_stale_faults();
                        if resolved > 0 {
                            info!("Auto-Resolved {} Stale Fault(s)", resolved);
                        }
                    }
                }
            })
        };

        // --- Task 7: RT command scheduler (0.5ms tick) ---
        let command_scheduler_task = {
            let command_scheduler = Arc::clone(&self.command_scheduler);
            let fault_manager = Arc::clone(&self.fault_manager);
            let network_manager = Arc::clone(&self.network_manager);
            let performance_tx = performance_tx.clone();
            let is_running = Arc::clone(&is_running);
            let pending_command_issued_at = Arc::clone(&pending_command_issued_at);

            tokio::spawn(async move {
                info!("Command Scheduler Task Online (0.5ms Resolution)");
                let mut itv = tokio::time::interval(Duration::from_micros(500));
                let mut tick: u64 = 0;
                let mut last_check = std::time::Instant::now();
                let scheduler_start = std::time::Instant::now();
                let mut last_uplink_sample_at: Option<std::time::Instant> = None;
                let mut severe_drift_window_started_at = std::time::Instant::now();
                let mut severe_drift_sum_ms: f64 = 0.0;
                let mut severe_drift_count: u64 = 0;
                let mut severe_drift_max_ms: f64 = 0.0;

                while *is_running.lock().await {
                    itv.tick().await;
                    tick += 1;

                    // Track scheduler drift against the ideal 0.5ms tick cadence.
                    let expected_elapsed_ms = tick as f64 * 0.5;
                    let actual_elapsed_ms = scheduler_start.elapsed().as_secs_f64() * 1000.0;
                    let drift_ms = (actual_elapsed_ms - expected_elapsed_ms).abs();

                    if drift_ms > 15.0 {
                        severe_drift_sum_ms += drift_ms;
                        severe_drift_count += 1;
                        severe_drift_max_ms = severe_drift_max_ms.max(drift_ms);
                    }

                    if severe_drift_window_started_at.elapsed() >= Duration::from_secs(10) {
                        if severe_drift_count > 0 {
                            let severe_drift_avg_ms = severe_drift_sum_ms / severe_drift_count as f64;
                            warn!(
                                "SCHEDULER DRIFT (10s): SevereSamples={} AvgSevereDrift={:.3}ms MaxSevereDrift={:.3}ms Threshold>15.0ms",
                                severe_drift_count,
                                severe_drift_avg_ms,
                                severe_drift_max_ms
                            );
                        }

                        severe_drift_window_started_at = std::time::Instant::now();
                        severe_drift_sum_ms = 0.0;
                        severe_drift_count = 0;
                        severe_drift_max_ms = 0.0;
                    }

                    let _ = performance_tx.send(PerformanceEvent {
                        timestamp: Utc::now(),
                        event_type: EventType::TaskExecutionDrift,
                        duration_ms: drift_ms,
                        metadata: {
                            let mut m = std::collections::HashMap::new();
                            m.insert("drift_ms".into(), format!("{drift_ms:.3}"));
                            m.insert("expected_elapsed_ms".into(), format!("{expected_elapsed_ms:.3}"));
                            m.insert("actual_elapsed_ms".into(), format!("{actual_elapsed_ms:.3}"));
                            m.insert("tick".into(), tick.to_string());
                            m
                        },
                    }).await;

                    if drift_ms > 15.0 {
                        let _ = performance_tx.send(PerformanceEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::SchedulerPrecisionViolation,
                            duration_ms: drift_ms,
                            metadata: {
                                let mut m = std::collections::HashMap::new();
                                m.insert("drift_ms".into(), format!("{drift_ms:.3}"));
                                m.insert("threshold_ms".into(), "15.0".into());
                                m.insert("tick".into(), tick.to_string());
                                m
                            },
                        }).await;
                    }

                    if tick % 2 == 0 {
                        // Sample uplink cadence/jitter on each dispatch cycle (ideal interval = 1ms).
                        let now = std::time::Instant::now();
                        if let Some(prev) = last_uplink_sample_at {
                            let interval_ms = (now - prev).as_secs_f64() * 1000.0;
                            let jitter_ms = (interval_ms - 1.0).abs();
                            let _ = performance_tx.send(PerformanceEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::UplinkIntervalSample,
                                duration_ms: interval_ms,
                                metadata: {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert("uplink_interval_ms".into(), format!("{interval_ms:.3}"));
                                    m.insert("uplink_jitter_ms".into(), format!("{jitter_ms:.3}"));
                                    m.insert("expected_interval_ms".into(), "1.000".into());
                                    m.insert("tick".into(), tick.to_string());
                                    m
                                },
                            }).await;

                            if jitter_ms > 10.0 {
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::JitterViolation,
                                    duration_ms: jitter_ms,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("uplink_jitter_ms".into(), format!("{jitter_ms:.3}"));
                                        m.insert("threshold_ms".into(), "10.0".into());
                                        m.insert("tick".into(), tick.to_string());
                                        m
                                    },
                                }).await;
                            }
                        }
                        last_uplink_sample_at = Some(now);

                        let cmds = {
                            let mut sched = command_scheduler.lock().await;
                            let mut fm = fault_manager.lock().await;
                            sched.process_dispatch_queue(&mut *fm, &*network_manager, Some(&performance_tx)).await
                        };
                        for c in &cmds {
                            let urgent = (c.priority as u8) <= 1;
                            let issued_at = Utc::now();
                            pending_command_issued_at
                                .lock().await
                                .insert(c.command_id.clone(), issued_at);

                            let _ = performance_tx.send(PerformanceEvent {
                                timestamp: issued_at,
                                event_type: if urgent { EventType::UrgentCommandDispatched } else { EventType::CommandDispatched },
                                duration_ms: 0.0,
                                metadata: {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert("command_id".into(), c.command_id.clone());
                                    m.insert("command_type".into(), format_debug_enum(&c.command_type));
                                    m.insert("priority".into(), (c.priority as u8).to_string());
                                    m.insert("target_system".into(), format_debug_enum(&c.target_system));
                                    m.insert("is_urgent".into(), urgent.to_string());
                                    m.insert("tick".into(), tick.to_string());
                                    m
                                },
                            }).await;
                        }
                    }

                    {
                        let sched = command_scheduler.lock().await;
                        for w in sched.get_commands_approaching_deadline() {
                            if w.time_to_deadline_ms < 0.1 {
                                error!("DEADLINE CRITICAL: {} ({:?}) {:.3}ms Remaining [Tick:{}]", w.command_id, w.command_type, w.time_to_deadline_ms, tick);
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::CommandDeadlineViolation,
                                    duration_ms: w.time_to_deadline_ms.abs(),
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("command_id".into(), w.command_id.clone());
                                        m.insert("command_type".into(), format_debug_enum(&w.command_type));
                                        m.insert("priority".into(), (w.priority as u8).to_string());
                                        m.insert("target_system".into(), format_debug_enum(&w.target_system));
                                        m.insert("severity".into(), "critical_emergency".into());
                                        m.insert("tick_precision".into(), "0.5ms".into());
                                        m
                                    },
                                }).await;
                            } else if w.time_to_deadline_ms < 0.5 {
                                error!("DEADLINE URGENT: {} ({:?}) {:.3}ms Remaining [Tick:{}]", w.command_id, w.command_type, w.time_to_deadline_ms, tick);
                            } else if w.time_to_deadline_ms < 1.0 {
                                warn!("Deadline Proximity Alert: {} ({:?}) {:.3}ms Remaining [Tick:{}]", w.command_id, w.command_type, w.time_to_deadline_ms, tick);
                            }
                        }
                    }

                    if tick % 10 == 0 {
                        let sched = command_scheduler.lock().await;
                        let rep = sched.get_unified_deadline_report();
                        if rep.network_violations > 0 || rep.deadline_violations > 0 {
                            let dt = last_check.elapsed().as_millis();
                            warn!("Recent Deadline Violations Over {dt}ms: Net={}/{} Total={}/{} (Net {:.1}%, Overall {:.1}%)",
                                  rep.network_violations, rep.total_urgent_commands,
                                  rep.deadline_violations, rep.total_urgent_commands,
                                  rep.network_adherence_rate, rep.adherence_rate);
                            if rep.avg_network_send_time > 1.8 {
                                error!("Average Network Send Time Approaching 2ms Threshold: {:.3}ms", rep.avg_network_send_time);
                            }
                        }
                        last_check = std::time::Instant::now();
                    }

                    if tick % 1000 == 0 {
                        let mut s = command_scheduler.lock().await;
                        s.prune_expired_commands().await;
                        s.refresh_safety_validation_cache().await;
                    }

                    if tick % 20000 == 0 {
                        let s = command_scheduler.lock().await;
                        let rep = s.get_unified_deadline_report();
                        info!("=== COMMAND SCHEDULER REPORT === Net<=2ms {:.1}% ({} OK / {} Urgent) AvgNet {:.3}ms Overall {:.1}% Violations={} Trend={}",
                              rep.network_adherence_rate,
                              rep.total_urgent_commands - rep.network_violations,
                              rep.total_urgent_commands,
                              rep.avg_network_send_time,
                              rep.adherence_rate,
                              rep.deadline_violations,
                              rep.performance_trend);
                        debug!("Scheduler Precision: 0.5ms Tick; Uptime {:.3}s", tick as f64 * 0.0005);
                    }

                    if tick % 100 == 0 { tokio::task::yield_now().await; }
                }
            })
        };

        // Task 8: Simulation – inject an emergency packet periodically
        let _fault_simulator_task = Self::launch_fault_simulation_task(telemetry_tx.clone());

        info!("All Tasks Online. Ground Control Is Operational.");

        // Run tasks
        let _ = tokio::try_join!(
            network_task,
            telemetry_task,
            missing_packet_task,
            fault_task,
            performance_task,
            health_task,
            command_scheduler_task
        )?;

        *self.is_running.lock().await = false;
        info!("Ground Control Shutdown Sequence Complete");
        Ok(())
    }

    pub async fn schedule_emergency_deadline_test(&self) -> Result<String> {
        let mut scheduler = self.command_scheduler.lock().await;
        let emergency_command = shared_protocol::Command::thermal_emergency_response(1, 95.0);
        let scheduled_command_id = scheduler.schedule_command(emergency_command)?;
        info!("Emergency Command Scheduled For Deadline Validation: {scheduled_command_id}");
        Ok(scheduled_command_id)
    }

    pub async fn collect_command_scheduler_metrics(&self) -> (EnhancedCommandSchedulerStats, UnifiedDeadlineReport) {
        let scheduler = self.command_scheduler.lock().await;
        (scheduler.get_enhanced_stats(), scheduler.get_unified_deadline_report())
    }

    // keep validation simple & enum-safe (typed protocol already validates the rest)
    fn has_valid_packet_id(packet: &shared_protocol::CommunicationPacket) -> bool {
        !packet.header.packet_id.is_empty()
    }

    pub async fn shutdown(&self) {
        info!("Initiating Graceful Shutdown Procedure...");
        *self.is_running.lock().await = false;
        tokio::time::sleep(Duration::from_secs(1)).await;

        let performance_tracker = self.performance_tracker.lock().await;
        let final_stats = performance_tracker.snapshot_current_stats();
        let fault_stats = self.fault_manager.lock().await.get_stats();
        let drift = self.network_manager.collect_drift_stats().await;

        info!("=== FINAL GROUND CONTROL SUMMARY ===");
        info!("Total Runtime: {:?}", Utc::now() - self.system_start_time);
        info!("Telemetry Packets Processed: {}", final_stats.total_packets_processed);
        info!("Average Processing Time: {:.3}ms (P95 {:.3}ms, P99 {:.3}ms)", final_stats.avg_processing_time_ms, final_stats.p95_processing_time_ms, final_stats.p99_processing_time_ms);
        info!("Processing Violations Above 3ms: {}", final_stats.processing_violations_3ms);
        info!("Packets Received From Network: {}", final_stats.total_packets_received);
        info!("Average Reception Latency: {:.1}ms", final_stats.avg_reception_latency_ms);
        info!("Late Arrival Events: {} (Average Delay {:.1}ms)", final_stats.delayed_packets_total, final_stats.avg_packet_delay_ms);
        info!("Reception Drift Violations: {}", final_stats.reception_drift_violations);
        info!("Retransmission Requests Issued: {}", final_stats.retransmission_requests);
        info!("Network Timeout Events: {}", final_stats.network_timeouts);
        info!(
            "Network Performance Report: TotalAnalyzed={} AvgDrift={:.1}ms MaxDrift={:.1}ms DriftViolations={} AvgDelay={:.1}ms Delayed={} Violations3ms={}",
            drift.total_packets_analyzed,
            drift.avg_drift_ms,
            drift.max_drift_ms,
            drift.drift_violations,
            final_stats.avg_packet_delay_ms,
            final_stats.delayed_packets_total,
            final_stats.processing_violations_3ms
        );
        info!(
            "Uplink Jitter: P95 {:.3}ms (P99 {:.3}ms, max {:.3}ms) | Avg Interval {:.3}ms",
            final_stats.p95_uplink_jitter_ms,
            final_stats.p99_uplink_jitter_ms,
            final_stats.max_uplink_jitter_ms,
            final_stats.avg_uplink_interval_ms
        );
        if final_stats.packet_to_uplink_latency_samples == 0 {
            info!("Packet-To-Uplink Latency: no samples observed in this run");
        } else {
            info!(
                "Packet-To-Uplink Latency: avg {:.3}ms (P95 {:.3}ms, P99 {:.3}ms, max {:.3}ms) from {} samples",
                final_stats.avg_packet_to_uplink_latency_ms,
                final_stats.p95_packet_to_uplink_latency_ms,
                final_stats.p99_packet_to_uplink_latency_ms,
                final_stats.max_packet_to_uplink_latency_ms,
                final_stats.packet_to_uplink_latency_samples
            );
        }
        info!(
            "Telemetry Backlog Queue: len avg {:.1} (P95 {:.1}, max {}) | age avg {:.3}ms (P95 {:.3}ms, max {:.3}ms) | warn {} crit {}",
            final_stats.backlog_avg_len,
            final_stats.backlog_p95_len,
            final_stats.backlog_max_len,
            final_stats.backlog_avg_age_ms,
            final_stats.backlog_p95_age_ms,
            final_stats.backlog_max_age_ms,
            final_stats.backlog_warn_events,
            final_stats.backlog_critical_events
        );
        info!("Faults Handled: {} | Critical Faults Active: {}", fault_stats.total_faults_detected, fault_stats.active_faults_count);
        info!(
            "Critical Ground Alerts (>100ms Fault Response): {}",
            fault_stats.response_time_critical_alerts
        );
        if final_stats.command_response_rtt_samples == 0 {
            info!("Command-To-Response Latency: no samples observed in this run");
        } else {
            info!(
                "Command-To-Response Latency: avg {:.3}ms (P95 {:.3}ms, P99 {:.3}ms, max {:.3}ms) from {} samples",
                final_stats.avg_command_response_rtt_ms,
                final_stats.p95_command_response_rtt_ms,
                final_stats.p99_command_response_rtt_ms,
                final_stats.max_command_response_rtt_ms,
                final_stats.command_response_rtt_samples
            );
        }
        info!(
            "Scheduler Drift: avg {:.3}ms (P95 {:.3}ms, P99 {:.3}ms, max {:.3}ms)",
            final_stats.avg_task_drift_ms,
            final_stats.p95_task_drift_ms,
            final_stats.p99_task_drift_ms,
            final_stats.max_task_drift_ms
        );
        info!(
            "Fault Recovery: AvgResponse {:.3}ms | AvgResolution {:.3}ms | MTTR {:.3}ms | MTBF {:.3}ms",
            fault_stats.avg_fault_response_time_ms,
            fault_stats.avg_fault_resolution_time_ms,
            fault_stats.mttr_avg_ms,
            fault_stats.mtbf_avg_ms
        );
        info!("System Health Score: {:.1}/100 | Uptime {:.2}%", final_stats.system_health_score, final_stats.uptime_percentage);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                        GROUND CONTROL BOOT SEQUENCE                      ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!("Launching Ground Control Simulation...");

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("ground_control=info".parse().unwrap())
                .add_directive("shared_protocol=info".parse().unwrap())
                .add_directive("tokio=warn".parse().unwrap())
        )
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .event_format(
            tracing_subscriber::fmt::format()
                .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
                .compact()
        )
        .init();

    info!("Ground Control Startup Initiated (Shared Protocol v{})", shared_protocol::PROTOCOL_VERSION);

    println!("┌─────────────────────────────────────────────────────────────────────────────┐");
    println!("│                       GROUND CONTROL CORE READY                            │");
    println!("└─────────────────────────────────────────────────────────────────────────────┘");

    let ground_control_system = GroundControlSystem::new().await?;
    let performance_tracker_handle = ground_control_system.get_performance_tracker_handle();
    info!("Ground Control System Initialized - All Components Ready");

    // system-load sampler (optional)
    {
        let perf_for_sampler = performance_tracker_handle.clone();
        tokio::spawn(async move {
            system_monitor::run_system_load_sampler(perf_for_sampler, 1000).await;
        });
    }

    // lightweight heartbeat
    {
        let perf_for_report = performance_tracker_handle.clone();
        tokio::spawn(async move {
            loop {
                {
                    let perf_tracker = perf_for_report.lock().await;
                    let current_stats = perf_tracker.snapshot_current_stats();
                    tracing::info!(
                        "SYSTEM LOAD HEARTBEAT: CPU {:.1}% (Avg {:.1}%, Peak {:.1}%) | MEM {:.1}% (Avg {:.1}%) | Load1 {:.2}",
                        current_stats.cpu_latest_percent, current_stats.cpu_avg_percent, current_stats.cpu_peak_percent,
                        current_stats.mem_latest_percent, current_stats.mem_avg_percent, current_stats.load1_latest
                    );
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
    }

    // Ctrl+C → graceful shutdown
    let gcs_arc = Arc::new(ground_control_system);
    {
        let gcs_for_shutdown = Arc::clone(&gcs_arc);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.expect("Ctrl+C");
            println!("┌─────────────────────────────────────────────────────────────────────────────┐");
            println!("│                        GROUND CONTROL SHUTTING DOWN                        │");
            println!("└─────────────────────────────────────────────────────────────────────────────┘");
            info!("Shutdown Signal Received - Ground Control Entering Shutdown");
            gcs_for_shutdown.shutdown().await;
            std::process::exit(0);
        });
    }

    println!("┌─────────────────────────────────────────────────────────────────────────────┐");
    println!("│                      GROUND CONTROL OPERATIONAL                            │");
    println!("└─────────────────────────────────────────────────────────────────────────────┘");
    info!("Ground Control Station Operational - Monitoring Satellite Systems");

    gcs_arc.run().await
}
