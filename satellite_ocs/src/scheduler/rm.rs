// src/scheduler/rm.rs
use crate::{config::Config, logging};
use super::PREEMPT_CH;

use std::cmp::Ordering;
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
    wcet_ms: f64,            
    rm_priority: u8,         
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

#[derive(Debug, Clone)]
struct Job {
    task_idx: usize,
    release: Instant,
    deadline: Instant,
    seq: u64,
    remaining_ms: f64,
    preemptions: u32,
}

pub async fn spawn_rm(_cfg: Config) {
    let now = Instant::now();

    let mut tasks = vec![
        RtTask::new("antenna_alignment",  50, 3.0, 1, now),   
        RtTask::new("data_compression",  100, 6.0, 2, now),   
        RtTask::new("health_monitor",   1000, 2.0, 3, now),   
    ];

    let (tx_preempt, mut rx_preempt) = mpsc::channel::<()>(16);
    let _ = PREEMPT_CH.set(tx_preempt);

    let mut ready: Vec<Job> = Vec::new();
    let mut win_start = Instant::now();
    let mut active_ms_acc: f64 = 0.0;

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
                t.next_release += t.period;
                t.next_deadline += t.deadline;
            }
        }
        ready.sort_by(|a, b| {
            let pa = if a.task_idx == usize::MAX { 0 } else { tasks[a.task_idx].rm_priority };
            let pb = if b.task_idx == usize::MAX { 0 } else { tasks[b.task_idx].rm_priority };
            match pa.cmp(&pb) {
                Ordering::Equal => a.deadline.cmp(&b.deadline),
                other => other,
            }
        });
    };

    let mut spawn_thermal_job = |ready: &mut Vec<Job>, now: Instant| {
        let job = Job {
            task_idx: usize::MAX,
            release: now,
            deadline: now + Duration::from_millis(20), 
            seq: 0,
            remaining_ms: 2.0, 
            preemptions: 0,
        };
        ready.push(job);
        ready.sort_by(|a, b| {
            if a.task_idx == usize::MAX && b.task_idx != usize::MAX { Ordering::Less }
            else if b.task_idx == usize::MAX && a.task_idx != usize::MAX { Ordering::Greater }
            else { Ordering::Equal }
        });
        info!("RM: thermal_control job injected (preemption)");
    };

    loop {
        let nowi = Instant::now();
        release_due(&mut tasks, &mut ready, nowi);

        if rx_preempt.try_recv().is_ok() {
            spawn_thermal_job(&mut ready, nowi);
        }

        if ready.is_empty() {
            maybe_emit_cpu(&mut win_start, &mut active_ms_acc).await;
            if let Some(sleep_until) = tasks.iter().map(|t| t.next_release).min() {
                tokio::select! {
                    _ = time::sleep_until(sleep_until) => {},
                    _ = rx_preempt.recv() => {
                        spawn_thermal_job(&mut ready, Instant::now());
                    }
                }
            } else {
                time::sleep(Duration::from_millis(1)).await;
            }
            continue;
        }

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
        let start_delay_ms = (actual_start.saturating_duration_since(expected_start)).as_secs_f64() * 1e3;

        while job.remaining_ms > 0.0 {
            let current_prio = if spec_is_thermal { 0 } else { tasks[job.task_idx].rm_priority };
            
            // Find how long we can run before a higher priority task is released
            let mut ms_to_next_preemption = job.remaining_ms;
            let now_calc = Instant::now();
            for t in tasks.iter() {
                if t.rm_priority < current_prio {
                    let until_release = t.next_release.saturating_duration_since(now_calc).as_secs_f64() * 1000.0;
                    if until_release < ms_to_next_preemption {
                        ms_to_next_preemption = until_release;
                    }
                }
            }

            let run_for_ms = ms_to_next_preemption.max(0.1); 
            
            // =========================================================================
            // CONCURRENT ASYNC YIELD: Simulates execution time without blocking Tokio
            // =========================================================================
            let target_end = Instant::now() + Duration::from_micros((run_for_ms * 1000.0) as u64);

            tokio::select! {
                _ = async {
                    while Instant::now() < target_end {
                        tokio::task::yield_now().await;
                    }
                } => {
                    job.remaining_ms -= run_for_ms;
                    active_ms_acc += run_for_ms;
                }
                _ = rx_preempt.recv() => {
                    spawn_thermal_job(&mut ready, Instant::now());
                    job.preemptions += 1;
                    ready.push(job.clone());
                    break; 
                }
            }
            // =========================================================================

            let now_after = Instant::now();
            release_due(&mut tasks, &mut ready, now_after);
            
            // If a higher priority task was released, push current job back to ready queue
            if let Some(next) = ready.first() {
                let p_nxt = if next.task_idx == usize::MAX { 0 } else { tasks[next.task_idx].rm_priority };
                if p_nxt < current_prio {
                    job.preemptions += 1;
                    ready.push(job.clone());
                    break;
                }
            }
        }

        if job.remaining_ms <= 0.0 {
            let finish = Instant::now();
            let completion_delay_ms = if finish > job.deadline {
                (finish - job.deadline).as_secs_f64() * 1e3
            } else {
                0.0
            };

            logging::csv::log_sched_event(
                task_name,
                job.seq,
                start_delay_ms,
                completion_delay_ms,
                if spec_is_thermal { 2.0 } else { tasks[job.task_idx].wcet_ms },
                job.preemptions,
                deadline_dur.as_secs_f64() * 1e3,
            ).await;

            if completion_delay_ms > 0.0 {
                warn!(task = task_name, seq = job.seq, start_delay_ms, completion_delay_ms, "deadline violation");
            }
        } else {
            // Re-sort queue if job was pushed back
            ready.sort_by(|a, b| {
                let pa = if a.task_idx == usize::MAX { 0 } else { tasks[a.task_idx].rm_priority };
                let pb = if b.task_idx == usize::MAX { 0 } else { tasks[b.task_idx].rm_priority };
                match pa.cmp(&pb) {
                    Ordering::Equal => a.deadline.cmp(&b.deadline),
                    other => other,
                }
            });
        }
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


