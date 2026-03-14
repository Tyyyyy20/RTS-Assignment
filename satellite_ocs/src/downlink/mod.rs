use once_cell::sync::OnceCell;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{self, Duration, Instant};
use tracing::{info, warn};

pub static DL: OnceCell<Downlink> = OnceCell::new();

#[derive(Debug, Clone, Copy)]
enum LinkState {
    Closed,
    Opening { opened_at: Instant, init_started: bool },
    Ready { opened_at: Instant, ready_at: Instant, degraded: bool },
}

#[derive(Clone)]
pub struct Downlink {
    inner: Arc<Mutex<LinkState>>,
}

impl Downlink {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LinkState::Closed)),
        }
    }

    async fn open(&self) {
        let mut state = self.inner.lock().await;
        *state = LinkState::Opening {
            opened_at: Instant::now(),
            init_started: false,
        };
        info!("downlink: window opened");
    }

    async fn close(&self) {
        let mut state = self.inner.lock().await;
        *state = LinkState::Closed;
        info!("downlink: window closed");
    }

    /// Called by batcher before a send; enforces 5ms init, checks 30ms prep.
    pub async fn pre_send(&self) -> DownlinkEvent {
        let mut state = self.inner.lock().await;
        let now = Instant::now();

        match *state {
            LinkState::Closed => DownlinkEvent::NotInWindow,

            LinkState::Opening {
                opened_at,
                init_started,
            } => {
                let since_open = now.duration_since(opened_at);

                if since_open > Duration::from_millis(5) && !init_started {
                    // Missed 5ms init — treat as missed comms for this window
                    warn!("downlink: init >5ms → missed communication");
                    *state = LinkState::Closed;
                    DownlinkEvent::MissedInit
                } else {
                    // Lazily start init on first attempt; become ready quickly (simulate)
                    let ready_at = now;

                    *state = LinkState::Ready {
                        opened_at,
                        ready_at,
                        degraded: false,
                    };

                    let prep_ms =
                        ready_at.duration_since(opened_at).as_secs_f64() * 1000.0;

                    if prep_ms > 30.0 {
                        DownlinkEvent::ReadyPrepLate { prep_ms }
                    } else {
                        DownlinkEvent::Ready
                    }
                }
            }

            LinkState::Ready {
                opened_at,
                ready_at,
                degraded,
            } => {
                let prep_ms =
                    ready_at.duration_since(opened_at).as_secs_f64() * 1000.0;

                if prep_ms > 30.0 {
                    DownlinkEvent::ReadyPrepLate { prep_ms }
                } else if degraded {
                    DownlinkEvent::ReadyDegraded
                } else {
                    DownlinkEvent::Ready
                }
            }
        }
    }

    pub async fn set_degraded(&self, on: bool) {
        let mut state = self.inner.lock().await;

        match *state {
            LinkState::Ready {
                opened_at,
                ready_at,
                ..
            } => {
                *state = LinkState::Ready {
                    opened_at,
                    ready_at,
                    degraded: on,
                };

                if on {
                    warn!("downlink: DEGRADED mode enabled (buffer > 80%)");
                } else {
                    info!("downlink: degraded mode cleared");
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DownlinkEvent {
    NotInWindow,
    MissedInit,
    Ready,
    ReadyPrepLate { prep_ms: f64 },
    ReadyDegraded,
}

/// Simulate visibility windows (e.g., every 5s open for 800ms)
pub fn init_and_spawn() {
    let downlink = DL.get_or_init(|| Downlink::new()).clone();

    tokio::spawn(async move {
        // every 5s open window for 800ms
        let mut ticker = time::interval(Duration::from_millis(5000));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            downlink.open().await;

            // Keep window open for 800ms
            time::sleep(Duration::from_millis(800)).await;
            downlink.close().await;
        }
    });
}