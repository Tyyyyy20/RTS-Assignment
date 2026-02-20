pub mod timing;
// src/scheduler/mod.rs
pub mod rm;

// A tiny preemption hook: thermal sensor can send here to preempt running work.
use once_cell::sync::OnceCell;
use tokio::sync::mpsc;

pub static PREEMPT_CH: OnceCell<mpsc::Sender<()>> = OnceCell::new();
