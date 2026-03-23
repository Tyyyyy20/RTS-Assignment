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

/// Network send time threshold for urgent commands.
/// If send_time_ms exceeds this the dispatch is recorded as a violation
/// and the command is additionally written to missed_deadlines.csv.
const URGENT_MS: f64 = 2.0;

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
pub struct UnifiedDeadlineReport {
    pub total_urgent_commands: u64,
    pub deadline_violations: u64,
    pub avg_network_send_time: f64,
    pub adherence_rate: f64,
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
    /// ASAP commands get scheduled_at = enqueued_at.
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
    //   pending_schedule      → future commands, not yet ready
    //   queue                 → ready to dispatch now (priority-ordered)
    //   interlock_retry_queue → blocked by active interlock
    pending_schedule: BinaryHeap<Reverse<PendingEntry>>,
    queue: VecDeque<Scheduled>,
    interlock_retry_queue: Vec<InterlockRetryEntry>,

    dispatched: u64,
    urgent_dispatched: u64,
    deadline_violations: u64,
    total_urgent: u64,
    pub commands_rejected_by_interlock: u64,
    pub commands_requeued_after_release: u64,
    pub commands_promoted: u64,
    pub schedule_precision_violations: u64,

    send_times_ms: VecDeque<f64>,
    /// Absolute lag between scheduled_at and actual dispatch time (ms),
    /// one sample per dispatched urgent command.
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
    /// queue immediately (late-start ASAP).
    pub fn schedule_command_at(&mut self, command: Command, at: DateTime<Utc>) -> Result<String> {
        self.validate_command(&command)?;
        let command_id = command.command_id.clone();
        let now = Utc::now();

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

        UnifiedDeadlineReport {
            total_urgent_commands: self.total_urgent,
            deadline_violations: self.deadline_violations,
            avg_network_send_time: Self::mean_of(&self.send_times_ms),
            adherence_rate,
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
    ///   3. Dispatch the ready queue.
    ///      - Urgent commands (priority ≤ 1): measure precision, write to
    ///        deadline_ops.csv, and if send_time_ms > URGENT_MS also write
    ///        to missed_deadlines.csv and increment deadline_violations.
    ///      - Non-urgent commands: dispatch and count only, no CSV writes.
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

                    if !blocking_id.is_empty() {
                        self.interlock_retry_queue.push(InterlockRetryEntry {
                            scheduled,
                            blocking_interlock_id: blocking_id,
                        });
                    } else {
                        error!(
                            "Command {} discarded — interlock active, no interlock ID",
                            scheduled.command.command_id
                        );
                    }
                    self.commands_rejected_by_interlock += 1;
                    continue;
                }
            }

            // ── Measure precision before network send ─────────────────────
            let dispatch_now = Utc::now();
            let precision_ms = (dispatch_now - scheduled.scheduled_at)
                .num_microseconds()
                .unwrap_or(0)
                .max(0) as f64
                / 1000.0;

            // ── Network send ──────────────────────────────────────────────
            let send_result = network
                .send_packet_with_deadline_guard(
                    CommunicationPacket::new_command(
                        scheduled.command.clone(),
                        Source::GroundControl,
                    ),
                    urgent,
                    None, // commands have no deadline field
                )
                .await;

            match send_result {
                Ok(sr) => {
                    if urgent {
                        // ── Urgent: full CSV audit + violation check ──────
                        self.total_urgent = self.total_urgent.saturating_add(1);

                        let violated = precision_ms > URGENT_MS;
                        if violated {
                            self.deadline_violations = self.deadline_violations.saturating_add(1);

                            error!(
                                "URGENT DISPATCH VIOLATION: {} precision_ms={:.3}ms threshold={:.1}ms",
                                scheduled.command.command_id, precision_ms, URGENT_MS
                            );

                            // Write to missed deadlines CSV
                            Self::append_missed_deadline_to_csv(
                                &scheduled.command,
                                sr.send_time_ms,
                                precision_ms - URGENT_MS,
                                "urgent_dispatch_over_2ms",
                            );
                        }

                        // Always write urgent commands to deadline_ops.csv
                        Self::append_deadline_operation_to_csv(
                            &scheduled.command,
                            true,               // is_urgent
                            sr.send_time_ms,
                            !violated,          // deadline_met
                            !violated,          // adherent
                            if violated { precision_ms - URGENT_MS } else { 0.0 },
                            precision_ms,
                            if violated { "urgent_dispatch_over_2ms" } else { "ok" },
                        );

                        // Record precision for urgent commands
                        self.record_precision_sample(precision_ms);
                        if precision_ms > URGENT_MS {
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
                                        m.insert("precision_ms".into(), format!("{:.3}", precision_ms));
                                        m.insert("threshold_ms".into(), format!("{:.1}", URGENT_MS));
                                        m
                                    },
                                }).await;
                            }
                        }

                        self.urgent_dispatched = self.urgent_dispatched.saturating_add(1);
                    }
                    // Non-urgent: no CSV, no deadline check.

                    self.dispatched = self.dispatched.saturating_add(1);
                    self.record_send_time_sample(sr.send_time_ms);
                    dispatched_commands.push(scheduled.command.clone());

                    // Record command dispatch latency for stats
                    if let Some(tx) = perf_tx {
                        let mut metadata = HashMap::new();
                        metadata.insert("command_id".into(), scheduled.command.command_id.clone());
                        metadata.insert("priority".into(), (scheduled.command.priority as u8).to_string());
                        let _ = tx.send(PerformanceEvent {
                            timestamp: dispatch_now,
                            event_type: EventType::CommandDispatched,
                            duration_ms: precision_ms,
                            metadata,
                        }).await;
                    }

                    info!(
                        "Dispatched {} (urgent={}, precision={:.3}ms, send={:.3}ms)",
                        scheduled.command.command_id, urgent, precision_ms, sr.send_time_ms
                    );
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

    // ── Pruning ───────────────────────────────────────────────────────────────
    // Prune because of staleness (interlock-held commands that have been waiting too long)
    pub async fn prune_expired_commands(&mut self) {
        let now = Utc::now();
        let stale_threshold_ms = 30_000i64; // 30 seconds

        let q0 = self.queue.len();
        let p0 = self.pending_schedule.len();
        let r0 = self.interlock_retry_queue.len();

        self.queue.retain(|s| {
            let age_ms = (now - s.enqueued_at).num_milliseconds();
            if age_ms > stale_threshold_ms {
                warn!(
                    "Pruning stale command {} (in queue {:.1}s)",
                    s.command.command_id, age_ms as f64 / 1000.0
                );
                false
            } else {
                true
            }
        });

        // For pending schedule, prune entries whose scheduled_at is so far in
        // the past they clearly missed their window.
        let fresh: Vec<_> = std::mem::take(&mut self.pending_schedule)
            .into_iter()
            .filter(|Reverse(e)| {
                let age_ms = (now - e.scheduled.enqueued_at).num_milliseconds();
                if age_ms > stale_threshold_ms {
                    warn!(
                        "Pruning stale pending command {} (enqueued {:.1}s ago)",
                        e.scheduled.command.command_id, age_ms as f64 / 1000.0
                    );
                    false
                } else {
                    true
                }
            })
            .collect();
        self.pending_schedule = fresh.into_iter().collect();

        self.interlock_retry_queue.retain(|e| {
            let age_ms = (now - e.scheduled.enqueued_at).num_milliseconds();
            if age_ms > stale_threshold_ms {
                warn!(
                    "Pruning stale interlock-held command {} (enqueued {:.1}s ago)",
                    e.scheduled.command.command_id, age_ms as f64 / 1000.0
                );
                false
            } else {
                true
            }
        });

        let removed = (q0 - self.queue.len())
            + (p0 - self.pending_schedule.len())
            + (r0 - self.interlock_retry_queue.len());

        if removed > 0 {
            info!(
                "Pruned {} stale commands ({} dispatch, {} pending, {} interlock hold)",
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
    /// Emits SchedulerPrecisionViolation and TaskExecutionDrift for late promotions.
    async fn promote_pending_commands(
        &mut self,
        perf_tx: Option<&mpsc::Sender<PerformanceEvent>>,
    ) {
        let now = Utc::now();

        loop {
            match self.pending_schedule.peek() {
                Some(Reverse(e)) if e.scheduled_at <= now => {}
                _ => break,
            }

            let Reverse(entry) = self.pending_schedule.pop().unwrap();

            let lateness_ms = (now - entry.scheduled_at)
                .num_microseconds()
                .unwrap_or(0)
                .max(0) as f64
                / 1000.0;

            const PROMOTION_WARN_MS: f64 = 1.0;   // more than one tick late
            const PROMOTION_VIOLATION_MS: f64 = 5.0; // genuinely stale promotion

            if lateness_ms > PROMOTION_VIOLATION_MS {
                warn!(
                    "Command {} promoted {:.3}ms late (scheduled for {})",
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
            } else if lateness_ms > PROMOTION_WARN_MS {
                // normal OS jitter — just log at debug level, no event
                warn!(
                    "Command {} promoted {:.3}ms late (minor tick jitter)",
                    entry.scheduled.command.command_id, lateness_ms
                );
            }

            self.insert_into_dispatch_queue(entry.scheduled);
            self.commands_promoted += 1;
        }
    }

    // ── Private: interlock requeue ────────────────────────────────────────────

    fn requeue_released_commands(&mut self, released: Vec<String>) {
        if released.is_empty() || self.interlock_retry_queue.is_empty() { return; }

        let mut still_waiting = Vec::new();
        let mut requeued = 0u32;

        for entry in std::mem::take(&mut self.interlock_retry_queue) {
            if !released.contains(&entry.blocking_interlock_id) {
                still_waiting.push(entry);
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
        if requeued > 0 {
            info!("Interlock release: {} requeued", requeued);
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
                     send_time_ms,deadline_met,adherent,\
                     violation_ms,precision_ms,reason"
                );
            }
        }
    }

    fn append_deadline_operation_to_csv(
        command: &Command,
        is_urgent: bool,
        send_time_ms: f64,
        deadline_met: bool,
        adherent: bool,
        violation_ms: f64,
        precision_ms: f64,
        reason: &str,
    ) {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(DEADLINE_LOG_PATH) {
            let _ = writeln!(
                f,
                "{},{},{:?},{:?},{},{},{:.3},{},{},{:.3},{:.3},{}",
                Utc::now().to_rfc3339(),
                command.command_id,
                command.command_type,
                command.target_system,
                command.priority as u8,
                is_urgent,
                send_time_ms,
                deadline_met,
                adherent,
                violation_ms,
                precision_ms,
                reason,
            );
        }
    }

    fn initialize_missed_deadline_csv() {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if !std::path::Path::new(MISSED_DEADLINE_LOG_PATH).exists() {
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(MISSED_DEADLINE_LOG_PATH)
            {
                let _ = writeln!(
                    f,
                    "ts,command_id,command_type,target_system,priority,\
                     send_time_ms,violation_ms,reason"
                );
            }
        }
    }

    fn append_missed_deadline_to_csv(
        command: &Command,
        send_time_ms: f64,
        violation_ms: f64,
        reason: &str,
    ) {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        let _ = fs::create_dir_all("logs");
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(MISSED_DEADLINE_LOG_PATH)
        {
            let _ = writeln!(
                f,
                "{},{},{:?},{:?},{},{:.3},{:.3},{}",
                Utc::now().to_rfc3339(),
                command.command_id,
                command.command_type,
                command.target_system,
                command.priority as u8,
                send_time_ms,
                violation_ms,
                reason,
            );
        }
    }
}
