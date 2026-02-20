use once_cell::sync::OnceCell;
use shared_protocol::{Priority, SensorReading};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Result of inserting into bounded buffer
#[derive(Debug, Clone)]
pub enum InsertResult {
    /// Inserted successfully (may still have evicted lower-priority items)
    Accepted,
    /// We had to drop something; report what was dropped
    Dropped {
        dropped_priority: Priority,
        dropped_count: usize,
    },
}

#[derive(Debug)]
struct Inner {
    capacity: usize,
    hi: VecDeque<SensorReading>,  // Emergency + Critical
    im: VecDeque<SensorReading>,  // Important
    lo: VecDeque<SensorReading>,  // Normal
}

#[derive(Clone, Debug)]
pub struct BufferHandle {
    inner: Arc<Mutex<Inner>>,
}

impl BufferHandle {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                capacity,
                hi: VecDeque::new(),
                im: VecDeque::new(),
                lo: VecDeque::new(),
            })),
        }
    }

    /// Current fill (total items)
    pub async fn len(&self) -> usize {
        let g = self.inner.lock().await;
        g.hi.len() + g.im.len() + g.lo.len()
    }

    pub fn capacity(&self) -> usize {
        // constant; no lock needed
        self.inner.blocking_lock().capacity
    }

    /// Push with priority-aware drop policy.
    /// If full, evict from the **lowest priority present** (Normal → Important → Critical).
    pub async fn push(&self, r: SensorReading) -> InsertResult {
        let mut g = self.inner.lock().await;

        let total = g.hi.len() + g.im.len() + g.lo.len();
        let target_q = match r.priority {
            Priority::Emergency | Priority::Critical => 0, // hi
            Priority::Important => 1,                      // im
            Priority::Normal => 2,                         // lo
        };

        let mut dropped: Option<Priority> = None;

        if total >= g.capacity {
            // Evict policy: drop from the lowest non-empty bucket
            if !g.lo.is_empty() {
                g.lo.pop_front();
                dropped = Some(Priority::Normal);
            } else if !g.im.is_empty() {
                g.im.pop_front();
                dropped = Some(Priority::Important);
            } else if !g.hi.is_empty() {
                // Only if completely flooded by critical/emergency traffic
                g.hi.pop_front();
                dropped = Some(Priority::Critical);
            } else {
                // Shouldn't happen; capacity says full but queues empty
            }
        }

        match target_q {
            0 => g.hi.push_back(r),
            1 => g.im.push_back(r),
            _ => g.lo.push_back(r),
        }

        if let Some(dp) = dropped {
            InsertResult::Dropped {
                dropped_priority: dp,
                dropped_count: 1,
            }
        } else {
            InsertResult::Accepted
        }
    }

    /// Pop up to `n` in priority order.
    pub async fn pop_many(&self, n: usize) -> Vec<SensorReading> {
        let mut g = self.inner.lock().await;
        let mut out = Vec::with_capacity(n);
        let mut need = n;

        let mut take_from = |q: &mut VecDeque<SensorReading>, need: &mut usize, out: &mut Vec<_>| {
            while *need > 0 {
                if let Some(x) = q.pop_front() {
                    out.push(x);
                    *need -= 1;
                } else {
                    break;
                }
            }
        };

        take_from(&mut g.hi, &mut need, &mut out);
        if need > 0 {
            take_from(&mut g.im, &mut need, &mut out);
        }
        if need > 0 {
            take_from(&mut g.lo, &mut need, &mut out);
        }

        out
    }

    /// Percent fill (0.0..=100.0)
    pub async fn fill_pct(&self) -> f64 {
        let g = self.inner.lock().await;
        let total = g.hi.len() + g.im.len() + g.lo.len();
        if g.capacity == 0 {
            0.0
        } else {
            (total as f64 / g.capacity as f64) * 100.0
        }
    }
}
