// src/monitor.rs

use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, error, debug};
use chrono::Utc;
use std::collections::HashMap;

use crate::performance::{PerformanceTracker, PerformanceEvent, EventType};

use sysinfo::System;

/// System load monitoring functionality
pub struct SystemLoadMonitor {
    sys: System,
    cpu_cores: u32,
}

impl SystemLoadMonitor {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();

        // no num_cpus: ask sysinfo
        let cpu_cores = sys.cpus().len() as u32;
        info!("System load monitor initialized for {} CPU cores", cpu_cores);

        Self { sys, cpu_cores }
    }

    /// Get current system load metrics
    pub async fn get_system_load(&mut self) -> Result<SystemLoadMetrics, Box<dyn std::error::Error + Send + Sync>> {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();

        let cpu_percent = self.sys.global_cpu_usage() as f64;

        let total_mem = self.sys.total_memory() as f64;       // KiB
        let avail_mem = self.sys.available_memory() as f64;   // KiB
        let used_mem = (total_mem - avail_mem).max(0.0);
        let memory_percent = if total_mem > 0.0 {
            (used_mem / total_mem) * 100.0
        } else {
            0.0
        };

        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd", target_os="dragonfly", target_os="netbsd", target_os="openbsd"))]
        let load1 = {
            let la = sysinfo::System::load_average();
            la.one
        };

        #[cfg(target_os = "windows")]
        let load1 = {
            (cpu_percent / 100.0) * (self.cpu_cores as f64)
        };

        Ok(SystemLoadMetrics {
            cpu_percent,
            memory_percent,
            load1,
            cpu_cores: self.cpu_cores,
            timestamp: Utc::now(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct SystemLoadMetrics {
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub load1: f64,
    pub cpu_cores: u32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Start the system load sampling task
pub async fn start_system_load_sampler(
    performance_tracker: Arc<Mutex<PerformanceTracker>>,
    interval_ms: u64,
) {
    info!("Starting system load sampler with {}ms interval", interval_ms);

    let mut monitor = SystemLoadMonitor::new();
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));
    let mut sample_count = 0u64;

    loop {
        interval.tick().await;
        sample_count += 1;

        match monitor.get_system_load().await {
            Ok(metrics) => {
                debug!(
                    "System load sample #{}: CPU={:.1}%, MEM={:.1}%, Load1={:.2}",
                    sample_count, metrics.cpu_percent, metrics.memory_percent, metrics.load1
                );

                let mut metadata = HashMap::new();
                metadata.insert("cpu_pct".to_string(), format!("{:.2}", metrics.cpu_percent));
                metadata.insert("mem_pct".to_string(), format!("{:.2}", metrics.memory_percent));
                metadata.insert("load1".to_string(), format!("{:.2}", metrics.load1));
                metadata.insert("cores".to_string(), metrics.cpu_cores.to_string());
                metadata.insert("sample_count".to_string(), sample_count.to_string());

                let system_health_event = PerformanceEvent {
                    timestamp: metrics.timestamp,
                    event_type: EventType::SystemHealthUpdate,
                    duration_ms: 0.0,
                    metadata,
                };

                {
                    let mut tracker = performance_tracker.lock().await;
                    tracker.record_event(system_health_event);
                }

                if sample_count % 30 == 0 && interval_ms == 1000 {
                    if metrics.cpu_percent > 80.0 {
                        warn!("High CPU usage detected: {:.1}%", metrics.cpu_percent);
                    }
                    if metrics.memory_percent > 80.0 {
                        warn!("High memory usage detected: {:.1}%", metrics.memory_percent);
                    }
                    if metrics.load1 > (metrics.cpu_cores as f64 * 0.8) {
                        warn!(
                            "High system load detected: {:.2} (cores: {})",
                            metrics.load1, metrics.cpu_cores
                        );
                    }
                }
            }
            Err(e) => {
                error!("Failed to collect system load metrics: {}", e);

                let mut error_metadata = HashMap::new();
                error_metadata.insert("error".to_string(), e.to_string());
                error_metadata.insert("sample_count".to_string(), sample_count.to_string());

                let error_event = PerformanceEvent {
                    timestamp: Utc::now(),
                    event_type: EventType::SystemHealthUpdate,
                    duration_ms: 0.0,
                    metadata: error_metadata,
                };

                let mut tracker = performance_tracker.lock().await;
                tracker.record_event(error_event);
            }
        }

        if sample_count % 300 == 0 && interval_ms == 1000 {
            info!(
                "System load sampler: {} samples collected over {:.1} minutes",
                sample_count,
                sample_count as f64 / 60.0
            );
        }
    }
} 
