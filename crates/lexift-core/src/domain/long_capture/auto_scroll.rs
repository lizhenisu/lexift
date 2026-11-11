//! Timing and frame acceptance for one automatic scroll run.
use super::Feedback;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Manual,
    Waiting,
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    TargetTimeout,
    TargetChanged,
    InputUnavailable,
    Unchanged,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub generation: u64,
    pub mode: Mode,
    pub reason: Option<Stop>,
}

impl State {
    pub(super) fn stop(&mut self, reason: Option<Stop>) {
        self.mode = Mode::Manual;
        self.reason = reason;
    }
}

pub const TARGET_WAIT: Duration = Duration::from_secs(5);
pub const TARGET_DWELL: Duration = Duration::from_millis(300);
const STEP_TIMEOUT: Duration = Duration::from_millis(1500);
const SETTLE: Duration = Duration::from_millis(180);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Wait,
    Scroll,
    Stop(Stop),
}

/// A step starts only after its predecessor has appended rows and settled.
#[derive(Default)]
pub struct Steps {
    sent: Option<Duration>,
    accepted: Option<Duration>,
    uncertain: bool,
}

impl Steps {
    pub fn sent(&mut self, now: Duration) {
        *self = Self {
            sent: Some(now),
            ..Self::default()
        };
    }

    pub fn frame(&mut self, feedback: Feedback, now: Duration) {
        if self.sent.is_none() {
            return;
        }
        match feedback {
            Feedback::Accepted => {
                self.accepted = Some(now);
                self.uncertain = false;
            }
            Feedback::Uncertain => self.uncertain = true,
            Feedback::Unchanged if self.uncertain => {
                if self.accepted.is_some() {
                    self.accepted = Some(now);
                }
                self.uncertain = false;
            }
            _ => {}
        }
    }

    pub fn next(&self, now: Duration, aligned: bool) -> Next {
        let Some(sent) = self.sent else {
            return if aligned {
                Next::Scroll
            } else {
                Next::Stop(Stop::Uncertain)
            };
        };
        if now.saturating_sub(sent) >= STEP_TIMEOUT {
            return Next::Stop(if self.uncertain || self.accepted.is_some() {
                Stop::Uncertain
            } else {
                Stop::Unchanged
            });
        }
        if aligned
            && self
                .accepted
                .is_some_and(|at| now.saturating_sub(at) >= SETTLE)
        {
            return Next::Scroll;
        }
        Next::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_accepted_rows_and_settling_then_times_out_on_duplicates() {
        let ms = Duration::from_millis;
        let mut steps = Steps::default();
        assert_eq!(steps.next(ms(0), false), Next::Stop(Stop::Uncertain));
        assert_eq!(steps.next(ms(0), true), Next::Scroll);
        steps.sent(ms(0));
        steps.frame(Feedback::Unchanged, ms(50));
        assert_eq!(steps.next(ms(250), true), Next::Wait);
        steps.frame(Feedback::Accepted, ms(300));
        steps.frame(Feedback::Accepted, ms(400));
        assert_eq!(steps.next(ms(579), true), Next::Wait);
        assert_eq!(steps.next(ms(580), true), Next::Scroll);
        steps.sent(ms(580));
        assert_eq!(steps.next(ms(2080), true), Next::Stop(Stop::Unchanged));
        steps.frame(Feedback::Uncertain, ms(2090));
        assert_eq!(steps.next(ms(2100), false), Next::Stop(Stop::Uncertain));
        steps.sent(ms(2200));
        steps.frame(Feedback::Accepted, ms(2250));
        steps.frame(Feedback::Uncertain, ms(2440));
        steps.frame(Feedback::Unchanged, ms(2460));
        assert_eq!(steps.next(ms(2639), true), Next::Wait);
        assert_eq!(steps.next(ms(2640), true), Next::Scroll);
        steps.sent(ms(3000));
        steps.frame(Feedback::Accepted, ms(4390));
        assert_eq!(steps.next(ms(4570), true), Next::Stop(Stop::Uncertain));
    }
}
