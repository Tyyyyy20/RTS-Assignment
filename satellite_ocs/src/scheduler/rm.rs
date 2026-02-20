// src/scheduler/rm.rs
use crate::{config::Config, logging};
use super::PREEMPT_CH;

use std::cmp::Ordering;
use std::time::Duration as StdDuration;
use tokio::{
    sync::mpsc,
    time::{self, Duration, Instant},
};
use tracing::{info, warn};

#[derive(Clone)]
struct RtTask {
    name: &'static str,
    period: Duration,
    deadline: Duration,
    wcet_ms: f64,            // simulated worst-case execution time (for accounting)
    rm_priority: u8,         // lower = higher priority (RM: shorter period wins)
    // runtime state
    next_release: Instant,
    next_deadline: Instant,
    seq: u64,
}

impl RtTask {
    fn new(name: &'static str, period_ms: u64, wcet_ms: f64, prio: u8, now: Instant) -> Self {
        let p = Duration::from_millis(period_ms);
        Self {
            name,
            period: p,
            deadline: p,
            wcet_ms,
            rm_priority: prio,
            next_release: now + p,
            next_deadline: now + p,
            seq: 0,
        }
    }
}

#[derive(Debug)]
struct Job {
    task_idx: usize,
    release: Instant,
    deadline: Instant,
    seq: u64,
    remaining_ms: f64,
    preemptions: u32,
}

/// Spawn the RM scheduler loop.
/// - Schedules: antenna_alignment(50ms), data_compression(100ms), health_monitor(1000ms)
/// - Preemption: a sporadic, highest-priority thermal_control job is injected when PREEMPT_CH fires.
pub async fn spawn_rm(cfg: Config) {
    let now = Instant::now();

    // RM priority by period (lower number = higher priority)
    // Moderately increased WCET to account for async overhead
    let mut tasks = vec![
        RtTask::new("antenna_alignment",  50, 3.0, 1, now),   // high - increased from 1.5
        RtTask::new("data_compression",  100, 6.0, 2, now),   // medium - increased from 3.0
        RtTask::new("health_monitor",   1000, 2.0, 3, now),   // low - increased from 1.0
    ];

    // Preemption channel (thermal control trigger)
    let (tx_preempt, mut rx_preempt) = mpsc::channel::<()>(16);
    let _ = PREEMPT_CH.set(tx_preempt);

    // Ready queue of released jobs
    let mut ready: Vec<Job> = Vec::new();

    // CPU accounting (scheduler-level utilization)
    let mut win_start = Instant::now();
    let mut active_ms_acc: f64 = 0.0;

    // Helper: push newly-released jobs into ready queue
    let mut release_due = |tasks: &mut [RtTask], ready: &mut Vec<Job>, now: Instant| {
        for (idx, t) in tasks.iter_mut().enumerate() {
            if now >= t.next_release {
                t.seq = t.seq.wrapping_add(1);
                let job = Job {
                    task_idx: idx,
                    release: t.next_release,
                    deadline: t.next_deadline,
                    seq: t.seq,
                    remaining_ms: t.wcet_ms,
                    preemptions: 0,
                };
                ready.push(job);
                // schedule next release/deadline
                t.next_release += t.period;
                t.next_deadline += t.deadline;
            }
        }
        // RM order: by task priority (rm_priority), then earliest deadline
        ready.sort_by(|a, b| {
            let pa = tasks[a.task_idx].rm_priority;
            let pb = tasks[b.task_idx].rm_priority;
            match pa.cmp(&pb) {
                Ordering::Equal => a.deadline.cmp(&b.deadline),
                other => other,
            }
        });
    };

    // Simulate the high-priority "thermal_control" sporadic job
    let mut spawn_thermal_job = |ready: &mut Vec<Job>, now: Instant| {
        // Use a synthetic "task index" = !0 to mark thermal_control
        let job = Job {
            task_idx: usize::MAX,
            release: now,
            deadline: now + Duration::from_millis(20), // tight deadline
            seq: 0,
            remaining_ms: 2.0, // simulate ~2ms of control work
            preemptions: 0,
        };
        ready.push(job);
        // Ensure it bubbles to the top (it is highest priority)
        ready.sort_by(|a, b| {
            // thermal first
            if a.task_idx == usize::MAX && b.task_idx != usize::MAX { Ordering::Less }
            else if b.task_idx == usize::MAX && a.task_idx != usize::MAX { Ordering::Greater }
            else { Ordering::Equal }
        });
        info!("RM: thermal_control job injected (preemption)");
    };

    // Main scheduler loop
    loop {
        let nowi = Instant::now();

        // 1) Release periodic jobs that are due
        release_due(&mut tasks, &mut ready, nowi);

        // 2) Inject thermal preemption job if requested
        if rx_preempt.try_recv().is_ok() {
            spawn_thermal_job(&mut ready, nowi);
        }

        // 3) If no jobs ready, idle until the next release or preempt signal
        if ready.is_empty() {
            // CPU window emit every 1s even when idle
            maybe_emit_cpu(&mut win_start, &mut active_ms_acc).await;

            // Sleep until the earliest next release (min next_release over tasks)
            if let Some(sleep_until) = tasks.iter().map(|t| t.next_release).min() {
                tokio::select! {
                    _ = time::sleep_until(sleep_until) => {},
                    // wake early if thermal preemption arrives
                    _ = rx_preempt.recv() => {
                        spawn_thermal_job(&mut ready, Instant::now());
                    }
                }
            } else {
                time::sleep(Duration::from_millis(1)).await; // fallback
            }
            continue;
        }

        // 4) Pick highest-priority ready (front of sorted vec)
        let mut job = ready.remove(0);
        let spec_is_thermal = job.task_idx == usize::MAX;
        let (task_name, deadline_dur) = if spec_is_thermal {
            ("thermal_control", Duration::from_millis(20))
        } else {
            let t = &tasks[job.task_idx];
            (t.name, t.deadline)
        };

        let actual_start = Instant::now();
        let expected_start = job.release;
        let start_delay_ms =
            (actual_start.saturating_duration_since(expected_start)).as_secs_f64() * 1e3;

        // 5) Run cooperatively in slices; preempt if a higher-priority job arrives
        const SLICE_MS: f64 = 0.5;
        let mut ran_ms: f64 = 0.0;

        // Process the job in larger chunks to reduce overhead
        while job.remaining_ms > 0.0 {
            // simulate "doing work" for one slice (we just account time; don't busy-spin)
            let slice = job.remaining_ms.min(SLICE_MS);
            time::sleep(Duration::from_micros((slice * 1000.0) as u64)).await;
            job.remaining_ms -= slice;
            ran_ms += slice;
            active_ms_acc += slice;

            // Only check for scheduling events every few slices to reduce overhead
            if ran_ms % 1.0 < SLICE_MS {  // Check roughly every 1ms of work
                // new releases?
                let nowi = Instant::now();
                release_due(&mut tasks, &mut ready, nowi);

                // thermal preempt?
                if rx_preempt.try_recv().is_ok() {
                    spawn_thermal_job(&mut ready, nowi);
                }

                // RM preemption: if a *higher-priority* job is now ready, preempt current
                if let Some(next) = ready.first() {
                    let higher_prio = if next.task_idx == usize::MAX {
                        true // thermal always higher
                    } else if spec_is_thermal {
                        false
                    } else {
                        let p_cur = tasks[job.task_idx].rm_priority;
                        let p_nxt = tasks[next.task_idx].rm_priority;
                        p_nxt < p_cur
                    };
                    if higher_prio {
                        job.preemptions += 1;
                        // put current job back into the ready queue
                        ready.push(job);
                        ready.sort_by(|a, b| {
                            // thermal first, then RM priority
                            if a.task_idx == usize::MAX && b.task_idx != usize::MAX { Ordering::Less }
                            else if b.task_idx == usize::MAX && a.task_idx != usize::MAX { Ordering::Greater }
                            else {
                                let pa = if a.task_idx == usize::MAX { 0 } else { tasks[a.task_idx].rm_priority };
                                let pb = if b.task_idx == usize::MAX { 0 } else { tasks[b.task_idx].rm_priority };
                                match pa.cmp(&pb) {
                                    Ordering::Equal => a.deadline.cmp(&b.deadline),
                                    other => other,
                                }
                            }
                        });
                        // Reschedule
                        job = ready.remove(0);
                        continue;
                    }
                }
            }
        }

        // 6) Completion + deadline checks
        let finish = Instant::now();
        let completion_delay_ms = if finish > job.deadline {
            (finish - job.deadline).as_secs_f64() * 1e3
        } else {
            0.0
        };

        // Log per-job row (meets: start/finish delay, jitter, preemptions)
        logging::csv::log_sched_event(
            task_name,
            job.seq,
            start_delay_ms,
            completion_delay_ms,
            ran_ms,
            job.preemptions,
            deadline_dur.as_secs_f64() * 1e3,
        ).await;

        if completion_delay_ms > 0.0 {
            warn!(
                task = task_name,
                seq = job.seq,
                start_delay_ms,
                completion_delay_ms,
                "deadline violation"
            );
        }

        // 7) Periodic CPU row (once per ~1s window)
        maybe_emit_cpu(&mut win_start, &mut active_ms_acc).await;
    }
}

async fn maybe_emit_cpu(win_start: &mut Instant, active_ms_acc: &mut f64) {
    let win = win_start.elapsed();
    if win >= Duration::from_secs(1) {
        let window_ms = win.as_secs_f64() * 1e3;
        let active_ms = *active_ms_acc;
        let idle_ms = (window_ms - active_ms).max(0.0);
        crate::logging::csv::log_cpu(window_ms as u64, active_ms, idle_ms).await;
        *win_start = Instant::now();
        *active_ms_acc = 0.0;
    }
}
