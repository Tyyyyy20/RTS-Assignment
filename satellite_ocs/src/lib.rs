// lib.rs — Re-exports for benchmarks and integration tests.
// The actual entry point remains main.rs; this file just makes
// the crate's modules accessible from `satellite_ocs::` in benches/.

pub mod config;
pub mod crypto;
pub mod errors;
pub mod net;
pub mod sensors;
pub mod scheduler;
pub mod telemetry;
pub mod commands;
pub mod health;
pub mod logging;
pub mod util;
pub mod downlink;
pub mod faults;
