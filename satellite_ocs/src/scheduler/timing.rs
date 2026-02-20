use std::time::Instant;

pub struct Tick {
    pub start: Instant,
    pub period_ns: u128,
}

impl Tick {
    pub fn jitter_ns(&self, last: Instant) -> i128 {
        let actual = self.start.duration_since(last).as_nanos() as i128;
        actual - self.period_ns as i128
    }
}
