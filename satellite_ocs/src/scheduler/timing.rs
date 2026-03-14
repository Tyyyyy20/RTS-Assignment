use std::time::Instant;

// Tick represents a single timing event for a periodic task.
// It stores the task start time and the expected period in nanoseconds.
pub struct Tick {
    pub start: Instant,     // The actual start time of the current task execution
    pub period_ns: u128,    // The expected period of the task (in nanoseconds)
}
impl Tick {
    // Calculate scheduling jitter in nanoseconds.
    // Jitter = actual interval between executions - expected period.
    // A positive value means the task started later than expected.
    // A negative value means the task started earlier than expected.
    pub fn jitter_ns(&self, last: Instant) -> i128 {
        let actual = self.start.duration_since(last).as_nanos() as i128;
        actual - self.period_ns as i128
    }
}