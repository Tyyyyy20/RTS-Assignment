//performance.rs

use std::collections::{HashMap, VecDeque};
use chrono::{DateTime, Utc, Duration};
use tracing::{info, warn, debug};
use serde::{Deserialize, Serialize};

use shared_protocol::Timestamp;

/// Tracks system performance metrics and timing requirements
#[derive(Debug)]
pub struct PerformanceTracker {
    events: VecDeque<PerformanceEvent>,
    telemetry_metrics: TelemetryMetrics,
    network_metrics: NetworkMetrics,
    system_metrics: SystemMetrics,
    
    // Performance requirements tracking
    telemetry_processing_times: VecDeque<f64>,
    command_dispatch_times: VecDeque<f64>,
    packet_reception_latencies: VecDeque<f64>,
    
    // Statistics
    total_events: u64,
    performance_violations: u64,
    
    // Configuration
    max_history_size: usize,
    performance_window_minutes: i64,

    //
    uplink_jitter_threshold_ms: f64,
    uplink_interarrival_ms: VecDeque<f64>,
    uplink_jitter_latest_ms: f64,          // holds the most recent jitter value
    uplink_jitter_samples_ms: VecDeque<f64>, // rolling window of jitter samples 

    task_drift_ms: VecDeque<f64>,            // NEW
    task_drift_warn_ms: f64,                 // NEW (optional)
    task_drift_critical_ms: f64,             // NEW (optional)

    telemetry_backlog_len_samples: VecDeque<u32>,          // number of itemns in telemetry queue
    telemetry_enqueue_times: HashMap<String, Timestamp>,   // key = packet_id/frame_id
    telemetry_backlog_age_ms: VecDeque<f64>,               // age of oldest item in telemetry queue

    backlog_warn_ratio: f64,       // e.g., 0.25  (25%)
    backlog_crit_ratio: f64,       // e.g., 0.70  (70%)
    backlog_capacity: u32,     // current capacity

    backlog_warn_events: u64,
    backlog_critical_events: u64,

    // telemetry_backlog_len_samples: VecDeque<u32>,
    // telemetry_enqueue_times: HashMap<String, Timestamp>,   // key = packet_id / frame_id
    // telemetry_backlog_age_ms: VecDeque<f64>,

    // task_schedule_times: HashMap<String, Timestamp>,       // key = task_id/command_id
    // task_drift_ms: VecDeque<f64>,

    cpu_warn_threshold: f64,   // e.g., 85
    mem_warn_threshold: f64,   // e.g., 85
    load_warn_per_core: f64,   // e.g., 0.8 * cores considered “busy”
    system_health_samples: u64,

    cpu_cores: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceEvent {
    pub timestamp: Timestamp,
    pub event_type: EventType,
    pub duration_ms: f64,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    // Telemetry processing events (3ms requirement)
    TelemetryProcessed,
    TelemetryProcessingViolation,
    
    // Network/communication events  
    PacketReceived,
    PacketDelayed,              // For reception drift logging
    PacketRetransmissionRequested,
    NetworkTimeout,
    
    // Command processing events (2ms requirement for urgent commands)
    CommandDispatched,
    UrgentCommandDelayed,
    CommandValidationFailed,
    UrgentCommandDispatched,           // NEW: For priority 0 or 1 commands
    CommandDeadlineViolation,          // NEW: For commands missing deadlines
    NetworkDeadlineViolation,          // NEW: For network send >2ms violations
    CommandDispatchError,              // NEW: For command dispatch failures
    
    // Fault handling events
    FaultDetected,
    EmergencyResponseTriggered,
    SafetyInterlockActivated,
    
    // System health events
    SystemHealthUpdate,
    PerformanceDegradation,
    ResourceUtilizationHigh,

    // DECODE TIME VIOLATIONS - NEW
    PacketDecodeViolation,     // if decode_time_ms > 3ms

    JitterViolation,
    TelemetryBacklogWarning,
    TelemetryBacklogCritical,
    TaskExecutionDrift,
    SchedulerPrecisionViolation,

    UplinkIntervalSample,    // will be carrying interval + expected + jitter in metadata

    TelemetryEnqueued,   // metadata: packet_id, queue_len, queue_capacity
    TelemetryDequeued,   // metadata: packet_id, queue_len, queue_capacity
    TelemetryDropped,    // metadata: packet_id, reason
}

#[derive(Debug, Clone)]
struct TelemetryMetrics {
    total_packets_processed: u64,
    avg_processing_time_ms: f64,
    max_processing_time_ms: f64,
    processing_violations_3ms: u64,  // Violating 3ms requirement
    sensor_readings_processed: u64,
    delayed_packets_detected: u64,
}

#[derive(Debug, Clone)]
struct NetworkMetrics {
    total_packets_received: u64,
    avg_reception_latency_ms: f64,
    max_reception_latency_ms: f64,
    packet_loss_count: u64,
    retransmission_requests: u64,
    network_timeouts: u64,
    reception_drift_violations: u64, // Packets outside expected timing
    packet_decode_violations_3ms: u64,
    network_deadline_violations_2ms: u64,
}

#[derive(Debug, Clone)]
struct SystemMetrics {
    uptime_seconds: u64,
    cpu_usage_avg: f64,
    memory_usage_avg: f64,
    active_faults: u32,
    emergency_responses: u64,
    system_degradation_events: u64,
    command_deadline_violations: u64,

    cpu_usage_latest: f64,
    memory_usage_latest: f64,
    load1_latest: f64,
    cpu_usage_peak: f64,
    memory_usage_peak: f64,
    load1_peak: f64,
    load1_avg: f64,
}

impl PerformanceTracker {
    pub fn new() -> Self {
        info!("Initializing Performance Tracker");
        
        Self {
            events: VecDeque::new(),
            telemetry_metrics: TelemetryMetrics {
                total_packets_processed: 0,
                avg_processing_time_ms: 0.0,
                max_processing_time_ms: 0.0,
                processing_violations_3ms: 0,
                sensor_readings_processed: 0,
                delayed_packets_detected: 0,
            },
            network_metrics: NetworkMetrics {
                total_packets_received: 0,
                avg_reception_latency_ms: 0.0,
                max_reception_latency_ms: 0.0,
                packet_loss_count: 0,
                retransmission_requests: 0,
                network_timeouts: 0,
                reception_drift_violations: 0,
                packet_decode_violations_3ms: 0,
                network_deadline_violations_2ms: 0,
            },
            system_metrics: SystemMetrics {
                uptime_seconds: 0,
                cpu_usage_avg: 0.0,
                memory_usage_avg: 0.0,
                active_faults: 0,
                emergency_responses: 0,
                system_degradation_events: 0,
                command_deadline_violations: 0,
                
                cpu_usage_latest: 0.0,
                memory_usage_latest: 0.0,
                load1_latest: 0.0,
                cpu_usage_peak: 0.0,
                memory_usage_peak: 0.0,
                load1_peak: 0.0,
                load1_avg: 0.0,
            },
            telemetry_processing_times: VecDeque::new(),
            command_dispatch_times: VecDeque::new(),
            packet_reception_latencies: VecDeque::new(),
            total_events: 0,
            performance_violations: 0,
            max_history_size: 10000,
            performance_window_minutes: 30,

            // new
            uplink_jitter_threshold_ms: 10.0,
            uplink_interarrival_ms: VecDeque::new(),
            uplink_jitter_latest_ms: 0.0,
            uplink_jitter_samples_ms: VecDeque::new(),

            task_drift_ms: VecDeque::new(),
            task_drift_warn_ms: 2.0,       // warn at >2ms
            task_drift_critical_ms: 5.0,   // violation at >5ms

            telemetry_backlog_len_samples: VecDeque::new(),
            telemetry_enqueue_times: HashMap::new(),
            telemetry_backlog_age_ms: VecDeque::new(),

            backlog_warn_ratio: 0.25,
            backlog_crit_ratio: 0.70,
            backlog_capacity: 0,

            backlog_warn_events: 0,
            backlog_critical_events: 0,

            // uplink_jitter_ms: 0.0,
            // uplink_interarrival_ms: VecDeque::new(),
            // telemetry_backlog_len_samples: VecDeque::new(),
            // telemetry_enqueue_times: HashMap::new(),
            // telemetry_backlog_age_ms: VecDeque::new(),
            // task_schedule_times: HashMap::new(),
            // task_drift_ms: VecDeque::new(),

            cpu_warn_threshold: 85.0,
            mem_warn_threshold: 85.0,
            load_warn_per_core: 0.8,
            system_health_samples: 0,

            cpu_cores: num_cpus::get() as u32,
        }
    }
    
    /// Record a performance event
    pub fn record_event(&mut self, event: PerformanceEvent) {
        debug!("Recording performance event: {:?} - {:.3}ms", 
            event.event_type, event.duration_ms);
        
        // Update metrics based on event type
        self.update_metrics(&event);
        
        // Check for performance violations
        self.check_performance_violations(&event);
        
        // Add to event history
        self.events.push_back(event);
        self.total_events += 1;
        
        // Maintain history size
        if self.events.len() > self.max_history_size {
            self.events.pop_front();
        }
        
        // Clean old timing data
        self.cleanup_old_timing_data();
    }
    
    /// Update metrics based on the recorded event
    fn update_metrics(&mut self, event: &PerformanceEvent) {
        match event.event_type {
            EventType::TelemetryProcessed => {
                self.telemetry_metrics.total_packets_processed += 1;
                self.telemetry_processing_times.push_back(event.duration_ms);
                
                // Update average processing time
                let total_packets = self.telemetry_metrics.total_packets_processed as f64;
                self.telemetry_metrics.avg_processing_time_ms = 
                    (self.telemetry_metrics.avg_processing_time_ms * (total_packets - 1.0) + 
                     event.duration_ms) / total_packets;
                
                // Update max processing time
                if event.duration_ms > self.telemetry_metrics.max_processing_time_ms {
                    self.telemetry_metrics.max_processing_time_ms = event.duration_ms;
                }
                
                // Count sensor readings if provided in metadata
                if let Some(sensor_count_str) = event.metadata.get("sensor_count") {
                    if let Ok(sensor_count) = sensor_count_str.parse::<u64>() {
                        self.telemetry_metrics.sensor_readings_processed += sensor_count;
                    }
                }
                
                // Count delayed packets if provided in metadata  
                if let Some(delayed_str) = event.metadata.get("delayed_packets") {
                    if let Ok(delayed_count) = delayed_str.parse::<u64>() {
                        self.telemetry_metrics.delayed_packets_detected += delayed_count;
                    }
                }
            }
            
            EventType::PacketReceived => {
                self.network_metrics.total_packets_received += 1;
                self.packet_reception_latencies.push_back(event.duration_ms);
                
                // Update average reception latency
                let total_packets = self.network_metrics.total_packets_received as f64;
                self.network_metrics.avg_reception_latency_ms = 
                    (self.network_metrics.avg_reception_latency_ms * (total_packets - 1.0) + 
                     event.duration_ms) / total_packets;
                
                if event.duration_ms > self.network_metrics.max_reception_latency_ms {
                    self.network_metrics.max_reception_latency_ms = event.duration_ms;
                }
            }
            
            EventType::PacketDelayed => {
                self.telemetry_metrics.delayed_packets_detected += 1;
                self.network_metrics.reception_drift_violations += 1;
            }
            
            EventType::PacketRetransmissionRequested => {
                self.network_metrics.retransmission_requests += 1;
            }
            
            EventType::NetworkTimeout => {
                self.network_metrics.network_timeouts += 1;
            }
            
            EventType::CommandDispatched => {
                self.command_dispatch_times.push_back(event.duration_ms);
            }
            
            EventType::UrgentCommandDelayed => {
                self.performance_violations += 1;
            }
            
            EventType::TelemetryProcessingViolation => {
                self.telemetry_metrics.processing_violations_3ms += 1;
                self.performance_violations += 1;
            }
            
            EventType::EmergencyResponseTriggered => {
                self.system_metrics.emergency_responses += 1;
            }
            
            EventType::PerformanceDegradation => {
                self.system_metrics.system_degradation_events += 1;
            }

            EventType::PacketDecodeViolation => {  // Was: PacketDecoded
                if event.duration_ms > 3.0 {
                    self.network_metrics.packet_decode_violations_3ms += 1;
                    self.performance_violations += 1;
                }
            }

            EventType::UplinkIntervalSample => {
                if let Some(interval_s) = event.metadata.get("uplink_interval_ms") {
                    if let Ok(v) = interval_s.parse::<f64>() {
                        self.uplink_interarrival_ms.push_back(v);
                        if self.uplink_interarrival_ms.len() > 1000 { self.uplink_interarrival_ms.pop_front(); }
                    }
                }
                if let Some(jitter_s) = event.metadata.get("uplink_jitter_ms") {
                    if let Ok(j) = jitter_s.parse::<f64>() {
                        self.uplink_jitter_latest_ms = j; // keep “latest” for quick view
                        self.uplink_jitter_samples_ms.push_back(j);
                        if self.uplink_jitter_samples_ms.len() > 1000 { self.uplink_jitter_samples_ms.pop_front(); }
                    }
                }
            }
            EventType::JitterViolation => {
                self.performance_violations += 1;
            }

            EventType::TaskExecutionDrift => {
                // // Expect "drift_ms" in metadata
                // if let Some(drift_s) = event.metadata.get("drift_ms") {
                //     if let Ok(drift) = drift_s.parse::<f64>() {
                //         self.task_drift_ms.push_back(drift);
                //         // keep memory bounded
                //         if self.task_drift_ms.len() > 1000 {
                //             self.task_drift_ms.pop_front();
                //         }

                //         // Optional: classify as violation using thresholds
                //         if drift > self.task_drift_critical_ms {
                //             warn!("TASK DRIFT VIOLATION: {:.3}ms (> {:.1}ms)", drift, self.task_drift_critical_ms);
                //             self.performance_violations += 1;
                //         } else if drift > self.task_drift_warn_ms {
                //             debug!("Task drift warning: {:.3}ms (> {:.1}ms)", drift, self.task_drift_warn_ms);
                //         }
                //     }
                // }

                // Prefer metadata, but fall back to duration_ms
                let drift = event
                    .metadata
                    .get("drift_ms")
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(event.duration_ms);

                self.task_drift_ms.push_back(drift);
                if self.task_drift_ms.len() > 1000 {
                    self.task_drift_ms.pop_front();
                }

                if drift > self.task_drift_critical_ms {
                    warn!("TASK DRIFT VIOLATION: {:.3}ms (> {:.1}ms)", drift, self.task_drift_critical_ms);
                    self.performance_violations += 1;
                } else if drift > self.task_drift_warn_ms {
                    debug!("Task drift warning: {:.3}ms (> {:.1}ms)", drift, self.task_drift_warn_ms);
                }
            }

            EventType::SchedulerPrecisionViolation => {
                // Treat this as a hard violation; also record the drift if provided
                if let Some(drift_s) = event.metadata.get("drift_ms") {
                    if let Ok(drift) = drift_s.parse::<f64>() {
                        self.task_drift_ms.push_back(drift);
                        if self.task_drift_ms.len() > 1000 {
                            self.task_drift_ms.pop_front();
                        }
                    }
                }
                self.performance_violations += 1;
            }

            EventType::TelemetryEnqueued => {
            // learn capacity/length
            if let Some(cap_str) = event.metadata.get("queue_capacity") {
                if let Ok(cap) = cap_str.parse::<u32>() {
                    self.backlog_capacity = cap;
                }
            }
            if let Some(len_str) = event.metadata.get("queue_len") {
                if let Ok(len) = len_str.parse::<u32>() {
                    self.telemetry_backlog_len_samples.push_back(len);
                    if self.telemetry_backlog_len_samples.len() > 1000 {
                        self.telemetry_backlog_len_samples.pop_front();
                    }

                    // threshold evaluation if we know capacity
                    if self.backlog_capacity > 0 {
                        let ratio = len as f64 / self.backlog_capacity as f64;
                        if ratio >= self.backlog_crit_ratio {
                            self.backlog_critical_events += 1;
                            self.performance_violations += 1;
                        } else if ratio >= self.backlog_warn_ratio {
                            self.backlog_warn_events += 1;
                        }
                    }
                }
            }
            // store enqueue time for age calc
            if let Some(id) = event.metadata.get("packet_id") {
                self.telemetry_enqueue_times.insert(id.clone(), event.timestamp);
            }
        }

            EventType::TelemetryDequeued => {
                // compute age from stored enqueue time
                if let Some(id) = event.metadata.get("packet_id") {
                    if let Some(enq_ts) = self.telemetry_enqueue_times.remove(id) {
                        let age_ms = (event.timestamp - enq_ts)
                            .num_microseconds()
                            .unwrap_or(0) as f64 / 1000.0;
                        self.telemetry_backlog_age_ms.push_back(age_ms);
                        if self.telemetry_backlog_age_ms.len() > 1000 {
                            self.telemetry_backlog_age_ms.pop_front();
                        }
                    }
                }
                // also record current queue length sample
                if let Some(len_str) = event.metadata.get("queue_len") {
                    if let Ok(len) = len_str.parse::<u32>() {
                        self.telemetry_backlog_len_samples.push_back(len);
                        if self.telemetry_backlog_len_samples.len() > 1000 {
                            self.telemetry_backlog_len_samples.pop_front();
                        }
                    }
                }
            }

            EventType::TelemetryDropped => {
                // cleanup if something was never dequeued
                if let Some(id) = event.metadata.get("packet_id") {
                    self.telemetry_enqueue_times.remove(id);
                }
                // treat severe drop as a violation if backlog was high
                self.performance_violations += 1;
            }

            EventType::TelemetryBacklogWarning => {
                self.backlog_warn_events += 1;
            }

            EventType::TelemetryBacklogCritical => {
                self.backlog_critical_events += 1;
                self.performance_violations += 1;
            }

            EventType::NetworkDeadlineViolation => {
                self.network_metrics.network_deadline_violations_2ms += 1;
                self.performance_violations += 1; // counts toward global violations
            }

            EventType::CommandDeadlineViolation => {
                self.system_metrics.command_deadline_violations += 1;
                self.performance_violations += 1;
            }

            EventType::SystemHealthUpdate => {
                if let (Some(c), Some(m), Some(l), Some(cores_s)) = (
                    event.metadata.get("cpu_pct"),
                    event.metadata.get("mem_pct"),
                    event.metadata.get("load1"),
                    event.metadata.get("cores"),
                ) {
                    if let (Ok(cpu), Ok(mem), Ok(load1), Ok(cores)) =
                        (c.parse::<f64>(), m.parse::<f64>(), l.parse::<f64>(), cores_s.parse::<usize>())
                    {
                        // latest
                        self.system_metrics.cpu_usage_latest = cpu;
                        self.system_metrics.memory_usage_latest = mem;
                        self.system_metrics.load1_latest = load1;

                        // running averages (simple incremental average)
                        self.system_health_samples += 1;
                        let n = self.system_health_samples as f64;
                        if n == 1.0 {
                            self.system_metrics.cpu_usage_avg = cpu;
                            self.system_metrics.memory_usage_avg = mem;
                            self.system_metrics.load1_avg = load1;
                        } else {
                            self.system_metrics.cpu_usage_avg =
                                (self.system_metrics.cpu_usage_avg * (n - 1.0) + cpu) / n;
                            self.system_metrics.memory_usage_avg =
                                (self.system_metrics.memory_usage_avg * (n - 1.0) + mem) / n;
                            self.system_metrics.load1_avg =
                                (self.system_metrics.load1_avg * (n - 1.0) + load1) / n;
                        }

                        // peaks
                        if cpu > self.system_metrics.cpu_usage_peak {
                            self.system_metrics.cpu_usage_peak = cpu;
                        }
                        if mem > self.system_metrics.memory_usage_peak {
                            self.system_metrics.memory_usage_peak = mem;
                        }
                        if load1 > self.system_metrics.load1_peak {
                            self.system_metrics.load1_peak = load1;
                        }

                        self.cpu_cores = cores as u32;

                        // Optional: raise a high-utilization event so it’s logged with a timestamp
                        let load_warn = self.load_warn_per_core * (cores as f64);
                        if cpu >= self.cpu_warn_threshold || mem >= self.mem_warn_threshold || load1 >= load_warn {
                            let mut md = std::collections::HashMap::new();
                            md.insert("cpu_pct".into(), format!("{:.2}", cpu));
                            md.insert("mem_pct".into(), format!("{:.2}", mem));
                            md.insert("load1".into(),    format!("{:.2}", load1));
                            md.insert("cores".into(),    format!("{}", cores));

                            // This creates a separate, timestamped event in your history
                            super::PerformanceTracker::record_event(self, PerformanceEvent {
                                timestamp: chrono::Utc::now(),
                                event_type: EventType::ResourceUtilizationHigh,
                                duration_ms: 0.0,
                                metadata: md,
                            });
                        }
                    }
                }
            }

            EventType::ResourceUtilizationHigh => {
                // you already count degradations here; leave as-is or add any counters you want
                self.system_metrics.system_degradation_events += 1;
            }

            _ => {
                // Handle other event types as needed
            }
        }
    }
    
    /// Check for performance violations and log warnings
    fn check_performance_violations(&mut self, event: &PerformanceEvent) {
        match event.event_type {
            EventType::TelemetryProcessed => {
                if event.duration_ms > 3.0 {
                    warn!("TELEMETRY PROCESSING VIOLATION: {:.3}ms exceeds 3ms requirement", 
                        event.duration_ms);
                    
                    self.telemetry_metrics.processing_violations_3ms += 1;
                    self.performance_violations += 1;
                }
            }
            
            EventType::CommandDispatched => {
                // Check if this was an urgent command that exceeded 2ms
                if let Some(priority_str) = event.metadata.get("priority") {
                    if let Ok(priority) = priority_str.parse::<u8>() {
                        if priority <= 1 && event.duration_ms > 2.0 {
                            warn!("URGENT COMMAND DELAY: {:.3}ms exceeds 2ms requirement", 
                                event.duration_ms);
                            self.performance_violations += 1;
                        }
                    }
                }
            }
            
            EventType::PacketReceived => {
                // Check for excessive reception latency
                if event.duration_ms > 200.0 {
                    warn!("HIGH PACKET LATENCY: {:.1}ms reception latency", event.duration_ms);
                }
            }
            
            _ => {}
        }
    }
    
    /// Clean up old timing data to prevent memory growth
    fn cleanup_old_timing_data(&mut self) {
        let max_timing_samples = 1000;
        
        if self.telemetry_processing_times.len() > max_timing_samples {
            self.telemetry_processing_times.pop_front();
        }
        
        if self.command_dispatch_times.len() > max_timing_samples {
            self.command_dispatch_times.pop_front();
        }
        
        if self.packet_reception_latencies.len() > max_timing_samples {
            self.packet_reception_latencies.pop_front();
        }

        if self.task_drift_ms.len() > max_timing_samples {
            self.task_drift_ms.pop_front();
        }
        
        // Clean events older than performance window
        let cutoff_time = Utc::now() - Duration::minutes(self.performance_window_minutes);
        while let Some(front_event) = self.events.front() {
            if front_event.timestamp < cutoff_time {
                self.events.pop_front();
            } else {
                break;
            }
        }

        if self.uplink_jitter_samples_ms.len() > max_timing_samples {
            self.uplink_jitter_samples_ms.pop_front();
        }

        if self.telemetry_backlog_len_samples.len() > max_timing_samples {
            self.telemetry_backlog_len_samples.pop_front();
        }

        if self.telemetry_backlog_age_ms.len() > max_timing_samples {
            self.telemetry_backlog_age_ms.pop_front();
        }
    }
    
    /// Get comprehensive performance statistics
    pub fn get_current_stats(&self) -> PerformanceStats {
        let recent_events = self.get_recent_events(Duration::minutes(5));
        let telemetry_violations_recent = recent_events.iter()
            .filter(|e| matches!(e.event_type, EventType::TelemetryProcessingViolation))
            .count() as u32;
        
        let delayed_packets_recent = recent_events.iter()
            .filter(|e| matches!(e.event_type, EventType::PacketDelayed))
            .count() as u64;
        
        // Calculate average packet delay from recent delayed packet events
        let recent_delays: Vec<f64> = recent_events.iter()
            .filter(|e| matches!(e.event_type, EventType::PacketDelayed))
            .map(|e| e.duration_ms)
            .collect();
        
        let avg_packet_delay_ms = if !recent_delays.is_empty() {
            recent_delays.iter().sum::<f64>() / recent_delays.len() as f64
        } else {
            0.0
        };

        let p95_uplink_jitter = self.calculate_percentile(&self.uplink_jitter_samples_ms, 95.0);
        let p99_uplink_jitter = self.calculate_percentile(&self.uplink_jitter_samples_ms, 99.0);
        let max_uplink_jitter  = self.uplink_jitter_samples_ms.iter().cloned().fold(0.0, f64::max);

        let avg_uplink_interval = if !self.uplink_interarrival_ms.is_empty() {
            self.uplink_interarrival_ms.iter().sum::<f64>() / self.uplink_interarrival_ms.len() as f64
        } else { 0.0 };
        
        // Calculate performance percentiles
        let p95_processing_time = self.calculate_percentile(&self.telemetry_processing_times, 95.0);
        let p99_processing_time = self.calculate_percentile(&self.telemetry_processing_times, 99.0);

        let (avg_task_drift, max_task_drift, p95_task_drift, p99_task_drift) = if !self.task_drift_ms.is_empty() {
            let sum = self.task_drift_ms.iter().sum::<f64>();
            let avg = sum / self.task_drift_ms.len() as f64;
            let max = self.task_drift_ms.iter().cloned().fold(0.0, f64::max);
            let p95 = self.calculate_percentile(&self.task_drift_ms, 95.0);
            let p99 = self.calculate_percentile(&self.task_drift_ms, 99.0);
            (avg, max, p95, p99)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        let backlog_avg_len = if !self.telemetry_backlog_len_samples.is_empty() {
            self.telemetry_backlog_len_samples.iter().map(|&v| v as f64).sum::<f64>()
                / self.telemetry_backlog_len_samples.len() as f64
        } else { 0.0 };

        let backlog_len_f64: std::collections::VecDeque<f64> =
            self.telemetry_backlog_len_samples.iter().map(|&v| v as f64).collect();
        let backlog_p95_len = self.calculate_percentile(&backlog_len_f64, 95.0);

        let backlog_max_len = self.telemetry_backlog_len_samples.iter().cloned().max().unwrap_or(0);

        let backlog_avg_age_ms = if !self.telemetry_backlog_age_ms.is_empty() {
            self.telemetry_backlog_age_ms.iter().sum::<f64>() / self.telemetry_backlog_age_ms.len() as f64
        } else { 0.0 };

        let backlog_p95_age_ms = self.calculate_percentile(&self.telemetry_backlog_age_ms, 95.0);
        let backlog_max_age_ms = self.telemetry_backlog_age_ms.iter().cloned().fold(0.0, f64::max);


        
        PerformanceStats {
            // Telemetry performance
            total_packets_received: self.network_metrics.total_packets_received,
            total_packets_processed: self.telemetry_metrics.total_packets_processed,
            avg_processing_time_ms: self.telemetry_metrics.avg_processing_time_ms,
            max_processing_time_ms: self.telemetry_metrics.max_processing_time_ms,
            p95_processing_time_ms: p95_processing_time,
            p99_processing_time_ms: p99_processing_time,
            
            // Requirement violations
            processing_violations_3ms: self.telemetry_metrics.processing_violations_3ms,
            recent_violations_count: telemetry_violations_recent,
            total_performance_violations: self.performance_violations,
            
            // Packet timing and delay statistics
            avg_reception_latency_ms: self.network_metrics.avg_reception_latency_ms,
            max_reception_latency_ms: self.network_metrics.max_reception_latency_ms,
            delayed_packets_total: self.telemetry_metrics.delayed_packets_detected,
            delayed_packets_recent: delayed_packets_recent,
            avg_packet_delay_ms,
            reception_drift_violations: self.network_metrics.reception_drift_violations,
            
            // Network performance
            packet_loss_count: self.network_metrics.packet_loss_count,
            retransmission_requests: self.network_metrics.retransmission_requests,
            network_timeouts: self.network_metrics.network_timeouts,
            
            // System health
            total_faults: self.system_metrics.active_faults as u64,
            emergency_responses: self.system_metrics.emergency_responses,
            uptime_percentage: self.calculate_uptime_percentage(),
            
            // Overall system performance
            system_health_score: self.calculate_system_health_score(),

            p95_uplink_jitter_ms: p95_uplink_jitter,
            p99_uplink_jitter_ms: p99_uplink_jitter,
            max_uplink_jitter_ms: max_uplink_jitter,
            avg_uplink_interval_ms: avg_uplink_interval,

            avg_task_drift_ms: avg_task_drift,
            max_task_drift_ms: max_task_drift,
            p95_task_drift_ms: p95_task_drift,
            p99_task_drift_ms: p99_task_drift,

            backlog_avg_len,
            backlog_p95_len,
            backlog_max_len,
            backlog_avg_age_ms,
            backlog_p95_age_ms,
            backlog_max_age_ms,
            backlog_warn_events: self.backlog_warn_events,
            backlog_critical_events: self.backlog_critical_events,

            network_deadline_violations_2ms: self.network_metrics.network_deadline_violations_2ms,

            cpu_latest_percent:  self.system_metrics.cpu_usage_latest,
            cpu_avg_percent:     self.system_metrics.cpu_usage_avg,
            cpu_peak_percent:    self.system_metrics.cpu_usage_peak,
            mem_latest_percent:  self.system_metrics.memory_usage_latest,
            mem_avg_percent:     self.system_metrics.memory_usage_avg,
            mem_peak_percent:    self.system_metrics.memory_usage_peak,
            load1_latest:        self.system_metrics.load1_latest,
            load1_avg:           self.system_metrics.load1_avg,
            load1_peak:          self.system_metrics.load1_peak,
            cpu_cores:           self.cpu_cores,
        }
    }
    
    /// Get events within a specific time window
    fn get_recent_events(&self, duration: Duration) -> Vec<&PerformanceEvent> {
        let cutoff = Utc::now() - duration;
        self.events.iter()
            .filter(|event| event.timestamp >= cutoff)
            .collect()
    }
    
    /// Calculate percentile for a set of timing values
    fn calculate_percentile(&self, values: &VecDeque<f64>, percentile: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        
        let mut sorted_values: Vec<f64> = values.iter().cloned().collect();
        sorted_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        let index = (percentile / 100.0 * (sorted_values.len() - 1) as f64) as usize;
        sorted_values.get(index).copied().unwrap_or(0.0)
    }
    
    /// Calculate uptime percentage (simplified)
    fn calculate_uptime_percentage(&self) -> f64 {
        // This would typically track actual downtime
        // For now, return high uptime unless many violations
        if self.performance_violations > 100 {
            95.0
        } else if self.performance_violations > 50 {
            98.0
        } else {
            99.5
        }
    }
    
    /// Calculate overall system health score (0-100)
    fn calculate_system_health_score(&self) -> f64 {
        let mut score = 100.0;
        
        // Deduct for processing violations
        score -= (self.telemetry_metrics.processing_violations_3ms as f64 * 0.5).min(20.0);
        
        // Deduct for network issuess
        score -= (self.network_metrics.network_timeouts as f64 * 0.2).min(15.0);
        
        // Deduct for delayed packets
        score -= (self.network_metrics.reception_drift_violations as f64 * 0.1).min(10.0);
        
        // Deduct for emergency responses
        score -= (self.system_metrics.emergency_responses as f64 * 2.0).min(25.0);
        
        // // NEW: penalize when high-utilization/degradation is logged
        // score -= (self.system_metrics.system_degradation_events as f64 * 1.5).min(20.0);

        // // NEW: a small nudge if current utilization is above warn thresholds
        // let load_warn = self.load_warn_per_core * (self.cpu_cores as f64);
        // if self.system_metrics.cpu_usage_latest >= self.cpu_warn_threshold
        //     || self.system_metrics.memory_usage_latest >= self.mem_warn_threshold
        //     || self.system_metrics.load1_latest >= load_warn
        // {
        //     score -= 5.0;
        // }

        // ✅ NEW: penalize when high utilization / degradation events were logged
        score -= (self.system_metrics.system_degradation_events as f64 * 1.0).min(10.0);
        
        score.max(0.0)
    }
    
    /// Generate a performance report
    pub fn generate_performance_report(&self, duration: Duration) -> PerformanceReport {
        let recent_events = self.get_recent_events(duration);
        let stats = self.get_current_stats();
        
        // Count different types of recent events
        let mut event_counts: HashMap<String, u32> = HashMap::new();
        for event in &recent_events {
            let event_type_str = format!("{:?}", event.event_type);
            *event_counts.entry(event_type_str).or_insert(0) += 1;
        }
        
        // Identify performance issues
        let mut issues = Vec::new();
        
        if stats.processing_violations_3ms > 0 {
            issues.push(format!("Telemetry processing violations: {} total, {} recent", 
                stats.processing_violations_3ms, stats.recent_violations_count));
        }
        
        if stats.avg_packet_delay_ms > 100.0 {
            issues.push(format!("High packet delays: {:.1}ms average", 
                stats.avg_packet_delay_ms));
        }
        
        if stats.reception_drift_violations > 10 {
            issues.push(format!("Reception timing violations: {}", 
                stats.reception_drift_violations));
        }
        
        if stats.network_timeouts > 5 {
            issues.push(format!("Network timeouts: {}", stats.network_timeouts));
        }
        
        // Performance recommendations
        let mut recommendations = Vec::new();
        
        if stats.p99_processing_time_ms > 2.5 {
            recommendations.push("Consider optimizing telemetry processing algorithms".to_string());
        }
        
        if stats.retransmission_requests > stats.total_packets_received / 20 {
            recommendations.push("High retransmission rate indicates network issues".to_string());
        }
        
        if stats.system_health_score < 90.0 {
            recommendations.push("System performance degradation detected - review logs".to_string());
        }

        if stats.p99_task_drift_ms > self.task_drift_warn_ms {
            issues.push(format!(
                "Scheduler drift high: p99 {:.2}ms (warn>{:.1}ms, crit>{:.1}ms)",
                stats.p99_task_drift_ms, self.task_drift_warn_ms, self.task_drift_critical_ms
            ));
        }

        if stats.backlog_p95_len > (self.backlog_crit_ratio * self.backlog_capacity as f64) && self.backlog_capacity > 0 {
            issues.push(format!(
                "Telemetry backlog very high: p95 len {:.0} / cap {}",
                stats.backlog_p95_len, self.backlog_capacity
            ));
        }
        if stats.backlog_p95_age_ms > 50.0 {
            issues.push(format!("Telemetry queueing age high: p95 {:.1}ms", stats.backlog_p95_age_ms));
        }

         if self.network_metrics.network_deadline_violations_2ms > 0 {
            issues.push(format!(
                "Network ≤2ms violations: {}",
                self.network_metrics.network_deadline_violations_2ms
            ));
        }
        
        PerformanceReport {
            report_timestamp: Utc::now(),
            time_window_minutes: duration.num_minutes(),
            total_events: recent_events.len() as u64,
            event_type_breakdown: event_counts,
            performance_stats: stats,
            identified_issues: issues,
            recommendations,
        }
    }
}

/// Comprehensive performance statistics
#[derive(Debug, Clone)]
pub struct PerformanceStats {
    // Processing performance
    pub total_packets_received: u64,
    pub total_packets_processed: u64,
    pub avg_processing_time_ms: f64,
    pub max_processing_time_ms: f64,
    pub p95_processing_time_ms: f64,
    pub p99_processing_time_ms: f64,
    
    // Requirement compliance
    pub processing_violations_3ms: u64,
    pub recent_violations_count: u32,
    pub total_performance_violations: u64,
    
    // Timing and delay metrics
    pub avg_reception_latency_ms: f64,
    pub max_reception_latency_ms: f64,
    pub delayed_packets_total: u64,
    pub delayed_packets_recent: u64,
    pub avg_packet_delay_ms: f64,
    pub reception_drift_violations: u64,
    
    // Network performance
    pub packet_loss_count: u64,
    pub retransmission_requests: u64,
    pub network_timeouts: u64,
    
    // System health
    pub total_faults: u64,
    pub emergency_responses: u64,
    pub uptime_percentage: f64,
    pub system_health_score: f64,

    pub p95_uplink_jitter_ms: f64,
    pub p99_uplink_jitter_ms: f64,
    pub max_uplink_jitter_ms: f64,
    pub avg_uplink_interval_ms: f64,

    pub avg_task_drift_ms: f64,
    pub max_task_drift_ms: f64,
    pub p95_task_drift_ms: f64,
    pub p99_task_drift_ms: f64,

    pub backlog_avg_len: f64,
    pub backlog_p95_len: f64,
    pub backlog_max_len: u32,

    pub backlog_avg_age_ms: f64,
    pub backlog_p95_age_ms: f64,
    pub backlog_max_age_ms: f64,

    pub backlog_warn_events: u64,
    pub backlog_critical_events: u64,

    pub network_deadline_violations_2ms: u64,

    // NEW: system-load exposure
    pub cpu_latest_percent: f64,
    pub cpu_avg_percent: f64,
    pub cpu_peak_percent: f64,
    pub mem_latest_percent: f64,
    pub mem_avg_percent: f64,
    pub mem_peak_percent: f64,
    pub load1_latest: f64,
    pub load1_avg: f64,
    pub load1_peak: f64,
    
    pub cpu_cores: u32,          // helpful for interpreting load thresholds externally
}

/// Detailed performance report
#[derive(Debug, Clone)]
pub struct PerformanceReport {
    pub report_timestamp: DateTime<Utc>,
    pub time_window_minutes: i64,
    pub total_events: u64,
    pub event_type_breakdown: HashMap<String, u32>,
    pub performance_stats: PerformanceStats,
    pub identified_issues: Vec<String>,
    pub recommendations: Vec<String>,
}


#[cfg(test)]
mod task4_perftracker_tests {
    use super::*;
    use chrono::Utc;

    fn perf_evt(ts: Timestamp, ty: EventType, dur_ms: f64, md: &[(&str, &str)]) -> PerformanceEvent {
        let mut m = std::collections::HashMap::new();
        for (k, v) in md {
            m.insert((*k).to_string(), (*v).to_string());
        }
        PerformanceEvent {
            timestamp: ts,
            event_type: ty,
            duration_ms: dur_ms,
            metadata: m,
        }
    }

    #[test]
    fn telemetry_backlog_len_and_age_metrics() {
        let mut pt = PerformanceTracker::new();

        // Set a capacity via a TelemetryEnqueued event
        let now = Utc::now();
        pt.record_event(perf_evt(
            now,
            EventType::TelemetryEnqueued,
            0.0,
            &[
                ("packet_id", "p1"),
                ("queue_len", "30"),
                ("queue_capacity", "100"),
            ],
        ));

        // Enqueue another to push into warn/crit bands
        pt.record_event(perf_evt(
            now,
            EventType::TelemetryEnqueued,
            0.0,
            &[
                ("packet_id", "p2"),
                ("queue_len", "80"), // 80/100 => critical (>=0.70)
                ("queue_capacity", "100"),
            ],
        ));

        // After some time, dequeue p1 to produce an age sample (~15ms)
        let later = now + chrono::Duration::milliseconds(15);
        pt.record_event(perf_evt(
            later,
            EventType::TelemetryDequeued,
            0.0,
            &[("packet_id", "p1"), ("queue_len", "79")],
        ));

        let s = pt.get_current_stats();

        // Length stats collected
        assert!(s.backlog_avg_len > 0.0, "avg backlog len should be >0");
        assert!(s.backlog_max_len >= 80, "max backlog len should reflect critical sample");

        // Age stats collected
        assert!(
            s.backlog_avg_age_ms >= 10.0,
            "avg backlog age should reflect ~15ms sample, got {:.2}ms",
            s.backlog_avg_age_ms
        );

        // Warn/critical counters bumped
        assert!(s.backlog_warn_events >= 1, "expected at least one warn");
        assert!(s.backlog_critical_events >= 1, "expected at least one critical");
    }

    #[test]
    fn system_health_high_utilization_is_logged() {
        let mut pt = PerformanceTracker::new();

        // Push a SystemHealthUpdate with high CPU or high load to exceed thresholds
        pt.record_event(perf_evt(
            Utc::now(),
            EventType::SystemHealthUpdate,
            0.0,
            &[
                ("cpu_pct", "95.0"),
                ("mem_pct", "40.0"),
                ("load1", "8.0"),
                ("cores", "8"),
            ],
        ));

        // That update emits a ResourceUtilizationHigh event internally and bumps system_degradation_events
        let s = pt.get_current_stats();
        assert!(
            s.system_health_score < 100.0,
            "health score should drop when high util is logged"
        );
        // also check peaks are recorded
        assert!(s.cpu_peak_percent >= 95.0, "cpu peak should reflect sample");
        assert!(s.load1_peak >= 8.0, "load1 peak should reflect sample");
    }

    #[test]
    fn can_record_missed_deadline_event() {
        let mut pt = PerformanceTracker::new();
        // Directly record a missed deadline (scheduler also emits this in certain paths)
        pt.record_event(perf_evt(
            Utc::now(),
            EventType::CommandDeadlineViolation,
            3.2, // violation amount
            &[("command_id", "late-1"), ("violation_ms", "3.2")],
        ));

        let s = pt.get_current_stats();
        assert!(
            s.total_performance_violations >= 1,
            "missed deadline should bump violations"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn now_ts() -> Timestamp { Utc::now() }

    #[test]
    fn network_deadline_violation_is_counted() {
        let mut perf = PerformanceTracker::new();

        // Simulate an urgent command whose network send exceeded 2ms
        let mut md = std::collections::HashMap::new();
        md.insert("command_id".into(), "C-1".into());
        md.insert("priority".into(), "0".into());
        md.insert("send_time_ms".into(), "2.7".into());
        perf.record_event(PerformanceEvent {
            timestamp: now_ts(),
            event_type: EventType::NetworkDeadlineViolation,
            duration_ms: 2.7,
            metadata: md,
        });

        let stats = perf.get_current_stats();
        assert_eq!(stats.network_deadline_violations_2ms, 1, "should bump network ≤2ms violations");
        assert_eq!(stats.total_performance_violations, 1, "should bump global violations too");
    }

    #[test]
    fn telemetry_backlog_warn_and_crit_triggers() {
        let mut perf = PerformanceTracker::new();

        // capacity learned from first enqueue
        let mut md1 = std::collections::HashMap::new();
        md1.insert("packet_id".into(), "P-1".into());
        md1.insert("queue_len".into(), "25".into());      // 25% of 100 -> warn
        md1.insert("queue_capacity".into(), "100".into());
        perf.record_event(PerformanceEvent {
            timestamp: now_ts(),
            event_type: EventType::TelemetryEnqueued,
            duration_ms: 0.0,
            metadata: md1,
        });

        // critical ratio
        let mut md2 = std::collections::HashMap::new();
        md2.insert("packet_id".into(), "P-2".into());
        md2.insert("queue_len".into(), "75".into());      // 75% of 100 -> critical
        // capacity stays remembered
        perf.record_event(PerformanceEvent {
            timestamp: now_ts(),
            event_type: EventType::TelemetryEnqueued,
            duration_ms: 0.0,
            metadata: md2,
        });

        let stats = perf.get_current_stats();
        assert_eq!(stats.backlog_warn_events, 1, "should count one warn");
        assert_eq!(stats.backlog_critical_events, 1, "should count one critical");
        assert!(stats.total_performance_violations >= 1, "critical should contribute to violations");
    }

    #[test]
    fn uplink_jitter_samples_are_aggregated() {
        let mut perf = PerformanceTracker::new();

        // Feed a few interval/jitter samples
        for (int_ms, jit_ms) in &[(1000.0, 0.0), (1003.0, 3.0), (1012.0, 12.0)] {
            let mut md = std::collections::HashMap::new();
            md.insert("uplink_interval_ms".into(), format!("{:.3}", int_ms));
            md.insert("uplink_jitter_ms".into(),   format!("{:.3}", jit_ms));
            md.insert("uplink_expected_ms".into(), "1000.0".into());
            perf.record_event(PerformanceEvent {
                timestamp: now_ts(),
                event_type: EventType::UplinkIntervalSample,
                duration_ms: *int_ms,
                metadata: md,
            });
        }

        let stats = perf.get_current_stats();
        // We pushed a 12ms jitter; percentiles should be >= 0
        assert!(stats.p95_uplink_jitter_ms >= 0.0);
        assert!(stats.p99_uplink_jitter_ms >= 0.0);
        assert!(stats.max_uplink_jitter_ms >= 12.0, "should reflect max jitter observed");
        assert!(stats.avg_uplink_interval_ms > 0.0, "should average intervals");
    }
}
