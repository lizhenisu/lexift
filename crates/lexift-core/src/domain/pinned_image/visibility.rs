//! User-owned window visibility, independent of annotation hiding and capture suspension.
use super::{Command as PinCommand, Event as PinEvent};
use crate::{AppCommand, AppEvent, AppState, Result, domain::geometry::Point};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub batch: u64,
    pub id: u64,
    pub hidden: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub id: u64,
    pub hidden: bool,
    pub anchor: Point,
    pub pending: BTreeSet<u64>,
    pub failed: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notice {
    pub batch: u64,
    pub anchor: Point,
    pub failed: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub pending: Option<Batch>,
    pub notice: Option<Notice>,
    pub serial: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Requested {
        hidden: bool,
        anchor: Point,
    },
    Applied {
        request: Request,
        result: Result<()>,
    },
}

/// Domain conflicts are checked again when a tray intent reaches Core.
pub fn idle(s: &AppState) -> bool {
    s.pin_visibility.pending.is_none()
        && !s.screenshot.active()
        && s.image_import.request.is_none()
        && s.editor_ocr.request.is_none()
        && s.annotation_export.request.is_none()
        && s.image_history.window.as_ref().is_none_or(|w| !w.busy)
        && s.image_history.editor.as_ref().is_none_or(|e| !e.busy)
        && !matches!(
            s.ocr.phase,
            crate::domain::ocr::OcrPhase::Checking
                | crate::domain::ocr::OcrPhase::Capturing
                | crate::domain::ocr::OcrPhase::Recognizing
                | crate::domain::ocr::OcrPhase::Translating
        )
        && s.pinned_images.values().all(|p| {
            !p.busy
                && p.recognition.is_none()
                && p.editing.is_none()
                && p.proposed.is_none()
                && p.interaction.pending.is_none()
        })
}
pub fn available(s: &AppState, hidden: bool) -> bool {
    idle(s) && s.pinned_images.values().any(|p| p.user_hidden != hidden)
}

/// A visibility batch cannot race the start of another image operation.
pub(crate) fn blocks_event(s: &AppState, e: &AppEvent) -> bool {
    if s.pin_visibility.pending.is_none() {
        return false;
    }
    matches!(
        e,
        AppEvent::Screenshot(
            crate::domain::screenshot::ScreenshotEvent::Requested(_)
                | crate::domain::screenshot::ScreenshotEvent::Delayed { .. }
        ) | AppEvent::Ocr(crate::domain::ocr::OcrEvent::Requested { .. })
            | AppEvent::EditorOcr(crate::domain::editor_ocr::Event::Requested { .. })
            | AppEvent::ImageImport(crate::domain::image_import::Event::Requested { .. })
            | AppEvent::ImageHistory(
                crate::domain::image_history::workflow::Event::Action { .. }
                    | crate::domain::image_history::workflow::Event::EditorOutput { .. }
            )
            | AppEvent::AnnotationExport(crate::domain::annotation_export::ExportEvent::Requested(
                _
            ))
            | AppEvent::PinnedImage(
                PinEvent::EditRequested { .. }
                    | PinEvent::TransformRequested { .. }
                    | PinEvent::Requested { .. }
                    | PinEvent::Recognize { .. }
                    | PinEvent::InteractionRequested { .. }
                    | PinEvent::RestoreMouseInput
            )
    )
}
fn settle(s: &mut AppState) {
    if s.pin_visibility
        .pending
        .as_ref()
        .is_some_and(|b| b.pending.is_empty())
    {
        let b = s.pin_visibility.pending.take().unwrap();
        s.pin_visibility.notice = (b.failed > 0).then_some(Notice {
            batch: b.id,
            anchor: b.anchor,
            failed: b.failed,
        });
    }
}
pub(super) fn closed(s: &mut AppState, id: u64) {
    if let Some(b) = &mut s.pin_visibility.pending {
        b.pending.remove(&id);
    }
    settle(s);
}
pub(super) fn reduce(s: &mut AppState, e: Event) -> Vec<AppCommand> {
    match e {
        Event::Requested { hidden, anchor } => {
            if !available(s, hidden) {
                return vec![];
            }
            s.pin_visibility.serial = s
                .pin_visibility
                .serial
                .checked_add(1)
                .expect("pin visibility exhausted");
            let batch = s.pin_visibility.serial;
            let targets: BTreeSet<_> = s
                .pinned_images
                .iter()
                .filter_map(|(&id, p)| (p.user_hidden != hidden).then_some(id))
                .collect();
            s.pin_visibility.notice = None;
            let commands = targets
                .iter()
                .map(|&id| {
                    AppCommand::PinnedImage(PinCommand::ApplyVisibility(Request {
                        batch,
                        id,
                        hidden,
                    }))
                })
                .collect();
            s.pin_visibility.pending = Some(Batch {
                id: batch,
                hidden,
                anchor,
                pending: targets,
                failed: 0,
            });
            commands
        }
        Event::Applied { request, result } => {
            let Some(b) = s.pin_visibility.pending.as_mut() else {
                return vec![];
            };
            if b.id != request.batch || b.hidden != request.hidden || !b.pending.remove(&request.id)
            {
                return vec![];
            }
            if let Some(pin) = s.pinned_images.get_mut(&request.id) {
                if result.is_ok() {
                    pin.user_hidden = request.hidden;
                } else {
                    b.failed += 1;
                }
            }
            settle(s);
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        annotation::Session,
        annotation_export::{ExportRequest, ExportSnapshot},
        pinned_image::{
            State as Pin,
            interaction::{Opacity, Pending, Policy},
        },
        screenshot::{ScreenshotEvent, ScreenshotPhase},
    };
    use std::sync::Arc;
    fn document() -> ExportRequest {
        ExportRequest {
            session: 1,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: Default::default(),
                scale: 1.,
                monitors: vec![],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        }
    }
    fn state() -> AppState {
        let mut s = AppState::default();
        for id in 1..=3 {
            s.pinned_images.insert(id, Pin::default());
        }
        s.pinned_images.get_mut(&1).unwrap().interaction.applied = Policy {
            locked: true,
            passthrough: true,
            opacity: Opacity::Twenty,
        };
        s.pinned_images.get_mut(&1).unwrap().document = Some(document());
        s.pinned_images.get_mut(&2).unwrap().hidden = true;
        s
    }
    fn start(s: &mut AppState, hidden: bool) -> Vec<Request> {
        s.reduce(AppEvent::PinnedImage(PinEvent::WindowVisibility(
            Event::Requested {
                hidden,
                anchor: Point { x: -300, y: 70 },
            },
        )))
        .into_iter()
        .map(|c| match c {
            AppCommand::PinnedImage(PinCommand::ApplyVisibility(q)) => q,
            _ => panic!("unexpected command"),
        })
        .collect()
    }
    fn done(s: &mut AppState, request: Request, success: bool) {
        s.reduce(AppEvent::PinnedImage(PinEvent::WindowVisibility(
            Event::Applied {
                request,
                result: if success {
                    Ok(())
                } else {
                    Err(crate::Error::new("fixture"))
                },
            },
        )));
    }
    #[test]
    fn partial_failure_new_pin_close_and_retry_keep_contents_and_policies() {
        let mut s = state();
        let doc = s.pinned_images[&1].document.clone();
        let policy = s.pinned_images[&1].interaction.clone();
        let history = s.image_history.clone();
        let last_region = s.screenshot.last_region.clone();
        assert!(start(&mut s, false).is_empty());
        let q = start(&mut s, true);
        assert_eq!(q.len(), 3);
        assert!(start(&mut s, true).is_empty());
        s.pinned_images.insert(4, Pin::default());
        done(&mut s, q[0], true);
        assert!(s.pin_visibility.notice.is_none());
        s.reduce(AppEvent::PinnedImage(PinEvent::Closed { id: 2 }));
        done(&mut s, q[1], true);
        done(&mut s, q[2], false);
        assert!(s.pin_visibility.pending.is_none());
        assert_eq!(s.pin_visibility.notice.unwrap().failed, 1);
        assert!(s.pinned_images[&1].user_hidden);
        assert!(!s.pinned_images[&3].user_hidden && !s.pinned_images[&4].user_hidden);
        let retry = start(&mut s, true);
        assert_eq!(retry.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3, 4]);
        assert!(s.pin_visibility.notice.is_none());
        done(&mut s, q[0], false);
        for r in retry {
            done(&mut s, r, true);
        }
        for r in start(&mut s, false) {
            done(&mut s, r, true);
        }
        assert!(s.pinned_images.values().all(|p| !p.user_hidden));
        assert_eq!(s.pinned_images[&1].document, doc);
        assert_eq!(s.pinned_images[&1].interaction, policy);
        assert_eq!(s.image_history, history);
        assert_eq!(s.screenshot.last_region, last_region);
    }
    #[test]
    fn all_business_conflicts_disable_both_intents_without_cancelling_work() {
        let changes: &[fn(&mut AppState)] = &[
            |s| s.screenshot.phase = ScreenshotPhase::CountingDown,
            |s| s.screenshot.phase = ScreenshotPhase::Selecting,
            |s| s.screenshot.phase = ScreenshotPhase::Editing,
            |s| s.screenshot.phase = ScreenshotPhase::Output,
            |s| s.ocr.phase = crate::domain::ocr::OcrPhase::Checking,
            |s| s.ocr.phase = crate::domain::ocr::OcrPhase::Recognizing,
            |s| s.ocr.phase = crate::domain::ocr::OcrPhase::Translating,
            |s| s.pinned_images.get_mut(&1).unwrap().busy = true,
            |s| s.pinned_images.get_mut(&1).unwrap().editing = Some(4),
            |s| s.pinned_images.get_mut(&1).unwrap().proposed = Some(document()),
            |s| {
                s.pinned_images.get_mut(&1).unwrap().interaction.pending = Some(Pending {
                    request: 1,
                    policy: Policy::default(),
                })
            },
            |s| s.annotation_export.request = Some(1),
        ];
        for change in changes {
            let mut s = state();
            s.pinned_images.get_mut(&2).unwrap().user_hidden = true;
            change(&mut s);
            let before = s.clone();
            assert!(!available(&s, true) && !available(&s, false));
            assert!(start(&mut s, true).is_empty() && start(&mut s, false).is_empty());
            assert_eq!(s, before);
        }
    }
    #[test]
    fn pending_batch_blocks_new_image_work_but_close_and_exit_retire_it() {
        let mut s = state();
        let q = start(&mut s, true);
        let events = [
            AppEvent::Screenshot(ScreenshotEvent::Requested(document())),
            AppEvent::Screenshot(ScreenshotEvent::Delayed {
                request: document(),
                seconds: 3,
            }),
            AppEvent::Ocr(crate::domain::ocr::OcrEvent::Requested {
                annotation_active: false,
            }),
            AppEvent::ImageImport(crate::domain::image_import::Event::Requested {
                target: crate::domain::image_import::Target::ClipboardPin,
                anchor: Point::default(),
                owner: None,
                title: String::new(),
            }),
            AppEvent::PinnedImage(PinEvent::EditRequested { id: 3 }),
            AppEvent::PinnedImage(PinEvent::Recognize { id: 3 }),
            AppEvent::PinnedImage(PinEvent::RestoreMouseInput),
            AppEvent::PinnedImage(PinEvent::InteractionRequested {
                id: 3,
                policy: Policy::default(),
            }),
            AppEvent::AnnotationExport(crate::domain::annotation_export::ExportEvent::Requested(
                document(),
            )),
        ];
        let before = s.clone();
        for e in events {
            assert!(s.reduce(e).is_empty());
            assert_eq!(s, before);
        }
        for id in 1..=3 {
            s.reduce(AppEvent::PinnedImage(PinEvent::Closed { id }));
        }
        assert!(s.pin_visibility.pending.is_none());
        for r in q {
            done(&mut s, r, true);
        }
        assert!(s.pinned_images.is_empty());
        s = state();
        start(&mut s, true);
        s.reduce(AppEvent::ExitRequested);
        assert!(s.pin_visibility.pending.is_none() && s.pinned_images.is_empty());
    }
    #[test]
    fn hidden_pins_reject_local_actions_and_empty_requests_are_noops() {
        let mut s = AppState::default();
        assert!(start(&mut s, true).is_empty() && start(&mut s, false).is_empty());
        s = state();
        for r in start(&mut s, true) {
            done(&mut s, r, true);
        }
        let before = s.clone();
        for e in [
            PinEvent::EditRequested { id: 1 },
            PinEvent::Recognize { id: 1 },
            PinEvent::VisibilityChanged {
                id: 1,
                hidden: true,
            },
            PinEvent::InteractionRequested {
                id: 1,
                policy: Policy::default(),
            },
        ] {
            assert!(s.reduce(AppEvent::PinnedImage(e)).is_empty());
        }
        assert_eq!(s, before);
        s.reduce(AppEvent::PinnedImage(PinEvent::RestoreMouseInput));
        assert!(
            s.pinned_images[&1].user_hidden,
            "passthrough recovery does not unhide windows"
        );
    }
}
