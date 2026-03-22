use std::fs::File;
use std::io::{BufRead, BufReader};
use chrono::{DateTime, Utc};
use tracing::{info, warn};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct Stats {
    pub values: Vec<f64>,
}

impl Stats {
    pub fn push(&mut self, v: f64) {
        self.values.push(v);
    }
    
    pub fn count(&self) -> usize {
        self.values.len()
    }
    
    pub fn min(&self) -> f64 {
        if self.values.is_empty() { return 0.0; }
        self.values.iter().cloned().fold(f64::INFINITY, f64::min)
    }
    
    pub fn max(&self) -> f64 {
        if self.values.is_empty() { return 0.0; }
        self.values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }
    
    pub fn mean(&self) -> f64 {
        if self.values.is_empty() { return 0.0; }
        self.values.iter().sum::<f64>() / self.values.len() as f64
    }
    
    pub fn percentile(&mut self, p: f64) -> f64 {
        if self.values.is_empty() { return 0.0; }
        // Sorting is required for accurate percentiles
        self.values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((self.values.len() as f64) * (p / 100.0)).round() as usize;
        let idx = idx.saturating_sub(1).min(self.values.len() - 1);
        self.values[idx]
    }
    
    pub fn format_stats(&mut self) -> String {
        if self.count() == 0 {
            return "N/A".to_string();
        }
        let m = self.mean();
        let min = self.min();
        let max = self.max();
        let p95 = self.percentile(95.0);
        let p99 = self.percentile(99.0);
        
        format!("Min={:.3} Mean={:.3} Max={:.3} P95={:.3} P99={:.3}", 
            min, m, max, p95, p99)
    }
}

pub async fn print_final_summary(start_time: DateTime<Utc>) {
    // --- 1. CPU Data Acquisition ---
    let mut cpu_stats = Stats::default();
    if let Ok(file) = File::open("logs/cpu.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 5 {
                if let Ok(pct) = parts[4].parse::<f64>() {
                    cpu_stats.push(pct);
                }
            }
        }
    }

    // --- 2. Scheduler & Timing Performance ---
    let mut sched_drift = Stats::default();
    let mut task_exec_jitter = Stats::default();
    let mut slack_stats = Stats::default(); // Tracks how much time was left before the deadline
    let mut preemption_count = 0;
    let mut total_tasks = 0;
    let mut hard_deadline_violations = 0;

    if let Ok(file) = File::open("logs/scheduler.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 8 {
                if let (Ok(start_delay), Ok(comp_delay), Ok(runtime), Ok(preempts), Ok(deadline)) = 
                    (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>(), parts[6].parse::<u32>(), parts[7].parse::<f64>()) 
                {
                    total_tasks += 1;
                    sched_drift.push(start_delay);
                    task_exec_jitter.push(runtime);
                    preemption_count += preempts;
                    
                    // ACCURATE DEADLINE CHECK: 
                    // A violation only occurs if completion is past the deadline threshold.
                    if comp_delay > deadline {
                        hard_deadline_violations += 1;
                    }
                    
                    // Slack is the remaining time: (Deadline - Completion Delay)
                    slack_stats.push(deadline - comp_delay);
                }
            }
        }
    }

    // --- 3. Sensor Acquisition Analysis ---
    let mut sensor_counts: HashMap<String, usize> = HashMap::new();
    let mut sensor_jitter: HashMap<String, Stats> = HashMap::new();
    let mut sensor_drift: HashMap<String, Stats> = HashMap::new();
    let mut sensor_latency: HashMap<String, Stats> = HashMap::new();
    
    // Track sequence numbers to detect missed data cycles
    let mut last_seqs: HashMap<String, i64> = HashMap::new();
    let mut total_missed_cycles = 0;

    if let Ok(file) = File::open("logs/sensors.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 6 {
                let sensor = parts[1].to_string();
                let current_seq = parts[2].parse::<i64>().unwrap_or(0);
                
                // Check for sequence gaps (Missed Cycles)
                if let Some(&last) = last_seqs.get(&sensor) {
                    if current_seq > last + 1 {
                        total_missed_cycles += current_seq - last - 1;
                    }
                }
                last_seqs.insert(sensor.clone(), current_seq);

                *sensor_counts.entry(sensor.clone()).or_default() += 1;
                if let (Ok(jitter), Ok(drift), Ok(lat)) = (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>()) {
                    sensor_jitter.entry(sensor.clone()).or_default().push(jitter);
                    sensor_drift.entry(sensor.clone()).or_default().push(drift);
                    sensor_latency.entry(sensor.clone()).or_default().push(lat);
                }
            }
        }
    }

    // --- 4. Data Pipeline & Downlink Performance ---
    let mut tx_queue_residency = Stats::default();
    let mut tx_buffer_fill = Stats::default();
    let mut total_pkts_gen = 0;
    let mut total_pkts_sent = 0;
    let mut dl_latency = Stats::default();
    let mut dl_intervals = Stats::default();
    let mut dl_missed_windows = 0;
    let mut last_sent_ts: Option<DateTime<Utc>> = None;
    let mut cmd_latency = Stats::default();

    if let Ok(file) = File::open("logs/txqueue.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 3 {
                if let (Ok(res), Ok(fill)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
                    tx_queue_residency.push(res);
                    tx_buffer_fill.push(fill);
                }
            }
        }
    }

    if let Ok(file) = File::open("logs/downlink.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 6 {
                let batch_size = parts[1].parse::<usize>().unwrap_or(0);
                let event = parts[5].trim();
                total_pkts_gen += batch_size;

                if event == "sent" {
                    total_pkts_sent += batch_size;
                    if let Ok(prep_ms) = parts[2].parse::<f64>() { dl_latency.push(prep_ms); }
                    if let Ok(ts) = DateTime::parse_from_rfc3339(parts[0]) {
                        let ts_utc = ts.with_timezone(&Utc);
                        if let Some(prev) = last_sent_ts {
                            dl_intervals.push((ts_utc - prev).num_milliseconds() as f64);
                        }
                        last_sent_ts = Some(ts_utc);
                    }
                } else if event == "missed_init_or_timeout" {
                    dl_missed_windows += 1;
                }
            }
        }
    }

    // --- Parse Command-Response Latency ---
    if let Ok(file) = File::open("logs/command_latency.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 3 {
                if let Ok(ms) = parts[2].parse::<f64>() {
                    cmd_latency.push(ms);
                }
            }
        }
    }

    // --- 5. Health & Fault Recovery Monitoring ---
    let mut active_faults = 0;
    let mut injected_faults = 0;
    let mut fault_types: HashMap<String, usize> = HashMap::new();
    let mut fault_recovery = Stats::default();
    let mut is_aborted = false;

    if let Ok(file) = File::open("logs/faults.csv") {
        let mut open_faults: HashMap<String, bool> = HashMap::new();
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 8 {
                let event = parts[1];
                let f_id = parts[2];
                let target = parts[3].to_string();

                if event == "inject" {
                    injected_faults += 1;
                    *fault_types.entry(target).or_default() += 1;
                    open_faults.insert(f_id.to_string(), true);
                } else if event == "recovery" {
                    open_faults.remove(f_id);
                    if let Ok(rec) = parts[7].parse::<f64>() {
                        fault_recovery.push(rec);
                        // Abort if recovery takes longer than 200ms
                        if rec > 200.0 { is_aborted = true; }
                    }
                }
            }
        }
        active_faults = open_faults.len();
    }

    // --- 6. Final State Assembly & Formatting ---
    let runtime = Utc::now() - start_time;
    let buffer_peak = tx_buffer_fill.max();
    
    // Determine status based on performance
    let mission_status = if is_aborted { 
        "ABORTED" 
    } else if buffer_peak > 80.0 || hard_deadline_violations > 0 { 
        "DEGRADED" 
    } else { 
        "NOMINAL" 
    };

    info!("========== FINAL SATELLITE SUMMARY ==========");
    info!("1. General System Overview");
    info!("Total Runtime: {}m {}s", runtime.num_minutes(), runtime.num_seconds() % 60);
    info!("Mission Status: {}", mission_status);
    info!("CPU Utilization (Avg): {:.2}%", cpu_stats.mean());
    info!("");

    info!("2. Timing & Real-Time Performance");
    info!("Scheduling Drift (ms): {}", sched_drift.format_stats());
    info!("Average Slack Time (ms): {:.3} (Safety Margin)", slack_stats.mean());
    info!("Preemption Count: {}", preemption_count);
    
    let miss_rate = if total_tasks > 0 { (hard_deadline_violations as f64 / total_tasks as f64) * 100.0 } else { 0.0 };
    info!("Hard Deadline Misses: {} (Rate: {:.2}%)", hard_deadline_violations, miss_rate);
    info!("Data Continuity: {} missed cycles", total_missed_cycles);
    info!("");

    info!("3. Sensor Acquisition");
    let mut sorted_sensors: Vec<_> = sensor_counts.keys().collect();
    sorted_sensors.sort();
    for sensor in sorted_sensors {
        let mut drift = sensor_drift.entry(sensor.clone()).or_default();
        let mut jitter = sensor_jitter.entry(sensor.clone()).or_default();
        let mut lat = sensor_latency.entry(sensor.clone()).or_default();
        info!("--- Sensor: {} ---", sensor.to_uppercase());
        info!("  Samples: {}", sensor_counts[sensor]);
        info!("  Drift (ms): {}", drift.format_stats());
        info!("  Jitter (ms): {}", jitter.format_stats());
        info!("  Latency (ms): {}", lat.format_stats());
    }
    info!("");

    info!("4. Data Pipeline & Downlink Performance");
    info!("Queue Residency (ms): {}", tx_queue_residency.format_stats());
    info!("Buffer Fill (%): {}", tx_buffer_fill.format_stats());
    info!("Downlink Success Rate: {:.2}%", (total_pkts_sent as f64 / total_pkts_gen.max(1) as f64) * 100.0);
    info!("Downlink Latency (ms): {}", dl_latency.format_stats());
    info!("Downlink Jitter (ms): {}", dl_intervals.format_stats());
    info!("Missed Windows: {}", dl_missed_windows);
    info!("Internal Command Latency (ms): {}", cmd_latency.format_stats());
    info!("");

    info!("5. System Health & Load");
    info!("Peak CPU Load: {:.2}%", cpu_stats.max());
    info!("Total Injected Faults: {}", injected_faults);
    for (target, count) in fault_types {
        info!(" -> {}: {} events", target.to_uppercase(), count);
    }
    info!("Currently Active: {}", active_faults);
    info!("Recovery Latency (ms): {}", fault_recovery.format_stats());

    info!("6. Safety & System State");
    info!("Abort Triggered: {}", if is_aborted { "YES" } else { "NO" });
    info!("Final State: {}", mission_status);
    info!("========== END SATELLITE SUMMARY ==========");
}

// use std::fs::File;
// use std::io::{BufRead, BufReader};
// use chrono::{DateTime, Utc};
// use tracing::{info, warn};
// use std::collections::HashMap;

// #[derive(Default, Clone)]
// pub struct Stats {
//     pub values: Vec<f64>,
// }

// impl Stats {
//     pub fn push(&mut self, v: f64) {
//         self.values.push(v);
//     }
    
//     pub fn count(&self) -> usize {
//         self.values.len()
//     }
    
//     pub fn min(&self) -> f64 {
//         if self.values.is_empty() { return 0.0; }
//         self.values.iter().cloned().fold(f64::INFINITY, f64::min)
//     }
    
//     pub fn max(&self) -> f64 {
//         if self.values.is_empty() { return 0.0; }
//         self.values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
//     }
    
//     pub fn mean(&self) -> f64 {
//         if self.values.is_empty() { return 0.0; }
//         self.values.iter().sum::<f64>() / self.values.len() as f64
//     }
    
//     pub fn percentile(&mut self, p: f64) -> f64 {
//         if self.values.is_empty() { return 0.0; }
//         // Sort is required for accurate percentile calculation
//         self.values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
//         let idx = ((self.values.len() as f64) * (p / 100.0)).round() as usize;
//         let idx = idx.saturating_sub(1).min(self.values.len() - 1);
//         self.values[idx]
//     }
    
//     pub fn format_stats(&mut self) -> String {
//         if self.count() == 0 {
//             return "N/A".to_string();
//         }
//         let m = self.mean();
//         let min = self.min();
//         let max = self.max();
//         let p95 = self.percentile(95.0);
//         let p99 = self.percentile(99.0);
        
//         format!("Min={:.3} Mean={:.3} Avg={:.3} Max={:.3} P95={:.3} P99={:.3}", 
//             min, m, m, max, p95, p99)
//     }
// }

// pub async fn print_final_summary(start_time: DateTime<Utc>) {
//     // --- 1. CPU Data Acquisition ---
//     let mut cpu_stats = Stats::default();
//     if let Ok(file) = File::open("logs/cpu.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 5 {
//                 if let Ok(pct) = parts[4].parse::<f64>() {
//                     cpu_stats.push(pct);
//                 }
//             }
//         }
//     }

//     // --- 2. Scheduler & Timing Performance ---
//     let mut sched_drift = Stats::default();
//     let mut task_exec_jitter = Stats::default();
//     let mut preemption_count = 0;
//     let mut total_tasks = 0;
//     let mut deadline_violations = 0;

//     if let Ok(file) = File::open("logs/scheduler.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 8 {
//                 if let (Ok(start_delay), Ok(comp_delay), Ok(runtime), Ok(preempts), Ok(deadline)) = 
//                     (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>(), parts[6].parse::<u32>(), parts[7].parse::<f64>()) 
//                 {
//                     total_tasks += 1;
//                     sched_drift.push(start_delay);
//                     task_exec_jitter.push(runtime);
//                     preemption_count += preempts;
//                     if comp_delay > 0.0 {
//                         deadline_violations += 1;
//                     }
//                 }
//             }
//         }
//     }

//     // --- 3. Sensor Acquisition Analysis ---
//     let mut sensor_counts: HashMap<String, usize> = HashMap::new();
//     let mut sensor_jitter: HashMap<String, Stats> = HashMap::new();
//     let mut sensor_drift: HashMap<String, Stats> = HashMap::new();
//     let mut sensor_latency: HashMap<String, Stats> = HashMap::new();

//     if let Ok(file) = File::open("logs/sensors.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 6 {
//                 let sensor = parts[1].to_string();
//                 *sensor_counts.entry(sensor.clone()).or_default() += 1;
//                 if let (Ok(jitter), Ok(drift), Ok(lat)) = (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>()) {
//                     sensor_jitter.entry(sensor.clone()).or_default().push(jitter);
//                     sensor_drift.entry(sensor.clone()).or_default().push(drift);
//                     sensor_latency.entry(sensor.clone()).or_default().push(lat);
//                 }
//             }
//         }
//     }

//     // --- 4. Data Pipeline & Downlink Performance ---
//     let mut tx_queue_residency = Stats::default();
//     let mut tx_buffer_fill = Stats::default();
//     let mut total_pkts_gen = 0;
//     let mut total_pkts_sent = 0;
//     let mut dl_latency = Stats::default();
//     let mut dl_intervals = Stats::default();
//     let mut dl_missed_windows = 0;
//     let mut last_sent_ts: Option<DateTime<Utc>> = None;

//     if let Ok(file) = File::open("logs/txqueue.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 3 {
//                 if let (Ok(res), Ok(fill)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
//                     tx_queue_residency.push(res);
//                     tx_buffer_fill.push(fill);
//                 }
//             }
//         }
//     }

//     if let Ok(file) = File::open("logs/downlink.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 6 {
//                 let batch_size = parts[1].parse::<usize>().unwrap_or(0);
//                 let event = parts[5].trim();
//                 total_pkts_gen += batch_size;

//                 if event == "sent" {
//                     total_pkts_sent += batch_size;
//                     if let Ok(prep_ms) = parts[2].parse::<f64>() { dl_latency.push(prep_ms); }
//                     if let Ok(ts) = DateTime::parse_from_rfc3339(parts[0]) {
//                         let ts_utc = ts.with_timezone(&Utc);
//                         if let Some(prev) = last_sent_ts {
//                             dl_intervals.push((ts_utc - prev).num_milliseconds() as f64);
//                         }
//                         last_sent_ts = Some(ts_utc);
//                     }
//                 } else if event == "missed_init_or_timeout" {
//                     dl_missed_windows += 1;
//                 }
//             }
//         }
//     }

//     // --- 5. Health & Fault Recovery Monitoring ---
//     let mut active_faults = 0;
//     let mut injected_faults = 0;
//     let mut fault_types: HashMap<String, usize> = HashMap::new(); // Tracks Thermal/Power/Attitude counts
//     let mut fault_recovery = Stats::default();
//     let mut is_aborted = false;

//     if let Ok(file) = File::open("logs/faults.csv") {
//         let mut open_faults: HashMap<String, bool> = HashMap::new();
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 8 {
//                 let event = parts[1];
//                 let f_id = parts[2];
//                 let target = parts[3].to_string(); // The component target (Thermal/Power/Attitude)

//                 if event == "inject" {
//                     injected_faults += 1;
//                     *fault_types.entry(target).or_default() += 1;
//                     open_faults.insert(f_id.to_string(), true);
//                 } else if event == "recovery" {
//                     open_faults.remove(f_id);
//                     if let Ok(rec) = parts[7].parse::<f64>() {
//                         fault_recovery.push(rec);
//                         if rec > 200.0 { is_aborted = true; }
//                     }
//                 }
//             }
//         }
//         active_faults = open_faults.len();
//     }

//     // --- 6. Final State Assembly & Formatting ---
//     let runtime = Utc::now() - start_time;
//     let mission_status = if is_aborted { "Aborted" } else if tx_buffer_fill.max() > 80.0 { "Degraded" } else { "Nominal" };

//     info!("========== FINAL SATELLITE SUMMARY ==========");
//     info!("1. General System Overview");
//     info!("Total Runtime: {}m {}s", runtime.num_minutes(), runtime.num_seconds() % 60);
//     info!("Mission Status: {}", mission_status);
//     info!("CPU Utilization (Avg): {:.2}%", cpu_stats.mean());
//     info!("");

//     info!("2. Timing & Real-Time Performance");
//     info!("Scheduling Drift (ms): {}", sched_drift.format_stats());
//     info!("Task Execution Jitter (ms): {}", task_exec_jitter.format_stats());
//     info!("Preemption Count: {}", preemption_count);
//     let miss_rate = if total_tasks > 0 { (deadline_violations as f64 / total_tasks as f64) * 100.0 } else { 0.0 };
//     info!("Deadline Violations: {} (Miss Rate: {:.2}%)", deadline_violations, miss_rate);
//     info!("");

//     info!("3. Sensor Acquisition");
//     let mut sorted_sensors: Vec<_> = sensor_counts.keys().collect();
//     sorted_sensors.sort();
//     for sensor in sorted_sensors {
//         let mut drift = sensor_drift.get(sensor).cloned().unwrap_or_default();
//         let mut jitter = sensor_jitter.get(sensor).cloned().unwrap_or_default();
//         let mut lat = sensor_latency.get(sensor).cloned().unwrap_or_default();
//         info!("--- Sensor: {} ---", sensor.to_uppercase());
//         info!("  Samples: {}", sensor_counts[sensor]);
//         info!("  Drift (ms): {}", drift.format_stats());
//         info!("  Jitter (ms): {}", jitter.format_stats());
//         info!("  Latency (ms): {}", lat.format_stats());
//     }
//     info!("");

//     info!("4. Data Pipeline & Downlink Performance");
//     info!("Queue Residency (ms): {}", tx_queue_residency.format_stats());
//     info!("Buffer Fill (%): {}", tx_buffer_fill.format_stats());
//     info!("Downlink Success Rate: {:.2}%", (total_pkts_sent as f64 / total_pkts_gen.max(1) as f64) * 100.0);
//     info!("Downlink Latency (ms): {}", dl_latency.format_stats());
//     info!("Downlink Jitter (ms): {}", dl_intervals.format_stats());
//     info!("Missed Windows: {}", dl_missed_windows);
//     info!("");

//     info!("5. System Health & Load");
//     info!("Peak CPU Load: {:.2}%", cpu_stats.max());
//     info!("Total Injected Faults: {}", injected_faults);

//     // Display the breakdown of specific faults
//     for (target, count) in fault_types {
//         info!(" -> {}: {} events", target.to_uppercase(), count);
//     }

//     info!("Currently Active: {}", active_faults);
//     info!("Recovery Latency (ms): {}", fault_recovery.format_stats());

//     info!("6. Safety & System State");
//     info!("Abort Triggered: {}", if is_aborted { "YES" } else { "NO" });
//     info!("Final State: {}", mission_status);
//     info!("========== END SATELLITE SUMMARY ==========");
// }

