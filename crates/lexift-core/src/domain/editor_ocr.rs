//! Copy-only OCR owned by a live image editor; source pixels remain in App caches.
use super::{
    geometry::Rect,
    ocr::{OcrOutcome, OcrRecognitionStrategy},
};
use crate::{AppCommand, AppEvent, AppState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Document { window: u64, editor: u64 },
    Long { session: u64, operation: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub token: u64,
    pub id: u64,
    pub owner: Owner,
    pub revision: u64,
    pub color: super::image_color::Color,
    pub orientation: super::image_transform::Orientation,
    pub source: Rect,
    pub bounds: Rect,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub received: u64,
    pub request: Option<Request>,
    pub result: Option<(Request, OcrOutcome)>,
    next: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Requested {
        token: u64,
        owner: Owner,
        revision: u64,
        color: super::image_color::Color,
        orientation: super::image_transform::Orientation,
        source: Rect,
        bounds: Rect,
    },
    Cancel {
        owner: Owner,
    },
    Retired {
        owner: Owner,
        token: u64,
    },
    Completed {
        request: Request,
        outcome: OcrOutcome,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub request: Request,
    pub strategy: OcrRecognitionStrategy,
}
/// A live host is required even when asynchronous source pixels remain cached.
pub fn valid(state: &AppState, owner: Owner) -> bool {
    match owner {
        Owner::Document { window, editor } => state
            .image_history
            .editor
            .as_ref()
            .is_some_and(|e| e.window == window && e.id == editor && e.ready && !e.busy),
        Owner::Long { session, operation } => {
            state.screenshot.session == session
                && state.screenshot.phase == super::screenshot::ScreenshotPhase::Editing
                && state.screenshot.long.as_ref().is_some_and(|l| {
                    l.operation == operation
                        && l.phase == super::long_capture::workflow::Phase::Editing
                })
        }
    }
}
pub(crate) fn reduce(state: &mut AppState, event: Event) -> Vec<AppCommand> {
    match event {
        Event::Requested {
            token,
            owner,
            revision,
            color,
            orientation,
            source,
            bounds,
        } => {
            state.editor_ocr.received = token;
            if state.editor_ocr.request.is_some()
                || !valid(state, owner)
                || state.image_import.request.is_some()
                || state.annotation_export.request.is_some()
                || state
                    .pinned_images
                    .values()
                    .any(|p| p.busy && p.recognition.is_none())
                || matches!(
                    state.ocr.phase,
                    super::ocr::OcrPhase::Checking
                        | super::ocr::OcrPhase::Capturing
                        | super::ocr::OcrPhase::Recognizing
                        | super::ocr::OcrPhase::Translating
                )
                || bounds.left < source.left
                || bounds.top < source.top
                || bounds.right > source.right
                || bounds.bottom > source.bottom
                || bounds.left >= bounds.right
                || bounds.top >= bounds.bottom
            {
                return vec![];
            }
            super::pinned_image::cancel_recognition(&mut state.pinned_images);
            state.editor_ocr.next = state
                .editor_ocr
                .next
                .checked_add(1)
                .expect("OCR identity exhausted");
            let request = Request {
                token,
                id: state.editor_ocr.next,
                owner,
                revision,
                color,
                orientation,
                source,
                bounds,
            };
            state.editor_ocr.request = Some(request);
            state.editor_ocr.result = None;
            vec![AppCommand::EditorOcr(Command {
                request,
                strategy: super::ocr::copy_strategy(&state.settings),
            })]
        }
        Event::Retired { owner, token } => {
            let belongs = |r: Request| r.owner == owner && r.token == token;
            if state.editor_ocr.request.is_some_and(belongs) {
                state.editor_ocr.request = None;
            }
            state.editor_ocr.result = state.editor_ocr.result.filter(|(r, _)| !belongs(*r));
            vec![]
        }
        Event::Cancel { owner } => {
            if state.editor_ocr.request.is_some_and(|r| r.owner == owner) {
                state.editor_ocr.request = None;
                state.editor_ocr.result = None;
            }
            vec![]
        }
        Event::Completed { request, outcome } => {
            if state.editor_ocr.request == Some(request) && valid(state, request.owner) {
                state.editor_ocr.request = None;
                state.editor_ocr.result = Some((request, outcome));
            }
            vec![]
        }
    }
}
/// Retire work after the owning reducer has accepted a transition, never on rejected input.
pub(crate) fn after(state: &mut AppState, event: &AppEvent) {
    let replacement = matches!(event, AppEvent::ExitRequested)
        || state
            .pinned_images
            .values()
            .any(|p| p.recognition.is_some())
        || matches!(
            state.ocr.phase,
            super::ocr::OcrPhase::Checking
                | super::ocr::OcrPhase::Capturing
                | super::ocr::OcrPhase::Recognizing
        );
    let owner = state
        .editor_ocr
        .request
        .map(|r| r.owner)
        .or_else(|| state.editor_ocr.result.map(|(r, _)| r.owner));
    if replacement || owner.is_some_and(|o| !valid(state, o)) {
        state.editor_ocr.request = None;
        state.editor_ocr.result = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        annotation_export::{ExportRequest, ExportSnapshot},
        long_capture::workflow::{Phase, State as Long},
        screenshot::{ScreenshotEvent, ScreenshotPhase},
    };
    fn setup() -> (AppState, Owner, Rect) {
        let mut s = AppState::default();
        s.screenshot.session = 7;
        s.screenshot.phase = ScreenshotPhase::Editing;
        s.screenshot.long = Some(Long {
            operation: 4,
            source: Rect::default(),
            phase: Phase::Editing,
            sequence: 1,
            width: 100,
            height: 2200,
            feedback: super::super::long_capture::Feedback::Unchanged,
            target: None,
        });
        (
            s,
            Owner::Long {
                session: 7,
                operation: 4,
            },
            Rect {
                left: -100,
                top: -50,
                right: 0,
                bottom: 2150,
            },
        )
    }
    fn start(s: &mut AppState, owner: Owner, source: Rect) -> Vec<AppCommand> {
        s.reduce(AppEvent::EditorOcr(Event::Requested {
            token: s.editor_ocr.next + 1,
            owner,
            revision: 91,
            color: Default::default(),
            orientation: Default::default(),
            source,
            bounds: Rect {
                left: -90,
                top: 0,
                right: -10,
                bottom: 2000,
            },
        }))
    }
    #[test]
    fn editor_ocr_keeps_host_and_history_for_every_outcome() {
        for outcome in [
            OcrOutcome::Copied,
            OcrOutcome::Empty,
            OcrOutcome::ResourcesUnavailable,
            OcrOutcome::RecognitionFailed,
            OcrOutcome::CopyFailed,
        ] {
            let (mut s, owner, source) = setup();
            let screenshot = s.screenshot.clone();
            let history = s.image_history.clone();
            assert!(matches!(
                &start(&mut s, owner, source)[..],
                [AppCommand::EditorOcr(_)]
            ));
            let request = s.editor_ocr.request.unwrap();
            assert!(start(&mut s, owner, source).is_empty());
            s.reduce(AppEvent::EditorOcr(Event::Completed { request, outcome }));
            assert_eq!(s.editor_ocr.result, Some((request, outcome)));
            assert_eq!(s.screenshot, screenshot);
            assert_eq!(s.image_history, history);
        }
    }
    #[test]
    fn cancel_reopen_and_foreign_completion_cannot_copy_or_restore_old_result() {
        let (mut s, owner, source) = setup();
        start(&mut s, owner, source);
        let request = s.editor_ocr.request.unwrap();
        s.reduce(AppEvent::EditorOcr(Event::Cancel { owner }));
        start(&mut s, owner, source);
        let next = s.editor_ocr.request.unwrap();
        assert_ne!(request.id, next.id);
        s.reduce(AppEvent::EditorOcr(Event::Completed {
            request,
            outcome: OcrOutcome::Copied,
        }));
        assert_eq!(s.editor_ocr.request, Some(next));
        assert!(s.editor_ocr.result.is_none());
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
            session: 7,
        }));
        assert!(s.editor_ocr.request.is_none());
        s.reduce(AppEvent::EditorOcr(Event::Completed {
            request: next,
            outcome: OcrOutcome::Copied,
        }));
        assert!(s.editor_ocr.result.is_none());
    }
    #[test]
    fn invalid_source_crop_and_busy_output_are_rejected() {
        let (mut s, owner, source) = setup();
        assert!(
            start(
                &mut s,
                Owner::Long {
                    session: 8,
                    operation: 4
                },
                source
            )
            .is_empty()
        );
        assert!(start(&mut s, owner, Rect::default()).is_empty());
        start(&mut s, owner, source);
        let document = ExportRequest {
            session: 7,
            request: 1,
            snapshot: std::sync::Arc::new(ExportSnapshot {
                session: std::sync::Arc::new(super::super::annotation::Session::new()),
                monitor: source,
                scale: 1.,
                monitors: vec![],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Output {
            session: 7,
            operation: 1,
            kind: super::super::screenshot::ScreenshotOutput::Copy,
            request: document,
        }));
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Editing);
        assert!(s.editor_ocr.request.is_some());
    }
    #[test]
    fn document_owner_replacement_retires_old_work_and_retirement_cannot_cancel_new_request() {
        let (mut s, _, source) = setup();
        s.screenshot = Default::default();
        s.image_history.editor = Some(super::super::image_history::workflow::Editor {
            ready: true,
            failed: false,
            hidden: true,
            window: 30,
            id: 40,
            origin: super::super::image_history::Origin(1),
            import_anchor: None,
            kind: super::super::image_history::Kind::Imported,
            operation: 0,
            busy: false,
        });
        let owner = Owner::Document {
            window: 30,
            editor: 40,
        };
        start(&mut s, owner, source);
        let old = s.editor_ocr.request.unwrap();
        s.reduce(AppEvent::EditorOcr(Event::Cancel { owner }));
        start(&mut s, owner, source);
        let next = s.editor_ocr.request.unwrap();
        s.reduce(AppEvent::EditorOcr(Event::Retired {
            owner: old.owner,
            token: old.token,
        }));
        assert_eq!(s.editor_ocr.request, Some(next));
        s.reduce(AppEvent::ImageHistory(
            super::super::image_history::workflow::Event::EditorClosed {
                window: 30,
                editor: 40,
            },
        ));
        assert!(s.editor_ocr.request.is_none());
        s.reduce(AppEvent::EditorOcr(Event::Completed {
            request: next,
            outcome: OcrOutcome::Copied,
        }));
        assert!(s.editor_ocr.result.is_none());
    }
}
