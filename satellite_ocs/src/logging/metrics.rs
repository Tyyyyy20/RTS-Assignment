use std::fs::File;
use std::io::{BufRead, BufReader};
use chrono::{DateTime, Utc};
use tracing::info;
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
        // Sort is required for percentiles
        self.values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((self.values.len() as f64) * (p / 100.0)).round() as usize;
        let idx = idx.saturating_sub(1).min(self.values.len() - 1);
        self.values[idx]
    }
    
    pub fn format_stats(&mut self) -> String {
        if self.count() == 0 {
            return "N/A".to_string();
        }
        // Mean and Avg are treated the same here
        let m = self.mean();
        let min = self.min();
        let max = self.max();
        let p95 = self.percentile(95.0);
        let p99 = self.percentile(99.0);
        
        format!("Min={:.3} Mean={:.3} Avg={:.3} Max={:.3} P95={:.3} P99={:.3}", 
            min, m, m, max, p95, p99)
    }
}

pub async fn print_final_summary(start_time: DateTime<Utc>) {
    // --- 1. CPU Data ---
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

    // --- 2. Scheduler & Timing ---
    let mut sched_drift = Stats::default();
    let mut task_exec_jitter = Stats::default();
    let mut preemption_count = 0;
    let mut total_tasks = 0;
    let mut deadline_violations = 0;

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
                    // FIX: Accurate deadline check
                    if comp_delay > deadline {
                        deadline_violations += 1;
                    }
                }
            }
        }
    }

    // --- 3. Sensor Acquisition ---
    let mut sensor_counts: HashMap<String, usize> = HashMap::new();
    let mut sensor_jitter: HashMap<String, Stats> = HashMap::new();
    let mut sensor_drift: HashMap<String, Stats> = HashMap::new();
    let mut sensor_latency: HashMap<String, Stats> = HashMap::new();

    if let Ok(file) = File::open("logs/sensors.csv") {
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 6 {
                let sensor = parts[1].to_string();
                *sensor_counts.entry(sensor.clone()).or_default() += 1;
                if let (Ok(jitter), Ok(drift), Ok(lat)) = (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>()) {
                    sensor_jitter.entry(sensor.clone()).or_default().push(jitter);
                    sensor_drift.entry(sensor.clone()).or_default().push(drift);
                    sensor_latency.entry(sensor.clone()).or_default().push(lat);
                }
            }
        }
    }

    // --- 4. Pipeline & Downlink ---
    let mut tx_queue_residency = Stats::default();
    let mut tx_buffer_fill = Stats::default();
    let mut total_pkts_gen = 0;
    let mut total_pkts_sent = 0;
    let mut dl_latency = Stats::default();
    let mut dl_intervals = Stats::default();
    let mut dl_missed_windows = 0;
    let mut last_sent_ts: Option<DateTime<Utc>> = None;

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
                let batch = parts[1].parse::<usize>().unwrap_or(0);
                let event = parts[5].trim();
                total_pkts_gen += batch;

                if event == "sent" {
                    total_pkts_sent += batch;
                    if let Ok(q_ms) = parts[2].parse::<f64>() { dl_latency.push(q_ms); }
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

    // --- 5. Health & Faults ---
    let mut active_faults = 0;
    let mut injected_faults = 0;
    let mut fault_recovery = Stats::default();
    let mut is_aborted = false;

    if let Ok(file) = File::open("logs/faults.csv") {
        let mut open_faults: HashMap<String, bool> = HashMap::new();
        for line in BufReader::new(file).lines().skip(1).flatten() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 8 {
                let event = parts[1];
                let f_id = parts[2];
                if event == "inject" {
                    injected_faults += 1;
                    open_faults.insert(f_id.to_string(), true);
                } else if event == "recovery" {
                    open_faults.remove(f_id);
                    if let Ok(rec) = parts[7].parse::<f64>() {
                        fault_recovery.push(rec);
                        if rec > 200.0 { is_aborted = true; }
                    }
                }
            }
        }
        active_faults = open_faults.len();
    }

    // --- Final Assembly & Formatting ---
    let runtime = Utc::now() - start_time;
    let mission_status = if is_aborted { "Aborted" } else if tx_buffer_fill.max() > 80.0 { "Degraded" } else { "Nominal" };

    info!("========== FINAL SATELLITE SUMMARY ==========");
    info!("1. General System Overview");
    info!("Total Runtime: {}m {}s", runtime.num_minutes(), runtime.num_seconds() % 60);
    info!("Mission Status: {}", mission_status);
    info!("CPU Utilization (Avg): {:.2}%", cpu_stats.mean());
    info!("");

    info!("2. Timing & Real-Time Performance");
    info!("Scheduling Drift (ms): {}", sched_drift.format_stats());
    info!("Task Execution Jitter (ms): {}", task_exec_jitter.format_stats());
    info!("Preemption Count: {}", preemption_count);
    let miss_rate = if total_tasks > 0 { (deadline_violations as f64 / total_tasks as f64) * 100.0 } else { 0.0 };
    info!("Deadline Violations: {} (Miss Rate: {:.2}%)", deadline_violations, miss_rate);
    info!("");

    info!("3. Sensor Acquisition");
    // FIX: Using Clones to satisfy the borrow checker
    let mut sorted_sensors: Vec<_> = sensor_counts.keys().collect();
    sorted_sensors.sort();

    for sensor in sorted_sensors {
        let mut drift = sensor_drift.get(sensor).cloned().unwrap_or_default();
        let mut jitter = sensor_jitter.get(sensor).cloned().unwrap_or_default();
        let mut lat = sensor_latency.get(sensor).cloned().unwrap_or_default();
        
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
    info!("");

    info!("5. System Health & Load");
    info!("Peak CPU Load: {:.2}%", cpu_stats.max());
    info!("Active Faults: {} / Total Injected: {}", active_faults, injected_faults);
    info!("Recovery Latency (ms): {}", fault_recovery.format_stats());
    info!("");

    info!("6. Safety & System State");
    info!("Abort Triggered: {}", if is_aborted { "YES" } else { "NO" });
    info!("Final State: {}", mission_status);
    info!("========== END SATELLITE SUMMARY ==========");
}

// use std::fs::File;
// use std::io::{BufRead, BufReader};
// use chrono::{DateTime, Utc};
// use tracing::info;
// use std::collections::HashMap;

// #[derive(Default, Clone)]
// pub struct Metrics;

// #[derive(Default)]
// pub struct Stats {
//     values: Vec<f64>,
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
//         self.values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
//         let idx = ((self.values.len() as f64) * (p / 100.0)).round() as usize;
//         let idx = idx.saturating_sub(1).min(self.values.len() - 1);
//         self.values[idx]
//     }
    
//     pub fn format_stats(&mut self) -> String {
//         if self.count() == 0 {
//             return "N/A".to_string();
//         }
//         format!("Min={:.3} Mean={:.3} Avg={:.3} Max={:.3} P95={:.3} P99={:.3}", 
//             self.min(), self.mean(), self.mean(), self.max(), self.percentile(95.0), self.percentile(99.0))
//     }
// }

// pub async fn print_final_summary(start_time: DateTime<Utc>) {
//     // 1. cpu.csv
//     let mut cpu_active = 0.0;
//     let mut cpu_window = 0.0;
//     if let Ok(file) = File::open("logs/cpu.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 4 {
//                 if let (Ok(win), Ok(act)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) {
//                     cpu_window += win;
//                     cpu_active += act;
//                 }
//             }
//         }
//     }
//     let cpu_util = if cpu_window > 0.0 { (cpu_active / cpu_window) * 100.0 } else { 0.0 };

//     // 2. scheduler.csv
//     let mut sched_drift = Stats::default();
//     let mut task_exec_jitter = Stats::default();
//     let mut preemption_count = 0;
//     let mut total_tasks = 0;
//     let mut deadline_violations = 0;
//     let mut sensor_sched_drift: HashMap<String, Stats> = HashMap::new();

//     if let Ok(file) = File::open("logs/scheduler.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 8 {
//                 let task = parts[1].to_string();
//                 if let (Ok(start_delay), Ok(comp_delay), Ok(runtime), Ok(preempts)) = 
//                     (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>(), parts[6].parse::<u32>()) 
//                 {
//                     total_tasks += 1;
//                     sched_drift.push(start_delay);
//                     task_exec_jitter.push(runtime);
//                     preemption_count += preempts;
//                     if comp_delay > 0.0 {
//                         deadline_violations += 1;
//                     }
//                     if task.contains("thermal") || task.contains("power") || task.contains("attitude") || task.contains("sensor") {
//                          sensor_sched_drift.entry(task.clone()).or_default().push(start_delay);
//                     }
//                 }
//             }
//         }
//     }

//     // 3. sensors.csv
//     let mut sensor_counts: HashMap<String, usize> = HashMap::new();
//     let mut sensor_drift: HashMap<String, Stats> = HashMap::new();
//     let mut sensor_jitter: HashMap<String, Stats> = HashMap::new();
//     let mut sensor_latency: HashMap<String, Stats> = HashMap::new();

//     if let Ok(file) = File::open("logs/sensors.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 8 {
//                 let sensor = parts[1].to_string();
//                 *sensor_counts.entry(sensor.clone()).or_default() += 1;
//                 if let (Ok(jitter), Ok(drift), Ok(lat)) = 
//                     (parts[3].parse::<f64>(), parts[4].parse::<f64>(), parts[5].parse::<f64>()) 
//                 {
//                     sensor_jitter.entry(sensor.clone()).or_default().push(jitter);
//                     sensor_drift.entry(sensor.clone()).or_default().push(drift);
//                     sensor_latency.entry(sensor.clone()).or_default().push(lat);
//                 }
//             }
//         }
//     }

//     // 4. drops.csv
//     let mut drops: HashMap<String, usize> = HashMap::new();
//     if let Ok(file) = File::open("logs/drops.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 3 {
//                 if let Ok(qty) = parts[2].parse::<usize>() {
//                     *drops.entry(parts[1].to_string()).or_default() += qty;
//                 }
//             }
//         }
//     }

//     // 5. txqueue.csv
//     let mut tx_queue_residency = Stats::default();
//     let mut tx_buffer_fill = Stats::default();
//     let mut degraded_duration_ms = 0.0;
//     let mut is_aborted = false;
//     let mut prev_ts: Option<DateTime<Utc>> = None;
//     let mut was_degraded = false;

//     if let Ok(file) = File::open("logs/txqueue.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 3 {
//                 if let (Ok(ts), Ok(res_ms), Ok(fill)) = 
//                     (DateTime::parse_from_rfc3339(parts[0]), parts[1].parse::<f64>(), parts[2].parse::<f64>()) 
//                 {
//                     let ts_utc = ts.with_timezone(&Utc);
//                     tx_queue_residency.push(res_ms);
//                     tx_buffer_fill.push(fill);
                    
//                     let is_degraded = fill > 80.0;
//                     if was_degraded {
//                         if let Some(prev) = prev_ts {
//                             degraded_duration_ms += (ts_utc - prev).num_milliseconds() as f64;
//                         }
//                     }
//                     was_degraded = is_degraded;
//                     prev_ts = Some(ts_utc);
//                 }
//             }
//         }
//     }

//     // 6. downlink.csv
//     let mut total_pkts_gen = 0;
//     let mut total_pkts_sent = 0;
//     let mut dl_latency = Stats::default();
//     let mut dl_intervals = Stats::default();
//     let mut dl_missed_windows = 0;
//     let mut dl_sent_events = 0;
//     let mut last_dl_sent_ts: Option<DateTime<Utc>> = None;

//     if let Ok(file) = File::open("logs/downlink.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 6 {
//                 if let (Ok(ts), Ok(batch), Ok(avg_q)) = 
//                     (DateTime::parse_from_rfc3339(parts[0]), parts[1].parse::<usize>(), parts[2].parse::<f64>()) 
//                 {
//                     let ts_utc = ts.with_timezone(&Utc);
//                     let event = parts[5].trim();
//                     if !event.contains("init") && !event.contains("timeout") { 
//                          // Treat not missed_init/timeout as prep equivalent. 
//                          total_pkts_gen += batch; 
//                     } else if event.contains("miss") || event.contains("timeout") {
//                          total_pkts_gen += batch; // Still generated/prepared
//                     }
                    
//                     if event == "sent" || event == "transmission_success" || !event.contains("miss") && !event.contains("time") {
//                         total_pkts_sent += batch;
//                         dl_latency.push(avg_q);
//                         dl_sent_events += 1;
//                         if let Some(prev) = last_dl_sent_ts {
//                             dl_intervals.push((ts_utc - prev).num_milliseconds() as f64);
//                         }
//                         last_dl_sent_ts = Some(ts_utc);
//                     } else if event.contains("missed") || event.contains("timeout") {
//                         dl_missed_windows += 1;
//                     }
//                 }
//             }
//         }
//     }

//     // 7. faults.csv
//     let mut injected_faults = 0;
//     let mut active_faults: HashMap<String, bool> = HashMap::new();
//     let mut fault_recovery = Stats::default();
//     let mut safety_alerts = 0;
    
//     if let Ok(file) = File::open("logs/faults.csv") {
//         for line in BufReader::new(file).lines().skip(1).flatten() {
//             let parts: Vec<&str> = line.split(',').collect();
//             if parts.len() >= 4 {
//                 let event = parts[1];
//                 let fault_id = parts[2].to_string();
                
//                 if event == "inject" {
//                     injected_faults += 1;
//                     active_faults.insert(fault_id, true);
//                 } else if event == "recovery" {
//                     active_faults.remove(&fault_id);
//                     if parts.len() >= 9 {
//                         if let Ok(rec_ms) = parts[7].parse::<f64>() {
//                             fault_recovery.push(rec_ms);
//                             if rec_ms > 200.0 || parts[8] == "true" {
//                                 is_aborted = true;
//                             }
//                         }
//                     }
//                 } else if event == "alert" || event.contains("safety") {
//                     safety_alerts += 1;
//                 }
//             }
//         }
//     }

//     let active_fault_count = active_faults.len();

//     let mission_status = if is_aborted {
//         "Aborted"
//     } else if degraded_duration_ms > 0.0 || was_degraded {
//         "Degraded"
//     } else {
//         "Nominal"
//     };

//     let runtime = Utc::now() - start_time;
//     let runtime_secs = runtime.num_seconds().max(0);

//     info!("========== FINAL SATELLITE SUMMARY ==========");
//     info!("1. General System Overview");
//     info!("Total Runtime: {}m {}s", runtime_secs / 60, runtime_secs % 60);
//     info!("Mission Status: {}", mission_status);
//     info!("CPU Utilization (%): {:.2}%", cpu_util);
//     info!("");

//     let miss_rate = if total_tasks > 0 { (deadline_violations as f64 / total_tasks as f64) * 100.0 } else { 0.0 };
//     info!("2. Timing & Real-Time Performance");
//     info!("Scheduling Drift (ms): {}", sched_drift.format_stats());
    
//     let mut all_sensor_jitter = Stats::default();
//     for j in sensor_jitter.values_mut() {
//         for v in &j.values {
//             all_sensor_jitter.push(*v);
//         }
//     }
//     info!("Sensor Jitter (ms): {}", all_sensor_jitter.format_stats());
//     info!("Task Execution Jitter (ms): {}", task_exec_jitter.format_stats());
//     info!("Preemption Count: {}", preemption_count);
//     info!("Deadline Violations: {} (Deadline Miss Rate: {:.2}%)", deadline_violations, miss_rate);
//     info!("");

//     info!("3. Sensor Acquisition (Per Sensor: Thermal, Power, Attitude)");
//     for sensor in ["CPU", "Radio", "Battery", "thermal_control"] {
//         let sensor_name = if sensor == "CPU" || sensor == "thermal_control" { "Thermal" }
//                           else if sensor == "Battery" { "Power" }
//                           else if sensor == "Radio" { "Attitude" }
//                           else { sensor };
        
//         // Skip duplicate printing for thermal_control if CPU is also collected
//         if sensor == "thermal_control" && sensor_counts.contains_key("CPU") { continue; }

//         let mut fallback_drift = Stats::default();
//         let mut fallback_jitter = Stats::default();
//         let mut fallback_lat = Stats::default();
//         let samples = sensor_counts.get(sensor).unwrap_or(&0);
        
//         let s_drift = sensor_sched_drift.get_mut(sensor).or(sensor_drift.get_mut(sensor)).unwrap_or(&mut fallback_drift);
//         let s_jitter = sensor_jitter.get_mut(sensor).unwrap_or(&mut fallback_jitter);
//         let s_lat = sensor_latency.get_mut(sensor).unwrap_or(&mut fallback_lat);
        
//         let prio = if sensor_name == "Thermal" { "critical" }
//                    else if sensor_name == "Power" { "important" }
//                    else if sensor_name == "Attitude" { "normal" }
//                    else { "unknown" };
//         let drop_count = drops.get(prio).unwrap_or(&0) + drops.get(sensor_name).unwrap_or(&0) + drops.get(sensor).unwrap_or(&0);
        
//         if *samples > 0 || drop_count > 0 {
//             info!("--- Sensor: {} ---", sensor_name);
//             info!("  Samples Processed: {}", samples);
//             info!("  Scheduling Drift (ms): {}", s_drift.format_stats());
//             info!("  Acquisition Jitter (ms): {}", s_jitter.format_stats());
//             info!("  Sensor-to-Buffer Latency (ms): {}", s_lat.format_stats());
//             info!("  Data Loss: {}", drop_count);
//         }
//     }
//     info!("");

//     info!("4. Data Pipeline & Downlink Performance");
//     let mut all_sensor_latency = Stats::default();
//     for l in sensor_latency.values_mut() {
//         for v in &l.values {
//             all_sensor_latency.push(*v);
//         }
//     }
//     info!("Sensor Processing Latency (ms): {}", all_sensor_latency.format_stats());
//     info!("Queue Residency Time (ms): {}", tx_queue_residency.format_stats());
//     info!("Buffer Fill Level (%): {}", tx_buffer_fill.format_stats());
//     info!("Total Packets Generated: {}", total_pkts_gen);
//     info!("Total Packets Transmitted: {}", total_pkts_sent);
//     let success_rate = if dl_sent_events + dl_missed_windows > 0 {
//         (dl_sent_events as f64 / (dl_sent_events + dl_missed_windows) as f64) * 100.0
//     } else { 100.0 };
//     info!("Downlink Batch Success Rate: {:.2}%", success_rate);
//     info!("Downlink Latency (ms): {}", dl_latency.format_stats());
//     info!("Downlink Jitter (ms): {}", dl_intervals.format_stats());
//     info!("Missed Windows: {}", dl_missed_windows);
//     info!("");

//     info!("5. System Health & Load");
//     info!("CPU Utilization (%): {:.2}%", cpu_util);
//     info!("Active Faults: {}", active_fault_count);
//     info!("Injected Faults: {}", injected_faults);
//     info!("Recovery Latency (ms): {}", fault_recovery.format_stats());
//     info!("Safety Alerts: {}", safety_alerts);
//     info!("");

//     info!("6. Safety & System State");
//     info!("Degraded Mode: {}", if was_degraded || degraded_duration_ms > 0.0 { "Triggered" } else { "Nominal" });
//     info!("Degraded Mode Duration: {:.1} ms", degraded_duration_ms);
//     info!("Abort Condition: {}", if is_aborted { "Triggered" } else { "Nominal" });
//     info!("System Status Timeline: Started as Nominal{}, ended {}", 
//           if degraded_duration_ms > 0.0 { ", entered Degraded" } else { "" },
//           mission_status);
//     info!("========== END SATELLITE SUMMARY ==========");
// }
