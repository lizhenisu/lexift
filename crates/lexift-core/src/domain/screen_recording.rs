//! Recording lifecycle and physical geometry, independent of native windows and codecs.
use crate::{Error, Result, domain::geometry::Rect};

pub const FPS: u32 = 30;
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
            (Phase::PreparingView, Event::ViewReady) => {
                (Phase::PreparingBackend, Some(Action::PrepareBackend))
            }
            (Phase::PreparingBackend, Event::BackendReady) => (Phase::Ready, None),
            (Phase::Ready, Event::Start) => (Phase::Starting, Some(Action::Start)),
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
                Phase::PreparingView | Phase::PreparingBackend | Phase::Ready,
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
