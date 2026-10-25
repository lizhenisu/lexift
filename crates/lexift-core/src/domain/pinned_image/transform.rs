//! Transactional direction changes share the document proposal and render lifetime.
use super::{Command, State};
use crate::{AppCommand, AppState, domain::image_transform::Action};
use std::sync::Arc;

pub fn available(s: &AppState, id: u64) -> bool {
    !s.screenshot.active()
        && s.pin_visibility.pending.is_none()
        && s.image_import.request.is_none()
        && s.editor_ocr.request.is_none()
        && s.annotation_export.request.is_none()
        && s.image_history.window.as_ref().is_none_or(|w| !w.busy)
        && s.image_history.editor.is_none()
        && !matches!(
            s.ocr.phase,
            crate::domain::ocr::OcrPhase::Checking
                | crate::domain::ocr::OcrPhase::Capturing
                | crate::domain::ocr::OcrPhase::Recognizing
                | crate::domain::ocr::OcrPhase::Translating
        )
        && s.pinned_images.get(&id).is_some_and(|p| {
            !p.user_hidden
                && !p.busy
                && p.proposed.is_none()
                && p.recognition.is_none()
                && p.interaction.pending.is_none()
                && !p.interaction.applied.locked
                && !p.interaction.applied.passthrough
                && p.document.is_some()
        })
}
pub(super) fn request(s: &mut AppState, id: u64, action: Action) -> Vec<AppCommand> {
    if !available(s, id) {
        return vec![];
    }
    let p = s.pinned_images.get_mut(&id).unwrap();
    let mut document = p.document.clone().unwrap();
    let mut snapshot = (*document.snapshot).clone();
    let mut session = (*snapshot.session).clone();
    session.image_source = session.image_source.or(p.source);
    if !session.orient_image(session.image_orientation.action(action)) {
        return vec![];
    }
    snapshot.session = Arc::new(session);
    document.snapshot = Arc::new(snapshot);
    document.request = document
        .request
        .checked_add(1)
        .expect("pin document exhausted");
    p.proposed = Some(document.clone());
    p.proposal_ready = false;
    p.transforming = true;
    p.busy = true;
    p.revision += 1;
    p.error = None;
    vec![AppCommand::PinnedImage(Command::Render {
        id,
        revision: p.revision,
        document,
        hidden: p.hidden,
    })]
}
pub(super) fn accept(p: &mut State, revision: u64, request: u64) {
    if p.transforming
        && p.revision == revision
        && p.proposal_ready
        && p.proposed.as_ref().is_some_and(|d| d.request == request)
    {
        p.document = p.proposed.take();
        p.proposal_ready = false;
        p.transforming = false;
        p.busy = false;
    }
}
pub(super) fn cancel(p: &mut State) {
    if p.transforming {
        p.transforming = false;
        p.busy = false;
        p.proposed = None;
        p.proposal_ready = false;
        p.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        annotation::Session,
        annotation_export::{ExportRequest, ExportSnapshot},
        geometry::Rect,
        image_transform::Orientation,
        pinned_image::{self as pin, Event},
    };
    fn state() -> AppState {
        let mut s = AppState::default();
        let full = Rect {
            left: -10,
            top: -20,
            right: 91,
            bottom: 43,
        };
        let mut session = Session::new();
        session.image_source = Some(full);
        let doc = ExportRequest {
            session: 0,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(session),
                monitor: full,
                scale: 1.,
                monitors: vec![],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        for id in [1, 2] {
            s.pinned_images.insert(
                id,
                State {
                    source: Some(full),
                    document: Some(doc.clone()),
                    ..Default::default()
                },
            );
        }
        s
    }
    fn finish(s: &mut AppState) {
        let p = &s.pinned_images[&1];
        let revision = p.revision;
        let request = p.proposed.as_ref().unwrap().request;
        pin::reduce(s, Event::Rendered { id: 1, revision });
        pin::reduce(
            s,
            Event::TransformAccepted {
                id: 1,
                revision,
                request,
            },
        );
    }
    #[test]
    fn every_direction_is_transactional_and_undoable_without_entering_edit() {
        for earlier in Orientation::ALL {
            for action in [
                Action::Left,
                Action::Right,
                Action::Horizontal,
                Action::Vertical,
            ] {
                let mut s = state();
                let old = s
                    .pinned_images
                    .get_mut(&1)
                    .unwrap()
                    .document
                    .as_mut()
                    .unwrap();
                Arc::make_mut(&mut Arc::make_mut(&mut old.snapshot).session).image_orientation =
                    earlier;
                let before = old.clone();
                let other = s.pinned_images[&2].clone();
                assert_eq!(request(&mut s, 1, action).len(), 1);
                assert_eq!(s.pinned_images[&1].document.as_ref(), Some(&before));
                assert!(request(&mut s, 1, action).is_empty());
                let p = &s.pinned_images[&1];
                let r = p.revision;
                let req = p.proposed.as_ref().unwrap().request;
                pin::reduce(
                    &mut s,
                    Event::TransformAccepted {
                        id: 1,
                        revision: r,
                        request: req,
                    },
                );
                assert!(
                    s.pinned_images[&1].transforming,
                    "unrendered ack must not commit"
                );
                finish(&mut s);
                let p = &s.pinned_images[&1];
                assert!(p.editing.is_none());
                assert!(!p.transforming);
                assert!(p.proposed.is_none());
                let mut doc = (*p.document.as_ref().unwrap().snapshot.session).clone();
                assert_eq!(doc.image_orientation, earlier.action(action));
                assert!(doc.history_changes_image_orientation(false));
                assert!(doc.undo());
                assert_eq!(doc.image_orientation, earlier);
                assert!(doc.redo());
                assert_eq!(doc.image_orientation, earlier.action(action));
                assert_eq!(s.pinned_images[&2], other);
                assert_eq!(s.image_history, state().image_history);
            }
        }
    }
    #[test]
    fn failures_cancellation_and_stale_results_preserve_the_accepted_document() {
        let mut s = state();
        let before = s.pinned_images[&1].document.clone();
        request(&mut s, 1, Action::Right);
        let old = s.pinned_images[&1].revision;
        pin::reduce(
            &mut s,
            Event::PreviewFailed {
                id: 1,
                revision: old,
                error: "test".into(),
            },
        );
        assert_eq!(s.pinned_images[&1].document, before);
        assert!(!s.pinned_images[&1].transforming);
        request(&mut s, 1, Action::Horizontal);
        let revision = s.pinned_images[&1].revision;
        pin::reduce(
            &mut s,
            Event::PreviewFailed {
                id: 1,
                revision: old,
                error: "stale".into(),
            },
        );
        assert!(s.pinned_images[&1].transforming);
        assert_eq!(s.pinned_images[&1].revision, revision);
        cancel(s.pinned_images.get_mut(&1).unwrap());
        pin::reduce(&mut s, Event::Rendered { id: 1, revision });
        pin::reduce(
            &mut s,
            Event::TransformAccepted {
                id: 1,
                revision,
                request: 2,
            },
        );
        assert_eq!(s.pinned_images[&1].document, before);
    }
    #[test]
    fn stale_edit_completion_cannot_cancel_new_direction_and_crop_history_is_shared() {
        let mut s = state();
        let p = s.pinned_images.get_mut(&1).unwrap();
        p.editing = Some(7);
        p.last_edit = 7;
        let doc = p.document.as_mut().unwrap();
        doc.session = 7;
        let session = Arc::make_mut(&mut Arc::make_mut(&mut doc.snapshot).session);
        let full = session.image_source.unwrap();
        let crop = crate::domain::geometry::Rect {
            left: 0,
            top: 0,
            right: 30,
            bottom: 30,
        };
        session.set_image_crop(crop, full);
        Arc::make_mut(&mut doc.snapshot).monitor = crop;
        request(&mut s, 1, Action::Horizontal);
        let revision = s.pinned_images[&1].revision;
        s.reduce(crate::AppEvent::PinnedImage(Event::EditFinished {
            id: 1,
            edit: 6,
        }));
        assert!(s.pinned_images[&1].transforming);
        assert_eq!(s.pinned_images[&1].revision, revision);
        finish(&mut s);
        let mut session = (*s.pinned_images[&1]
            .document
            .as_ref()
            .unwrap()
            .snapshot
            .session)
            .clone();
        assert_eq!(session.image_crop, Some(crop));
        assert!(session.undo());
        assert_eq!(session.image_crop, Some(crop));
        assert!(session.undo());
        assert_eq!(session.image_crop, None);
        assert!(session.redo());
        assert_eq!(session.image_crop, Some(crop));
        request(&mut s, 1, Action::Right);
        s.reduce(crate::AppEvent::PinnedImage(Event::EditFinished {
            id: 1,
            edit: 7,
        }));
        assert!(!s.pinned_images[&1].busy);
        assert!(!s.pinned_images[&1].transforming);
    }
    #[test]
    fn locked_and_conflicting_tasks_reject_all_four_actions() {
        for action in [
            Action::Left,
            Action::Right,
            Action::Horizontal,
            Action::Vertical,
        ] {
            for conflict in 0..6 {
                let mut s = state();
                let p = s.pinned_images.get_mut(&1).unwrap();
                match conflict {
                    0 => p.interaction.applied.locked = true,
                    1 => p.user_hidden = true,
                    2 => p.interaction.applied.passthrough = true,
                    3 => p.busy = true,
                    4 => s.screenshot.phase = crate::domain::screenshot::ScreenshotPhase::Selecting,
                    _ => p.proposed = p.document.clone(),
                }
                assert!(request(&mut s, 1, action).is_empty());
            }
        }
    }
}
