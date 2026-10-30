//! Pure recording time: paused wall time never advances the video timeline.
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Clock {
    fps: u32,
    origin: Duration,
    paused_at: Option<Duration>,
    paused: Duration,
    end: Option<Duration>,
}

impl Clock {
    pub fn new(fps: u32, origin: Duration) -> Option<Self> {
        (1..=60).contains(&fps).then_some(Self {
            fps,
            origin,
            paused_at: None,
            paused: Duration::ZERO,
            end: None,
        })
    }

    pub fn pause(&mut self, now: Duration) -> bool {
        if self.end.is_some() || self.paused_at.is_some() {
            return false;
        }
        self.paused_at = Some(now.max(self.origin + self.paused));
        true
    }

    pub fn resume(&mut self, now: Duration) -> bool {
        if self.end.is_some() {
            return false;
        }
        let Some(at) = self.paused_at.take() else {
            return false;
        };
        self.paused += now.saturating_sub(at);
        true
    }

    /// Freeze once. A repeated stop cannot extend the accepted recording.
    pub fn stop(&mut self, now: Duration) -> Duration {
        let end = self.end.unwrap_or_else(|| self.active(now));
        self.end = Some(end);
        end
    }

    pub fn active(&self, now: Duration) -> Duration {
        self.end.unwrap_or_else(|| {
            self.paused_at
                .unwrap_or(now)
                .saturating_sub(self.origin)
                .saturating_sub(self.paused)
        })
    }

    /// Number of samples covering the active interval (including a fresh first frame).
    pub fn frames(&self, now: Duration) -> u64 {
        let ticks = self.active(now).as_nanos() * u128::from(self.fps);
        u64::try_from(ticks.div_ceil(1_000_000_000).max(1)).unwrap_or(u64::MAX)
    }

    /// Absolute 100ns timestamps avoid accumulating 333333-tick rounding errors.
    pub fn sample(&self, index: u64) -> Option<(i64, i64)> {
        let ticks = |i: u128| i * 10_000_000 / u128::from(self.fps);
        let start = ticks(u128::from(index));
        let end = ticks(u128::from(index) + 1);
        Some((i64::try_from(start).ok()?, i64::try_from(end - start).ok()?))
    }

    pub fn backlog_exceeded(&self, now: Duration, submitted: u64) -> bool {
        self.frames(now).saturating_sub(submitted) > u64::from(self.fps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn pause_resume_and_duplicate_commands_preserve_active_time() {
        let mut clock = Clock::new(30, ms(100)).unwrap();
        assert_eq!(clock.frames(ms(1100)), 30);
        assert!(clock.pause(ms(1100)));
        assert!(!clock.pause(ms(2100)));
        assert_eq!(clock.frames(ms(8100)), 30);
        assert!(clock.resume(ms(8100)));
        assert!(!clock.resume(ms(9100)));
        assert_eq!(clock.stop(ms(9100)), ms(2000));
        assert_eq!(clock.stop(ms(15100)), ms(2000));
        assert!(!clock.resume(ms(16100)));
    }

    #[test]
    fn static_and_delayed_sources_do_not_shorten_output() {
        let clock = Clock::new(30, ms(0)).unwrap();
        assert_eq!(clock.frames(ms(0)), 1);
        assert_eq!(clock.frames(ms(10001)), 301);
        assert!(clock.backlog_exceeded(ms(2000), 29));
        assert!(!clock.backlog_exceeded(ms(2000), 30));
    }

    #[test]
    fn stop_while_paused_freezes_at_pause_boundary() {
        let mut clock = Clock::new(30, ms(0)).unwrap();
        assert!(clock.pause(ms(1000)));
        assert_eq!(clock.stop(ms(60000)), ms(1000));
        assert_eq!(clock.frames(ms(120000)), 30);
        assert!(!clock.resume(ms(130000)));
        assert!(!clock.pause(ms(140000)));
    }

    #[test]
    fn repeated_pauses_and_long_static_time_keep_exact_frame_count() {
        let mut clock = Clock::new(30, ms(0)).unwrap();
        for index in 0..100 {
            assert!(clock.pause(ms(index * 2000 + 1000)));
            assert!(clock.resume(ms(index * 2000 + 2000)));
        }
        assert_eq!(clock.frames(ms(200000)), 3000);
        assert_eq!(clock.stop(ms(700000)), ms(600000));
        assert_eq!(clock.frames(ms(900000)), 18000);
    }

    #[test]
    fn absolute_sample_times_are_contiguous_over_long_recordings() {
        let clock = Clock::new(30, ms(0)).unwrap();
        let mut total = 0;
        for i in 0..108_000 {
            let (pts, duration) = clock.sample(i).unwrap();
            assert_eq!(pts, total);
            total += duration;
        }
        assert_eq!(total, 36_000_000_000);
        assert!(clock.sample(u64::MAX).is_none());
        assert!(Clock::new(0, ms(0)).is_none());
    }
}
