//! Integer video-frame boundaries shared by preview and exact export.
use std::time::Duration;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    pub end: u64,
}
impl Range {
    pub fn full(frames: u64) -> Self {
        Self {
            start: 0,
            end: frames,
        }
    }
    pub fn valid(self, frames: u64) -> bool {
        self.start < self.end && self.end <= frames
    }
    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
    pub fn time(frame: u64, fps: u32) -> Duration {
        Duration::from_nanos(
            ((u128::from(frame) * 1_000_000_000) / u128::from(fps.max(1))).min(u128::from(u64::MAX))
                as u64,
        )
    }
    pub fn ticks(frame: u64, fps: u32) -> Option<i64> {
        i64::try_from(u128::from(frame) * 10_000_000 / u128::from(fps.max(1))).ok()
    }
    pub fn clamp_time(self, time: Duration, fps: u32) -> Duration {
        let frame = ((time.as_nanos() * u128::from(fps.max(1)) + 500_000_000) / 1_000_000_000)
            .min(u128::from(u64::MAX)) as u64;
        Self::time(frame.max(self.start).min(self.end.saturating_sub(1)), fps)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_frame_ranges_exclude_end_and_keep_long_clock_precision() {
        for fps in [30, 60] {
            let r = Range {
                start: 17,
                end: 36001,
            };
            assert!(r.valid(36001));
            assert_eq!(r.len(), 35984);
            assert_eq!(Range::ticks(u64::from(fps) * 600, fps), Some(6_000_000_000));
            assert_eq!(r.clamp_time(Duration::ZERO, fps), Range::time(17, fps));
            assert_eq!(r.clamp_time(Duration::MAX, fps), Range::time(36000, fps));
        }
        assert!(!Range { start: 1, end: 1 }.valid(2));
        assert!(!Range { start: 0, end: 3 }.valid(2));
        assert!(Range { start: 1, end: 2 }.valid(2));
    }
}
