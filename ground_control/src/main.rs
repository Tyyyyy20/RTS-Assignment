// src/main.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, interval};
use tracing::{info, warn, error, debug};
use tracing_subscriber::EnvFilter;
use chrono::{DateTime, Utc};
use anyhow::Result;

mod network;
mod telemetry;
mod fault;
mod performance;
mod command;
mod monitor;

use network::{NetworkManager, DriftSeverity};
use telemetry::TelemetryProcessor;
use fault::{FaultManager, FaultSimulator};
use performance::{PerformanceTracker, PerformanceEvent, EventType};
use command::{CommandScheduler, EnhancedCommandSchedulerStats, UnifiedDeadlineReport};

/// Handy: stringify any Debug-able enum for logs/metadata
fn enum_s<T: std::fmt::Debug>(e: &T) -> String { format!("{:?}", e) }

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
fn map_severity(s: &str) -> fault::Severity {
    match s.to_ascii_lowercase().as_str() {
        "critical" | "emergency" => fault::Severity::Critical,
        "high" => fault::Severity::High,
        "medium" => fault::Severity::Medium,
        "low" => fault::Severity::Low,
        _ => fault::Severity::Low,
    }
}

impl GroundControlSystem {
    pub async fn new() -> Result<Self> {
        info!("Initializing Ground Control System…");

        let system_start_time = Utc::now();

        // Components
        let network_manager = Arc::new(NetworkManager::new().await?);
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

    pub fn performance_tracker_handle(&self) -> Arc<Mutex<PerformanceTracker>> {
        Arc::clone(&self.performance_tracker)
    }

    /// Main execution loop
    pub async fn run(&self) -> Result<()> {
        info!("Starting Ground Control at {}", self.system_start_time);
        *self.is_running.lock().await = true;

        // Channels
        const TELEMETRY_Q_CAP: usize = 100;
        let (telemetry_tx, mut telemetry_rx) = mpsc::channel(TELEMETRY_Q_CAP);
        let (fault_tx, mut fault_rx) = mpsc::channel(50);
        let (performance_tx, mut performance_rx) = mpsc::channel(200);

        // Clones
        let network_manager = Arc::clone(&self.network_manager);
        let telemetry_processor = Arc::clone(&self.telemetry_processor);
        let fault_manager = Arc::clone(&self.fault_manager);
        let performance_tracker = Arc::clone(&self.performance_tracker);
        let is_running = Arc::clone(&self.is_running);
        let telemetry_backlog = Arc::new(AtomicUsize::new(0));

        // --- Task 1: network receive ---
        let network_task = {
            let telemetry_tx = telemetry_tx.clone();
            let performance_tx = performance_tx.clone();
            let fault_manager = Arc::clone(&fault_manager);
            let is_running = Arc::clone(&is_running);
            let fault_tx_network = fault_tx.clone();
            let backlog_ctr = Arc::clone(&telemetry_backlog);
            let mut packet_count: u64 = 0;

            tokio::spawn(async move {
                info!("Network reception task started");
                while *is_running.lock().await {
                    match network_manager.receive_packet_with_timing().await {
                        Ok((packet, timing)) => {
                            if !Self::validate_packet_format(&packet) {
                                warn!("Invalid packet format: {}", packet.header.packet_id);
                                continue;
                            }
                            // success → reset consecutive failures
                            {
                                let mut fm = fault_manager.lock().await;
                                fm.record_successful_communication();
                            }

                            // enqueue
                            let q_after = backlog_ctr.fetch_add(1, Ordering::Relaxed) + 1;
                            if let Err(e) = telemetry_tx.send((packet, timing.reception_time)).await {
                                backlog_ctr.fetch_sub(1, Ordering::Relaxed);
                                error!("telemetry enqueue failed: {e}");
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::TelemetryDropped,
                                    duration_ms: 0.0,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("reason".into(), "mpsc_send_failed".into());
                                        m.insert("packet_id".into(), timing.packet_id.clone());
                                        m.insert("queue_len_after".into(), q_after.saturating_sub(1).to_string());
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
                                        m.insert("queue_len".into(), q_after.to_string());
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

                            packet_count += 1;
                            if packet_count % 50 == 0 {
                                info!(
                                    "Drift summary: n={}, latency={:.1}ms, drift={:.1}ms, jitter={:.1}ms",
                                    packet_count, timing.end_to_end_latency_ms,
                                    timing.reception_drift_ms, timing.jitter_ms
                                );
                            }
                        }
                        Err(e) => {
                            let is_timeout = e.to_string().contains("timeout");
                            if is_timeout {
                                let loss_detected = {
                                    let mut fm = fault_manager.lock().await;
                                    fm.increment_consecutive_failures();
                                    fm.is_loss_of_contact()
                                };
                                if loss_detected {
                                    warn!("Loss of contact detected → emergency handling");
                                    let res = {
                                        let mut fm = fault_manager.lock().await;
                                        fm.handle_loss_of_contact().await
                                    };
                                    if let Err(e) = res { error!("loss-of-contact handler failed: {e}"); }
                                }
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                continue;
                            }

                            warn!("Network reception error: {e}");
                            let _ = fault_tx_network.send(fault::FaultEvent {
                                timestamp: Utc::now(),
                                fault_type: fault::FaultType::NetworkError,
                                severity: fault::Severity::Medium,
                                description: format!("Network reception error: {e}"),
                                affected_systems: vec!["network".into()],
                            }).await;
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            })
        };

        // --- Task 2: telemetry processing (≤3ms target) ---
        let telemetry_task = {
            let fault_tx_telemetry = fault_tx.clone();
            let performance_tx = performance_tx.clone();
            let backlog_ctr = Arc::clone(&telemetry_backlog);

            tokio::spawn(async move {
                info!("Telemetry processing task started");
                while let Some((packet, reception_time)) = telemetry_rx.recv().await {
                    let start = std::time::Instant::now();
                    let _q_after = backlog_ctr.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
                    let queued_ms = (Utc::now() - reception_time).num_microseconds().unwrap_or(0) as f64 / 1000.0;

                    let mut tp = telemetry_processor.lock().await;
                    match tp.process_packet(packet, reception_time).await {
                        Ok(result) => {
                            let t_ms = start.elapsed().as_secs_f64() * 1000.0;
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
                                warn!("Telemetry processing exceeded 3ms: {t_ms:.3}ms");
                            }

                            for f in result.detected_faults {
                                let _ = fault_tx_telemetry.send(f).await;
                            }
                            for delayed in &result.delayed_packets_detected {
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: delayed.actual_time,
                                    event_type: EventType::PacketDelayed,
                                    duration_ms: delayed.delay_amount_ms,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), delayed.packet_id.clone());
                                        m.insert("packet_type".into(), delayed.packet_type.clone());
                                        m.insert("expected_time".into(), delayed.expected_time.to_rfc3339());
                                        m.insert("actual_time".into(), delayed.actual_time.to_rfc3339());
                                        m.insert("delay_severity".into(),
                                            if delayed.delay_amount_ms > 500.0 { "severe" }
                                            else if delayed.delay_amount_ms > 200.0 { "moderate" }
                                            else { "minor" }
                                        .to_string());
                                        m
                                    },
                                }).await;
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
                            error!("Telemetry processing error: {e}");
                            let _ = fault_tx_telemetry.send(fault::FaultEvent {
                                timestamp: Utc::now(),
                                fault_type: fault::FaultType::TelemetryError,
                                severity: fault::Severity::High,
                                description: format!("Telemetry processing failed: {e}"),
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
                info!("Missing-packet re-request task started");
                let mut itv = interval(Duration::from_millis(500));
                let mut rr_count = 0u64;

                while *is_running.lock().await {
                    itv.tick().await;

                    let missing = {
                        let mut p = telemetry_processor.lock().await;
                        let list = p.get_missing_packets();
                        p.cleanup_old_missing_packets();
                        p.cleanup_old_delayed_packets();
                        list
                    };

                    for packet_id in missing {
                        rr_count += 1;
                        match network_manager.request_retransmission(&packet_id).await {
                            Ok(_) => {
                                {
                                    let mut p = telemetry_processor.lock().await;
                                    p.mark_packet_re_requested(&packet_id);
                                }
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::PacketRetransmissionRequested,
                                    duration_ms: 0.0,
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("packet_id".into(), packet_id.clone());
                                        m.insert("re_request_number".into(), rr_count.to_string());
                                        m
                                    },
                                }).await;
                            }
                            Err(e) => {
                                error!("re-request failed for {packet_id}: {e}");
                                let _ = fault_tx_rerequest.send(fault::FaultEvent {
                                    timestamp: Utc::now(),
                                    fault_type: fault::FaultType::NetworkError,
                                    severity: fault::Severity::Medium,
                                    description: format!("Failed packet re-request for {packet_id}: {e}"),
                                    affected_systems: vec!["network".into(), "telemetry".into()],
                                }).await;
                            }
                        }
                    }
                }
            })
        };

        // --- Task 4: fault management ---
        let fault_task = {
            tokio::spawn(async move {
                info!("Fault management task started");
                while let Some(fe) = fault_rx.recv().await {
                    let mut fm = fault_manager.lock().await;
                    if let Err(e) = fm.handle_fault(fe).await {
                        error!("Fault handling error: {e}");
                    }
                }
            })
        };

        // --- Task 5: performance monitor ---
        let performance_task = {
            tokio::spawn(async move {
                info!("Performance monitor task started");
                while let Some(ev) = performance_rx.recv().await {
                    let mut pt = performance_tracker.lock().await;
                    pt.record_event(ev);
                }
            })
        };

        // --- Task 6: health heartbeat ---
        let health_task = {
            let is_running = Arc::clone(&is_running);
            let performance_tracker = Arc::clone(&self.performance_tracker);
            let fault_manager = Arc::clone(&self.fault_manager);
            let network_manager = Arc::clone(&self.network_manager);

            tokio::spawn(async move {
                let mut itv = interval(Duration::from_secs(5));
                let mut count: u32 = 0;

                while *is_running.lock().await {
                    itv.tick().await;

                    let stats = { performance_tracker.lock().await.get_current_stats() };
                    let fstats = { fault_manager.lock().await.get_stats() };
                    let drift = network_manager.get_drift_stats().await;

                    info!(
                        "Health: pkts={} avg_proc={:.2}ms faults={} delayed={} net_fail={}/{} avg_drift={:.1}ms",
                        stats.total_packets_received,
                        stats.avg_processing_time_ms,
                        stats.total_faults,
                        stats.delayed_packets_total,
                        fstats.consecutive_network_failures,
                        3,
                        drift.avg_drift_ms
                    );

                    if fstats.active_critical_faults > 0 {
                        error!("{} critical faults active!", fstats.active_critical_faults);
                    }

                    count += 1;
                    if count % 6 == 0 {
                        info!("=== PERF REPORT === total_analyzed={} avg_drift={:.1}ms max_drift={:.1}ms violations={} avg_delay={:.1}ms delayed={} 3ms_violations={}",
                              drift.total_packets_analyzed, drift.avg_drift_ms, drift.max_drift_ms, drift.drift_violations,
                              stats.avg_packet_delay_ms, stats.delayed_packets_total, stats.processing_violations_3ms);
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

            tokio::spawn(async move {
                info!("Command scheduler task started (0.5ms resolution)");
                let mut itv = tokio::time::interval(Duration::from_micros(500));
                let mut tick: u64 = 0;
                let mut last_check = std::time::Instant::now();

                while *is_running.lock().await {
                    itv.tick().await;
                    tick += 1;

                    if tick % 2 == 0 {
                        let cmds = {
                            let mut sched = command_scheduler.lock().await;
                            let mut fm = fault_manager.lock().await;
                            sched.process_pending_commands(&mut *fm, &*network_manager, Some(&performance_tx)).await
                        };
                        for c in &cmds {
                            let urgent = (c.priority as u8) <= 1;
                            let _ = performance_tx.send(PerformanceEvent {
                                timestamp: Utc::now(),
                                event_type: if urgent { EventType::UrgentCommandDispatched } else { EventType::CommandDispatched },
                                duration_ms: 0.0,
                                metadata: {
                                    let mut m = std::collections::HashMap::new();
                                    m.insert("command_id".into(), c.command_id.clone());
                                    m.insert("command_type".into(), enum_s(&c.command_type));
                                    m.insert("priority".into(), (c.priority as u8).to_string());
                                    m.insert("target_system".into(), enum_s(&c.target_system));
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
                                error!("CRITICAL: {} ({:?}) {:.3}ms to deadline [tick:{}]", w.command_id, w.command_type, w.time_to_deadline_ms, tick);
                                let _ = performance_tx.send(PerformanceEvent {
                                    timestamp: Utc::now(),
                                    event_type: EventType::CommandDeadlineViolation,
                                    duration_ms: w.time_to_deadline_ms.abs(),
                                    metadata: {
                                        let mut m = std::collections::HashMap::new();
                                        m.insert("command_id".into(), w.command_id.clone());
                                        m.insert("command_type".into(), enum_s(&w.command_type));
                                        m.insert("priority".into(), (w.priority as u8).to_string());
                                        m.insert("target_system".into(), enum_s(&w.target_system));
                                        m.insert("severity".into(), "critical_emergency".into());
                                        m.insert("tick_precision".into(), "0.5ms".into());
                                        m
                                    },
                                }).await;
                            } else if w.time_to_deadline_ms < 0.5 {
                                error!("URGENT: {} ({:?}) {:.3}ms to deadline [tick:{}]", w.command_id, w.command_type, w.time_to_deadline_ms, tick);
                            } else if w.time_to_deadline_ms < 1.0 {
                                warn!("Deadline alert: {} ({:?}) {:.3}ms to deadline [tick:{}]", w.command_id, w.command_type, w.time_to_deadline_ms, tick);
                            }
                        }
                    }

                    if tick % 10 == 0 {
                        let sched = command_scheduler.lock().await;
                        let rep = sched.get_unified_deadline_report();
                        if rep.network_violations > 0 || rep.deadline_violations > 0 {
                            let dt = last_check.elapsed().as_millis();
                            warn!("Deadline violations in last {dt}ms: net={}/{} total={}/{} (net {:.1}%, overall {:.1}%)",
                                  rep.network_violations, rep.total_urgent_commands,
                                  rep.deadline_violations, rep.total_urgent_commands,
                                  rep.network_adherence_rate, rep.adherence_rate);
                            if rep.avg_network_send_time > 1.8 {
                                error!("Avg network send time approaching 2ms: {:.3}ms", rep.avg_network_send_time);
                            }
                        }
                        last_check = std::time::Instant::now();
                    }

                    if tick % 1000 == 0 {
                        let mut s = command_scheduler.lock().await;
                        s.cleanup_expired_commands().await;
                        s.update_safety_validation_cache().await;
                    }

                    if tick % 20000 == 0 {
                        let s = command_scheduler.lock().await;
                        let rep = s.get_unified_deadline_report();
                        let stats = s.get_enhanced_stats();
                        info!("=== CMD SCHED REPORT === net≤2ms {:.1}% ({} ok / {} urg) avg_net {:.3}ms overall {:.1}% viol={} trend={}",
                              rep.network_adherence_rate,
                              rep.total_urgent_commands - rep.network_violations,
                              rep.total_urgent_commands,
                              rep.avg_network_send_time,
                              rep.adherence_rate,
                              rep.deadline_violations,
                              rep.performance_trend);
                        debug!("scheduler precision: 0.5ms; uptime {:.3}s", tick as f64 * 0.0005);
                    }

                    if tick % 100 == 0 { tokio::task::yield_now().await; }
                }
            })
        };

        // Task 8: Simulation – inject an emergency packet periodically
        let _fault_simulator_task = {
            let telemetry_tx_sim = telemetry_tx.clone();
            tokio::spawn(async move {
                let mut fault_simulator = FaultSimulator::new();
                let mut itv = tokio::time::interval(Duration::from_secs(45));
                loop {
                    itv.tick().await;
                    let emergency = fault_simulator.create_random_fault();
                    let packet = fault_simulator.create_fault_packet(emergency);
                    let now = Utc::now();
                    if let Err(e) = telemetry_tx_sim.send((packet, now)).await {
                        error!("Failed to inject simulated fault packet: {e}");
                    } else {
                        info!("SIM: injected EmergencyAlert into telemetry path");
                    }
                }
            })
        };

        info!("All tasks started. Ground Control operational.");

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
        info!("Ground Control shutdown complete");
        Ok(())
    }

    pub async fn schedule_emergency_command_test(&self) -> Result<String> {
        let mut s = self.command_scheduler.lock().await;
        let cmd = shared_protocol::Command::thermal_emergency_response(1, 95.0);
        let id = s.schedule_command(cmd)?;
        info!("Emergency command scheduled for deadline testing: {id}");
        Ok(id)
    }

    pub async fn get_command_scheduler_metrics(&self) -> (EnhancedCommandSchedulerStats, UnifiedDeadlineReport) {
        let s = self.command_scheduler.lock().await;
        (s.get_enhanced_stats(), s.get_unified_deadline_report())
    }

    // keep validation simple & enum-safe (typed protocol already validates the rest)
    fn validate_packet_format(packet: &shared_protocol::CommunicationPacket) -> bool {
        !packet.header.packet_id.is_empty()
    }

    pub async fn shutdown(&self) {
        info!("Initiating graceful shutdown…");
        *self.is_running.lock().await = false;
        tokio::time::sleep(Duration::from_secs(1)).await;

        let perf = self.performance_tracker.lock().await;
        let final_stats = perf.get_current_stats();
        let drift_stats = self.network_manager.get_drift_stats().await;
        let fault_stats = self.fault_manager.lock().await.get_stats();

        info!("=== FINAL GROUND CONTROL STATS ===");
        info!("Runtime: {:?}", Utc::now() - self.system_start_time);
        info!("Packets processed: {}", final_stats.total_packets_processed);
        info!("Avg processing: {:.3}ms (p95 {:.3}ms, p99 {:.3}ms)", final_stats.avg_processing_time_ms, final_stats.p95_processing_time_ms, final_stats.p99_processing_time_ms);
        info!("3ms violations: {}", final_stats.processing_violations_3ms);
        info!("Packets received: {}", final_stats.total_packets_received);
        info!("Avg reception latency: {:.1}ms", final_stats.avg_reception_latency_ms);
        info!("Delayed packets: {} (avg delay {:.1}ms)", final_stats.delayed_packets_total, final_stats.avg_packet_delay_ms);
        info!("Reception drift violations: {}", final_stats.reception_drift_violations);
        info!("Retransmissions: {}", final_stats.retransmission_requests);
        info!("Network timeouts: {}", final_stats.network_timeouts);
        info!("Faults handled: {} | Critical active: {}", fault_stats.total_faults_detected, fault_stats.active_faults_count);
        info!("System health score: {:.1}/100 | Uptime {:.2}%", final_stats.system_health_score, final_stats.uptime_percentage);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                        GROUND CONTROL STATION STARTING                   ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!("Starting simulation...");

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

    info!("Ground Control Station starting… (Shared Protocol v{})", shared_protocol::PROTOCOL_VERSION);

    println!("┌─────────────────────────────────────────────────────────────────────────────┐");
    println!("│                      GROUND CONTROL SYSTEM INITIALIZED                     │");
    println!("└─────────────────────────────────────────────────────────────────────────────┘");

    let gcs = GroundControlSystem::new().await?;
    let perf = gcs.performance_tracker_handle();
    info!("Ground Control System initialized - All components ready");

    // system-load sampler (optional)
    {
        let perf_for_sampler = perf.clone();
        tokio::spawn(async move {
            monitor::start_system_load_sampler(perf_for_sampler, 1000).await;
        });
    }

    // lightweight heartbeat
    {
        let perf_for_report = perf.clone();
        tokio::spawn(async move {
            loop {
                {
                    let p = perf_for_report.lock().await;
                    let s = p.get_current_stats();
                    tracing::info!(
                        "SYSLOAD hb: CPU {:.1}% (avg {:.1}%, peak {:.1}%) | MEM {:.1}% (avg {:.1}%) | load1 {:.2}",
                        s.cpu_latest_percent, s.cpu_avg_percent, s.cpu_peak_percent,
                        s.mem_latest_percent, s.mem_avg_percent, s.load1_latest
                    );
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
    }

    // Ctrl+C → graceful shutdown
    let gcs_arc = Arc::new(gcs);
    {
        let g = Arc::clone(&gcs_arc);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.expect("Ctrl+C");
            println!("┌─────────────────────────────────────────────────────────────────────────────┐");
            println!("│                        GROUND CONTROL SHUTTING DOWN                        │");
            println!("└─────────────────────────────────────────────────────────────────────────────┘");
            info!("Shutdown signal received - Ground Control shutting down");
            g.shutdown().await;
            std::process::exit(0);
        });
    }

    println!("┌─────────────────────────────────────────────────────────────────────────────┐");
    println!("│                      GROUND CONTROL OPERATIONAL                            │");
    println!("└─────────────────────────────────────────────────────────────────────────────┘");
    info!("Ground Control Station operational - Monitoring satellite systems");

    gcs_arc.run().await
}
