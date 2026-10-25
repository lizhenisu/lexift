//! User-requested image acquisition. Native handles and pixels are owned by adapters.
use super::{
    geometry::{Point, Rect},
    image_history::{Kind, workflow::Editor},
};
use crate::ports::{
    annotation_save::{ImageOpenRequest, SaveOutcome},
    magnifier::WindowToken,
};
use crate::{AppCommand, AppEvent, AppState, Result};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    FileEditor,
    ClipboardPin,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Reading,
    Preparing,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub target: Target,
    pub anchor: Point,
    pub phase: Phase,
    pub editor_owner: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    Failed,
    NoImage,
    Conflict,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub request: Option<Request>,
    pub notice: Option<(u64, Point, Feedback)>,
    next: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Requested {
        target: Target,
        anchor: Point,
        owner: Option<WindowToken>,
        title: String,
    },
    Loaded {
        id: u64,
        result: Result<Option<Rect>>,
    },
    Completed {
        id: u64,
        result: Result<SaveOutcome>,
    },
    Cancel {
        id: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Read {
        id: u64,
        target: Target,
        request: ImageOpenRequest,
    },
    Present {
        id: u64,
        target: Target,
    },
    Cancel {
        id: u64,
    },
}
pub fn available(s: &AppState) -> bool {
    s.pin_visibility.pending.is_none()
        && s.image_import.request.is_none()
        && !super::image_history::workflow::blocked(s)
        && s.annotation_export.request.is_none()
        && !matches!(
            s.ocr.phase,
            super::ocr::OcrPhase::Checking
                | super::ocr::OcrPhase::Capturing
                | super::ocr::OcrPhase::Recognizing
                | super::ocr::OcrPhase::Translating
        )
        && !s.pinned_images.values().any(|p| p.proposed.is_some())
        && s.image_history.window.as_ref().is_none_or(|w| !w.busy)
}
pub(crate) fn reduce(s: &mut AppState, event: Event) -> Vec<AppCommand> {
    let command = |c| vec![AppCommand::ImageImport(c)];
    match event {
        Event::Requested {
            target,
            anchor,
            owner,
            title,
        } => {
            if s.image_import.request.is_some() {
                return vec![];
            }
            s.image_import.next = s
                .image_import
                .next
                .checked_add(1)
                .expect("image import identity exhausted");
            let id = s.image_import.next;
            if !available(s) {
                s.image_import.notice = Some((id, anchor, Feedback::Conflict));
                return vec![];
            }
            s.image_import.notice = None;
            s.image_import.request = Some(Request {
                id,
                target,
                anchor,
                phase: Phase::Reading,
                editor_owner: None,
            });
            s.image_history.window = None;
            command(Command::Read {
                id,
                target,
                request: ImageOpenRequest { owner, title },
            })
        }
        Event::Loaded { id, result }
            if s.image_import
                .request
                .as_ref()
                .is_some_and(|r| r.id == id && r.phase == Phase::Reading) =>
        {
            let r = s.image_import.request.as_ref().unwrap().clone();
            match result {
                Ok(Some(bounds))
                    if bounds.left == 0
                        && bounds.top == 0
                        && bounds.right > 0
                        && bounds.bottom > 0
                        && i64::from(bounds.right) * i64::from(bounds.bottom) <= 40_000_000 =>
                {
                    s.image_import.request.as_mut().unwrap().phase = Phase::Preparing;
                    if r.target == Target::FileEditor {
                        let owner = s.image_history.editor_owner();
                        s.image_import.request.as_mut().unwrap().editor_owner = Some(owner);
                        let origin = s
                            .image_history
                            .cache
                            .origin()
                            .expect("image origin exhausted");
                        s.image_history.editor = Some(Editor {
                            ready: false,
                            failed: false,
                            hidden: false,
                            window: owner,
                            id: owner,
                            origin,
                            import_anchor: Some(r.anchor),
                            kind: Kind::Imported,
                            operation: 0,
                            busy: false,
                        });
                    } else {
                        s.image_history.start_import(id);
                    }
                    command(Command::Present {
                        id,
                        target: r.target,
                    })
                }
                Ok(None) => {
                    s.image_import.request = None;
                    if r.target == Target::ClipboardPin {
                        s.image_import.notice = Some((id, r.anchor, Feedback::NoImage));
                    }
                    command(Command::Cancel { id })
                }
                _ => {
                    s.image_import.request = None;
                    s.image_import.notice = Some((id, r.anchor, Feedback::Failed));
                    command(Command::Cancel { id })
                }
            }
        }
        Event::Completed { id, result }
            if s.image_import.request.as_ref().is_some_and(|r| r.id == id) =>
        {
            let r = s.image_import.request.take().unwrap();
            if result.is_err() {
                s.image_import.notice = Some((id, r.anchor, Feedback::Failed));
            }
            command(Command::Cancel { id })
        }
        Event::Cancel { id } if s.image_import.request.as_ref().is_some_and(|r| r.id == id) => {
            s.image_import.request = None;
            if s.image_history
                .editor
                .as_ref()
                .is_some_and(|e| e.import_anchor.is_some() && e.id == id)
            {
                s.image_history.editor = None;
            }
            command(Command::Cancel { id })
        }
        _ => vec![],
    }
}
/// Presentation acknowledgement uses the common editor lifecycle, independently of history browsing.
pub(crate) fn after(s: &mut AppState, event: &AppEvent) -> Vec<AppCommand> {
    if let AppEvent::ImageHistory(super::image_history::workflow::Event::EditorClosed {
        window,
        editor,
    }) = event
        && s.image_import.request.as_ref().is_some_and(|r| {
            r.target == Target::FileEditor
                && r.editor_owner == Some(*window)
                && r.editor_owner == Some(*editor)
        })
    {
        let r = s.image_import.request.take().unwrap();
        return vec![AppCommand::ImageImport(Command::Cancel { id: r.id })];
    }
    if matches!(event, AppEvent::ExitRequested) {
        return s
            .image_import
            .request
            .take()
            .map(|r| vec![AppCommand::ImageImport(Command::Cancel { id: r.id })])
            .unwrap_or_default();
    }
    if let AppEvent::ImageHistory(super::image_history::workflow::Event::Completed {
        window,
        operation,
        result,
    }) = event
        && s.image_import.request.as_ref().is_some_and(|r| {
            r.target == Target::FileEditor
                && r.editor_owner == Some(*window)
                && r.editor_owner == Some(*operation)
        })
    {
        let r = s.image_import.request.take().unwrap();
        if result.is_err() {
            s.image_import.notice = Some((r.id, r.anchor, Feedback::Failed));
            s.image_history.editor = None;
        }
        return vec![AppCommand::ImageImport(Command::Cancel { id: r.id })];
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(s: &mut AppState, target: Target) -> u64 {
        let commands = s.reduce(AppEvent::ImageImport(Event::Requested {
            target,
            anchor: Point { x: 100, y: 200 },
            owner: None,
            title: String::new(),
        }));
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::ImageImport(Command::Read { .. })]
        ));
        s.image_import.request.as_ref().unwrap().id
    }
    fn loaded(s: &mut AppState, id: u64) {
        s.reduce(AppEvent::ImageImport(Event::Loaded {
            id,
            result: Ok(Some(Rect {
                left: 0,
                top: 0,
                right: 2,
                bottom: 3,
            })),
        }));
    }
    #[test]
    fn file_open_is_not_history_and_sources_are_distinct() {
        let mut s = AppState::default();
        let id = request(&mut s, Target::FileEditor);
        loaded(&mut s, id);
        let editor = s.image_history.editor.clone().unwrap();
        assert_eq!(editor.kind, Kind::Imported);
        assert!(s.image_history.window.is_none());
        assert!(s.image_history.cache.entries().is_empty());
        s.reduce(AppEvent::ImageHistory(
            super::super::image_history::workflow::Event::Completed {
                window: id,
                operation: id,
                result: Ok(SaveOutcome::Cancelled),
            },
        ));
        assert!(s.image_import.request.is_none());
        s.reduce(AppEvent::ImageHistory(
            super::super::image_history::workflow::Event::EditorClosed {
                window: id,
                editor: id,
            },
        ));
        let next = request(&mut s, Target::FileEditor);
        loaded(&mut s, next);
        assert_ne!(
            editor.origin,
            s.image_history.editor.as_ref().unwrap().origin
        );
        assert!(s.image_history.cache.entries().is_empty());
    }
    #[test]
    fn duplicate_cancel_and_old_results_cannot_open_an_editor() {
        let mut s = AppState::default();
        let id = request(&mut s, Target::FileEditor);
        assert!(
            s.reduce(AppEvent::ImageImport(Event::Requested {
                target: Target::ClipboardPin,
                anchor: Point::default(),
                owner: None,
                title: String::new()
            }))
            .is_empty()
        );
        s.reduce(AppEvent::ImageImport(Event::Cancel { id }));
        let next = request(&mut s, Target::FileEditor);
        loaded(&mut s, id);
        assert!(s.image_history.editor.is_none());
        assert_eq!(s.image_import.request.as_ref().unwrap().id, next);
        s.reduce(AppEvent::ImageImport(Event::Loaded {
            id: next,
            result: Ok(None),
        }));
        assert!(s.image_import.notice.is_none());
        assert!(s.image_import.request.is_none());
    }
    #[test]
    fn clipboard_empty_failure_and_pin_success_do_not_touch_region_history() {
        let mut s = AppState::default();
        let previous = super::super::screenshot::LastRegion {
            bounds: Rect {
                left: -40,
                top: 10,
                right: 80,
                bottom: 50,
            },
            layout: vec![Rect {
                left: -100,
                top: 0,
                right: 100,
                bottom: 100,
            }],
        };
        s.screenshot.last_region = Some(previous.clone());
        let id = request(&mut s, Target::ClipboardPin);
        s.reduce(AppEvent::ImageImport(Event::Loaded {
            id,
            result: Ok(None),
        }));
        assert_eq!(s.image_import.notice.unwrap().2, Feedback::NoImage);
        let id = request(&mut s, Target::ClipboardPin);
        loaded(&mut s, id);
        let key = super::super::image_history::workflow::OutputKey::Import { operation: id };
        let (ticket, kind) = s.image_history.ticket(key).unwrap();
        assert_eq!(kind, Kind::Imported);
        assert!(s.image_history.cache.entries().is_empty());
        let entry = super::super::image_history::Entry {
            ticket,
            kind,
            hidden: false,
            bounds: Rect {
                left: 0,
                top: 0,
                right: 2,
                bottom: 3,
            },
            updated_ms: 0,
            resources: vec![super::super::image_history::Resource {
                id: super::super::image_history::ResourceId(1),
                bytes: 24,
            }],
        };
        s.reduce(AppEvent::ImageHistory(
            super::super::image_history::workflow::Event::Prepared { key, entry },
        ));
        s.reduce(AppEvent::ImageImport(Event::Completed {
            id,
            result: Ok(SaveOutcome::Saved),
        }));
        assert_eq!(s.image_history.cache.entries().len(), 1);
        assert!(s.image_import.request.is_none());
        assert_eq!(s.screenshot.last_region, Some(previous));
    }
    #[test]
    fn editing_conflicts_and_exit_are_bounded() {
        let mut s = AppState::default();
        let id = request(&mut s, Target::FileEditor);
        loaded(&mut s, id);
        assert!(!available(&s));
        s.reduce(AppEvent::ExitRequested);
        assert!(s.image_import.request.is_none());
        assert!(s.image_history.editor.is_none());
    }
    #[test]
    fn old_history_owner_cannot_acknowledge_an_imported_editor() {
        use super::super::image_history::workflow::Event as H;
        let mut s = AppState::default();
        s.reduce(AppEvent::ImageHistory(H::Open {
            anchor: Point::default(),
        }));
        let old = s.image_history.window.as_ref().unwrap().id;
        s.reduce(AppEvent::ImageHistory(H::Close { window: old }));
        let id = request(&mut s, Target::FileEditor);
        loaded(&mut s, id);
        let owner = s
            .image_import
            .request
            .as_ref()
            .unwrap()
            .editor_owner
            .unwrap();
        assert_ne!(owner, old);
        s.reduce(AppEvent::ImageHistory(H::Completed {
            window: old,
            operation: old,
            result: Ok(SaveOutcome::Cancelled),
        }));
        assert!(s.image_import.request.is_some());
        s.reduce(AppEvent::ImageHistory(H::Completed {
            window: owner,
            operation: owner,
            result: Ok(SaveOutcome::Cancelled),
        }));
        assert!(s.image_import.request.is_none());
    }
    #[test]
    fn every_active_ocr_stage_blocks_import_without_starting_acquisition() {
        for phase in [
            super::super::ocr::OcrPhase::Checking,
            super::super::ocr::OcrPhase::Capturing,
            super::super::ocr::OcrPhase::Recognizing,
            super::super::ocr::OcrPhase::Translating,
        ] {
            let mut s = AppState::default();
            s.ocr.phase = phase;
            assert!(!available(&s));
            let commands = s.reduce(AppEvent::ImageImport(Event::Requested {
                target: Target::FileEditor,
                anchor: Point::default(),
                owner: None,
                title: String::new(),
            }));
            assert!(commands.is_empty());
            assert!(s.image_import.request.is_none());
            assert_eq!(s.image_import.notice.unwrap().2, Feedback::Conflict);
        }
    }
}
