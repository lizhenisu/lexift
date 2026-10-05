//! Session-owned precise waits; no process-wide timer resolution changes.
use lexift_core::ports::annotation_render::RenderStamp;
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::*,
    },
    core::Result,
};

// Comparison retries also pay for capture enumeration; a full 1 ms sleep adds
// avoidable latency after that work. Normal display ticks retain their 60 Hz cadence.
pub(super) const SOURCE_RETRY_DELAY: Duration = Duration::from_micros(500);
/// Worker-local cadence. Scene revisions can advance one attempt without turning
/// an unpresented revision into an unbounded retry loop.
pub(super) struct Cadence {
    period: Duration,
    regular_deadline: Instant,
    next_attempt: Instant,
    attempted: Option<RenderStamp>,
}
impl Cadence {
    pub fn new(now: Instant, period: Duration) -> Self {
        Self {
            period,
            regular_deadline: now,
            next_attempt: now,
            attempted: None,
        }
    }
    pub fn reset(&mut self, now: Instant) {
        *self = Self::new(now, self.period);
    }
    /// Only the active capture backend supplies arrival; unrelated wakes do not
    /// advance a retry. The mailbox has already coalesced scene submissions.
    pub fn remaining(&mut self, now: Instant, stamp: RenderStamp, arrival: bool) -> Duration {
        if self.attempted != Some(stamp) || arrival {
            self.next_attempt = now;
        }
        self.next_attempt.saturating_duration_since(now)
    }
    pub fn begin(&mut self, now: Instant, stamp: RenderStamp) {
        self.attempted = Some(stamp);
        // Keep the regular phase for early attempts and drop missed ticks.
        if self.regular_deadline <= now {
            self.regular_deadline += self.period;
            if self.regular_deadline <= now {
                self.regular_deadline = now + self.period;
            }
        }
        self.next_attempt = self.regular_deadline;
    }
    pub fn retry_source(&mut self, now: Instant) {
        self.next_attempt = self.next_attempt.min(now + SOURCE_RETRY_DELAY);
    }
}

pub(super) struct Signal(HANDLE);

fn finite_timeout_ms(duration: Duration) -> u32 {
    duration
        .as_nanos()
        .div_ceil(1_000_000)
        .min(u128::from(INFINITE - 1)) as u32
}
// Kernel event operations are thread safe. Arc ownership outlives every waiter.
unsafe impl Send for Signal {}
unsafe impl Sync for Signal {}
impl Signal {
    pub fn new() -> Result<Self> {
        unsafe { CreateEventW(None, false, false, None).map(Self) }
    }
    pub fn wake(&self) {
        let _ = unsafe { SetEvent(self.0) };
    }
    /// Timer fallback: the event retains a notification sent before wait entry.
    /// Returns true for a signal, false for timeout; never requests an infinite wait.
    pub fn wait(&self, duration: Duration) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.0, finite_timeout_ms(duration)) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(windows::core::Error::from_thread()),
        }
    }
}
impl Drop for Signal {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wake {
    Signal,
    Timer,
}
#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) struct WaitSample {
    pub before: Instant,
    pub after: Instant,
    pub requested: Duration,
    pub wake: Wake,
}
pub(super) struct Clock {
    handle: HANDLE,
    #[cfg(test)]
    recording: std::cell::Cell<bool>,
    #[cfg(test)]
    samples: std::cell::RefCell<Vec<WaitSample>>,
    #[cfg(test)]
    dropped: std::cell::Cell<usize>,
}
impl Clock {
    pub fn new() -> Result<Self> {
        unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .map(|handle| Self {
                handle,
                #[cfg(test)]
                recording: Default::default(),
                #[cfg(test)]
                samples: Default::default(),
                #[cfg(test)]
                dropped: Default::default(),
            })
        }
    }
    pub fn wait(&self, duration: Duration, signal: &Signal) -> Result<()> {
        let ticks = duration.as_nanos().div_ceil(100).clamp(1, i64::MAX as u128) as i64;
        unsafe {
            SetWaitableTimer(self.handle, &-ticks, 0, None, None, false)?;
            #[cfg(test)]
            let before = self.recording.get().then(Instant::now);
            let result = WaitForMultipleObjects(&[signal.0, self.handle], false, INFINITE);
            #[cfg(test)]
            if let Some(before) = before {
                let after = Instant::now();
                let wake = if result == WAIT_OBJECT_0 {
                    Some(Wake::Signal)
                } else if result.0 == WAIT_OBJECT_0.0 + 1 {
                    Some(Wake::Timer)
                } else {
                    None
                };
                if let Some(wake) = wake {
                    let mut samples = self.samples.borrow_mut();
                    if samples.len() < 65_536 {
                        samples.push(WaitSample {
                            before,
                            after,
                            requested: duration,
                            wake,
                        });
                    } else {
                        self.dropped.set(self.dropped.get() + 1);
                    }
                }
            }
            let _ = CancelWaitableTimer(self.handle);
            if result == WAIT_OBJECT_0 || result.0 == WAIT_OBJECT_0.0 + 1 {
                Ok(())
            } else {
                Err(windows::core::Error::from_thread())
            }
        }
    }
    #[cfg(test)]
    pub fn record(&self, enabled: bool) {
        self.recording.set(enabled);
    }
    #[cfg(test)]
    pub fn take_waits(&self) -> (Vec<WaitSample>, usize) {
        (
            std::mem::take(&mut *self.samples.borrow_mut()),
            self.dropped.replace(0),
        )
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        unsafe {
            let _ = CancelWaitableTimer(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stamp(revision: u64) -> RenderStamp {
        RenderStamp {
            session: 1,
            revision,
        }
    }

    #[test]
    fn fallback_retains_pre_wait_notifications_and_coalesces_them_once() {
        let mailbox = std::sync::Mutex::new(());
        let condition = std::sync::Condvar::new();
        condition.notify_one();
        let (_old_guard, old_wait) = condition
            .wait_timeout(mailbox.lock().unwrap(), Duration::ZERO)
            .unwrap();
        assert!(
            old_wait.timed_out(),
            "condition notifications are not buffered"
        );
        let signal = Signal::new().unwrap();
        // Source callbacks do not take the mailbox mutex. Unlike a condition
        // notification, this wake survives the gap before the worker waits.
        signal.wake();
        signal.wake();
        assert!(signal.wait(Duration::ZERO).unwrap());
        assert!(!signal.wait(Duration::ZERO).unwrap());
        signal.wake();
        assert!(signal.wait(Duration::from_secs(1)).unwrap());
        assert!(!signal.wait(Duration::ZERO).unwrap());
    }

    #[test]
    fn fallback_rounds_up_short_deadlines_without_converting_to_infinite_wait() {
        assert_eq!(finite_timeout_ms(Duration::ZERO), 0);
        assert_eq!(finite_timeout_ms(Duration::from_nanos(1)), 1);
        assert_eq!(finite_timeout_ms(SOURCE_RETRY_DELAY), 1);
        assert_eq!(finite_timeout_ms(Duration::from_millis(16)), 16);
        assert_eq!(finite_timeout_ms(Duration::from_micros(16_001)), 17);
        assert_eq!(finite_timeout_ms(Duration::MAX), INFINITE - 1);
    }

    #[test]
    fn wait_diagnostics_report_actual_kernel_wake_without_recording_by_default() {
        let clock = Clock::new().unwrap();
        let signal = Signal::new().unwrap();
        signal.wake();
        clock.wait(Duration::from_millis(10), &signal).unwrap();
        assert!(clock.take_waits().0.is_empty());
        clock.record(true);
        signal.wake();
        clock.wait(Duration::from_millis(10), &signal).unwrap();
        clock.wait(Duration::from_micros(200), &signal).unwrap();
        let (samples, dropped) = clock.take_waits();
        assert_eq!(dropped, 0);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].wake, Wake::Signal);
        assert_eq!(samples[1].wake, Wake::Timer);
        assert_eq!(samples[0].requested, Duration::from_millis(10));
        assert!(samples.iter().all(|s| s.before <= s.after));
        assert!(clock.take_waits().0.is_empty());
        clock.record(false);
        signal.wake();
        clock.wait(Duration::from_millis(10), &signal).unwrap();
        assert!(clock.take_waits().0.is_empty());
    }

    #[test]
    fn new_scene_advances_once_without_changing_regular_phase() {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / 60.);
        let mut cadence = Cadence::new(start, period);
        cadence.begin(start, stamp(1));
        let event = start + Duration::from_millis(1);
        // Before this change the worker used this remaining regular deadline
        // even after the scene submission woke its kernel event.
        assert!(cadence.next_attempt.duration_since(event) > Duration::from_millis(15));
        assert_eq!(cadence.remaining(event, stamp(2), false), Duration::ZERO);
        cadence.begin(event, stamp(2));
        assert_eq!(
            cadence.remaining(event, stamp(2), false),
            period - Duration::from_millis(1)
        );
        assert_eq!(cadence.regular_deadline, start + period);
        // Coalesced revisions require only the latest mailbox scene.
        assert_eq!(cadence.remaining(event, stamp(4), false), Duration::ZERO);
        cadence.begin(event, stamp(4));
        assert!(!cadence.remaining(event, stamp(4), false).is_zero());
    }

    #[test]
    fn source_arrival_and_retry_preserve_throttling() {
        let start = Instant::now();
        let period = Duration::from_millis(16);
        let mut cadence = Cadence::new(start, period);
        cadence.begin(start, stamp(1));
        let event = start + Duration::from_millis(2);
        assert!(!cadence.remaining(event, stamp(1), false).is_zero());
        assert_eq!(cadence.remaining(event, stamp(1), true), Duration::ZERO);
        cadence.begin(event, stamp(1));
        cadence.retry_source(event);
        assert_eq!(
            cadence.remaining(event, stamp(1), false),
            SOURCE_RETRY_DELAY
        );
        let retry = event + SOURCE_RETRY_DELAY;
        assert_eq!(cadence.remaining(retry, stamp(1), false), Duration::ZERO);
        cadence.begin(retry, stamp(1));
        assert!(!cadence.remaining(retry, stamp(1), false).is_zero());
    }

    #[test]
    fn missed_ticks_and_new_generation_do_not_replay_old_attempts() {
        let start = Instant::now();
        let period = Duration::from_millis(16);
        let mut cadence = Cadence::new(start, period);
        cadence.begin(start, stamp(1));
        let late = start + Duration::from_millis(90);
        cadence.begin(late, stamp(1));
        assert_eq!(cadence.remaining(late, stamp(1), false), period);
        cadence.reset(late);
        assert_eq!(cadence.remaining(late, stamp(1), false), Duration::ZERO);
        cadence.begin(late, stamp(1));
        assert_eq!(cadence.remaining(late, stamp(1), false), period);
        assert_eq!(
            cadence.remaining(
                late,
                RenderStamp {
                    session: 2,
                    revision: 1
                },
                false
            ),
            Duration::ZERO
        );
    }

    #[test]
    #[ignore = "requires Windows high-resolution waitable timers"]
    fn precise_wait_is_interruptible_and_reusable() {
        let clock = Clock::new().unwrap();
        let signal = std::sync::Arc::new(Signal::new().unwrap());
        let wake = signal.clone();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            wake.wake();
        });
        let start = std::time::Instant::now();
        clock.wait(Duration::from_secs(1), &signal).unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
        sender.join().unwrap();
        let start = std::time::Instant::now();
        clock.wait(Duration::from_millis(5), &signal).unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(4),
            "the canceled event must not prematurely wake a new wait"
        );
    }
}
