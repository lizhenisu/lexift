use super::*;
use crate::domain::{
    annotation::Session,
    annotation_export::{ExportRequest, ExportSnapshot},
    geometry::{Point, Rect},
    screenshot::ScreenshotOutput,
};
use std::sync::Arc;

fn bounds() -> Rect {
    Rect {
        left: -80,
        top: -20,
        right: 120,
        bottom: 280,
    }
}
fn request(session: u64, operation: u64) -> ExportRequest {
    ExportRequest {
        session,
        request: operation,
        snapshot: Arc::new(ExportSnapshot {
            session: Arc::new(Session::new()),
            monitor: bounds(),
            scale: 1.25,
            monitors: vec![(bounds(), 1.25)],
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
        }),
    }
}
fn start(state: &mut AppState, kind: ScreenshotOutput, session: u64) -> OutputKey {
    state.screenshot.session = session;
    state.screenshot.phase = ScreenshotPhase::Editing;
    state.screenshot.bounds = Some(bounds());
    state.screenshot.layout = vec![bounds()];
    state.screenshot.operation = 0;
    let commands = state.reduce(AppEvent::Screenshot(ScreenshotEvent::Output {
        session,
        operation: 1,
        kind,
        request: request(session, 1),
    }));
    assert!(matches!(
        &commands[0],
        AppCommand::Screenshot(ScreenshotCommand::Output { .. })
    ));
    OutputKey::Screenshot {
        session,
        operation: 1,
    }
}
fn prepared(state: &mut AppState, key: OutputKey) -> Ticket {
    let (ticket, kind) = state.image_history.ticket(key).unwrap();
    state.reduce(AppEvent::ImageHistory(Event::Prepared {
        key,
        entry: Entry {
            ticket,
            kind,
            hidden: false,
            bounds: bounds(),
            updated_ms: 10,
            resources: vec![Resource {
                id: ResourceId(ticket.version),
                bytes: 4096,
            }],
        },
    }));
    ticket
}
fn finish(state: &mut AppState, key: OutputKey, result: crate::Result<SaveOutcome>) {
    let event = match key {
        OutputKey::Screenshot { session, operation } => {
            AppEvent::Screenshot(ScreenshotEvent::Completed {
                session,
                operation,
                result,
            })
        }
        OutputKey::Pin { id, operation } => AppEvent::PinnedImage(pin::Event::Completed {
            id,
            operation,
            result,
        }),
        OutputKey::History { window, operation } => AppEvent::ImageHistory(Event::Completed {
            window,
            operation,
            result,
        }),
        OutputKey::Editor {
            window,
            editor,
            operation,
        } => AppEvent::ImageHistory(Event::EditorCompleted {
            window,
            editor,
            operation,
            result,
        }),
    };
    state.reduce(event);
}
#[test]
fn copy_save_and_native_pin_require_both_success_and_prepared_assets() {
    for (session, kind) in [
        (1, ScreenshotOutput::Copy),
        (2, ScreenshotOutput::Save),
        (3, ScreenshotOutput::Pin),
    ] {
        let mut state = AppState::default();
        let key = start(&mut state, kind, session);
        if kind == ScreenshotOutput::Pin {
            let commands = state.reduce(AppEvent::PinnedImage(pin::Event::Opened {
                id: 1,
                source: (session, 1),
            }));
            assert!(matches!(
                &commands[0],
                AppCommand::PinnedImage(pin::Command::Retain { .. })
            ));
        }
        assert!(state.image_history.cache.entries().is_empty());
        finish(&mut state, key, Ok(SaveOutcome::Saved));
        assert!(state.image_history.cache.entries().is_empty());
        let ticket = prepared(&mut state, key);
        assert_eq!(state.image_history.cache.entries()[0].ticket, ticket);
    }
}
#[test]
fn failed_cancelled_and_late_success_never_record() {
    for result in [
        Ok(SaveOutcome::Cancelled),
        Err(crate::Error::new("fixture")),
    ] {
        let mut s = AppState::default();
        let key = start(&mut s, ScreenshotOutput::Save, 1);
        prepared(&mut s, key);
        finish(&mut s, key, result);
        assert!(s.image_history.cache.entries().is_empty());
        assert!(s.image_history.pending.is_empty());
        finish(&mut s, key, Ok(SaveOutcome::Saved));
        assert!(s.image_history.cache.entries().is_empty());
    }
    let mut s = AppState::default();
    let key = start(&mut s, ScreenshotOutput::Copy, 1);
    prepared(&mut s, key);
    s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
        session: 1,
    }));
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    assert!(s.image_history.cache.entries().is_empty());
}
#[test]
fn descendant_pin_keeps_origin_after_cache_deletion_and_old_completion_is_ignored() {
    let mut s = AppState::default();
    let key = start(&mut s, ScreenshotOutput::Pin, 1);
    let ticket = prepared(&mut s, key);
    s.reduce(AppEvent::PinnedImage(pin::Event::Opened {
        id: 1,
        source: (1, 1),
    }));
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    s.reduce(AppEvent::ImageHistory(Event::Delete { ticket }));
    s.reduce(AppEvent::PinnedImage(pin::Event::Requested {
        id: 1,
        kind: pin::Output::Copy,
        owner: None,
        title: String::new(),
    }));
    let key = OutputKey::Pin {
        id: 1,
        operation: 1,
    };
    let descendant = prepared(&mut s, key);
    assert_eq!(ticket.origin, descendant.origin);
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    assert_eq!(s.image_history.cache.entries()[0].ticket, descendant);
    finish(
        &mut s,
        OutputKey::Screenshot {
            session: 1,
            operation: 1,
        },
        Ok(SaveOutcome::Saved),
    );
    assert_eq!(s.image_history.cache.entries()[0].ticket, descendant);
}
fn seeded() -> (AppState, Ticket, u64) {
    let mut s = AppState::default();
    let key = start(&mut s, ScreenshotOutput::Copy, 1);
    let ticket = prepared(&mut s, key);
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    s.reduce(AppEvent::ImageHistory(Event::Open {
        anchor: Point { x: 20, y: 30 },
    }));
    let window = s.image_history.window.as_ref().unwrap().id;
    (s, ticket, window)
}
#[test]
fn direct_output_preserves_origin_and_region_history_and_close_invalidates_completion() {
    let (mut s, ticket, window) = seeded();
    let region = s.screenshot.last_region.clone();
    s.reduce(AppEvent::ImageHistory(Event::Action {
        window,
        ticket,
        action: Action::Copy,
        owner: None,
        title: String::new(),
    }));
    let key = OutputKey::History {
        window,
        operation: 1,
    };
    let next = prepared(&mut s, key);
    assert_eq!(next.origin, ticket.origin);
    s.reduce(AppEvent::ImageHistory(Event::Clear));
    assert_eq!(s.image_history.cache.entries().len(), 1);
    s.reduce(AppEvent::ImageHistory(Event::Close { window }));
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    assert_eq!(s.image_history.cache.entries()[0].ticket, ticket);
    assert_eq!(s.screenshot.last_region, region);
}
#[test]
fn selection_is_stable_and_old_ticket_cannot_delete_new_version() {
    let (mut s, ticket, window) = seeded();
    s.reduce(AppEvent::ImageHistory(Event::Action {
        window,
        ticket,
        action: Action::Save,
        owner: None,
        title: String::new(),
    }));
    let key = OutputKey::History {
        window,
        operation: 1,
    };
    let next = prepared(&mut s, key);
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    s.reduce(AppEvent::ImageHistory(Event::Delete { ticket }));
    assert_eq!(s.image_history.cache.entries()[0].ticket, next);
    assert_eq!(
        s.image_history.window.as_ref().unwrap().selected,
        Some(ticket.origin)
    );
}
#[test]
fn editor_owns_its_source_after_delete_and_only_new_output_updates_history() {
    let (mut s, ticket, window) = seeded();
    s.reduce(AppEvent::ImageHistory(Event::Action {
        window,
        ticket,
        action: Action::Edit,
        owner: None,
        title: String::new(),
    }));
    assert!(s.image_history.editor.is_some());
    finish(
        &mut s,
        OutputKey::History {
            window,
            operation: 1,
        },
        Ok(SaveOutcome::Cancelled),
    );
    s.reduce(AppEvent::ImageHistory(Event::Delete { ticket }));
    assert_eq!(s.image_history.editor.as_ref().unwrap().ticket, ticket);
    assert!(
        !s.image_history.retains(ticket),
        "deleted history releases its finished output, while the editor owns its source"
    );
    s.reduce(AppEvent::ImageHistory(Event::EditorOutput {
        window,
        editor: 1,
        operation: 1,
        kind: ScreenshotOutput::Copy,
        document: request(1, 1),
    }));
    let key = OutputKey::Editor {
        window,
        editor: 1,
        operation: 1,
    };
    let next = prepared(&mut s, key);
    finish(&mut s, key, Ok(SaveOutcome::Saved));
    assert_eq!(next.origin, ticket.origin);
    assert_eq!(s.image_history.cache.entries()[0].ticket, next);
    s.reduce(AppEvent::ImageHistory(Event::EditorClosed {
        window,
        editor: 1,
    }));
    assert!(!s.image_history.retains(ticket));
}
#[test]
fn preparation_blocks_window_but_active_edit_allows_browsing_only() {
    let (mut s, ticket, window) = seeded();
    s.screenshot.phase = ScreenshotPhase::Editing;
    assert!(
        s.reduce(AppEvent::ImageHistory(Event::Action {
            window,
            ticket,
            action: Action::Copy,
            owner: None,
            title: String::new()
        }))
        .is_empty()
    );
    s.reduce(AppEvent::ImageHistory(Event::Delete { ticket }));
    assert!(s.image_history.cache.entries().is_empty());
    s.screenshot.phase = ScreenshotPhase::Selecting;
    s.reduce(AppEvent::ImageHistory(Event::Open {
        anchor: Default::default(),
    }));
    assert!(s.image_history.window.is_none());
}
