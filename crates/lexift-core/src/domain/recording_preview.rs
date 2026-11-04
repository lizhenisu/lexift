//! Playback belongs to a sealed recording, with its own command generation.
use super::{
    geometry::Rect,
    screen_recording::{Artifact, Identity as RecordingIdentity},
};
use crate::ports::magnifier::WindowToken;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub session: u64,
    pub operation: u64,
    pub artifact: Artifact,
    pub instance: u64,
    pub request: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub window: WindowToken,
    pub bounds: Rect,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Paused,
    Playing,
    Seeking,
    Ended,
    Failed,
    Retiring,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Save,
    Discard,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Target(Target),
    PlayPause,
    BeginSeek,
    CancelSeek,
    Seek(Duration),
    Mute,
    Save,
    Close,
    Retry,
    Failed(String),
    DiscardConfirmed(bool),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Frame {
        position: Duration,
        duration: Duration,
        has_audio: bool,
    },
    Visible,
    Paused,
    Ended,
    Failed(String),
    Retired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PrepareView,
    Open(Target),
    Play,
    Pause,
    Seek(Duration),
    Mute(bool),
    Close,
    Handoff,
    Install,
    Save,
    ConfirmDiscard,
    Discard,
    DestroyView,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub identity: Identity,
    pub action: Action,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub identity: Identity,
    pub phase: Phase,
    pub position: Duration,
    pub duration: Duration,
    pub has_audio: bool,
    pub muted: bool,
    pub error: Option<String>,
    pub presented: bool,
    pub visible: bool,
    resume: bool,
    seek_pending: bool,
    intent: Option<Intent>,
    cleanup_failed: bool,
}
impl State {
    pub fn new(id: RecordingIdentity, artifact: Artifact) -> Self {
        Self {
            identity: Identity {
                session: id.session,
                operation: id.operation,
                artifact,
                instance: 1,
                request: 0,
            },
            phase: Phase::Preparing,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            has_audio: false,
            muted: false,
            error: None,
            presented: false,
            visible: false,
            resume: false,
            seek_pending: false,
            intent: None,
            cleanup_failed: false,
        }
    }
    pub fn command(&self, action: Action) -> Command {
        Command {
            identity: self.identity,
            action,
        }
    }
    fn next(&mut self, action: Action) -> Option<Command> {
        self.identity.request += 1;
        Some(self.command(action))
    }
    pub fn input(&mut self, id: Identity, input: Input) -> Option<Command> {
        if id != self.identity || matches!(self.phase, Phase::Retiring | Phase::Closed) {
            return None;
        }
        // Confirmation owns input until it resolves. In particular, a delayed
        // target or retry must not replace its request/window underneath it.
        if self.intent.is_some() && !matches!(input, Input::DiscardConfirmed(_)) {
            return None;
        }
        if input == Input::Save && !self.visible && self.phase != Phase::Failed {
            return None;
        }
        match input {
            Input::Target(target) if self.phase == Phase::Preparing => {
                self.next(Action::Open(target))
            }
            Input::PlayPause
                if self.intent.is_none() && matches!(self.phase, Phase::Paused | Phase::Ended) =>
            {
                if self.phase == Phase::Ended {
                    self.resume = true;
                    self.seek_pending = true;
                    self.phase = Phase::Seeking;
                    self.next(Action::Seek(Duration::ZERO))
                } else {
                    self.phase = Phase::Playing;
                    self.next(Action::Play)
                }
            }
            Input::PlayPause if self.phase == Phase::Playing => {
                self.phase = Phase::Paused;
                self.next(Action::Pause)
            }
            Input::BeginSeek
                if self.intent.is_none()
                    && matches!(
                        self.phase,
                        Phase::Paused | Phase::Playing | Phase::Seeking | Phase::Ended
                    ) =>
            {
                if self.phase != Phase::Seeking {
                    self.resume = self.phase == Phase::Playing;
                }
                self.phase = Phase::Seeking;
                self.seek_pending = false;
                // Begin and commit may arrive in one UI turn. Pausing does not
                // create a new decoded result; only the committed seek advances
                // its generation. Frames from before the commit remain ignored.
                Some(self.command(Action::Pause))
            }
            Input::Seek(position) if self.phase == Phase::Seeking && self.intent.is_none() => {
                self.seek_pending = true;
                self.next(Action::Seek(
                    position.min(self.duration.saturating_sub(Duration::from_nanos(1))),
                ))
            }
            Input::CancelSeek if self.phase == Phase::Seeking => {
                self.resume = false;
                self.seek_pending = false;
                self.phase = Phase::Paused;
                self.next(Action::Pause)
            }
            Input::Mute
                if self.has_audio
                    && self.intent.is_none()
                    && matches!(
                        self.phase,
                        Phase::Paused | Phase::Playing | Phase::Seeking | Phase::Ended
                    ) =>
            {
                self.muted = !self.muted;
                // Audio policy does not replace a pending video frame. A seek
                // completing concurrently must retain its valid generation.
                Some(self.command(Action::Mute(self.muted)))
            }
            Input::Save | Input::Close if self.intent.is_none() => {
                let intent = if input == Input::Save {
                    Intent::Save
                } else {
                    Intent::Discard
                };
                self.intent = Some(intent);
                self.resume = false;
                if matches!(self.phase, Phase::Failed | Phase::Preparing) {
                    Some(self.command(if intent == Intent::Save {
                        Action::Save
                    } else {
                        Action::ConfirmDiscard
                    }))
                } else {
                    self.phase = Phase::Paused;
                    self.next(Action::Pause)
                }
            }
            Input::DiscardConfirmed(false) if self.intent == Some(Intent::Discard) => {
                self.intent = None;
                if self.phase == Phase::Preparing && self.identity.request == 0 {
                    Some(self.command(Action::PrepareView))
                } else if self.phase == Phase::Paused && !self.visible && self.presented {
                    Some(self.command(Action::Handoff))
                } else {
                    None
                }
            }
            Input::DiscardConfirmed(true) if self.intent == Some(Intent::Discard) => {
                Some(self.command(Action::Discard))
            }
            Input::Retry if self.phase == Phase::Failed => {
                self.identity.instance += 1;
                self.identity.request = 0;
                self.phase = Phase::Preparing;
                self.error = None;
                self.intent = None;
                self.resume = false;
                self.presented = false;
                self.muted = false;
                Some(self.command(Action::PrepareView))
            }
            Input::Failed(error) => {
                self.error = Some(error);
                self.phase = Phase::Failed;
                self.intent = None;
                None
            }
            _ => None,
        }
    }
    pub fn notice(&mut self, id: Identity, notice: Notice) -> Option<Command> {
        if id != self.identity {
            return None;
        }
        match notice {
            Notice::Visible
                if !self.visible
                    && matches!(
                        self.phase,
                        Phase::Paused | Phase::Playing | Phase::Seeking | Phase::Ended
                    ) =>
            {
                self.visible = true;
                Some(self.command(Action::Install))
            }
            Notice::Frame {
                position,
                duration,
                has_audio,
            } if !matches!(self.phase, Phase::Retiring | Phase::Closed | Phase::Failed) => {
                self.duration = duration;
                self.has_audio = has_audio;
                if self.phase == Phase::Seeking && !self.seek_pending {
                    return None;
                }
                self.position = position.min(duration);
                if self.phase == Phase::Preparing {
                    self.phase = Phase::Paused;
                    self.presented = true;
                    return Some(self.command(Action::Handoff));
                }
                if self.phase == Phase::Seeking {
                    self.seek_pending = false;
                    self.phase = if self.resume {
                        Phase::Playing
                    } else {
                        Phase::Paused
                    };
                    if self.resume {
                        return self.next(Action::Play);
                    }
                }
                None
            }
            Notice::Paused => self.intent.map(|i| {
                self.command(if i == Intent::Save {
                    Action::Save
                } else {
                    Action::ConfirmDiscard
                })
            }),
            Notice::Ended if self.phase == Phase::Playing => {
                self.phase = Phase::Ended;
                self.position = self.duration;
                None
            }
            Notice::Failed(error) if self.phase != Phase::Retiring => {
                self.error = Some(error);
                self.intent = None;
                if !self.visible {
                    self.cleanup_failed = true;
                    Some(self.close())
                } else {
                    self.phase = Phase::Failed;
                    None
                }
            }
            Notice::Retired if self.phase == Phase::Retiring && self.cleanup_failed => {
                self.cleanup_failed = false;
                self.phase = Phase::Failed;
                Some(self.command(Action::DestroyView))
            }
            Notice::Retired if self.phase == Phase::Retiring => {
                self.phase = Phase::Closed;
                Some(self.command(Action::DestroyView))
            }
            _ => None,
        }
    }
    pub fn save_finished(&mut self) {
        self.intent = None;
    }
    pub fn confirming_discard(&self) -> bool {
        self.intent == Some(Intent::Discard)
    }
    pub fn close(&mut self) -> Command {
        self.phase = Phase::Retiring;
        self.identity.request += 1;
        self.command(Action::Close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        State::new(
            RecordingIdentity {
                session: 1,
                operation: 2,
                request: 3,
            },
            Artifact(4),
        )
    }
    #[test]
    fn closing_during_preparation_resumes_the_owned_handoff_when_cancelled() {
        let mut s = state();
        let id = s.identity;
        let target = Target {
            window: WindowToken(3),
            bounds: Rect {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
        };
        assert_eq!(
            s.input(id, Input::Close).unwrap().action,
            Action::ConfirmDiscard
        );
        assert!(s.input(id, Input::Target(target)).is_none());
        assert_eq!(
            s.input(id, Input::DiscardConfirmed(false)).unwrap().action,
            Action::PrepareView
        );
        let open = s.input(id, Input::Target(target)).unwrap();
        assert!(s.input(id, Input::Target(target)).is_none());
        s.notice(open.identity, frame());
        let pause = s.input(s.identity, Input::Close).unwrap();
        assert_eq!(
            s.notice(pause.identity, Notice::Paused).unwrap().action,
            Action::ConfirmDiscard
        );
        assert_eq!(
            s.input(s.identity, Input::DiscardConfirmed(false))
                .unwrap()
                .action,
            Action::Handoff
        );
    }
    #[test]
    fn confirmation_blocks_retry_and_stale_cancellation_cannot_cancel_a_save() {
        let mut s = state();
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        s.notice(s.identity, Notice::Failed("failed".into()));
        s.input(s.identity, Input::Close);
        assert!(s.input(s.identity, Input::Retry).is_none());
        s.input(s.identity, Input::DiscardConfirmed(false));
        assert_eq!(
            s.input(s.identity, Input::Save).unwrap().action,
            Action::Save
        );
        assert!(
            s.input(s.identity, Input::DiscardConfirmed(false))
                .is_none()
        );
        assert!(s.input(s.identity, Input::Retry).is_none());
        s.save_finished();
        assert_eq!(
            s.input(s.identity, Input::Retry).unwrap().action,
            Action::PrepareView
        );
    }
    #[test]
    fn a_hidden_decoded_frame_cannot_start_saving_and_lose_its_handoff() {
        let mut s = state();
        s.notice(s.identity, frame());
        assert!(s.input(s.identity, Input::Save).is_none());
        s.notice(s.identity, Notice::Visible);
        assert_eq!(
            s.input(s.identity, Input::Save).unwrap().action,
            Action::Pause
        );
    }
    #[test]
    fn muting_during_a_pending_seek_does_not_invalidate_its_frame() {
        let mut s = state();
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        s.input(s.identity, Input::BeginSeek);
        let seek = s
            .input(s.identity, Input::Seek(Duration::from_secs(4)))
            .unwrap();
        let mute = s.input(s.identity, Input::Mute).unwrap();
        assert_eq!(mute.identity, seek.identity);
        s.notice(
            seek.identity,
            Notice::Frame {
                position: Duration::from_secs(4),
                duration: Duration::from_secs(10),
                has_audio: true,
            },
        );
        assert_eq!(s.phase, Phase::Paused);
        assert_eq!(s.position, Duration::from_secs(4));
        assert!(s.muted);
    }
    #[test]
    fn redrag_before_seek_completion_preserves_the_newest_target_and_resume_policy() {
        let mut s = state();
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        s.input(s.identity, Input::PlayPause);
        s.input(s.identity, Input::BeginSeek);
        let old = s
            .input(s.identity, Input::Seek(Duration::from_secs(8)))
            .unwrap();
        assert_eq!(
            s.input(s.identity, Input::BeginSeek).unwrap().action,
            Action::Pause
        );
        assert!(s.notice(old.identity, frame()).is_none());
        assert_eq!(s.phase, Phase::Seeking);
        let latest = s
            .input(s.identity, Input::Seek(Duration::from_secs(2)))
            .unwrap();
        let resume = s
            .notice(
                latest.identity,
                Notice::Frame {
                    position: Duration::from_secs(2),
                    duration: Duration::from_secs(10),
                    has_audio: true,
                },
            )
            .unwrap();
        assert_eq!(resume.action, Action::Play);
        assert_eq!(s.position, Duration::from_secs(2));
    }
    fn frame() -> Notice {
        Notice::Frame {
            position: Duration::ZERO,
            duration: Duration::from_secs(10),
            has_audio: true,
        }
    }
    #[test]
    fn first_frame_is_paused_and_stale_seek_cannot_install() {
        let mut s = state();
        assert_eq!(
            s.notice(s.identity, frame()).unwrap().action,
            Action::Handoff
        );
        assert_eq!(s.phase, Phase::Paused);
        s.input(s.identity, Input::PlayPause);
        s.input(s.identity, Input::BeginSeek);
        s.input(s.identity, Input::Seek(Duration::from_secs(2)));
        let old = s.identity;
        s.input(s.identity, Input::Seek(Duration::from_secs(8)));
        assert!(s.notice(old, frame()).is_none());
        assert_eq!(s.phase, Phase::Seeking);
        assert_eq!(s.notice(s.identity, frame()).unwrap().action, Action::Play);
    }
    #[test]
    fn save_and_discard_wait_for_pause_and_do_not_resume() {
        let mut s = state();
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        s.input(s.identity, Input::PlayPause);
        assert_eq!(
            s.input(s.identity, Input::Save).unwrap().action,
            Action::Pause
        );
        assert_eq!(
            s.notice(s.identity, Notice::Paused).unwrap().action,
            Action::Save
        );
        s.save_finished();
        assert_eq!(s.phase, Phase::Paused);
        s.input(s.identity, Input::Close);
        assert_eq!(
            s.notice(s.identity, Notice::Paused).unwrap().action,
            Action::ConfirmDiscard
        );
        assert!(
            s.input(s.identity, Input::DiscardConfirmed(false))
                .is_none()
        );
        assert_eq!(s.phase, Phase::Paused);
    }
    #[test]
    fn seek_begin_and_commit_share_the_same_view_snapshot() {
        let mut s = state();
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        let rendered = s.identity;
        assert_eq!(
            s.input(rendered, Input::BeginSeek).unwrap().action,
            Action::Pause
        );
        let seek = s
            .input(rendered, Input::Seek(Duration::from_secs(4)))
            .unwrap();
        assert!(seek.identity.request > rendered.request);
        assert!(s.notice(rendered, frame()).is_none());
        assert_eq!(s.phase, Phase::Seeking);
        s.notice(seek.identity, frame());
        assert_eq!(s.phase, Phase::Paused);
    }
    #[test]
    fn failed_hidden_player_retires_before_retry_and_old_results_are_rejected() {
        let mut s = state();
        let old = s.identity;
        let close = s.notice(old, Notice::Failed("failed".into())).unwrap();
        assert_eq!(close.action, Action::Close);
        assert_eq!(s.phase, Phase::Retiring);
        assert!(s.input(s.identity, Input::Retry).is_none());
        assert_eq!(
            s.notice(s.identity, Notice::Retired).unwrap().action,
            Action::DestroyView
        );
        assert_eq!(s.phase, Phase::Failed);
        let retry = s.input(s.identity, Input::Retry).unwrap();
        assert_eq!(retry.action, Action::PrepareView);
        assert!(retry.identity.instance > old.instance);
        assert!(s.notice(old, frame()).is_none());
        assert!(!s.presented);
    }
    #[test]
    fn visible_failure_keeps_result_and_ended_playback_seeks_before_restarting() {
        let mut s = state();
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        s.notice(s.identity, Notice::Visible);
        s.input(s.identity, Input::PlayPause);
        s.notice(s.identity, Notice::Ended);
        assert_eq!(s.phase, Phase::Ended);
        assert_eq!(
            s.input(s.identity, Input::PlayPause).unwrap().action,
            Action::Seek(Duration::ZERO)
        );
        assert_eq!(s.notice(s.identity, frame()).unwrap().action, Action::Play);
        assert!(
            s.notice(s.identity, Notice::Failed("failed".into()))
                .is_none()
        );
        assert_eq!(s.phase, Phase::Failed);
        assert_eq!(
            s.input(s.identity, Input::Save).unwrap().action,
            Action::Save
        );
    }
}
