// lib.rs — Re-exports for benchmarks and integration tests.
// The actual entry point remains main.rs; this file just makes
// selected modules accessible from `ground_control::` in benches/.

pub mod telemetry_processor;
pub mod command_scheduler;
pub mod fault_management;
pub mod network_manager;
pub mod performance_tracker;
pub mod system_monitor;
