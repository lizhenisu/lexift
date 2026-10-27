//! History owns an image editor independently of the desktop screenshot workflow.
use super::*;
use lexift_core::domain::annotation_export::ExportRequest;
use lexift_core::domain::image_history::{
    ImageSnapshot,
    workflow::{Editor, Event},
};
use std::sync::Arc;

struct Recovery {
    window: u64,
    editor: u64,
    image: ImageSnapshot,
    area: Rect,
    document: editor_document::EditorDocument,
    lifecycle: WindowLifecycleCallbacks,
    position: slint::PhysicalPosition,
    collapsed: bool,
    open: bool,
    presented: bool,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
}
thread_local! {
    static ACTIVE: RefCell<Option<Recovery>> = const { RefCell::new(None) };
    static CORE: RefCell<Option<Editor>> = const { RefCell::new(None) };
}
fn emit(event: Event) {
    crate::bridge::image_history::emit(event);
}
pub(crate) fn active() -> bool {
    ACTIVE.with(|s| s.borrow().is_some())
}
pub(super) fn imported() -> bool {
    CORE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|e| e.import_anchor.is_some())
    })
}
pub(super) fn identity() -> Option<(u64, u64)> {
    ACTIVE.with(|s| s.borrow().as_ref().map(|s| (s.window, s.editor)))
}
pub(super) fn current(window: u64, editor: u64) -> bool {
    CORE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|e| e.window == window && e.id == editor)
    })
}
pub(super) fn busy() -> bool {
    CORE.with(|s| s.borrow().as_ref().is_some_and(|e| e.busy || !e.ready))
}
pub(super) fn hidden() -> bool {
    CORE.with(|s| s.borrow().as_ref().is_some_and(|e| e.hidden))
}
pub(super) fn area() -> Option<Rect> {
    ACTIVE.with(|s| s.borrow().as_ref().map(|s| s.area))
}
pub(super) fn source_metadata(
    snapshot: &mut lexift_core::domain::annotation_export::ExportSnapshot,
) {
    ACTIVE.with(|s| {
        if let Some(s) = s.borrow().as_ref() {
            snapshot.monitors = s.image.document.snapshot.monitors.clone();
            snapshot.scale = s.image.document.snapshot.scale;
        }
    });
}
pub(crate) fn sync(editor: Option<Editor>) {
    CORE.with(|s| *s.borrow_mut() = editor.clone());
    if let Some((window, id)) = identity()
        && !editor
            .as_ref()
            .is_some_and(|e| e.window == window && e.id == id)
    {
        restore();
    }
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut().filter(|_| active()) {
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_output_busy(busy());
                main.set_feedback(
                    if editor.as_ref().is_some_and(|e| e.failed) {
                        crate::i18n::tr("Operation failed. Try again.")
                    } else {
                        String::new()
                    }
                    .into(),
                );
            }
            if let Some(w) = screenshot::long::editor() {
                w.set_annotations_hidden(hidden());
                w.set_image_output_busy(busy());
            }
            r.queue_render();
        }
    });
}
pub(crate) fn open(window: u64, editor: u64, image: ImageSnapshot) {
    if !current(window, editor) || active() {
        return;
    }
    let recovery = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if r.screenshot.is_some() {
            return None;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        cancel_active_gesture(r);
        let position = r
            .toolbar_state
            .main
            .get()
            .map(|w| w.window().position())
            .unwrap_or_default();
        let open = r.toolbar_state.main.get().is_some();
        let collapsed = r
            .toolbar_state
            .main
            .get()
            .is_some_and(|w| w.get_collapsed());
        let area = (r.lifecycle.popup_work_area)(
            CORE.with(|s| s.borrow().as_ref().and_then(|e| e.import_anchor))
                .or_else(crate::bridge::image_history::anchor)
                .unwrap_or(Point {
                    x: position.x,
                    y: position.y,
                }),
        )?;
        let mut origin = r.lifecycle.annotation_foreground.as_ref().map(|q| q());
        // A slow decode must not turn an unrelated external foreground into a
        // fresh activation authorization when the editor finally opens.
        if CORE.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|e| e.import_anchor.is_some())
        }) {
            let owner = r
                .toolbar_state
                .main
                .get()
                .and_then(|w| (r.lifecycle.annotation_window_token)(w.window()));
            if owner.is_none() || origin != Some(owner) {
                origin = Some(None);
            }
        }
        let document = std::mem::take(&mut r.document);
        let lifecycle = r.lifecycle.clone();
        r.close();
        r.document.session = image.document.snapshot.session.restored_image();
        r.document.session.image_source = Some(image.base.bounds);
        r.document.mode = InteractionMode::Mouse;
        r.sync_edit_scope();
        let frames = Arc::new(
            std::iter::once((*image.base).clone())
                .chain(image.samples.regions.iter().cloned())
                .collect::<Vec<_>>(),
        );
        r.lifecycle.annotation_screen_capture =
            Some(Arc::new(crate::FrozenCapture::new(frames.clone())));
        r.lifecycle.annotation_renderer = None;
        r.screenshot = Some(screenshot::Editing::image(
            editor,
            image.base.bounds,
            frames,
        ));
        Some(Recovery {
            window,
            editor,
            image,
            area,
            document,
            lifecycle,
            position,
            collapsed,
            open,
            presented: false,
            origin,
        })
    });
    let Some(recovery) = recovery else {
        unavailable(window, editor);
        return;
    };
    let base = recovery.image.base.clone();
    ACTIVE.with(|s| *s.borrow_mut() = Some(recovery));
    toggle(Instant::now());
    screenshot::long_editor::open_document(window, editor, base);
}
pub(super) fn unavailable(window: u64, editor: u64) {
    emit(Event::Completed {
        window,
        operation: editor,
        result: Err(lexift_core::Error::new("History image unavailable")),
    });
    emit(Event::EditorClosed { window, editor });
}
pub(crate) fn close() {
    if let Some((window, editor)) = identity() {
        emit(Event::EditorClosed { window, editor });
    }
}
fn restore() {
    let old = ACTIVE.with(|s| s.borrow_mut().take());
    let Some(old) = old else {
        return;
    };
    screenshot::long_editor::clear();
    screenshot::long::clear();
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.screenshot = None;
            r.close();
            r.document = old.document;
            r.lifecycle = old.lifecycle;
            r.set_mode(InteractionMode::Mouse);
            r.toolbar_state.resume = Some((old.position, old.collapsed));
        }
    });
    if old.open {
        toggle(Instant::now());
    }
    crate::bridge::schedule_idle_memory_trim();
}
/// The first actual feedback frame authorizes the one-shot focus handoff.
pub(super) fn presented() {
    if screenshot::long::editor().is_none_or(|w| w.window().take_snapshot().is_err()) {
        return;
    }
    let context = ACTIVE.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut()?;
        if s.presented {
            return None;
        }
        s.presented = true;
        Some((s.window, s.editor, s.origin))
    });
    let Some((window, editor, origin)) = context else {
        return;
    };
    if !current(window, editor) {
        return;
    }
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow().as_ref()
            && let Some(w) = screenshot::long::editor()
            && origin
                .zip(r.lifecycle.annotation_foreground.as_ref().map(|q| q()))
                .is_some_and(|(a, b)| a.is_some() && a == b)
        {
            (r.lifecycle.activate_user_requested_window)(w.window());
            w.invoke_focus_input();
        }
    });
    emit(Event::Completed {
        window,
        operation: editor,
        result: Ok(lexift_core::ports::annotation_save::SaveOutcome::Cancelled),
    });
}
pub(super) fn output(kind: lexift_core::domain::screenshot::ScreenshotOutput) {
    if image_resize::busy()
        || image_transform::busy()
        || editor_ocr::busy()
        || busy()
        || image_crop::active()
    {
        return;
    }
    let Some((window, editor)) = identity() else {
        return;
    };
    let operation = CORE.with(|s| s.borrow().as_ref().map(|e| e.operation + 1));
    let Some(operation) = operation else {
        return;
    };
    let request = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        let full = r.document.session.image_source?;
        let mut snapshot =
            export::snapshot_at(r, r.document.session.image_crop.unwrap_or(full), 1.)?;
        source_metadata(&mut snapshot);
        Some(ExportRequest {
            session: editor,
            request: operation,
            snapshot: Arc::new(snapshot),
        })
    });
    if let Some(document) = request {
        emit(Event::EditorOutput {
            window,
            editor,
            operation,
            kind,
            document,
        });
    }
}
pub(super) fn visibility(hidden: bool) {
    if let Some((window, editor)) = identity() {
        emit(Event::EditorVisibility {
            window,
            editor,
            hidden,
        });
    }
}

pub(super) fn source_dpi(point: (f32, f32)) -> Option<f32> {
    ACTIVE.with(|s| {
        s.borrow().as_ref().map(|s| {
            let snapshot = &s.image.document.snapshot;
            snapshot
                .monitors
                .iter()
                .find(|(b, _)| {
                    point.0 >= b.left as f32
                        && point.0 < b.right as f32
                        && point.1 >= b.top as f32
                        && point.1 < b.bottom as f32
                })
                .map_or(snapshot.scale, |(_, scale)| *scale)
        })
    })
}
