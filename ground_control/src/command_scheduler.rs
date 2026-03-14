// src/command.rs 

use std::collections::{VecDeque, HashMap};
use chrono::{DateTime, Utc};
use anyhow::{Result, anyhow};
use tracing::{info, warn, error};
use tokio::sync::mpsc;

use crate::fault_management::FaultManager;
use crate::network_manager::NetworkManager;
use crate::performance_tracker::{PerformanceEvent, EventType};

use shared_protocol::{
    Command, CommunicationPacket, Source, Priority, CommandType, TargetSystem,
};

// Constants for configuration
const MAX_SEND_TIMES_HISTORY: usize = 1000;
const NETWORK_DEADLINE_THRESHOLD_MS: f64 = 2.0;

#[derive(Debug, Clone)]
pub struct EnhancedCommandSchedulerStats {
    pub queued: usize,
    pub dispatched: u64,
    pub urgent_dispatched: u64,
    pub avg_send_time_ms: f64,
}

#[derive(Debug, Clone)]
pub struct DeadlineWarning {
    pub command_id: String,
    pub command_type: CommandType,
    pub priority: Priority,
    pub target_system: TargetSystem,
    pub time_to_deadline_ms: f64,
}

#[derive(Debug, Clone)]
pub struct UnifiedDeadlineReport {
    pub total_urgent_commands: u64,
    pub network_violations: u64,
    pub deadline_violations: u64,
    pub avg_network_send_time: f64,
    pub adherence_rate: f64,
    pub network_adherence_rate: f64,
    pub performance_trend: String,
}

#[derive(Clone)]
#[derive(Debug)]
struct Scheduled {
    command: Command,
    _enqueued_at: DateTime<Utc>,
    retry_count: u32,
}

#[derive(Debug)]
pub struct CommandScheduler {
    queue: VecDeque<Scheduled>,
    dispatched: u64,
    urgent_dispatched: u64,
    send_times_ms: VecDeque<f64>, // Changed to VecDeque for efficient removal
    network_violations: u64,
    deadline_violations: u64,
    total_urgent: u64,
}

impl CommandScheduler {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            dispatched: 0,
            urgent_dispatched: 0,
            send_times_ms: VecDeque::with_capacity(MAX_SEND_TIMES_HISTORY),
            network_violations: 0,
            deadline_violations: 0,
            total_urgent: 0,
        }
    }

    pub fn get_enhanced_stats(&self) -> EnhancedCommandSchedulerStats {
        let avg_send_time_ms = if self.send_times_ms.is_empty() { 
            0.0 
        } else {
            self.send_times_ms.iter().sum::<f64>() / self.send_times_ms.len() as f64
        };
        
        EnhancedCommandSchedulerStats {
            queued: self.queue.len(),
            dispatched: self.dispatched,
            urgent_dispatched: self.urgent_dispatched,
            avg_send_time_ms,
        }
    }

    pub fn get_unified_deadline_report(&self) -> UnifiedDeadlineReport {
        let avg_network_send_time = if self.send_times_ms.is_empty() { 
            0.0 
        } else {
            self.send_times_ms.iter().sum::<f64>() / self.send_times_ms.len() as f64
        };
        
        let adherence_rate = if self.total_urgent == 0 { 
            100.0 
        } else { 
            100.0 * ((self.total_urgent.saturating_sub(self.deadline_violations)) as f64) / (self.total_urgent as f64) 
        };
        
        let network_adherence_rate = if self.total_urgent == 0 { 
            100.0 
        } else { 
            100.0 * ((self.total_urgent.saturating_sub(self.network_violations)) as f64) / (self.total_urgent as f64) 
        };
        
        UnifiedDeadlineReport {
            total_urgent_commands: self.total_urgent,
            network_violations: self.network_violations,
            deadline_violations: self.deadline_violations,
            avg_network_send_time,
            adherence_rate,
            network_adherence_rate,
            performance_trend: self.evaluate_performance_trend(),
        }
    }

    /// Calculate performance trend based on recent send times
    fn evaluate_performance_trend(&self) -> String {
        if self.send_times_ms.len() < 10 {
            return "insufficient_data".to_string();
        }
        
        let recent_half = self.send_times_ms.len() / 2;
        let recent_avg: f64 = self.send_times_ms.iter().skip(recent_half).sum::<f64>() 
            / (self.send_times_ms.len() - recent_half) as f64;
        let older_avg: f64 = self.send_times_ms.iter().take(recent_half).sum::<f64>() 
            / recent_half as f64;
        
        let improvement = (older_avg - recent_avg) / older_avg * 100.0;
        
        if improvement > 5.0 {
            "improving".to_string()
        } else if improvement < -5.0 {
            "degrading".to_string()
        } else {
            "stable".to_string()
        }
    }

    /// Add send time to history with bounded size
    fn record_send_time_sample(&mut self, send_time_ms: f64) {
        if self.send_times_ms.len() >= MAX_SEND_TIMES_HISTORY {
            self.send_times_ms.pop_front();
        }
        self.send_times_ms.push_back(send_time_ms);
    }

    /// Map a `CommandType` to the interlock category string used in `FaultManager`.
    /// These strings must match the `blocked_command_types` used when activating safety interlocks.
    fn map_command_type_to_interlock_category(ct: CommandType) -> &'static str {
        match ct {
            CommandType::ThermalControl  => "heating",
            CommandType::PowerControl    => "high_power",
            CommandType::AttitudeControl => "precise_maneuver",
            CommandType::Diagnostic      => "cpu_intensive_tasks",
            CommandType::Maintenance     => "non_essential",
            CommandType::DataRequest     => "payload_activation",
            // Emergency and Recovery commands must never be blocked by interlocks.
            CommandType::Emergency       => "emergency_bypass",
            CommandType::Recovery        => "recovery_bypass",
        }
    }

    /// Map a `TargetSystem` to the interlock system string used in `FaultManager`.
    fn map_target_system_to_interlock_category(ts: TargetSystem) -> &'static str {
        match ts {
            TargetSystem::ThermalManagement => "thermal_management",
            TargetSystem::PowerManagement   => "power_management",
            TargetSystem::AttitudeControl   => "attitude_control",
            TargetSystem::AllSystems        => "all_systems",
        }
    }

    pub async fn process_dispatch_queue(
        &mut self,
        fault_manager: &mut FaultManager,
        network: &NetworkManager,
        perf_tx: Option<&mpsc::Sender<PerformanceEvent>>,
    ) -> Vec<Command> {
        let mut dispatched_commands = Vec::new();
        let mut urgent_retry_queue = VecDeque::new();
        let mut normal_retry_queue = VecDeque::new();

        // Process commands, separating failed ones by priority
        while let Some(mut scheduled) = self.queue.pop_front() {
            let urgent = (scheduled.command.priority as u8) <= 1;
            let deadline = scheduled.command.deadline;

            // ── Safety interlock check ──────────────────────────────────────
            // Emergency / Recovery commands bypass interlocks so we can always
            // send critical responses regardless of active faults.
            let bypasses_interlocks = matches!(
                scheduled.command.command_type,
                CommandType::Emergency | CommandType::Recovery
            );

            if !bypasses_interlocks {
                let cmd_cat = Self::map_command_type_to_interlock_category(scheduled.command.command_type);
                let sys_cat = Self::map_target_system_to_interlock_category(scheduled.command.target_system);
                let submit_time = Utc::now();

                let (blocked, reasons, block_event) = fault_manager.is_command_blocked(
                    cmd_cat,
                    sys_cat,
                    &scheduled.command.command_id,
                    submit_time,
                );

                if blocked {
                    let reason_str = reasons.join("; ");
                    warn!(
                        "COMMAND REJECTED — id={} type={:?} target={:?} | reason: {}",
                        scheduled.command.command_id,
                        scheduled.command.command_type,
                        scheduled.command.target_system,
                        reason_str
                    );

                    if let Some(be) = block_event {
                        fault_manager.record_command_block_event(be);
                    }

                    if let Some(tx) = perf_tx {
                        let _ = tx.send(PerformanceEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::CommandValidationFailed,
                            duration_ms: 0.0,
                            metadata: {
                                let mut m = HashMap::new();
                                m.insert("command_id".into(), scheduled.command.command_id.clone());
                                m.insert("command_type".into(), format!("{:?}", scheduled.command.command_type));
                                m.insert("target_system".into(), format!("{:?}", scheduled.command.target_system));
                                m.insert("rejection_reason".into(), reason_str);
                                m.insert("interlock_active".into(), "true".into());
                                m
                            },
                        }).await;
                    }

                    // Discard the blocked command (do not requeue).
                    error!(
                        "Command {} discarded due to active safety interlock",
                        scheduled.command.command_id
                    );
                    continue;
                }
            }
            // ───────────────────────────────────────────────────────────────

            let send_result = network
                .send_packet_with_deadline_guard(
                    CommunicationPacket::new_command(scheduled.command.clone(), Source::GroundControl),
                    urgent,
                    deadline,
                )
                .await;

            match send_result {
                Ok(send_result) => {
                    // Use saturating arithmetic to prevent overflow
                    self.dispatched = self.dispatched.saturating_add(1);
                    
                    if urgent {
                        self.urgent_dispatched = self.urgent_dispatched.saturating_add(1);
                        self.total_urgent = self.total_urgent.saturating_add(1);
                        
                        if send_result.send_time_ms > NETWORK_DEADLINE_THRESHOLD_MS { 
                            self.network_violations = self.network_violations.saturating_add(1);
                        }
                        if !send_result.deadline_met { 
                            self.deadline_violations = self.deadline_violations.saturating_add(1);
                        }
                    }
                    
                    self.record_send_time_sample(send_result.send_time_ms);
                    dispatched_commands.push(scheduled.command.clone());
                    
                    info!("Command Dispatched Successfully: {}", scheduled.command.command_id);
                }
                Err(dispatch_error) => {
                    // Increment retry count and handle failed commands
                    scheduled.retry_count += 1;
                    
                    // Log the error
                      warn!("Command Dispatch Failed {} (Retry {}): {}", 
                          scheduled.command.command_id, scheduled.retry_count, dispatch_error);
                    
                    // Send performance event with proper error handling
                    if let Some(performance_tx) = perf_tx {
                        let performance_event = PerformanceEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::CommandDispatchError,
                            duration_ms: 0.0,
                            metadata: {
                                let mut metadata = HashMap::new();
                                metadata.insert("error".into(), dispatch_error.to_string());
                                metadata.insert("command_id".into(), scheduled.command.command_id.clone());
                                metadata.insert("retry_count".into(), scheduled.retry_count.to_string());
                                metadata
                            },
                        };
                        
                        if let Err(perf_send_error) = performance_tx.send(performance_event).await {
                            warn!("Performance Event Channel Send Failed: {}", perf_send_error);
                        }
                    }
                    
                    // Separate failed commands by priority for better scheduling
                    if urgent {
                        urgent_retry_queue.push_back(scheduled);
                    } else {
                        normal_retry_queue.push_back(scheduled);
                    }
                }
            }
        }
        
        // Rebuild queue with high priority failed commands first
        self.queue = urgent_retry_queue;
        self.queue.extend(normal_retry_queue);
        
        dispatched_commands
    }

    pub fn get_commands_approaching_deadline(&self) -> Vec<DeadlineWarning> {
        let now = Utc::now();
        self.queue.iter()
            .filter_map(|scheduled| scheduled.command.deadline.map(|deadline| (scheduled, deadline)))
            .map(|(scheduled, deadline)| {
                // Handle potential negative time differences more robustly
                let time_to_deadline_ms = match (deadline - now).num_microseconds() {
                    Some(micros) if micros >= 0 => micros as f64 / 1000.0,
                    _ => 0.0, // Deadline has passed or calculation failed
                };
                
                DeadlineWarning {
                    command_id: scheduled.command.command_id.clone(),
                    command_type: scheduled.command.command_type.clone(),
                    priority: scheduled.command.priority,
                    target_system: scheduled.command.target_system.clone(),
                    time_to_deadline_ms,
                }
            })
            .collect()
    }

    pub async fn prune_expired_commands(&mut self) {
        let now = Utc::now();
        let initial_queue_len = self.queue.len();
        
        self.queue.retain(|scheduled| {
            scheduled.command.deadline.map_or(true, |deadline| deadline > now)
        });
        
        let removed_count = initial_queue_len - self.queue.len();
        if removed_count > 0 {
            info!("Pruned {} Expired Commands", removed_count);
        }
    }

    pub async fn refresh_safety_validation_cache(&mut self) {
        // Hook for future safety validation implementation
        // This could validate command parameters, check system states, etc.
    }

    pub fn schedule_command(&mut self, command: Command) -> Result<String> {
        // Validate command deadline
        if let Some(deadline) = command.deadline {
            if deadline <= Utc::now() {
                return Err(anyhow!("Command deadline is in the past: {}", deadline));
            }
        }
        
        // Validate command ID is not empty
        if command.command_id.trim().is_empty() {
            return Err(anyhow!("Command ID cannot be empty"));
        }
        
        // Check for duplicate command IDs in queue
        if self.queue.iter().any(|scheduled| scheduled.command.command_id == command.command_id) {
            return Err(anyhow!("Command with ID {} is already scheduled", command.command_id));
        }
        
        let scheduled_command_id = command.command_id.clone();
        let scheduled = Scheduled { 
            command, 
            _enqueued_at: Utc::now(),
            retry_count: 0,
        };
        
        // Insert high priority commands at the front
        let is_urgent = (scheduled.command.priority as u8) <= 1;
        if is_urgent {
            self.queue.push_front(scheduled);
        } else {
            self.queue.push_back(scheduled);
        }
        
        info!("Command {} Scheduled (Urgent: {})", scheduled_command_id, is_urgent);
        Ok(scheduled_command_id)
    }
    
}