//! Playback belongs to a sealed recording, with its own command generation.
use super::recording_trim::{ExportAudio, FrameCrop, Range, Speed};
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
    BeginTrim,
    SetRange { range: Range, end_handle: bool },
    ResetRange,
    BeginFrameCrop,
    CancelFrameCrop,
    SetFrameCrop(Option<FrameCrop>),
    CropGuide(FrameCrop),
    CancelSeek,
    Seek(Duration),
    Mute,
    SetSpeed(Speed),
    SetExportAudio(ExportAudio),
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
    FrameCropPresented(bool),
    SpeedConfigured(bool),
    ExportAudioConfigured {
        request: u64,
        success: bool,
    },
    Paused,
    Ended,
    Failed(String),
    Retired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PrepareView,
    Open {
        target: Target,
        range: Range,
        fps: u32,
        version: u64,
        crop: Option<FrameCrop>,
        speed: Speed,
        muted: bool,
        audio: ExportAudio,
    },
    Play,
    Pause,
    Seek(Duration),
    SetRange {
        range: Range,
        fps: u32,
        version: u64,
        position: Duration,
    },
    SetFrameCrop(Option<FrameCrop>),
    CropGuide(Option<FrameCrop>),
    Mute(bool),
    SetExportAudio {
        request: u64,
        mode: ExportAudio,
    },
    SetSpeed(Speed),
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
impl Command {
    pub fn dispatch_failure(self, message: String) -> Notice {
        match self.action {
            Action::SetExportAudio { request, .. } => Notice::ExportAudioConfigured {
                request,
                success: false,
            },
            _ => Notice::Failed(message),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub identity: Identity,
    pub phase: Phase,
    pub range: Range,
    pub frames: u64,
    pub fps: u32,
    pub range_version: u64,
    pub source_size: (u32, u32),
    pub frame_crop: Option<FrameCrop>,
    pub frame_crop_pending: Option<CropChange>,
    pub frame_crop_editing: bool,
    pub frame_crop_error: bool,
    pub position: Duration,
    pub duration: Duration,
    pub has_audio: bool,
    pub muted: bool,
    pub speed: Speed,
    pub speed_pending: Option<Speed>,
    pub speed_error: bool,
    pub export_audio: ExportAudio,
    pub export_audio_error: bool,
    audio_request: u64,
    audio_pending: Option<(u64, ExportAudio)>,
    pub error: Option<String>,
    pub presented: bool,
    pub visible: bool,
    resume: bool,
    seek_pending: bool,
    intent: Option<Intent>,
    cleanup_failed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropChange {
    Begin,
    Cancel,
    Apply(Option<FrameCrop>),
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
            range: Range::full(0),
            frames: 0,
            fps: 30,
            range_version: 0,
            source_size: (0, 0),
            frame_crop: None,
            frame_crop_pending: None,
            frame_crop_editing: false,
            frame_crop_error: false,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            has_audio: false,
            muted: false,
            speed: Speed::Normal,
            speed_pending: None,
            speed_error: false,
            export_audio: ExportAudio::Original,
            export_audio_error: false,
            audio_request: 0,
            audio_pending: None,
            error: None,
            presented: false,
            visible: false,
            resume: false,
            seek_pending: false,
            intent: None,
            cleanup_failed: false,
        }
    }
    pub fn can_set_speed(&self) -> bool {
        self.visible
            && self.intent.is_none()
            && self.speed_pending.is_none()
            && !self.export_audio_pending()
            && !self.frame_crop_editing
            && self.frame_crop_pending.is_none()
            && matches!(self.phase, Phase::Paused | Phase::Playing | Phase::Ended)
    }

    pub fn can_set_export_audio(&self) -> bool {
        self.has_audio && self.can_set_speed()
    }

    pub fn export_audio_pending(&self) -> bool {
        self.audio_pending.is_some()
    }

    pub fn configure(&mut self, frames: u64, fps: u32) {
        self.frames = frames;
        self.fps = fps;
        self.range = Range::full(frames);
        self.duration = Range::time(frames, fps);
    }
    fn range_time(&self, time: Duration) -> Duration {
        if self.frames == 0 {
            time.min(self.duration.saturating_sub(Duration::from_nanos(1)))
        } else {
            self.range.clamp_time(time, self.fps)
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
        if self.frame_crop_pending.is_some()
            || (self.speed_pending.is_some() || self.export_audio_pending())
                && !matches!(input, Input::Failed(_))
        {
            return None;
        }
        match input {
            Input::SetExportAudio(mode)
                if self.can_set_export_audio() && self.export_audio != mode =>
            {
                self.audio_request += 1;
                let request = self.audio_request;
                self.audio_pending = Some((request, mode));
                self.export_audio_error = false;
                Some(self.command(Action::SetExportAudio { request, mode }))
            }
            Input::SetSpeed(speed) if self.can_set_speed() && self.speed != speed => {
                if self.phase != Phase::Ended {
                    self.phase = Phase::Paused;
                }
                self.speed_pending = Some(speed);
                self.speed_error = false;
                self.next(Action::SetSpeed(speed))
            }
            Input::BeginFrameCrop
                if !self.frame_crop_editing
                    && self.visible
                    && matches!(self.phase, Phase::Paused | Phase::Playing | Phase::Ended) =>
            {
                self.phase = Phase::Paused;
                self.frame_crop_error = false;
                self.frame_crop_pending = Some(CropChange::Begin);
                self.next(Action::SetFrameCrop(None))
            }
            Input::CancelFrameCrop if self.frame_crop_editing => {
                self.frame_crop_pending = Some(CropChange::Cancel);
                self.next(Action::SetFrameCrop(self.frame_crop))
            }
            Input::CropGuide(crop)
                if self.frame_crop_editing
                    && crop.right <= self.source_size.0
                    && crop.bottom <= self.source_size.1 =>
            {
                Some(self.command(Action::CropGuide(Some(crop))))
            }
            Input::SetFrameCrop(crop)
                if self.frame_crop_editing
                    && crop.is_none_or(|c| c.valid(self.source_size.0, self.source_size.1)) =>
            {
                self.frame_crop_error = false;
                self.frame_crop_pending = Some(CropChange::Apply(crop));
                self.next(Action::SetFrameCrop(crop))
            }
            _ if self.frame_crop_editing && !matches!(input, Input::Close | Input::Failed(_)) => {
                None
            }
            Input::Target(target) if self.phase == Phase::Preparing => self.next(Action::Open {
                target,
                range: self.range,
                fps: self.fps,
                version: self.range_version,
                crop: self.frame_crop,
                speed: self.speed,
                muted: self.muted,
                audio: self.export_audio,
            }),
            Input::PlayPause
                if self.intent.is_none() && matches!(self.phase, Phase::Paused | Phase::Ended) =>
            {
                if self.phase == Phase::Ended {
                    self.resume = true;
                    self.seek_pending = true;
                    self.phase = Phase::Seeking;
                    self.next(Action::Seek(self.range_time(Duration::ZERO)))
                } else {
                    self.phase = Phase::Playing;
                    self.next(Action::Play)
                }
            }
            Input::PlayPause if self.phase == Phase::Playing => {
                self.phase = Phase::Paused;
                self.next(Action::Pause)
            }
            Input::BeginTrim
                if self.visible
                    && matches!(
                        self.phase,
                        Phase::Paused | Phase::Playing | Phase::Seeking | Phase::Ended
                    ) =>
            {
                self.resume = false;
                self.seek_pending = false;
                self.phase = Phase::Seeking;
                Some(self.command(Action::Pause))
            }
            Input::ResetRange | Input::SetRange { .. }
                if self.visible
                    && self.frames > 0
                    && matches!(
                        self.phase,
                        Phase::Paused | Phase::Playing | Phase::Seeking | Phase::Ended
                    ) =>
            {
                let (range, end_handle) = match input {
                    Input::SetRange { range, end_handle } => (range, end_handle),
                    _ => (Range::full(self.frames), false),
                };
                if !range.valid(self.frames) {
                    return None;
                }
                if self.range != range {
                    self.range_version += 1;
                    self.range = range;
                }
                self.resume = false;
                self.seek_pending = true;
                self.phase = Phase::Seeking;
                let position = Range::time(
                    if end_handle {
                        range.end - 1
                    } else {
                        range.start
                    },
                    self.fps,
                );
                self.next(Action::SetRange {
                    range,
                    fps: self.fps,
                    version: self.range_version,
                    position,
                })
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
                self.next(Action::Seek(self.range_time(position)))
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
                self.speed_error = false;
                self.export_audio_error = false;
                Some(self.command(Action::PrepareView))
            }
            Input::Failed(error) => {
                self.audio_pending = None;
                self.speed_pending = None;
                self.frame_crop_pending = None;
                self.frame_crop_editing = false;
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
            Notice::ExportAudioConfigured { request, success } => {
                if let Some((pending, mode)) = self.audio_pending
                    && pending == request
                {
                    self.audio_pending = None;
                    self.export_audio_error = !success;
                    if success {
                        self.export_audio = mode;
                        self.range_version += 1;
                    }
                }
                None
            }
            Notice::SpeedConfigured(success) => {
                if let Some(speed) = self.speed_pending.take() {
                    self.speed_error = !success;
                    if success {
                        self.speed = speed;
                        self.range_version += 1;
                    }
                }
                None
            }
            Notice::FrameCropPresented(success) => {
                if let Some(change) = self.frame_crop_pending.take() {
                    self.frame_crop_error = !success;
                    if success {
                        match change {
                            CropChange::Begin => self.frame_crop_editing = true,
                            CropChange::Cancel => self.frame_crop_editing = false,
                            CropChange::Apply(crop) => {
                                self.frame_crop_editing = false;
                                if crop != self.frame_crop {
                                    self.frame_crop = crop;
                                    self.range_version += 1;
                                }
                            }
                        }
                    }
                }
                None
            }
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
                if self.frames > 0
                    && (position
                        < Range::time(self.range.start, self.fps)
                            .saturating_sub(Duration::from_nanos(100))
                        || position >= Range::time(self.range.end, self.fps))
                {
                    return None;
                }
                if self.frames == 0 {
                    self.duration = duration;
                }
                self.has_audio = has_audio;
                if self.phase == Phase::Seeking && !self.seek_pending {
                    return None;
                }
                self.position = self.range_time(position);
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
            Notice::Ended if matches!(self.phase, Phase::Playing | Phase::Paused) => {
                self.phase = Phase::Ended;
                self.position = if self.frames == 0 {
                    self.duration
                } else {
                    Range::time(self.range.end - 1, self.fps)
                };
                None
            }
            Notice::Failed(error) if self.phase != Phase::Retiring => {
                self.audio_pending = None;
                self.speed_pending = None;
                self.frame_crop_pending = None;
                self.frame_crop_editing = false;
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
    fn export_audio_confirmation_is_independent_of_playback_and_mute() {
        let mut s = state();
        s.configure(120, 30);
        s.position = Range::time(12, 30);
        s.phase = Phase::Playing;
        s.visible = true;
        s.muted = true;
        let mode = ExportAudio::HalfVolume;
        assert_eq!(s.export_audio, ExportAudio::Original);
        assert!(s.input(s.identity, Input::SetExportAudio(mode)).is_none());
        s.has_audio = true;
        let identity = s.identity;
        let change = s.input(identity, Input::SetExportAudio(mode)).unwrap();
        let Action::SetExportAudio { request, .. } = change.action else {
            panic!("expected audio command");
        };
        assert_eq!(change.identity, identity);
        assert_eq!(s.phase, Phase::Playing);
        assert!(s.input(identity, Input::Save).is_none());
        assert!(s.input(identity, Input::BeginSeek).is_none());
        s.notice(
            identity,
            Notice::ExportAudioConfigured {
                request: request + 1,
                success: true,
            },
        );
        assert!(s.export_audio_pending());
        let stale = Identity {
            instance: identity.instance + 1,
            ..identity
        };
        s.notice(
            stale,
            Notice::ExportAudioConfigured {
                request,
                success: true,
            },
        );
        assert_eq!(s.export_audio, ExportAudio::Original);
        s.notice(
            identity,
            change.dispatch_failure("test dispatch error".into()),
        );
        assert!(s.export_audio_error);
        assert!(!s.export_audio_pending());
        assert_eq!(s.phase, Phase::Playing);
        s.input(identity, Input::SetExportAudio(mode)).unwrap();
        s.notice(
            identity,
            Notice::ExportAudioConfigured {
                request,
                success: true,
            },
        );
        assert!(s.export_audio_pending());
        s.notice(
            identity,
            Notice::ExportAudioConfigured {
                request: request + 1,
                success: true,
            },
        );
        assert_eq!(s.export_audio, mode);
        assert_eq!(s.range_version, 1);
        assert_eq!(s.position, Range::time(12, 30));
        assert_eq!(s.phase, Phase::Playing);
        assert!(s.muted);
        assert!(!s.export_audio_error);

        s.input(identity, Input::Failed("test player failure".into()));
        s.input(identity, Input::Retry).unwrap();
        let open = s
            .input(
                s.identity,
                Input::Target(Target {
                    window: WindowToken(10),
                    bounds: Rect::default(),
                }),
            )
            .unwrap();
        assert!(matches!(
            open.action,
            Action::Open {
                muted: true,
                audio: ExportAudio::HalfVolume,
                ..
            }
        ));
    }

    #[test]
    fn speed_waits_for_the_owned_confirmation_and_preserves_edit_state() {
        let mut s = state();
        s.visible = true;
        s.phase = Phase::Playing;
        s.configure(120, 30);
        s.range = Range { start: 8, end: 80 };
        s.position = Range::time(24, 30);
        s.muted = true;

        let old = s.identity;
        let change = s.input(old, Input::SetSpeed(Speed::Double)).unwrap();
        assert_eq!(s.phase, Phase::Paused);
        assert_eq!(s.speed, Speed::Normal);
        assert!(s.input(change.identity, Input::Save).is_none());
        s.notice(old, Notice::SpeedConfigured(true));
        assert_eq!(s.speed, Speed::Normal);
        s.notice(change.identity, Notice::SpeedConfigured(false));
        assert!(s.speed_error);
        let retry = s
            .input(s.identity, Input::SetSpeed(Speed::OneAndHalf))
            .unwrap();
        s.notice(retry.identity, Notice::SpeedConfigured(true));
        assert_eq!(s.speed, Speed::OneAndHalf);
        assert!(!s.speed_error);
        assert_eq!(s.range_version, 1);
        assert_eq!(s.range, Range { start: 8, end: 80 });
        assert_eq!(s.position, Range::time(24, 30));
        assert!(s.muted);
        assert!(s.input(s.identity, Input::SetSpeed(s.speed)).is_none());
    }

    #[test]
    fn frame_crop_only_commits_after_current_presentation_and_keeps_time_range() {
        let mut s = state();
        s.source_size = (320, 240);
        s.visible = true;
        s.phase = Phase::Paused;
        s.range = Range {
            start: 10,
            end: 300,
        };
        s.position = Range::time(30, 60);
        let position = s.position;
        let before = s.identity;
        let begin = s.input(before, Input::BeginFrameCrop).unwrap();
        assert_eq!(begin.action, Action::SetFrameCrop(None));
        assert!(s.notice(before, Notice::FrameCropPresented(true)).is_none());
        assert!(!s.frame_crop_editing);
        assert!(s.input(begin.identity, Input::Close).is_none());
        s.notice(begin.identity, Notice::FrameCropPresented(true));
        assert!(s.frame_crop_editing);
        assert!(s.input(s.identity, Input::Save).is_none());
        let crop = FrameCrop {
            left: 20,
            top: 16,
            right: 200,
            bottom: 180,
        };
        let rejected = FrameCrop { left: 21, ..crop };
        assert!(
            s.input(s.identity, Input::SetFrameCrop(Some(rejected)))
                .is_none()
        );
        let apply = s
            .input(s.identity, Input::SetFrameCrop(Some(crop)))
            .unwrap();
        s.notice(apply.identity, Notice::FrameCropPresented(false));
        assert!(s.frame_crop_editing && s.frame_crop_error);
        assert!(s.frame_crop.is_none());
        let apply = s
            .input(s.identity, Input::SetFrameCrop(Some(crop)))
            .unwrap();
        s.notice(apply.identity, Notice::FrameCropPresented(true));
        assert!(!s.frame_crop_editing);
        assert_eq!(s.frame_crop, Some(crop));
        assert_eq!(s.range_version, 1);
        assert_eq!(
            s.range,
            Range {
                start: 10,
                end: 300
            }
        );
        assert_eq!(s.position, position);
    }

    #[test]
    fn trimming_uses_exclusive_frame_bounds_and_rejects_old_results() {
        let mut s = state();
        s.configure(600, 60);
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        let old = s.identity;
        let range = Range {
            start: 17,
            end: 103,
        };
        let command = s
            .input(
                old,
                Input::SetRange {
                    range,
                    end_handle: true,
                },
            )
            .unwrap();
        assert!(
            matches!(command.action,Action::SetRange{position,..} if position==Range::time(102,60))
        );
        assert_eq!(s.range_version, 1);
        assert!(s.notice(old, frame()).is_none());
        s.notice(
            s.identity,
            Notice::Frame {
                position: Range::time(102, 60),
                duration: Duration::from_secs(11),
                has_audio: true,
            },
        );
        assert_eq!(s.duration, Duration::from_secs(10));
        assert_eq!(s.phase, Phase::Paused);
        let version = s.range_version;
        s.input(s.identity, Input::BeginTrim);
        s.input(s.identity, Input::CancelSeek);
        assert_eq!(s.range, range);
        assert_eq!(s.range_version, version);
        assert!(
            s.input(
                s.identity,
                Input::SetRange {
                    range: Range {
                        start: 103,
                        end: 103
                    },
                    end_handle: false
                }
            )
            .is_none()
        );
        s.input(s.identity, Input::ResetRange);
        assert_eq!(s.range, Range::full(600));
        assert_eq!(s.range_version, version + 1);
    }
    #[test]
    fn selected_playback_restarts_from_range_start_and_save_keeps_range() {
        let mut s = state();
        s.configure(300, 30);
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        let range = Range { start: 23, end: 24 };
        s.input(
            s.identity,
            Input::SetRange {
                range,
                end_handle: false,
            },
        );
        s.notice(
            s.identity,
            Notice::Frame {
                position: Range::time(23, 30),
                duration: Duration::from_secs(10),
                has_audio: true,
            },
        );
        s.input(s.identity, Input::PlayPause);
        s.notice(s.identity, Notice::Ended);
        assert_eq!(
            s.input(s.identity, Input::PlayPause).unwrap().action,
            Action::Seek(Range::time(23, 30))
        );
        s.notice(
            s.identity,
            Notice::Frame {
                position: Range::time(23, 30),
                duration: Duration::from_secs(10),
                has_audio: true,
            },
        );
        s.input(s.identity, Input::Save);
        s.save_finished();
        assert_eq!(s.range, range);
    }
    #[test]
    fn retry_restores_selected_range_before_the_new_window_handoff() {
        let mut s = state();
        s.configure(300, 60);
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        let range = Range {
            start: 17,
            end: 103,
        };
        s.input(
            s.identity,
            Input::SetRange {
                range,
                end_handle: false,
            },
        );
        s.notice(s.identity, Notice::Failed("failed".into()));
        s.input(s.identity, Input::Retry);
        let target = Target {
            window: WindowToken(3),
            bounds: Rect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 360,
            },
        };
        assert!(
            matches!(s.input(s.identity,Input::Target(target)).unwrap().action,Action::Open{range:r,..} if r==range)
        );
        assert!(s.notice(s.identity, frame()).is_none());
        assert!(!s.presented);
        assert_eq!(
            s.notice(
                s.identity,
                Notice::Frame {
                    position: Range::time(17, 60),
                    duration: Duration::from_secs(5),
                    has_audio: true
                }
            )
            .unwrap()
            .action,
            Action::Handoff
        );
    }
    #[test]
    fn press_and_commit_in_one_turn_keep_the_input_generation_valid() {
        let mut s = state();
        s.configure(300, 60);
        s.notice(s.identity, frame());
        s.notice(s.identity, Notice::Visible);
        let id = s.identity;
        assert_eq!(s.input(id, Input::BeginTrim).unwrap().identity, id);
        assert!(
            s.input(
                id,
                Input::SetRange {
                    range: Range {
                        start: 17,
                        end: 103
                    },
                    end_handle: false
                }
            )
            .is_some()
        );
        assert_eq!(
            s.range,
            Range {
                start: 17,
                end: 103
            }
        );
        assert!(s.identity.request > id.request);
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
