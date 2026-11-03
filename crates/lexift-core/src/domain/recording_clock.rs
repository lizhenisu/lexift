//! Pure recording time: paused wall time never advances the video timeline.
use std::time::Duration;

/// Signed age in nanoseconds. Some delivered WGC frames have a small compositor
/// clock lead; accept at most one output frame into the future.
pub fn capture_offset(received_qpc: i64, source_qpc: i64, fps: u32) -> Option<i64> {
    if fps == 0 {
        return None;
    }
    let ticks = received_qpc.checked_sub(source_qpc)?;
    if ticks < -(10_000_000 / i64::from(fps)) {
        return None;
    }
    ticks.checked_mul(100)
}

#[derive(Debug, Clone)]
pub struct Clock {
    fps: u32,
    origin: Duration,
    paused_at: Option<Duration>,
    paused: Duration,
    end: Option<Duration>,
    floor: Duration,
}

impl Clock {
    /// Derive audio indices from the same active duration and pause boundaries
    /// as video, without accumulating rounded per-packet durations.
    pub fn audio_sample(&self, now: Duration) -> crate::Result<u64> {
        super::recording_audio::frames_for(self.active(now))
    }
    pub fn new(fps: u32, origin: Duration) -> Option<Self> {
        (1..=60).contains(&fps).then_some(Self {
            fps,
            origin,
            paused_at: None,
            paused: Duration::ZERO,
            end: None,
            floor: Duration::ZERO,
        })
    }

    pub fn pause(&mut self, now: Duration) -> bool {
        if self.end.is_some() || self.paused_at.is_some() {
            return false;
        }
        self.paused_at = Some(now.max(self.origin + self.paused + self.floor));
        true
    }

    pub fn resume(&mut self, now: Duration) -> bool {
        if self.end.is_some() {
            return false;
        }
        let Some(at) = self.paused_at.take() else {
            return false;
        };
        self.floor = at
            .saturating_sub(self.origin)
            .saturating_sub(self.paused)
            .max(self.floor);
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
                .max(self.floor)
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

    /// Prefer a new source for the single retained tail slot. An unchanged
    /// desktop still advances at the next absolute deadline, without time drift.
    pub fn frame_ready(&self, now: Duration, submitted: u64, source_changed: bool) -> bool {
        let due = self.frames(now);
        submitted < due && (submitted == 1 || source_changed || submitted.saturating_add(1) < due)
    }

    pub fn backlog_exceeded(&self, now: Duration, submitted: u64) -> bool {
        self.frames(now).saturating_sub(submitted) > u64::from(self.fps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_source_tail_slot_is_bounded_for_dynamic_static_and_delayed_frames() {
        for fps in [30, 60] {
            let c = Clock::new(fps, Duration::ZERO).unwrap();
            assert!(c.frame_ready(Duration::ZERO, 0, true));
            let one = Duration::from_nanos(1_000_000_000 / u64::from(fps) + 1);
            assert!(c.frame_ready(one, 1, false));
            assert!(c.frame_ready(one, 1, true));
            let two = Duration::from_nanos(2_000_000_000 / u64::from(fps) + 1);
            assert!(c.frame_ready(two, 1, false));
            assert!(!c.frame_ready(two, 2, false));
            assert!(!c.frame_ready(two, 3, true));
        }
    }
    #[test]
    fn both_rates_keep_exact_long_duration_and_audio_boundaries() {
        for fps in [30, 60] {
            let mut c = Clock::new(fps, Duration::ZERO).unwrap();
            let count = 60 * 60 * 10 * u64::from(fps);
            let (pts, duration) = c.sample(count - 1).unwrap();
            assert_eq!(pts + duration, 360_000_000_000);
            assert_eq!(c.frames(Duration::from_secs(36000)), count);
            c.pause(Duration::from_millis(101));
            c.resume(Duration::from_millis(2101));
            assert_eq!(c.audio_sample(Duration::from_millis(2201)).unwrap(), 9648);
            c.stop(Duration::from_millis(2201));
            assert_eq!(
                c.frames(Duration::from_secs(100)),
                (201 * u64::from(fps)).div_ceil(1000)
            );
        }
    }
    #[test]
    fn presentation_clock_lead_is_bounded_and_resume_cannot_move_media_backwards() {
        assert_eq!(capture_offset(1_000_000, 930_000, 30), Some(7_000_000));
        assert_eq!(capture_offset(1_000_000, 1_070_000, 30), Some(-7_000_000));
        assert_eq!(capture_offset(1_000_000, 1_400_000, 30), None);
        assert_eq!(capture_offset(i64::MAX, 0, 30), None);
        let mut clock = Clock::new(30, Duration::ZERO).unwrap();
        clock.pause(Duration::from_secs(1));
        clock.resume(Duration::from_millis(1407));
        assert_eq!(
            clock.active(Duration::from_millis(1400)),
            Duration::from_secs(1)
        );
        assert_eq!(
            clock.audio_sample(Duration::from_millis(1400)).unwrap(),
            48000
        );
        assert_eq!(
            clock.active(Duration::from_millis(1417)),
            Duration::from_millis(1010)
        );
    }
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
