use std::collections::{BinaryHeap, VecDeque, HashMap};
use std::cmp::Reverse;
use chrono::{DateTime, Utc, Duration};
use anyhow::{Result, anyhow};
use tracing::{info, warn, error};
use tokio::sync::mpsc;

use crate::fault_management::FaultManager;
use crate::network_manager::NetworkManager;
use crate::performance_tracker::{PerformanceEvent, EventType};

use shared_protocol::{
    Command, CommunicationPacket, Source, Priority, CommandType, TargetSystem,
};

// ── Constants ────────────────────────────────────────────────────────────────

const MAX_SEND_TIMES_HISTORY: usize = 1000;
const NETWORK_DEADLINE_THRESHOLD_MS: f64 = 2.0;

/// Warn if a command is scheduled_at within this many ms of its deadline.
const NEAR_DEADLINE_WARN_MS: f64 = 5.0;

/// If actual dispatch lags scheduled_at by more than this, record a precision
/// violation and emit SchedulerPrecisionViolation to the performance tracker.
const SCHEDULE_PRECISION_THRESHOLD_MS: f64 = 2.0;

const DEADLINE_LOG_PATH: &str = "logs/ground_control_deadline_ops.csv";
const MISSED_DEADLINE_LOG_PATH: &str = "logs/ground_control_missed_deadlines.csv";

// ── Public stats types ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EnhancedCommandSchedulerStats {
    pub queued: usize,
    pub pending_scheduled: usize,
    pub dispatched: u64,
    pub urgent_dispatched: u64,
    pub commands_promoted: u64,
    pub avg_send_time_ms: f64,
    pub avg_schedule_precision_ms: f64,
    pub p95_schedule_precision_ms: f64,
    pub schedule_precision_violations: u64,
    pub commands_rejected_by_interlock: u64,
    pub commands_requeued_after_release: u64,
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
    pub avg_schedule_precision_ms: f64,
    pub p95_schedule_precision_ms: f64,
    pub schedule_precision_violations: u64,
}

// ── Internal types ───────────────────────────────────────────────────────────

/// A command in the ready-to-dispatch queue.
#[derive(Clone, Debug)]
struct Scheduled {
    command: Command,
    enqueued_at: DateTime<Utc>,
    /// When this command was originally planned to run.
    /// ASAP commands get scheduled_at = enqueued_at; precision is measured
    /// against this field at actual dispatch time.
    scheduled_at: DateTime<Utc>,
    retry_count: u32,
}

/// An entry sitting in the future pending schedule (not yet ready to dispatch).
/// Wrapped in Reverse<> when pushed into the BinaryHeap so the heap gives the
/// *earliest* scheduled_at first (min-heap behaviour).
#[derive(Debug, Clone)]
struct PendingEntry {
    scheduled_at: DateTime<Utc>,
    scheduled: Scheduled,
}

impl PartialEq for PendingEntry {
    fn eq(&self, other: &Self) -> bool {
        self.scheduled_at == other.scheduled_at
            && self.scheduled.command.command_id == other.scheduled.command.command_id
    }
}
impl Eq for PendingEntry {}

impl PartialOrd for PendingEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Natural ascending order; BinaryHeap + Reverse gives min-heap (earliest first).
        self.scheduled_at.cmp(&other.scheduled_at)
    }
}

/// A command held back by an active interlock, waiting for release.
#[derive(Debug, Clone)]
struct InterlockRetryEntry {
    scheduled: Scheduled,
    blocking_interlock_id: String,
}

// ── CommandScheduler ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CommandScheduler {
    // Three-tier queue structure:
    //   pending_schedule  → future commands, not yet ready
    //   queue             → ready to dispatch now (priority-ordered)
    //   interlock_retry_queue → blocked by active interlock
    pending_schedule: BinaryHeap<Reverse<PendingEntry>>,
    queue: VecDeque<Scheduled>,
    interlock_retry_queue: Vec<InterlockRetryEntry>,

    dispatched: u64,
    urgent_dispatched: u64,
    network_violations: u64,
    deadline_violations: u64,
    total_urgent: u64,
    pub commands_rejected_by_interlock: u64,
    pub commands_requeued_after_release: u64,
    pub commands_promoted: u64,
    pub schedule_precision_violations: u64,

    send_times_ms: VecDeque<f64>,
    /// Absolute lag between scheduled_at and actual dispatch time (ms), one
    /// sample per dispatched command.
    schedule_precision_samples: VecDeque<f64>,
}

impl CommandScheduler {
    pub fn new() -> Self {
        Self::initialize_deadline_operations_csv();
        Self::initialize_missed_deadline_csv();
        Self {
            pending_schedule: BinaryHeap::new(),
            queue: VecDeque::new(),
            interlock_retry_queue: Vec::new(),
            dispatched: 0,
            urgent_dispatched: 0,
            network_violations: 0,
            deadline_violations: 0,
            total_urgent: 0,
            commands_rejected_by_interlock: 0,
            commands_requeued_after_release: 0,
            commands_promoted: 0,
            schedule_precision_violations: 0,
            send_times_ms: VecDeque::with_capacity(MAX_SEND_TIMES_HISTORY),
            schedule_precision_samples: VecDeque::with_capacity(MAX_SEND_TIMES_HISTORY),
        }
    }

    // ── Public scheduling API ────────────────────────────────────────────────

    /// Schedule a command for **immediate dispatch** (ASAP).
    /// The command enters the dispatch queue directly with scheduled_at = now.
    pub fn schedule_command(&mut self, command: Command) -> Result<String> {
        self.validate_command(&command)?;
        let command_id = command.command_id.clone();
        let now = Utc::now();
        self.insert_into_dispatch_queue(Scheduled {
            command,
            enqueued_at: now,
            scheduled_at: now,
            retry_count: 0,
        });
        info!("Command {} scheduled for immediate dispatch", command_id);
        Ok(command_id)
    }

    /// Schedule a command to dispatch at a **specific UTC wall-clock time**.
    ///
    /// If `at` is already in the past the command is promoted to the dispatch
    /// queue immediately (late-start ASAP). A warning is logged if `at` leaves
    /// less than NEAR_DEADLINE_WARN_MS of headroom before the command deadline.
    pub fn schedule_command_at(&mut self, command: Command, at: DateTime<Utc>) -> Result<String> {
        self.validate_command(&command)?;
        let command_id = command.command_id.clone();
        let now = Utc::now();

        if let Some(deadline) = command.deadline {
            let headroom_ms = (deadline - at).num_microseconds().unwrap_or(0) as f64 / 1000.0;
            if headroom_ms < NEAR_DEADLINE_WARN_MS {
                warn!(
                    "Command {} scheduled_at is only {:.2}ms before its deadline — risk of miss",
                    command_id, headroom_ms
                );
            }
        }

        let scheduled = Scheduled {
            command,
            enqueued_at: now,
            scheduled_at: at,
            retry_count: 0,
        };

        if at <= now {
            // Already due — promote immediately.
            self.insert_into_dispatch_queue(scheduled);
            info!("Command {} at is past — queued immediately", command_id);
        } else {
            self.pending_schedule
                .push(Reverse(PendingEntry { scheduled_at: at, scheduled }));
            info!(
                "Command {} pending until {}",
                command_id,
                at.format("%H:%M:%S%.3f")
            );
        }

        Ok(command_id)
    }

    /// Schedule a command to dispatch **delay_ms milliseconds from now**.
    pub fn schedule_command_relative(&mut self, command: Command, delay_ms: f64) -> Result<String> {
        let at = Utc::now() + Duration::microseconds((delay_ms * 1000.0) as i64);
        self.schedule_command_at(command, at)
    }

    /// Number of commands in the future pending schedule.
    pub fn pending_count(&self) -> usize {
        self.pending_schedule.len()
    }

    /// Wall-clock time of the next pending dispatch, if any.
    pub fn next_scheduled_at(&self) -> Option<DateTime<Utc>> {
        self.pending_schedule
            .peek()
            .map(|Reverse(e)| e.scheduled_at)
    }

    // ── Stats ────────────────────────────────────────────────────────────────

    pub fn get_enhanced_stats(&self) -> EnhancedCommandSchedulerStats {
        EnhancedCommandSchedulerStats {
            queued: self.queue.len(),
            pending_scheduled: self.pending_schedule.len(),
            dispatched: self.dispatched,
            urgent_dispatched: self.urgent_dispatched,
            commands_promoted: self.commands_promoted,
            avg_send_time_ms: Self::mean_of(&self.send_times_ms),
            avg_schedule_precision_ms: Self::mean_of(&self.schedule_precision_samples),
            p95_schedule_precision_ms: Self::percentile_of(&self.schedule_precision_samples, 95.0),
            schedule_precision_violations: self.schedule_precision_violations,
            commands_rejected_by_interlock: self.commands_rejected_by_interlock,
            commands_requeued_after_release: self.commands_requeued_after_release,
        }
    }

    pub fn get_unified_deadline_report(&self) -> UnifiedDeadlineReport {
        let adherence_rate = if self.total_urgent == 0 { 100.0 } else {
            100.0 * self.total_urgent.saturating_sub(self.deadline_violations) as f64
                / self.total_urgent as f64
        };
        let network_adherence_rate = if self.total_urgent == 0 { 100.0 } else {
            100.0 * self.total_urgent.saturating_sub(self.network_violations) as f64
                / self.total_urgent as f64
        };
        UnifiedDeadlineReport {
            total_urgent_commands: self.total_urgent,
            network_violations: self.network_violations,
            deadline_violations: self.deadline_violations,
            avg_network_send_time: Self::mean_of(&self.send_times_ms),
            adherence_rate,
            network_adherence_rate,
            performance_trend: self.evaluate_performance_trend(),
            avg_schedule_precision_ms: Self::mean_of(&self.schedule_precision_samples),
            p95_schedule_precision_ms: Self::percentile_of(&self.schedule_precision_samples, 95.0),
            schedule_precision_violations: self.schedule_precision_violations,
        }
    }

    // ── Core dispatch cycle ──────────────────────────────────────────────────

    /// Called every 0.5ms tick.
    ///
    /// Flow:
    ///   1. Promote pending commands whose scheduled_at ≤ now.
    ///   2. Requeue commands released by interlocks since the last cycle.
    ///   3. Dispatch the ready queue, measuring precision per command.
    pub async fn process_dispatch_queue(
        &mut self,
        fault_manager: &mut FaultManager,
        network: &NetworkManager,
        perf_tx: Option<&mpsc::Sender<PerformanceEvent>>,
    ) -> Vec<Command> {
        // 1. Pending → ready.
        self.promote_pending_commands(perf_tx).await;

        // 2. Released interlocks → requeue held commands.
        let released = fault_manager.drain_released_interlock_ids();
        self.requeue_released_commands(released);

        let mut dispatched_commands = Vec::new();
        let mut urgent_retry = VecDeque::new();
        let mut normal_retry = VecDeque::new();

        while let Some(mut scheduled) = self.queue.pop_front() {
            let urgent = (scheduled.command.priority as u8) <= 1;
            let deadline = scheduled.command.deadline;

            // Safety interlock check — Emergency/Recovery always bypass.
            let bypasses = matches!(
                scheduled.command.command_type,
                CommandType::Emergency | CommandType::Recovery
            );

            if !bypasses {
                let cmd_cat = Self::map_cmd_category(scheduled.command.command_type);
                let sys_cat = Self::map_sys_category(scheduled.command.target_system);
                let submit_time = Utc::now();

                let (blocked, reasons, block_event) = fault_manager.is_command_blocked(
                    cmd_cat, sys_cat,
                    &scheduled.command.command_id,
                    submit_time,
                );

                if blocked {
                    let reason_str = reasons.join("; ");
                    warn!(
                        "COMMAND REJECTED — id={} type={:?} target={:?} | {}",
                        scheduled.command.command_id,
                        scheduled.command.command_type,
                        scheduled.command.target_system,
                        reason_str
                    );

                    let blocking_id = block_event
                        .as_ref()
                        .map(|be| be.blocking_interlock_id.clone())
                        .unwrap_or_default();

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

                    let deadline_ok = scheduled.command.deadline
                        .map(|d| d > Utc::now())
                        .unwrap_or(true);

                    if deadline_ok && !blocking_id.is_empty() {
                        self.interlock_retry_queue.push(InterlockRetryEntry {
                            scheduled,
                            blocking_interlock_id: blocking_id,
                        });
                    } else {
                        error!(
                            "Command {} discarded — interlock active and deadline expired",
                            scheduled.command.command_id
                        );
                    }
                    self.commands_rejected_by_interlock += 1;
                    continue;
                }
            }

            // ── Precision measurement ─────────────────────────────────────
            // Record how late we are relative to when this command was
            // supposed to run.  For ASAP commands this is near-zero; for
            // timed commands it captures real-time scheduling error.
            let dispatch_now = Utc::now();
            let precision_ms = (dispatch_now - scheduled.scheduled_at)
                .num_microseconds()
                .unwrap_or(0)
                .max(0) as f64
                / 1000.0;

            self.record_precision_sample(precision_ms);

            if precision_ms > SCHEDULE_PRECISION_THRESHOLD_MS {
                self.schedule_precision_violations += 1;
                if let Some(tx) = perf_tx {
                    let _ = tx.send(PerformanceEvent {
                        timestamp: dispatch_now,
                        event_type: EventType::SchedulerPrecisionViolation,
                        duration_ms: precision_ms,
                        metadata: {
                            let mut m = HashMap::new();
                            m.insert("phase".into(), "dispatch".into());
                            m.insert("command_id".into(), scheduled.command.command_id.clone());
                            m.insert("scheduled_at".into(), scheduled.scheduled_at.to_rfc3339());
                            m.insert("precision_ms".into(), format!("{:.3}", precision_ms));
                            m.insert("threshold_ms".into(), format!("{:.1}", SCHEDULE_PRECISION_THRESHOLD_MS));
                            m
                        },
                    }).await;
                }
            }

            // We'll emit the combined latency after the network send below

            // ── Network send ──────────────────────────────────────────────
            let send_result = network
                .send_packet_with_deadline_guard(
                    CommunicationPacket::new_command(
                        scheduled.command.clone(),
                        Source::GroundControl,
                    ),
                    urgent,
                    deadline,
                )
                .await;

            match send_result {
                Ok(sr) => {
                    let combined_latency_ms = precision_ms + sr.send_time_ms;
                    let network_met = sr.send_time_ms <= NETWORK_DEADLINE_THRESHOLD_MS;
                    let deadline_met = sr.success && sr.deadline_met;
                    let adherent = sr.success && network_met && deadline_met;
                    let mut reasons = Vec::new();

                    if urgent { self.total_urgent = self.total_urgent.saturating_add(1); }
                    if !sr.success          { reasons.push("dispatch_rejected_past_deadline"); }
                    if !network_met         { reasons.push("network_send_over_2ms"); self.network_violations += 1; }
                    if !deadline_met        { reasons.push("deadline_missed"); self.deadline_violations += 1; }

                    let reason = if reasons.is_empty() { "ok".into() } else { reasons.join(";") };

                    Self::append_deadline_operation_to_csv(
                        &scheduled.command, urgent, sr.send_time_ms,
                        deadline_met, network_met, adherent,
                        sr.deadline_violation_ms, precision_ms, &reason,
                    );
                    if !deadline_met {
                        Self::append_missed_deadline_to_csv(
                            &scheduled.command, sr.send_time_ms, sr.deadline_violation_ms, &reason,
                        );
                    }

                    if let Some(tx) = perf_tx {
                        if !network_met {
                            let _ = tx.send(PerformanceEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::NetworkDeadlineViolation,
                                duration_ms: sr.send_time_ms,
                                metadata: {
                                    let mut m = HashMap::new();
                                    m.insert("command_id".into(), scheduled.command.command_id.clone());
                                    m.insert("send_time_ms".into(), format!("{:.3}", sr.send_time_ms));
                                    m.insert("threshold_ms".into(), format!("{:.1}", NETWORK_DEADLINE_THRESHOLD_MS));
                                    m
                                },
                            }).await;
                        }
                        if !deadline_met {
                            let _ = tx.send(PerformanceEvent {
                                timestamp: Utc::now(),
                                event_type: EventType::CommandDeadlineViolation,
                                duration_ms: sr.deadline_violation_ms,
                                metadata: {
                                    let mut m = HashMap::new();
                                    m.insert("command_id".into(), scheduled.command.command_id.clone());
                                    m.insert("deadline_violation_ms".into(), format!("{:.3}", sr.deadline_violation_ms));
                                    m
                                },
                            }).await;
                        }
                    }

                    if !sr.success {
                        error!(
                            "Command {} missed deadline before dispatch (violation {:.3}ms)",
                            scheduled.command.command_id, sr.deadline_violation_ms
                        );
                        continue;
                    }

                    self.dispatched = self.dispatched.saturating_add(1);
                    if urgent { self.urgent_dispatched = self.urgent_dispatched.saturating_add(1); }
                    self.record_send_time_sample(sr.send_time_ms);
                    dispatched_commands.push(scheduled.command.clone());

                    info!(
                        "Dispatched {} (precision: {:.3}ms, send: {:.3}ms, total_latency: {:.3}ms)",
                        scheduled.command.command_id, precision_ms, sr.send_time_ms, combined_latency_ms
                    );

                    // Emit combined latency as CommandDispatched event
                    if let Some(tx) = perf_tx {
                        let _ = tx.send(PerformanceEvent {
                            timestamp: dispatch_now,
                            event_type: EventType::CommandDispatched,
                            duration_ms: combined_latency_ms,
                            metadata: {
                                let mut m = HashMap::new();
                                m.insert("command_id".into(), scheduled.command.command_id.clone());
                                m.insert("scheduled_at".into(), scheduled.scheduled_at.to_rfc3339());
                                m.insert("dispatched_at".into(), dispatch_now.to_rfc3339());
                                m.insert("precision_ms".into(), format!("{:.3}", precision_ms));
                                m.insert("send_time_ms".into(), format!("{:.3}", sr.send_time_ms));
                                m.insert("total_latency_ms".into(), format!("{:.3}", combined_latency_ms));
                                m
                            },
                        }).await;
                    }
                }
                Err(e) => {
                    scheduled.retry_count += 1;
                    warn!(
                        "Dispatch failed {} (retry {}): {}",
                        scheduled.command.command_id, scheduled.retry_count, e
                    );
                    if let Some(tx) = perf_tx {
                        let _ = tx.send(PerformanceEvent {
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
                        }).await;
                    }
                    if urgent { urgent_retry.push_back(scheduled); }
                    else      { normal_retry.push_back(scheduled); }
                }
            }
        }

        // Rebuild retry queue: urgent first.
        self.queue = urgent_retry;
        self.queue.extend(normal_retry);

        dispatched_commands
    }

    // ── Deadline proximity ────────────────────────────────────────────────────

    pub fn get_commands_approaching_deadline(&self) -> Vec<DeadlineWarning> {
        let now = Utc::now();
        let from_queue   = self.queue.iter().map(|s| &s.command);
        let from_pending = self.pending_schedule.iter().map(|Reverse(e)| &e.scheduled.command);

        from_queue.chain(from_pending)
            .filter_map(|cmd| cmd.deadline.map(|dl| (cmd, dl)))
            .map(|(cmd, dl)| DeadlineWarning {
                command_id: cmd.command_id.clone(),
                command_type: cmd.command_type,
                priority: cmd.priority,
                target_system: cmd.target_system,
                time_to_deadline_ms: (dl - now).num_microseconds()
                    .map(|us| (us.max(0)) as f64 / 1000.0)
                    .unwrap_or(0.0),
            })
            .collect()
    }

    // ── Pruning ───────────────────────────────────────────────────────────────

    pub async fn prune_expired_commands(&mut self) {
        let now = Utc::now();
        let q0 = self.queue.len();
        let p0 = self.pending_schedule.len();
        let r0 = self.interlock_retry_queue.len();

        self.queue.retain(|s| s.command.deadline.map_or(true, |d| d > now));

        let fresh: Vec<_> = std::mem::take(&mut self.pending_schedule)
            .into_iter()
            .filter(|Reverse(e)| e.scheduled.command.deadline.map_or(true, |d| d > now))
            .collect();
        self.pending_schedule = fresh.into_iter().collect();

        self.interlock_retry_queue.retain(|e| {
            e.scheduled.command.deadline.map_or(true, |d| {
                if d <= now {
                    warn!("Interlock-held command {} expired during prune", e.scheduled.command.command_id);
                    false
                } else { true }
            })
        });

        let removed = (q0 - self.queue.len())
            + (p0 - self.pending_schedule.len())
            + (r0 - self.interlock_retry_queue.len());

        if removed > 0 {
            info!(
                "Pruned {} expired commands ({} dispatch, {} pending, {} interlock hold)",
                removed,
                q0 - self.queue.len(),
                p0 - self.pending_schedule.len(),
                r0 - self.interlock_retry_queue.len(),
            );
        }
    }

    pub async fn refresh_safety_validation_cache(&mut self) {}

    // ── Private: pending promotion ───────────────────────────────────────────

    /// Drain all pending entries whose scheduled_at ≤ now into the dispatch queue.
    /// Emits SchedulerPrecisionViolation and TaskExecutionDrift for any late promotions.
    async fn promote_pending_commands(
        &mut self,
        perf_tx: Option<&mpsc::Sender<PerformanceEvent>>,
    ) {
        let now = Utc::now();

        loop {
            match self.pending_schedule.peek() {
                Some(Reverse(e)) if e.scheduled_at <= now => {}
                _ => break, // nothing due yet
            }

            let Reverse(entry) = self.pending_schedule.pop().unwrap();

            let lateness_ms = (now - entry.scheduled_at)
                .num_microseconds()
                .unwrap_or(0)
                .max(0) as f64
                / 1000.0;

            if lateness_ms > SCHEDULE_PRECISION_THRESHOLD_MS {
                warn!(
                    "Command {} promoted {:.3}ms late (was scheduled for {})",
                    entry.scheduled.command.command_id,
                    lateness_ms,
                    entry.scheduled_at.format("%H:%M:%S%.3f"),
                );
                if let Some(tx) = perf_tx {
                    let _ = tx.send(PerformanceEvent {
                        timestamp: now,
                        event_type: EventType::SchedulerPrecisionViolation,
                        duration_ms: lateness_ms,
                        metadata: {
                            let mut m = HashMap::new();
                            m.insert("phase".into(), "promotion".into());
                            m.insert("command_id".into(), entry.scheduled.command.command_id.clone());
                            m.insert("lateness_ms".into(), format!("{:.3}", lateness_ms));
                            m
                        },
                    }).await;
                    let _ = tx.send(PerformanceEvent {
                        timestamp: now,
                        event_type: EventType::TaskExecutionDrift,
                        duration_ms: lateness_ms,
                        metadata: {
                            let mut m = HashMap::new();
                            m.insert("source".into(), "pending_promotion".into());
                            m.insert("command_id".into(), entry.scheduled.command.command_id.clone());
                            m
                        },
                    }).await;
                }
            }

            self.insert_into_dispatch_queue(entry.scheduled);
            self.commands_promoted += 1;
        }
    }

    // ── Private: interlock requeue ────────────────────────────────────────────

    fn requeue_released_commands(&mut self, released: Vec<String>) {
        if released.is_empty() || self.interlock_retry_queue.is_empty() { return; }

        let now = Utc::now();
        let mut still_waiting = Vec::new();
        let mut requeued = 0u32;
        let mut expired  = 0u32;

        for entry in std::mem::take(&mut self.interlock_retry_queue) {
            if !released.contains(&entry.blocking_interlock_id) {
                still_waiting.push(entry);
                continue;
            }
            if entry.scheduled.command.deadline.map_or(false, |d| d <= now) {
                warn!("Interlock-held command {} expired while waiting", entry.scheduled.command.command_id);
                expired += 1;
                continue;
            }
            info!(
                "Requeueing {} after '{}' released",
                entry.scheduled.command.command_id,
                entry.blocking_interlock_id
            );
            self.insert_into_dispatch_queue(entry.scheduled);
            requeued += 1;
            self.commands_requeued_after_release += 1;
        }

        self.interlock_retry_queue = still_waiting;
        if requeued > 0 || expired > 0 {
            info!("Interlock release: {} requeued, {} expired", requeued, expired);
        }
    }

    // ── Private: insert into ready queue ─────────────────────────────────────

    fn insert_into_dispatch_queue(&mut self, s: Scheduled) {
        if (s.command.priority as u8) <= 1 {
            self.queue.push_front(s);
        } else {
            self.queue.push_back(s);
        }
    }

    // ── Private: validation ───────────────────────────────────────────────────

    fn validate_command(&self, command: &Command) -> Result<()> {
        if command.command_id.trim().is_empty() {
            return Err(anyhow!("Command ID cannot be empty"));
        }
        if let Some(dl) = command.deadline {
            if dl <= Utc::now() {
                return Err(anyhow!("Command deadline is in the past: {}", dl));
            }
        }
        // Duplicate check across all three tiers.
        let id = &command.command_id;
        let dup = self.queue.iter().any(|s| &s.command.command_id == id)
            || self.pending_schedule.iter().any(|Reverse(e)| &e.scheduled.command.command_id == id)
            || self.interlock_retry_queue.iter().any(|e| &e.scheduled.command.command_id == id);
        if dup {
            return Err(anyhow!("Command {} is already scheduled", id));
        }
        Ok(())
    }

    // ── Private: category mappings ────────────────────────────────────────────

    fn map_cmd_category(ct: CommandType) -> &'static str {
        match ct {
            CommandType::ThermalControl  => "heating",
            CommandType::PowerControl    => "high_power",
            CommandType::AttitudeControl => "precise_maneuver",
            CommandType::Diagnostic      => "cpu_intensive_tasks",
            CommandType::Maintenance     => "non_essential",
            CommandType::DataRequest     => "payload_activation",
            CommandType::Emergency       => "emergency_bypass",
            CommandType::Recovery        => "recovery_bypass",
        }
    }

    fn map_sys_category(ts: TargetSystem) -> &'static str {
        match ts {
            TargetSystem::ThermalManagement => "thermal_management",
            TargetSystem::PowerManagement   => "power_management",
            TargetSystem::AttitudeControl   => "attitude_control",
            TargetSystem::AllSystems        => "all_systems",
        }
    }

    // ── Private: sample buffers ───────────────────────────────────────────────

    fn record_send_time_sample(&mut self, ms: f64) {
        if self.send_times_ms.len() >= MAX_SEND_TIMES_HISTORY { self.send_times_ms.pop_front(); }
        self.send_times_ms.push_back(ms);
    }

    fn record_precision_sample(&mut self, ms: f64) {
        if self.schedule_precision_samples.len() >= MAX_SEND_TIMES_HISTORY {
            self.schedule_precision_samples.pop_front();
        }
        self.schedule_precision_samples.push_back(ms);
    }

    // ── Private: statistics ───────────────────────────────────────────────────

    fn mean_of(buf: &VecDeque<f64>) -> f64 {
        if buf.is_empty() { return 0.0; }
        buf.iter().sum::<f64>() / buf.len() as f64
    }

    fn percentile_of(buf: &VecDeque<f64>, p: f64) -> f64 {
        if buf.is_empty() { return 0.0; }
        let mut v: Vec<f64> = buf.iter().cloned().collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((p / 100.0) * (v.len() as f64 - 1.0)).round() as usize;
        v[idx.min(v.len() - 1)]
    }

    fn evaluate_performance_trend(&self) -> String {
        if self.send_times_ms.len() < 10 { return "insufficient_data".into(); }
        let half = self.send_times_ms.len() / 2;
        let recent = self.send_times_ms.iter().skip(half).sum::<f64>()
            / (self.send_times_ms.len() - half) as f64;
        let older  = self.send_times_ms.iter().take(half).sum::<f64>() / half as f64;
        let delta  = (older - recent) / older * 100.0;
        if delta > 5.0 { "improving".into() }
        else if delta < -5.0 { "degrading".into() }
        else { "stable".into() }
    }

    // ── CSV ───────────────────────────────────────────────────────────────────

    fn initialize_deadline_operations_csv() {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if !std::path::Path::new(DEADLINE_LOG_PATH).exists() {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(DEADLINE_LOG_PATH) {
                let _ = writeln!(
                    f,
                    "ts,command_id,command_type,target_system,priority,is_urgent,\
                     send_time_ms,deadline_met,network_met,adherent,\
                     deadline_violation_ms,precision_ms,reason"
                );
            }
        }
    }

    fn append_deadline_operation_to_csv(
        command: &Command, is_urgent: bool, send_time_ms: f64,
        deadline_met: bool, network_met: bool, adherent: bool,
        deadline_violation_ms: f64, precision_ms: f64, reason: &str,
    ) {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(DEADLINE_LOG_PATH) {
            let _ = writeln!(
                f,
                "{},{},{:?},{:?},{},{},{:.3},{},{},{},{:.3},{:.3},{}",
                Utc::now().to_rfc3339(), command.command_id,
                command.command_type, command.target_system,
                command.priority as u8, is_urgent, send_time_ms,
                deadline_met, network_met, adherent,
                deadline_violation_ms, precision_ms, reason,
            );
        }
    }

    fn initialize_missed_deadline_csv() {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if !std::path::Path::new(MISSED_DEADLINE_LOG_PATH).exists() {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(MISSED_DEADLINE_LOG_PATH) {
                let _ = writeln!(
                    f,
                    "ts,command_id,command_type,target_system,priority,\
                     send_time_ms,deadline_violation_ms,reason"
                );
            }
        }
    }

    fn append_missed_deadline_to_csv(
        command: &Command, send_time_ms: f64, deadline_violation_ms: f64, reason: &str,
    ) {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(MISSED_DEADLINE_LOG_PATH) {
            let _ = writeln!(
                f,
                "{},{},{:?},{:?},{},{:.3},{:.3},{}",
                Utc::now().to_rfc3339(), command.command_id,
                command.command_type, command.target_system,
                command.priority as u8, send_time_ms, deadline_violation_ms, reason,
            );
        }
    }
}
// impl CommandScheduler {
//     /// Spawns a background task to maintain and dispatch the real-time command schedule.
//     ///
//     /// This function should be called once during system startup.
//     pub fn spawn_realtime_schedule_maintainer(
//         scheduler: std::sync::Arc<tokio::sync::Mutex<Self>>,
//         mut fault_manager: crate::fault_management::FaultManager,
//         network: std::sync::Arc<crate::network_manager::NetworkManager>,
//         perf_tx: Option<tokio::sync::mpsc::Sender<crate::performance_tracker::PerformanceEvent>>,
//         poll_interval_ms: u64,
//     ) {
//         tokio::spawn(async move {
//             let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(poll_interval_ms));
//             loop {
//                 ticker.tick().await;
//                 let mut sched = scheduler.lock().await;
//                 // Prune expired commands
//                 let _ = sched.prune_expired_commands().await;
//                 // Dispatch due commands
//                 let _ = sched.process_dispatch_queue(
//                     &mut fault_manager,
//                     &*network,
//                     perf_tx.as_ref(),
//                 ).await;
//             }
//         });
//     }
// }
// // src/command_scheduler.rs
// //
// // Presentation map:
// // - B2.1: real-time command queue maintenance and dispatch ordering.
// // - B2.2: <=2ms urgent dispatch guard and deadline adherence logging.
// // - B2.3/B3.2: interlock-based command validation before dispatch.
// // - B2.4/B3.4: rejection reasons + deadline audit CSV persistence.
// // - S5: shared safety interlock enforcement integration.

// use std::collections::{VecDeque, HashMap};
// use chrono::{DateTime, Utc};
// use anyhow::{Result, anyhow};
// use tracing::{info, warn, error};
// use tokio::sync::mpsc;

// use crate::fault_management::FaultManager;
// use crate::network_manager::NetworkManager;
// use crate::performance_tracker::{PerformanceEvent, EventType};

// use shared_protocol::{
//     Command, CommunicationPacket, Source, Priority, CommandType, TargetSystem,
// };

// // Constants used by deadline evidence paths and CSV audit output.
// const MAX_SEND_TIMES_HISTORY: usize = 1000;
// const NETWORK_DEADLINE_THRESHOLD_MS: f64 = 2.0;
// const DEADLINE_LOG_PATH: &str = "logs/ground_control_deadline_ops.csv";
// const MISSED_DEADLINE_LOG_PATH: &str = "logs/ground_control_missed_deadlines.csv";

// #[derive(Debug, Clone)]
// pub struct EnhancedCommandSchedulerStats {
//     pub queued: usize,
//     pub dispatched: u64,
//     pub urgent_dispatched: u64,
//     pub avg_send_time_ms: f64,
//     pub commands_rejected_by_interlock: u64,  
//     pub commands_requeued_after_release: u64,  
// }

// #[derive(Debug, Clone)]
// pub struct DeadlineWarning {
//     pub command_id: String,
//     pub command_type: CommandType,
//     pub priority: Priority,
//     pub target_system: TargetSystem,
//     pub time_to_deadline_ms: f64,
// }

// #[derive(Debug, Clone)]
// pub struct UnifiedDeadlineReport {
//     pub total_urgent_commands: u64,
//     pub network_violations: u64,
//     pub deadline_violations: u64,
//     pub avg_network_send_time: f64,
//     pub adherence_rate: f64,
//     pub network_adherence_rate: f64,
//     pub performance_trend: String,
// }

// #[derive(Clone, Debug)]
// struct Scheduled {
//     command: Command,
//     _enqueued_at: DateTime<Utc>,
//     retry_count: u32,
// }

// /// A command held back by an active safety interlock, waiting for it to release.
// #[derive(Debug, Clone)]
// struct InterlockRetryEntry {
//     scheduled: Scheduled,
//     /// The interlock ID that caused this command to be blocked.
//     blocking_interlock_id: String,
// }

// #[derive(Debug)]
// pub struct CommandScheduler {
//     queue: VecDeque<Scheduled>,
//     /// Commands held back by active interlocks, waiting for release.
//     interlock_retry_queue: Vec<InterlockRetryEntry>,
//     dispatched: u64,
//     urgent_dispatched: u64,
//     send_times_ms: VecDeque<f64>,
//     network_violations: u64,
//     deadline_violations: u64,
//     total_urgent: u64,
//     pub commands_rejected_by_interlock: u64,   // add
//     pub commands_requeued_after_release: u64,  // add
// }

// impl CommandScheduler {
//     pub fn new() -> Self {
//         Self::initialize_deadline_operations_csv();
//         Self::initialize_missed_deadline_csv();

//         Self {
//             queue: VecDeque::new(),
//             interlock_retry_queue: Vec::new(),
//             dispatched: 0,
//             urgent_dispatched: 0,
//             send_times_ms: VecDeque::with_capacity(MAX_SEND_TIMES_HISTORY),
//             network_violations: 0,
//             deadline_violations: 0,
//             total_urgent: 0,
//             commands_rejected_by_interlock: 0,
//             commands_requeued_after_release: 0,
//         }
//     }

//     pub fn get_enhanced_stats(&self) -> EnhancedCommandSchedulerStats {
//         let avg_send_time_ms = if self.send_times_ms.is_empty() {
//             0.0
//         } else {
//             self.send_times_ms.iter().sum::<f64>() / self.send_times_ms.len() as f64
//         };

//         EnhancedCommandSchedulerStats {
//             queued: self.queue.len(),
//             dispatched: self.dispatched,
//             urgent_dispatched: self.urgent_dispatched,
//             avg_send_time_ms,
//             commands_rejected_by_interlock: self.commands_rejected_by_interlock,
//             commands_requeued_after_release: self.commands_requeued_after_release,
//         }
//     }

//     pub fn get_unified_deadline_report(&self) -> UnifiedDeadlineReport {
//         let avg_network_send_time = if self.send_times_ms.is_empty() {
//             0.0
//         } else {
//             self.send_times_ms.iter().sum::<f64>() / self.send_times_ms.len() as f64
//         };

//         let adherence_rate = if self.total_urgent == 0 {
//             100.0
//         } else {
//             100.0 * ((self.total_urgent.saturating_sub(self.deadline_violations)) as f64) / (self.total_urgent as f64)
//         };

//         let network_adherence_rate = if self.total_urgent == 0 {
//             100.0
//         } else {
//             100.0 * ((self.total_urgent.saturating_sub(self.network_violations)) as f64) / (self.total_urgent as f64)
//         };

//         UnifiedDeadlineReport {
//             total_urgent_commands: self.total_urgent,
//             network_violations: self.network_violations,
//             deadline_violations: self.deadline_violations,
//             avg_network_send_time,
//             adherence_rate,
//             network_adherence_rate,
//             performance_trend: self.evaluate_performance_trend(),
//         }
//     }

//     /// Calculate performance trend based on recent send times
//     fn evaluate_performance_trend(&self) -> String {
//         if self.send_times_ms.len() < 10 {
//             return "insufficient_data".to_string();
//         }

//         let recent_half = self.send_times_ms.len() / 2;
//         let recent_avg: f64 = self.send_times_ms.iter().skip(recent_half).sum::<f64>()
//             / (self.send_times_ms.len() - recent_half) as f64;
//         let older_avg: f64 = self.send_times_ms.iter().take(recent_half).sum::<f64>()
//             / recent_half as f64;

//         let improvement = (older_avg - recent_avg) / older_avg * 100.0;

//         if improvement > 5.0 {
//             "improving".to_string()
//         } else if improvement < -5.0 {
//             "degrading".to_string()
//         } else {
//             "stable".to_string()
//         }
//     }

//     /// Add send time to history with bounded size
//     fn record_send_time_sample(&mut self, send_time_ms: f64) {
//         if self.send_times_ms.len() >= MAX_SEND_TIMES_HISTORY {
//             self.send_times_ms.pop_front();
//         }
//         self.send_times_ms.push_back(send_time_ms);
//     }

//     fn initialize_deadline_operations_csv() {
//         use std::fs::{self, OpenOptions};
//         use std::io::Write;

//         let _ = fs::create_dir_all("logs");

//         if !std::path::Path::new(DEADLINE_LOG_PATH).exists() {
//             match OpenOptions::new().create(true).append(true).open(DEADLINE_LOG_PATH) {
//                 Ok(mut csv_file) => {
//                     let _ = writeln!(
//                         csv_file,
//                         "ts,command_id,command_type,target_system,priority,is_urgent,send_time_ms,deadline_met,network_met,adherent,deadline_violation_ms,reason"
//                     );
//                 }
//                 Err(write_error) => {
//                     warn!("Unable To Initialize ground_control_deadline_ops.csv: {}", write_error);
//                 }
//             }
//         }
//     }

//     fn append_deadline_operation_to_csv(
//         command: &Command,
//         is_urgent: bool,
//         send_time_ms: f64,
//         deadline_met: bool,
//         network_met: bool,
//         adherent: bool,
//         deadline_violation_ms: f64,
//         reason: &str,
//     ) {
//         use std::fs::{self, OpenOptions};
//         use std::io::Write;

//         let _ = fs::create_dir_all("logs");
//         let should_write_header = !std::path::Path::new(DEADLINE_LOG_PATH).exists();

//         match OpenOptions::new().create(true).append(true).open(DEADLINE_LOG_PATH) {
//             Ok(mut csv_file) => {
//                 if should_write_header {
//                     let _ = writeln!(
//                         csv_file,
//                         "ts,command_id,command_type,target_system,priority,is_urgent,send_time_ms,deadline_met,network_met,adherent,deadline_violation_ms,reason"
//                     );
//                 }

//                 let _ = writeln!(
//                     csv_file,
//                     "{},{},{:?},{:?},{},{},{:.3},{},{},{},{:.3},{}",
//                     Utc::now().to_rfc3339(),
//                     command.command_id,
//                     command.command_type,
//                     command.target_system,
//                     command.priority as u8,
//                     is_urgent,
//                     send_time_ms,
//                     deadline_met,
//                     network_met,
//                     adherent,
//                     deadline_violation_ms,
//                     reason,
//                 );
//             }
//             Err(write_error) => {
//                 warn!("Unable To Append ground_control_deadline_ops.csv: {}", write_error);
//             }
//         }
//     }

//     fn initialize_missed_deadline_csv() {
//         use std::fs::{self, OpenOptions};
//         use std::io::Write;

//         let _ = fs::create_dir_all("logs");

//         if !std::path::Path::new(MISSED_DEADLINE_LOG_PATH).exists() {
//             match OpenOptions::new().create(true).append(true).open(MISSED_DEADLINE_LOG_PATH) {
//                 Ok(mut csv_file) => {
//                     let _ = writeln!(
//                         csv_file,
//                         "ts,command_id,command_type,target_system,priority,send_time_ms,deadline_violation_ms,reason"
//                     );
//                 }
//                 Err(write_error) => {
//                     warn!("Unable To Initialize ground_control_missed_deadlines.csv: {}", write_error);
//                 }
//             }
//         }
//     }

//     fn append_missed_deadline_to_csv(
//         command: &Command,
//         send_time_ms: f64,
//         deadline_violation_ms: f64,
//         reason: &str,
//     ) {
//         use std::fs::{self, OpenOptions};
//         use std::io::Write;

//         let _ = fs::create_dir_all("logs");
//         let should_write_header = !std::path::Path::new(MISSED_DEADLINE_LOG_PATH).exists();

//         match OpenOptions::new().create(true).append(true).open(MISSED_DEADLINE_LOG_PATH) {
//             Ok(mut csv_file) => {
//                 if should_write_header {
//                     let _ = writeln!(
//                         csv_file,
//                         "ts,command_id,command_type,target_system,priority,send_time_ms,deadline_violation_ms,reason"
//                     );
//                 }

//                 let _ = writeln!(
//                     csv_file,
//                     "{},{},{:?},{:?},{},{:.3},{:.3},{}",
//                     Utc::now().to_rfc3339(),
//                     command.command_id,
//                     command.command_type,
//                     command.target_system,
//                     command.priority as u8,
//                     send_time_ms,
//                     deadline_violation_ms,
//                     reason,
//                 );
//             }
//             Err(write_error) => {
//                 warn!("Unable To Append ground_control_missed_deadlines.csv: {}", write_error);
//             }
//         }
//     }

//     /// Map a `CommandType` to the interlock category used by FaultManager (B2.3, S5).
//     /// These strings must match the `blocked_command_types` used when activating safety interlocks.
//     fn map_command_type_to_interlock_category(ct: CommandType) -> &'static str {
//         match ct {
//             CommandType::ThermalControl  => "heating",
//             CommandType::PowerControl    => "high_power",
//             CommandType::AttitudeControl => "precise_maneuver",
//             CommandType::Diagnostic      => "cpu_intensive_tasks",
//             CommandType::Maintenance     => "non_essential",
//             CommandType::DataRequest     => "payload_activation",
//             // Emergency and Recovery commands must never be blocked by interlocks.
//             CommandType::Emergency       => "emergency_bypass",
//             CommandType::Recovery        => "recovery_bypass",
//         }
//     }

//     /// Map a `TargetSystem` to the interlock system category used by FaultManager.
//     fn map_target_system_to_interlock_category(ts: TargetSystem) -> &'static str {
//         match ts {
//             TargetSystem::ThermalManagement => "thermal_management",
//             TargetSystem::PowerManagement   => "power_management",
//             TargetSystem::AttitudeControl   => "attitude_control",
//             TargetSystem::AllSystems        => "all_systems",
//         }
//     }

//     /// Requeue any interlock-held commands whose blocking interlock has been released.
//     /// Commands whose deadline has already expired are dropped instead.
//     fn requeue_released_commands(&mut self, released_interlock_ids: Vec<String>) {
//         if released_interlock_ids.is_empty() || self.interlock_retry_queue.is_empty() {
//             return;
//         }

//         let now = Utc::now();
//         let mut still_waiting = Vec::new();
//         let mut requeued = 0u32;
//         let mut expired = 0u32;

//         for entry in std::mem::take(&mut self.interlock_retry_queue) {
//             if !released_interlock_ids.contains(&entry.blocking_interlock_id) {
//                 // Still blocked by a different interlock — keep waiting.
//                 still_waiting.push(entry);
//                 continue;
//             }
//             // Interlock released — check deadline before requeueing.
//             if let Some(deadline) = entry.scheduled.command.deadline {
//                 if deadline <= now {
//                     warn!(
//                         "Interlock-held command {} expired while waiting, dropping",
//                         entry.scheduled.command.command_id
//                     );
//                     expired += 1;
//                     continue;
//                 }
//             }
//             let is_urgent = (entry.scheduled.command.priority as u8) <= 1;
//             info!(
//                 "Requeueing command {} after interlock '{}' released (urgent: {})",
//                 entry.scheduled.command.command_id,
//                 entry.blocking_interlock_id,
//                 is_urgent
//             );
//             if is_urgent {
//                 self.queue.push_front(entry.scheduled);
//             } else {
//                 self.queue.push_back(entry.scheduled);
//             }
//             requeued += 1;
//             self.commands_requeued_after_release += 1;  // add
//         }

//         self.interlock_retry_queue = still_waiting;

//         if requeued > 0 || expired > 0 {
//             info!(
//                 "Interlock release: {} command(s) requeued, {} expired and dropped",
//                 requeued, expired
//             );
//         }
//     }

//     pub async fn process_dispatch_queue(
//         &mut self,
//         fault_manager: &mut FaultManager,
//         network: &NetworkManager,
//         perf_tx: Option<&mpsc::Sender<PerformanceEvent>>,
//     ) -> Vec<Command> {
//         // Before processing, requeue any commands released by interlocks that
//         // resolved since the last dispatch cycle.
//         let released = fault_manager.drain_released_interlock_ids();
//         self.requeue_released_commands(released);

//         let mut dispatched_commands = Vec::new();
//         let mut urgent_retry_queue = VecDeque::new();
//         let mut normal_retry_queue = VecDeque::new();

//         // Process commands, separating failed ones by priority
//         while let Some(mut scheduled) = self.queue.pop_front() {
//             let urgent = (scheduled.command.priority as u8) <= 1;
//             let deadline = scheduled.command.deadline;

//             // Safety interlock check (B2.3, B3.2, S5).
//             // Emergency / Recovery commands bypass interlocks so we can always
//             // send critical responses regardless of active faults.
//             let bypasses_interlocks = matches!(
//                 scheduled.command.command_type,
//                 CommandType::Emergency | CommandType::Recovery
//             );

//             if !bypasses_interlocks {
//                 let cmd_cat = Self::map_command_type_to_interlock_category(scheduled.command.command_type);
//                 let sys_cat = Self::map_target_system_to_interlock_category(scheduled.command.target_system);
//                 let submit_time = Utc::now();

//                 let (blocked, reasons, block_event) = fault_manager.is_command_blocked(
//                     cmd_cat,
//                     sys_cat,
//                     &scheduled.command.command_id,
//                     submit_time,
//                 );

//                 if blocked {
//                     let reason_str = reasons.join("; ");
//                     warn!(
//                         "COMMAND REJECTED — id={} type={:?} target={:?} | reason: {}",
//                         scheduled.command.command_id,
//                         scheduled.command.command_type,
//                         scheduled.command.target_system,
//                         reason_str
//                     );

//                     let blocking_interlock_id = block_event
//                         .as_ref()
//                         .map(|be| be.blocking_interlock_id.clone())
//                         .unwrap_or_default();

//                     if let Some(be) = block_event {
//                         fault_manager.record_command_block_event(be);
//                     }

//                     if let Some(tx) = perf_tx {
//                         let _ = tx.send(PerformanceEvent {
//                             timestamp: Utc::now(),
//                             event_type: EventType::CommandValidationFailed,
//                             duration_ms: 0.0,
//                             metadata: {
//                                 let mut m = HashMap::new();
//                                 m.insert("command_id".into(), scheduled.command.command_id.clone());
//                                 m.insert("command_type".into(), format!("{:?}", scheduled.command.command_type));
//                                 m.insert("target_system".into(), format!("{:?}", scheduled.command.target_system));
//                                 m.insert("rejection_reason".into(), reason_str);
//                                 m.insert("interlock_active".into(), "true".into());
//                                 m
//                             },
//                         }).await;
//                     }

//                     // Hold the command for automatic requeue when the interlock releases.
//                     // Drop it immediately if the deadline has already passed.
//                     let deadline_ok = scheduled.command.deadline
//                         .map(|d| d > Utc::now())
//                         .unwrap_or(true);

//                     if deadline_ok && !blocking_interlock_id.is_empty() {
//                         info!(
//                             "Command {} held for interlock release (interlock: {})",
//                             scheduled.command.command_id, blocking_interlock_id
//                         );
//                         self.interlock_retry_queue.push(InterlockRetryEntry {
//                             scheduled,
//                             blocking_interlock_id,
//                         });
//                     } else {
//                         error!(
//                             "Command {} discarded — interlock active and deadline expired",
//                             scheduled.command.command_id
//                         );
//                     }
//                     self.commands_rejected_by_interlock += 1;  // add — counts both held and discarded
//                     continue;
//                 }
//             }
//             // ───────────────────────────────────────────────────────────────

//             let send_result = network
//                 .send_packet_with_deadline_guard(
//                     CommunicationPacket::new_command(scheduled.command.clone(), Source::GroundControl),
//                     urgent,
//                     deadline,
//                 )
//                 .await;

//             match send_result {
//                 Ok(send_result) => {
//                     let network_met = send_result.send_time_ms <= NETWORK_DEADLINE_THRESHOLD_MS;
//                     let deadline_met = send_result.success && send_result.deadline_met;
//                     let adherent = send_result.success && network_met && deadline_met;
//                     let mut reasons = Vec::new();

//                     if urgent {
//                         self.total_urgent = self.total_urgent.saturating_add(1);
//                     }
//                     if !send_result.success {
//                         reasons.push("dispatch_rejected_past_deadline");
//                     }
//                     if !network_met {
//                         reasons.push("network_send_over_2ms");
//                         self.network_violations = self.network_violations.saturating_add(1);
//                     }
//                     if !deadline_met {
//                         reasons.push("deadline_missed");
//                         self.deadline_violations = self.deadline_violations.saturating_add(1);
//                     }

//                     let reason = if reasons.is_empty() {
//                         "ok".to_string()
//                     } else {
//                         reasons.join(";")
//                     };

//                     Self::append_deadline_operation_to_csv(
//                         &scheduled.command,
//                         urgent,
//                         send_result.send_time_ms,
//                         deadline_met,
//                         network_met,
//                         adherent,
//                         send_result.deadline_violation_ms,
//                         &reason,
//                     );

//                     if !deadline_met {
//                         Self::append_missed_deadline_to_csv(
//                             &scheduled.command,
//                             send_result.send_time_ms,
//                             send_result.deadline_violation_ms,
//                             &reason,
//                         );
//                     }

//                     if let Some(performance_tx) = perf_tx {
//                         if !network_met {
//                             let _ = performance_tx.send(PerformanceEvent {
//                                 timestamp: Utc::now(),
//                                 event_type: EventType::NetworkDeadlineViolation,
//                                 duration_ms: send_result.send_time_ms,
//                                 metadata: {
//                                     let mut metadata = HashMap::new();
//                                     metadata.insert("command_id".into(), scheduled.command.command_id.clone());
//                                     metadata.insert("send_time_ms".into(), format!("{:.3}", send_result.send_time_ms));
//                                     metadata.insert("threshold_ms".into(), format!("{:.1}", NETWORK_DEADLINE_THRESHOLD_MS));
//                                     metadata
//                                 },
//                             }).await;
//                         }
//                         if !deadline_met {
//                             let _ = performance_tx.send(PerformanceEvent {
//                                 timestamp: Utc::now(),
//                                 event_type: EventType::CommandDeadlineViolation,
//                                 duration_ms: send_result.deadline_violation_ms,
//                                 metadata: {
//                                     let mut metadata = HashMap::new();
//                                     metadata.insert("command_id".into(), scheduled.command.command_id.clone());
//                                     metadata.insert("deadline_violation_ms".into(), format!("{:.3}", send_result.deadline_violation_ms));
//                                     metadata
//                                 },
//                             }).await;
//                         }
//                     }

//                     if !send_result.success {
//                         error!(
//                             "Command {} missed deadline before dispatch (violation {:.3}ms)",
//                             scheduled.command.command_id,
//                             send_result.deadline_violation_ms
//                         );
//                         continue;
//                     }

//                     self.dispatched = self.dispatched.saturating_add(1);
//                     if urgent {
//                         self.urgent_dispatched = self.urgent_dispatched.saturating_add(1);
//                     }

//                     self.record_send_time_sample(send_result.send_time_ms);
//                     dispatched_commands.push(scheduled.command.clone());

//                     info!("Command Dispatched Successfully: {}", scheduled.command.command_id);
//                 }
//                 Err(dispatch_error) => {
//                     // Increment retry count and handle failed commands
//                     scheduled.retry_count += 1;

//                     warn!("Command Dispatch Failed {} (Retry {}): {}",
//                         scheduled.command.command_id, scheduled.retry_count, dispatch_error);

//                     if let Some(performance_tx) = perf_tx {
//                         let performance_event = PerformanceEvent {
//                             timestamp: Utc::now(),
//                             event_type: EventType::CommandDispatchError,
//                             duration_ms: 0.0,
//                             metadata: {
//                                 let mut metadata = HashMap::new();
//                                 metadata.insert("error".into(), dispatch_error.to_string());
//                                 metadata.insert("command_id".into(), scheduled.command.command_id.clone());
//                                 metadata.insert("retry_count".into(), scheduled.retry_count.to_string());
//                                 metadata
//                             },
//                         };

//                         if let Err(perf_send_error) = performance_tx.send(performance_event).await {
//                             warn!("Performance Event Channel Send Failed: {}", perf_send_error);
//                         }
//                     }

//                     // Separate failed commands by priority for better scheduling
//                     if urgent {
//                         urgent_retry_queue.push_back(scheduled);
//                     } else {
//                         normal_retry_queue.push_back(scheduled);
//                     }
//                 }
//             }
//         }

//         // Rebuild queue with high priority failed commands first
//         self.queue = urgent_retry_queue;
//         self.queue.extend(normal_retry_queue);

//         dispatched_commands
//     }

//     pub fn get_commands_approaching_deadline(&self) -> Vec<DeadlineWarning> {
//         let now = Utc::now();
//         self.queue.iter()
//             .filter_map(|scheduled| scheduled.command.deadline.map(|deadline| (scheduled, deadline)))
//             .map(|(scheduled, deadline)| {
//                 let time_to_deadline_ms = match (deadline - now).num_microseconds() {
//                     Some(micros) if micros >= 0 => micros as f64 / 1000.0,
//                     _ => 0.0,
//                 };

//                 DeadlineWarning {
//                     command_id: scheduled.command.command_id.clone(),
//                     command_type: scheduled.command.command_type.clone(),
//                     priority: scheduled.command.priority,
//                     target_system: scheduled.command.target_system.clone(),
//                     time_to_deadline_ms,
//                 }
//             })
//             .collect()
//     }

//     pub async fn prune_expired_commands(&mut self) {
//         let now = Utc::now();
//         let initial_queue_len = self.queue.len();

//         self.queue.retain(|scheduled| {
//             scheduled.command.deadline.map_or(true, |deadline| deadline > now)
//         });

//         // Also prune expired commands from the interlock retry queue.
//         let initial_retry_len = self.interlock_retry_queue.len();
//         self.interlock_retry_queue.retain(|entry| {
//             entry.scheduled.command.deadline.map_or(true, |deadline| {
//                 if deadline <= now {
//                     warn!(
//                         "Interlock-held command {} expired during prune, dropping",
//                         entry.scheduled.command.command_id
//                     );
//                     false
//                 } else {
//                     true
//                 }
//             })
//         });

//         let removed = (initial_queue_len - self.queue.len())
//             + (initial_retry_len - self.interlock_retry_queue.len());
//         if removed > 0 {
//             info!("Pruned {} Expired Commands ({} from interlock hold queue)",
//                 removed,
//                 initial_retry_len - self.interlock_retry_queue.len()
//             );
//         }
//     }

//     pub async fn refresh_safety_validation_cache(&mut self) {
//         // Hook for future safety validation implementation
//         // This could validate command parameters, check system states, etc.
//     }

//     pub fn schedule_command(&mut self, command: Command) -> Result<String> {
//         // Validate command deadline
//         if let Some(deadline) = command.deadline {
//             if deadline <= Utc::now() {
//                 return Err(anyhow!("Command deadline is in the past: {}", deadline));
//             }
//         }

//         // Validate command ID is not empty
//         if command.command_id.trim().is_empty() {
//             return Err(anyhow!("Command ID cannot be empty"));
//         }

//         // Check for duplicate command IDs in both queues
//         if self.queue.iter().any(|s| s.command.command_id == command.command_id)
//             || self.interlock_retry_queue.iter().any(|e| e.scheduled.command.command_id == command.command_id)
//         {
//             return Err(anyhow!("Command with ID {} is already scheduled", command.command_id));
//         }

//         let scheduled_command_id = command.command_id.clone();
//         let scheduled = Scheduled {
//             command,
//             _enqueued_at: Utc::now(),
//             retry_count: 0,
//         };

//         // Insert high priority commands at the front
//         let is_urgent = (scheduled.command.priority as u8) <= 1;
//         if is_urgent {
//             self.queue.push_front(scheduled);
//         } else {
//             self.queue.push_back(scheduled);
//         }

//         info!("Command {} Scheduled (Urgent: {})", scheduled_command_id, is_urgent);
//         Ok(scheduled_command_id)
//     }
// }

