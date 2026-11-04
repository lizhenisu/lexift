//! Recording lifecycle and physical geometry, independent of native windows and codecs.
use crate::{Error, Result, domain::geometry::Rect};

/// Restricted recording rates keep timeline and codec negotiation in agreement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameRate {
    #[default]
    Fps30,
    Fps60,
}
impl FrameRate {
    pub const fn get(self) -> u32 {
        match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    Standard,
    #[default]
    High,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Video {
    pub frame_rate: FrameRate,
    pub quality: Quality,
}
impl Video {
    /// Derive the target without intermediate rounding or overflowing codec fields.
    pub fn bitrate(self, width: u32, height: u32) -> Result<u32> {
        let base = match self.quality {
            Quality::Standard => 8_000_000u64,
            Quality::High => 12_000_000,
        };
        base.checked_mul(u64::from(width))
            .and_then(|v| v.checked_mul(u64::from(height)))
            .and_then(|v| v.checked_mul(u64::from(self.frame_rate.get())))
            .map(|v| v / (1920 * 1080 * 30))
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| Error::new("Recording bitrate is unsupported"))
    }
}
pub const MAX_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;

/// Expand odd dimensions without moving either edge outside the owning monitor.
/// The screenshot region remains unchanged; callers present this actual video region.
pub fn recording_bounds(selection: Rect, monitor: Rect) -> Result<Rect> {
    if selection.left < monitor.left
        || selection.top < monitor.top
        || selection.right > monitor.right
        || selection.bottom > monitor.bottom
        || selection.left >= selection.right
        || selection.top >= selection.bottom
    {
        return Err(Error::new("Recording requires a region inside one monitor"));
    }
    let mut region = selection;
    if (i64::from(region.right) - i64::from(region.left)) % 2 != 0 {
        if region.right < monitor.right {
            region.right += 1;
        } else if region.left > monitor.left {
            region.left -= 1;
        } else {
            return Err(Error::new(
                "Recording width cannot be expanded inside the monitor",
            ));
        }
    }
    if (i64::from(region.bottom) - i64::from(region.top)) % 2 != 0 {
        if region.bottom < monitor.bottom {
            region.bottom += 1;
        } else if region.top > monitor.top {
            region.top -= 1;
        } else {
            return Err(Error::new(
                "Recording height cannot be expanded inside the monitor",
            ));
        }
    }
    capture_bytes(region)?;
    Ok(region)
}

/// Three BGRA source buffers and two NV12 buffers; native codec/GPU allocations
/// and the annotation renderer have separate ownership and must be reported separately.
pub fn capture_bytes(bounds: Rect) -> Result<u64> {
    let width = i64::from(bounds.right) - i64::from(bounds.left);
    let height = i64::from(bounds.bottom) - i64::from(bounds.top);
    if width <= 0 || height <= 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::new("Recording dimensions must be positive and even"));
    }
    (width as u64)
        .checked_mul(height as u64)
        .and_then(|n| n.checked_mul(15))
        .filter(|n| *n <= MAX_CAPTURE_BYTES)
        .ok_or_else(|| Error::new("Recording capture budget exceeded"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub session: u64,
    pub operation: u64,
    pub request: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Artifact(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    PreparingView,
    PreparingBackend,
    Ready,
    PreparingCountdown,
    CountingDown,
    RetiringCountdown,
    RestoringReady,
    PreparingAudio,
    PreparingVideo,
    Starting,
    Recording,
    Pausing,
    Paused,
    Resuming,
    PresentingFinalContent,
    Finishing,
    PendingSave,
    Saving,
    Retiring,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    AudioUnavailable,
    LayoutChanged,
    CaptureClosed,
    CaptureUnavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishReason {
    Completed,
    Interrupted(Interruption),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    DiscardCompleted,
    StartAfter(u8),
    CountdownPrepared(std::time::Instant),
    CountdownElapsed(std::time::Instant),
    CountdownCancelled,
    CountdownRetired,
    CountdownFailed,
    ReadyRestored,
    StartRejected,
    ConfigureVideo(Video),
    ConfigureAudio(super::recording_audio::Mode),
    VideoConfigured,
    VideoConfigurationFailed,
    AudioConfigured,
    AudioConfigurationFailed,
    ViewReady,
    BackendReady,
    Start,
    Started,
    Pause,
    Paused,
    Resume,
    Resumed,
    Finish,
    FinalContentPresented,
    Interrupted(Interruption),
    Finalized(Artifact),
    Save,
    Saved,
    SaveCancelled,
    SaveFailed,
    Cancel,
    Fatal,
    Retired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PrepareCountdown(u8),
    WaitCountdown(std::time::Instant),
    RetireCountdown,
    RestoreReady,
    ConfigureVideo(Video),
    ConfigureAudio(super::recording_audio::Mode),
    PrepareBackend,
    Start,
    Pause,
    Resume,
    PresentFinalContent,
    Finish(FinishReason),
    Save(Artifact),
    Release,
    RestoreScreenshot,
    EndScreenshot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub identity: Identity,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub countdown_deadline: Option<std::time::Instant>,
    pub video: Video,
    pub pending_video: Option<Video>,
    pub audio_devices: super::recording_audio::Devices,
    pub audio_ready: bool,
    pub audio: super::recording_audio::Mode,
    pub pending_audio: Option<super::recording_audio::Mode>,
    pub identity: Identity,
    pub source: Rect,
    pub phase: Phase,
    pub artifact: Option<Artifact>,
    pub finish_reason: FinishReason,
    pub restore_editor: bool,
    saved: bool,
}
impl State {
    pub fn new(session: u64, operation: u64, source: Rect, monitor: Rect) -> Result<Self> {
        Ok(Self {
            countdown_deadline: None,
            video: Video::default(),
            pending_video: None,
            audio_devices: Default::default(),
            audio_ready: true,
            audio: Default::default(),
            pending_audio: None,
            identity: Identity {
                session,
                operation,
                request: 0,
            },
            source: recording_bounds(source, monitor)?,
            phase: Phase::PreparingView,
            artifact: None,
            finish_reason: FinishReason::Completed,
            restore_editor: true,
            saved: false,
        })
    }
    pub fn can_annotate(&self) -> bool {
        matches!(self.phase, Phase::Ready | Phase::Recording | Phase::Paused)
    }
    pub fn active(&self) -> bool {
        self.phase != Phase::Closed
    }

    /// Display changes invalidate the return view, not a sealed video or an
    /// in-flight save identity. Live tasks stop at their last accepted frame.
    pub fn source_layout_changed(&mut self, session: u64, operation: u64) -> Option<Command> {
        if session != self.identity.session
            || operation != self.identity.operation
            || !self.active()
        {
            return None;
        }
        self.restore_editor = false;
        if matches!(
            self.phase,
            Phase::PendingSave | Phase::Saving | Phase::Finishing | Phase::Retiring
        ) {
            return None;
        }
        self.apply(
            self.identity,
            Event::Interrupted(Interruption::LayoutChanged),
        )
    }

    /// Events acknowledge exactly the current request. Every accepted transition
    /// advances its identity, so duplicate/late completions cannot perform an action twice.
    pub fn apply(&mut self, identity: Identity, event: Event) -> Option<Command> {
        if identity != self.identity || !self.active() {
            return None;
        }
        let (phase, action) = match (self.phase, event) {
            (Phase::Ready, Event::StartAfter(seconds))
                if matches!(seconds, 3 | 5 | 10) && self.audio_ready =>
            {
                (
                    Phase::PreparingCountdown,
                    Some(Action::PrepareCountdown(seconds)),
                )
            }
            (Phase::PreparingCountdown, Event::CountdownPrepared(deadline)) => {
                self.countdown_deadline = Some(deadline);
                (Phase::CountingDown, Some(Action::WaitCountdown(deadline)))
            }
            (Phase::CountingDown, Event::CountdownElapsed(now))
                if self.countdown_deadline.is_some_and(|d| now >= d) =>
            {
                (Phase::RetiringCountdown, Some(Action::RetireCountdown))
            }
            (Phase::RetiringCountdown, Event::CountdownRetired) => {
                self.countdown_deadline = None;
                (Phase::Starting, Some(Action::Start))
            }
            (
                Phase::PreparingCountdown | Phase::CountingDown | Phase::RetiringCountdown,
                Event::CountdownCancelled | Event::CountdownFailed,
            ) => (Phase::RestoringReady, Some(Action::RestoreReady)),
            (Phase::RestoringReady, Event::ReadyRestored) => {
                self.countdown_deadline = None;
                (Phase::Ready, None)
            }
            (Phase::Ready, Event::ConfigureVideo(video)) if video != self.video => {
                self.pending_video = Some(video);
                (Phase::PreparingVideo, Some(Action::ConfigureVideo(video)))
            }
            (Phase::PreparingVideo, Event::VideoConfigured) => {
                self.video = self.pending_video.take()?;
                (Phase::Ready, None)
            }
            (Phase::PreparingVideo, Event::VideoConfigurationFailed) => {
                self.pending_video = None;
                (Phase::Ready, None)
            }
            (Phase::Ready, Event::ConfigureAudio(mode))
                if mode != self.audio || !self.audio_ready =>
            {
                self.pending_audio = Some(mode);
                (Phase::PreparingAudio, Some(Action::ConfigureAudio(mode)))
            }
            (Phase::PreparingAudio, Event::AudioConfigured) => {
                self.audio = self.pending_audio.take()?;
                self.audio_ready = true;
                (Phase::Ready, None)
            }
            (Phase::PreparingAudio, Event::AudioConfigurationFailed) => {
                self.pending_audio = None;
                (Phase::Ready, None)
            }
            (Phase::PreparingView, Event::ViewReady) => {
                (Phase::PreparingBackend, Some(Action::PrepareBackend))
            }
            (Phase::PreparingBackend, Event::BackendReady) => (Phase::Ready, None),
            (Phase::Ready, Event::Start) if self.audio_ready => {
                (Phase::Starting, Some(Action::Start))
            }
            (Phase::Starting, Event::StartRejected) => {
                self.audio_ready = false;
                (Phase::Ready, None)
            }
            (Phase::Starting, Event::Started) => (Phase::Recording, None),
            (Phase::Recording, Event::Pause) => (Phase::Pausing, Some(Action::Pause)),
            (Phase::Pausing, Event::Paused) => (Phase::Paused, None),
            (Phase::Paused, Event::Resume) => (Phase::Resuming, Some(Action::Resume)),
            (Phase::Resuming, Event::Resumed) => (Phase::Recording, None),
            (Phase::Recording | Phase::Paused, Event::Finish) => (
                Phase::PresentingFinalContent,
                Some(Action::PresentFinalContent),
            ),
            (Phase::PresentingFinalContent, Event::FinalContentPresented) => {
                (Phase::Finishing, Some(Action::Finish(self.finish_reason)))
            }
            (Phase::Finishing, Event::Interrupted(reason)) => {
                // The stop is already outstanding. Preserve its completion ID
                // while marking a capture-side interruption during the final fence.
                self.finish_reason = FinishReason::Interrupted(reason);
                if reason == Interruption::LayoutChanged {
                    self.restore_editor = false;
                }
                return None;
            }
            (
                Phase::PreparingView
                | Phase::PreparingBackend
                | Phase::Ready
                | Phase::PreparingAudio
                | Phase::PreparingVideo
                | Phase::PreparingCountdown
                | Phase::CountingDown
                | Phase::RetiringCountdown
                | Phase::RestoringReady,
                Event::Interrupted(reason),
            ) => {
                self.finish_reason = FinishReason::Interrupted(reason);
                if reason == Interruption::LayoutChanged {
                    self.restore_editor = false;
                }
                (Phase::Retiring, Some(Action::Release))
            }
            (
                Phase::Starting
                | Phase::Recording
                | Phase::Pausing
                | Phase::Paused
                | Phase::Resuming
                | Phase::PresentingFinalContent,
                Event::Interrupted(reason),
            ) => {
                self.finish_reason = FinishReason::Interrupted(reason);
                if reason == Interruption::LayoutChanged {
                    self.restore_editor = false;
                }
                (Phase::Finishing, Some(Action::Finish(self.finish_reason)))
            }
            (Phase::Finishing, Event::Finalized(artifact)) => {
                self.artifact = Some(artifact);
                (Phase::PendingSave, None)
            }
            (Phase::PendingSave, Event::Save) => {
                (Phase::Saving, Some(Action::Save(self.artifact?)))
            }
            (Phase::Saving, Event::SaveCancelled | Event::SaveFailed) => (Phase::PendingSave, None),
            (Phase::Saving, Event::Saved) => {
                self.saved = true;
                (Phase::Retiring, Some(Action::Release))
            }
            (Phase::PendingSave, Event::DiscardCompleted) => {
                self.restore_editor = false;
                (Phase::Retiring, Some(Action::Release))
            }
            (Phase::Retiring, Event::Retired) => {
                self.artifact = None;
                (
                    Phase::Closed,
                    Some(if self.saved || !self.restore_editor {
                        Action::EndScreenshot
                    } else {
                        Action::RestoreScreenshot
                    }),
                )
            }
            (Phase::Retiring, _) => return None,
            (_, Event::Cancel | Event::Fatal) => (Phase::Retiring, Some(Action::Release)),
            _ => return None,
        };
        self.phase = phase;
        self.identity.request += 1;
        action.map(|action| Command {
            identity: self.identity,
            action,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_countdown_preserves_configuration_and_layout_loss_retires_the_task() {
        let mut s = State::new(1, 2, rect(0, 0, 1920, 1080), rect(0, 0, 1920, 1080)).unwrap();
        send(&mut s, Event::ViewReady);
        send(&mut s, Event::BackendReady);
        let video = Video {
            frame_rate: FrameRate::Fps60,
            quality: Quality::Standard,
        };
        send(&mut s, Event::ConfigureVideo(video));
        send(&mut s, Event::VideoConfigured);
        send(&mut s, Event::StartAfter(5));
        let old = s.identity;
        assert_eq!(
            send(&mut s, Event::CountdownFailed),
            Some(Action::RestoreReady)
        );
        send(&mut s, Event::ReadyRestored);
        assert_eq!(s.video, video);
        assert!(
            s.apply(old, Event::CountdownPrepared(std::time::Instant::now()))
                .is_none()
        );
        send(&mut s, Event::StartAfter(3));
        assert_eq!(
            send(&mut s, Event::Interrupted(Interruption::LayoutChanged)),
            Some(Action::Release)
        );
        assert!(!s.restore_editor);
    }
    #[test]
    fn countdown_is_monotonic_cancellable_and_requires_retirement_before_capture() {
        let mut s = State::new(1, 2, rect(0, 0, 1920, 1080), rect(0, 0, 1920, 1080)).unwrap();
        send(&mut s, Event::ViewReady);
        send(&mut s, Event::BackendReady);
        let old = s.identity;
        let now = std::time::Instant::now();
        let deadline = now + std::time::Duration::from_secs(3);
        assert_eq!(
            send(&mut s, Event::StartAfter(3)),
            Some(Action::PrepareCountdown(3))
        );
        assert!(s.apply(old, Event::CountdownPrepared(deadline)).is_none());
        assert!(send(&mut s, Event::StartAfter(5)).is_none());
        assert_eq!(
            send(&mut s, Event::CountdownPrepared(deadline)),
            Some(Action::WaitCountdown(deadline))
        );
        assert!(send(&mut s, Event::CountdownElapsed(now)).is_none());
        assert_eq!(
            send(&mut s, Event::CountdownElapsed(deadline)),
            Some(Action::RetireCountdown)
        );
        let expired = s.identity;
        assert_eq!(
            send(&mut s, Event::CountdownCancelled),
            Some(Action::RestoreReady)
        );
        assert!(s.apply(expired, Event::CountdownRetired).is_none());
        send(&mut s, Event::ReadyRestored);
        assert_eq!(s.phase, Phase::Ready);
        assert!(send(&mut s, Event::StartAfter(2)).is_none());
        send(&mut s, Event::StartAfter(10));
        send(&mut s, Event::CountdownPrepared(deadline));
        send(&mut s, Event::CountdownElapsed(deadline));
        assert_eq!(send(&mut s, Event::CountdownRetired), Some(Action::Start));
        assert!(send(&mut s, Event::CountdownCancelled).is_none());
    }
    #[test]
    fn video_bitrate_and_configuration_are_atomic_and_mutually_exclusive() {
        let mut s = State::new(1, 2, rect(0, 0, 1920, 1080), rect(0, 0, 1920, 1080)).unwrap();
        send(&mut s, Event::ViewReady);
        send(&mut s, Event::BackendReady);
        assert_eq!(s.video, Video::default());
        let initial = s.video;
        assert!(send(&mut s, Event::ConfigureVideo(initial)).is_none());
        for (rate, quality, expected) in [
            (FrameRate::Fps30, Quality::Standard, 8_000_000),
            (FrameRate::Fps30, Quality::High, 12_000_000),
            (FrameRate::Fps60, Quality::Standard, 16_000_000),
            (FrameRate::Fps60, Quality::High, 24_000_000),
        ] {
            let video = Video {
                frame_rate: rate,
                quality,
            };
            assert_eq!(video.bitrate(1920, 1080).unwrap(), expected);
            assert_eq!(video.bitrate(960, 540).unwrap(), expected / 4);
        }
        assert!(Video::default().bitrate(u32::MAX, u32::MAX).is_err());
        let video = Video {
            frame_rate: FrameRate::Fps60,
            quality: Quality::Standard,
        };
        let old = s.identity;
        assert_eq!(
            send(&mut s, Event::ConfigureVideo(video)),
            Some(Action::ConfigureVideo(video))
        );
        assert!(s.apply(old, Event::VideoConfigured).is_none());
        assert!(send(&mut s, Event::Start).is_none());
        assert!(
            send(
                &mut s,
                Event::ConfigureAudio(super::super::recording_audio::Mode::System)
            )
            .is_none()
        );
        send(&mut s, Event::VideoConfigurationFailed);
        assert_eq!(s.video, Video::default());
        send(&mut s, Event::ConfigureVideo(video));
        send(&mut s, Event::VideoConfigured);
        assert_eq!(s.video, video);
        send(
            &mut s,
            Event::ConfigureAudio(super::super::recording_audio::Mode::System),
        );
        assert!(send(&mut s, Event::ConfigureVideo(Video::default())).is_none());
        send(&mut s, Event::AudioConfigured);
        send(&mut s, Event::Start);
        assert!(send(&mut s, Event::ConfigureVideo(Video::default())).is_none());
    }
    #[test]
    fn rejected_start_preserves_preparation_and_requires_explicit_reprepare() {
        let b = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let mut state = State::new(1, 2, b, b).unwrap();
        send(&mut state, Event::ViewReady);
        send(&mut state, Event::BackendReady);
        send(&mut state, Event::Start);
        let old = state.identity;
        send(&mut state, Event::StartRejected);
        assert_eq!(state.phase, Phase::Ready);
        assert!(!state.audio_ready);
        assert!(send(&mut state, Event::Start).is_none());
        assert!(state.apply(old, Event::Started).is_none());
        assert!(
            send(
                &mut state,
                Event::ConfigureAudio(super::super::recording_audio::Mode::Silent)
            )
            .is_some()
        );
        send(&mut state, Event::AudioConfigured);
        assert!(state.audio_ready);
        assert!(send(&mut state, Event::Start).is_some());
    }
    fn rect(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }
    fn state() -> State {
        State::new(1, 2, rect(-1920, 0, 0, 1080), rect(-1920, 0, 0, 1080)).unwrap()
    }
    fn send(s: &mut State, e: Event) -> Option<Action> {
        s.apply(s.identity, e).map(|c| c.action)
    }
    fn recording() -> State {
        let mut s = state();
        send(&mut s, Event::ViewReady);
        send(&mut s, Event::BackendReady);
        send(&mut s, Event::Start);
        send(&mut s, Event::Started);
        s
    }
    #[test]
    fn outward_expansion_preserves_selection_and_monitor_edges() {
        let monitor = rect(-1920, -1080, 0, 0);
        let selection = rect(-101, -101, 0, 0);
        assert_eq!(
            recording_bounds(selection, monitor).unwrap(),
            rect(-102, -102, 0, 0)
        );
        assert_eq!(
            recording_bounds(rect(-1920, -1080, -1919, -1079), monitor).unwrap(),
            rect(-1920, -1080, -1918, -1078)
        );
        assert!(recording_bounds(rect(0, 0, 101, 101), rect(0, 0, 101, 101)).is_err());
        assert!(recording_bounds(rect(-1, 0, 100, 100), rect(0, 0, 1920, 1080)).is_err());
    }
    #[test]
    fn audio_configuration_is_serial_and_failed_changes_keep_the_confirmed_mode() {
        use super::super::recording_audio::Mode;
        let mut s = state();
        send(&mut s, Event::ViewReady);
        send(&mut s, Event::BackendReady);
        assert_eq!(s.audio, Mode::Silent);
        assert_eq!(send(&mut s, Event::ConfigureAudio(Mode::Silent)), None);
        let before = s.identity;
        assert_eq!(
            send(&mut s, Event::ConfigureAudio(Mode::Mixed)),
            Some(Action::ConfigureAudio(Mode::Mixed))
        );
        assert!(!s.can_annotate());
        assert_eq!(send(&mut s, Event::Start), None);
        assert_eq!(send(&mut s, Event::ConfigureAudio(Mode::System)), None);
        assert!(s.apply(before, Event::AudioConfigured).is_none());
        send(&mut s, Event::AudioConfigurationFailed);
        assert_eq!(s.audio, Mode::Silent);
        assert_eq!(s.phase, Phase::Ready);
        send(&mut s, Event::ConfigureAudio(Mode::System));
        send(&mut s, Event::AudioConfigured);
        assert_eq!(s.audio, Mode::System);
        send(&mut s, Event::Start);
        send(&mut s, Event::Started);
        assert!(send(&mut s, Event::ConfigureAudio(Mode::Silent)).is_none());
        send(&mut s, Event::Pause);
        send(&mut s, Event::Paused);
        assert!(send(&mut s, Event::ConfigureAudio(Mode::Silent)).is_none());
    }
    #[test]
    fn capture_budget_checks_before_allocation() {
        assert_eq!(capture_bytes(rect(0, 0, 1920, 1080)).unwrap(), 31_104_000);
        assert!(capture_bytes(rect(i32::MIN, i32::MIN, i32::MAX - 1, i32::MAX - 1)).is_err());
        assert!(capture_bytes(rect(0, 0, 3840, 2160)).is_err());
    }
    #[test]
    fn duplicate_completion_and_old_session_cannot_start_or_close() {
        let mut s = state();
        let old = s.identity;
        assert_eq!(send(&mut s, Event::ViewReady), Some(Action::PrepareBackend));
        assert!(s.apply(old, Event::BackendReady).is_none());
        assert_eq!(s.phase, Phase::PreparingBackend);
        send(&mut s, Event::Cancel);
        let retirement = s.identity;
        assert!(s.apply(old, Event::Retired).is_none());
        assert_eq!(s.phase, Phase::Retiring);
        assert_eq!(
            s.apply(retirement, Event::Retired).unwrap().action,
            Action::RestoreScreenshot
        );
    }
    #[test]
    fn finish_waits_for_final_content_and_save_cancel_keeps_artifact() {
        let mut s = recording();
        assert!(s.can_annotate());
        assert_eq!(
            send(&mut s, Event::Finish),
            Some(Action::PresentFinalContent)
        );
        assert!(!s.can_annotate());
        assert!(send(&mut s, Event::Finalized(Artifact(9))).is_none());
        assert_eq!(
            send(&mut s, Event::FinalContentPresented),
            Some(Action::Finish(FinishReason::Completed))
        );
        send(&mut s, Event::Finalized(Artifact(9)));
        send(&mut s, Event::Save);
        send(&mut s, Event::SaveCancelled);
        assert_eq!(s.artifact, Some(Artifact(9)));
        send(&mut s, Event::Save);
        send(&mut s, Event::SaveFailed);
        assert_eq!(s.phase, Phase::PendingSave);
        send(&mut s, Event::Save);
        send(&mut s, Event::Saved);
        assert_eq!(send(&mut s, Event::Retired), Some(Action::EndScreenshot));
        assert!(!s.active());
        assert!(s.artifact.is_none());
    }
    #[test]
    fn pause_allows_annotations_but_resume_requires_acknowledgement() {
        let mut s = recording();
        send(&mut s, Event::Pause);
        assert!(!s.can_annotate());
        send(&mut s, Event::Paused);
        assert!(s.can_annotate());
        send(&mut s, Event::Resume);
        assert!(!s.can_annotate());
        send(&mut s, Event::Resumed);
        assert!(s.can_annotate());
    }
    #[test]
    fn interrupted_clip_can_be_saved_without_restoring_invalid_layout() {
        let mut s = recording();
        assert_eq!(
            send(&mut s, Event::Interrupted(Interruption::LayoutChanged)),
            Some(Action::Finish(FinishReason::Interrupted(
                Interruption::LayoutChanged
            )))
        );
        send(&mut s, Event::Finalized(Artifact(3)));
        send(&mut s, Event::Cancel);
        assert_eq!(send(&mut s, Event::Retired), Some(Action::EndScreenshot));
    }
    #[test]
    fn layout_change_while_saving_keeps_the_save_identity() {
        let mut s = recording();
        send(&mut s, Event::Finish);
        send(&mut s, Event::FinalContentPresented);
        send(&mut s, Event::Finalized(Artifact(8)));
        send(&mut s, Event::Save);
        let saving = s.identity;
        assert!(
            s.source_layout_changed(saving.session, saving.operation)
                .is_none()
        );
        assert_eq!(s.identity, saving);
        assert_eq!(s.artifact, Some(Artifact(8)));
        send(&mut s, Event::SaveCancelled);
        send(&mut s, Event::Cancel);
        assert_eq!(send(&mut s, Event::Retired), Some(Action::EndScreenshot));
    }
    #[test]
    fn layout_change_before_start_does_not_fabricate_a_clip() {
        let mut s = state();
        let id = s.identity;
        assert_eq!(
            s.source_layout_changed(id.session, id.operation)
                .unwrap()
                .action,
            Action::Release
        );
        assert_eq!(s.phase, Phase::Retiring);
        assert!(s.artifact.is_none());
        assert_eq!(send(&mut s, Event::Retired), Some(Action::EndScreenshot));
    }
    #[test]
    fn interruption_while_finishing_keeps_the_seal_request() {
        let mut s = recording();
        send(&mut s, Event::Finish);
        send(&mut s, Event::FinalContentPresented);
        let finishing = s.identity;
        send(&mut s, Event::Interrupted(Interruption::CaptureClosed));
        assert_eq!(s.identity, finishing);
        assert_eq!(
            s.finish_reason,
            FinishReason::Interrupted(Interruption::CaptureClosed)
        );
        send(&mut s, Event::Finalized(Artifact(19)));
        assert_eq!(s.phase, Phase::PendingSave);
    }
}
