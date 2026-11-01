//! Bounded aggregates: recording duration must not grow diagnostic memory.
use std::time::{Duration, Instant};

pub(super) struct Timing {
    bins: [u64; 201],
    count: u64,
    max: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            bins: [0; 201],
            count: 0,
            max: Duration::ZERO,
        }
    }
}
impl Timing {
    pub fn record(&mut self, value: Duration) {
        // Millisecond upper bounds; values over 200ms use the observed maximum.
        let bucket = value.as_micros().div_ceil(1000).min(200) as usize;
        self.bins[bucket] += 1;
        self.count += 1;
        self.max = self.max.max(value);
    }
    pub fn p95_ms(&self) -> f64 {
        if self.count == 0 {
            return 0.;
        }
        let target = self.count.saturating_mul(95).div_ceil(100);
        let mut count = 0;
        for (index, samples) in self.bins.iter().enumerate() {
            count += samples;
            if count >= target {
                return if index == 200 {
                    self.max.as_secs_f64() * 1000.
                } else {
                    index as f64
                };
            }
        }
        unreachable!()
    }
}

#[derive(Default)]
pub(super) struct Diagnostics {
    pub read: Timing,
    pub delivery: Timing,
    pub conversion: Timing,
    pub submit: Timing,
    pub source_frames: u64,
    pub content_updates: u64,
    pub started: Option<Instant>,
    pub stopping: Option<Instant>,
    pub first_submitted: Option<Duration>,
    pub first_encoded: Option<Duration>,
    pub encoded_statistics_unavailable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_percentile_reports_empty_tail_and_overflow() {
        let mut timing = Timing::default();
        assert_eq!(timing.p95_ms(), 0.);
        for _ in 0..95 {
            timing.record(Duration::from_micros(1200));
        }
        for _ in 0..5 {
            timing.record(Duration::from_millis(350));
        }
        assert_eq!(timing.p95_ms(), 2.);
        timing.record(Duration::from_millis(350));
        assert_eq!(timing.p95_ms(), 350.);
    }
}
