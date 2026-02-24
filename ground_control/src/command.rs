// src/command.rs 

use std::collections::{VecDeque, HashMap};
use chrono::{DateTime, Utc};
use anyhow::{Result, anyhow};
use tracing::{info, warn};
use tokio::sync::mpsc;

use crate::fault::FaultManager;
use crate::network::NetworkManager;
use crate::performance::{PerformanceEvent, EventType};

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
    enqueued_at: DateTime<Utc>,
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
        let avg = if self.send_times_ms.is_empty() { 
            0.0 
        } else {
            self.send_times_ms.iter().sum::<f64>() / self.send_times_ms.len() as f64
        };
        
        EnhancedCommandSchedulerStats {
            queued: self.queue.len(),
            dispatched: self.dispatched,
            urgent_dispatched: self.urgent_dispatched,
            avg_send_time_ms: avg,
        }
    }

    pub fn get_unified_deadline_report(&self) -> UnifiedDeadlineReport {
        let avg = if self.send_times_ms.is_empty() { 
            0.0 
        } else {
            self.send_times_ms.iter().sum::<f64>() / self.send_times_ms.len() as f64
        };
        
        let adherence = if self.total_urgent == 0 { 
            100.0 
        } else { 
            100.0 * ((self.total_urgent.saturating_sub(self.deadline_violations)) as f64) / (self.total_urgent as f64) 
        };
        
        let net_adherence = if self.total_urgent == 0 { 
            100.0 
        } else { 
            100.0 * ((self.total_urgent.saturating_sub(self.network_violations)) as f64) / (self.total_urgent as f64) 
        };
        
        UnifiedDeadlineReport {
            total_urgent_commands: self.total_urgent,
            network_violations: self.network_violations,
            deadline_violations: self.deadline_violations,
            avg_network_send_time: avg,
            adherence_rate: adherence,
            network_adherence_rate: net_adherence,
            performance_trend: self.calculate_performance_trend(),
        }
    }

    /// Calculate performance trend based on recent send times
    fn calculate_performance_trend(&self) -> String {
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
    fn record_send_time(&mut self, send_time_ms: f64) {
        if self.send_times_ms.len() >= MAX_SEND_TIMES_HISTORY {
            self.send_times_ms.pop_front();
        }
        self.send_times_ms.push_back(send_time_ms);
    }

    pub async fn process_pending_commands(
        &mut self,
        _fault_manager: &mut FaultManager,
        network: &NetworkManager,
        perf_tx: Option<&mpsc::Sender<PerformanceEvent>>,
    ) -> Vec<Command> {
        let mut dispatched_now = Vec::new();
        let mut high_priority_failed = VecDeque::new();
        let mut low_priority_failed = VecDeque::new();

        // Process commands, separating failed ones by priority
        while let Some(mut scheduled) = self.queue.pop_front() {
            let urgent = (scheduled.command.priority as u8) <= 1;
            let deadline = scheduled.command.deadline;

            let send_res = network
                .send_packet_with_deadline_check(
                    CommunicationPacket::new_command(scheduled.command.clone(), Source::GroundControl),
                    urgent,
                    deadline,
                )
                .await;

            match send_res {
                Ok(sr) => {
                    // Use saturating arithmetic to prevent overflow
                    self.dispatched = self.dispatched.saturating_add(1);
                    
                    if urgent {
                        self.urgent_dispatched = self.urgent_dispatched.saturating_add(1);
                        self.total_urgent = self.total_urgent.saturating_add(1);
                        
                        if sr.send_time_ms > NETWORK_DEADLINE_THRESHOLD_MS { 
                            self.network_violations = self.network_violations.saturating_add(1);
                        }
                        if !sr.deadline_met { 
                            self.deadline_violations = self.deadline_violations.saturating_add(1);
                        }
                    }
                    
                    self.record_send_time(sr.send_time_ms);
                    dispatched_now.push(scheduled.command.clone());
                    
                    info!("Successfully dispatched command {}", scheduled.command.command_id);
                }
                Err(e) => {
                    // Increment retry count and handle failed commands
                    scheduled.retry_count += 1;
                    
                    // Log the error
                    warn!("Failed to dispatch command {} (retry {}): {}", 
                          scheduled.command.command_id, scheduled.retry_count, e);
                    
                    // Send performance event with proper error handling
                    if let Some(tx) = perf_tx {
                        let performance_event = PerformanceEvent {
                            timestamp: Utc::now(),
                            event_type: EventType::CommandDispatchError,
                            duration_ms: 0.0,
                            metadata: {
                                let mut m = HashMap::new();
                                m.insert("error".into(), e.to_string());
                                m.insert("command_id".into(), scheduled.command.command_id.clone());
                                m.insert("retry_count".into(), scheduled.retry_count.to_string());
                                m
                            },
                        };
                        
                        if let Err(send_err) = tx.send(performance_event).await {
                            warn!("Failed to send performance event: {}", send_err);
                        }
                    }
                    
                    // Separate failed commands by priority for better scheduling
                    if urgent {
                        high_priority_failed.push_back(scheduled);
                    } else {
                        low_priority_failed.push_back(scheduled);
                    }
                }
            }
        }
        
        // Rebuild queue with high priority failed commands first
        self.queue = high_priority_failed;
        self.queue.extend(low_priority_failed);
        
        dispatched_now
    }

    pub fn get_commands_approaching_deadline(&self) -> Vec<DeadlineWarning> {
        let now = Utc::now();
        self.queue.iter()
            .filter_map(|s| s.command.deadline.map(|dl| (s, dl)))
            .map(|(s, dl)| {
                // Handle potential negative time differences more robustly
                let ttd = match (dl - now).num_microseconds() {
                    Some(micros) if micros >= 0 => micros as f64 / 1000.0,
                    _ => 0.0, // Deadline has passed or calculation failed
                };
                
                DeadlineWarning {
                    command_id: s.command.command_id.clone(),
                    command_type: s.command.command_type.clone(),
                    priority: s.command.priority,
                    target_system: s.command.target_system.clone(),
                    time_to_deadline_ms: ttd,
                }
            })
            .collect()
    }

    pub async fn cleanup_expired_commands(&mut self) {
        let now = Utc::now();
        let initial_len = self.queue.len();
        
        self.queue.retain(|s| {
            s.command.deadline.map_or(true, |d| d > now)
        });
        
        let removed = initial_len - self.queue.len();
        if removed > 0 {
            info!("Cleaned up {} expired commands", removed);
        }
    }

    pub async fn update_safety_validation_cache(&mut self) { 
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
        if self.queue.iter().any(|s| s.command.command_id == command.command_id) {
            return Err(anyhow!("Command with ID {} is already scheduled", command.command_id));
        }
        
        let id = command.command_id.clone();
        let scheduled = Scheduled { 
            command, 
            enqueued_at: Utc::now(),
            retry_count: 0,
        };
        
        // Insert high priority commands at the front
        let is_urgent = (scheduled.command.priority as u8) <= 1;
        if is_urgent {
            self.queue.push_front(scheduled);
        } else {
            self.queue.push_back(scheduled);
        }
        
        info!("Scheduled command {} (urgent: {})", id, is_urgent);
        Ok(id)
    }
    
    /// Get queue statistics for monitoring
    pub fn get_queue_stats(&self) -> HashMap<String, u64> {
        let mut stats = HashMap::new();
        
        let (urgent, normal) = self.queue.iter().fold((0, 0), |(u, n), s| {
            if (s.command.priority as u8) <= 1 {
                (u + 1, n)
            } else {
                (u, n + 1)
            }
        });
        
        stats.insert("total_queued".to_string(), self.queue.len() as u64);
        stats.insert("urgent_queued".to_string(), urgent);
        stats.insert("normal_queued".to_string(), normal);
        stats.insert("total_dispatched".to_string(), self.dispatched);
        stats.insert("urgent_dispatched".to_string(), self.urgent_dispatched);
        
        stats
    }
}