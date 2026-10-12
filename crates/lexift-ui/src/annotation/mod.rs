//! Owns the ephemeral annotation preview windows and per-tool UI values.
use crate::bridge::dispatch_pointer_input as dispatch;
use crate::{
    AnnotationCanvas, AnnotationPanel, AnnotationTextInput, AnnotationToolbar, AnnotationValues,
    AnnotationWatermarkInput,
    bridge::{
        PassiveWindowPreparation, PopupDismissCause, PopupPointerInput, WindowLifecycleCallbacks,
    },
    placement,
};
#[cfg(test)]
use lexift_core::domain::annotation::{TextBackground, TextOutline};
use lexift_core::domain::{
    annotation::{
        Bounds, EditScope, Endpoint, Extra, Hit, Kind, MagnifierConnector, Object, ObjectId,
        SequenceAnnotation, SequenceFormat, SequenceShape, Session, Style, TextAnnotation,
        Watermark, magnifier_output_for_source,
    },
    geometry::{Point, Rect},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

mod canvas;
mod choice;
use canvas::{Bootstrap as CanvasBootstrap, Layer as CanvasLayer};
mod color;
mod cursor;
mod drag_timing;
mod drawing;
#[cfg(test)]
use drawing::arrow_drag_commits;
#[cfg(test)]
use drawing::polyline_preview;
use drawing::{ArrowPending, PendingState, Polyline};
#[cfg(test)]
mod collapse_tests;
mod editor;
#[path = "erase.rs"]
mod eraser;
mod export;
mod toolbar;
mod tools;
#[cfg(test)]
use editor::CaretBlink;
#[cfg(test)]
use editor::viewport as text_editor_viewport;
use editor::{TextEdit, preview as editor_preview};
#[cfg(test)]
use parameters::preserve_text_dimensions;
#[cfg(test)]
use std::cell::Cell;
#[path = "gesture.rs"]
mod gestures;
use gestures::{Action, Active, Gesture};
mod focus;
mod font_service;
mod font_workflow;
mod fonts;
mod frame;
#[path = "mosaic_session.rs"]
mod mosaic;
mod numeric;
mod panel;
#[path = "parameter_transaction.rs"]
mod parameter_edit;
mod parameters;
use parameters::endpoint;
mod presenter;
mod watermark;
mod window;
#[cfg(test)]
use parameters::defaults;
use parameters::text_dimensions_to_values;

type EventHandler = Rc<dyn Fn(lexift_core::AppEvent)>;
thread_local! { static EXPORT_HANDLER: RefCell<Option<EventHandler>> = const { RefCell::new(None) }; }
pub(crate) fn set_handler(handler: EventHandler) {
    EXPORT_HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}
fn emit_export(event: lexift_core::domain::annotation_export::ExportEvent) {
    let handler = EXPORT_HANDLER.with(|h| h.borrow().clone());
    if let Some(handler) = handler {
        handler(lexift_core::AppEvent::AnnotationExport(event));
    }
}
fn defer_export(event: lexift_core::domain::annotation_export::ExportEvent) {
    slint::Timer::single_shot(Duration::ZERO, move || emit_export(event));
}
pub(crate) fn sync_export(state: &lexift_core::domain::annotation_export::ExportState) {
    export::sync(state);
}

thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

const GROUPS: [&[i32]; 7] = [
    &[0, 2],
    &[3, 4],
    &[5, 6, 7],
    &[8, 9],
    &[10],
    &[11, 12],
    &[13],
];
const NAMES: [&str; 14] = [
    "Geometry",
    "Ellipse",
    "Spotlight",
    "Pencil",
    "Highlighter",
    "Arrow",
    "Polyline",
    "Magnifier",
    "Text",
    "Watermark",
    "Sequence",
    "Mosaic",
    "Mosaic sticker",
    "Eraser",
];

fn fields(tool: i32) -> Vec<i32> {
    match tool {
        0 | 1 => vec![1, 4, 0, 2, 3],
        2 => vec![1, 5],
        3 => vec![0],
        4 => vec![0, 6],
        5 | 6 => vec![0, 2],
        7 => vec![1, 0, 2, 7, 8, 9, 10],
        8 => vec![11, 12, 13],
        9 => vec![11, 14, 12, 13, 5],
        10 => vec![15, 16, 13],
        11 | 12 => vec![17, 0, 18, 5],
        _ => vec![17, 0],
    }
}

pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    REGISTRY.with(|slot| {
        *slot.borrow_mut() = Some(Registry {
            toolbar_state: ToolbarState {
                main: window::Slot::default(),
                panel: window::Slot::default(),
                watermark: window::Slot::default(),
                menus: choice::State::default(),
                toolbar_owner: None,
                toolbar_press: None,
                toolbar_drag: None,
                panel_preparation: Default::default(),
                group: None,
                layout_timer: slint::Timer::default(),
            },
            canvas_state: CanvasState {
                canvases: Vec::new(),
                pending_displays: Vec::new(),
                bootstrap_cursor: 0,
                annotation_ready: false,
                cold_open_measured: false,
                cold_open_timing: None,
            },
            render_state: RenderState {
                scene_compiler: Default::default(),
                scene_revision: 0,
                native_presenter: Default::default(),
                mosaic: mosaic::State::default(),
                frame_tasks: frame::Tasks::default(),
                magnifier_sync: MagnifierSync::Cold,
                magnifier_sync_revision: 0,
                first_magnifier_timing: None,
                first_magnifier_measured: false,
                magnifier_timer: slint::Timer::default(),
            },
            export: export::State::default(),
            editor: editor::State::default(),
            fonts: fonts::Directory::default(),
            focus: focus::Coordinator::default(),
            session: {
                let mut session = Session::new();
                session.set_edit_scope(EditScope::Geometry);
                session
            },
            hovered: None,
            gesture: None,
            pending_drawing: PendingState::default(),
            mode: InteractionMode::Tool(0),
            parameter_edit: Default::default(),
            deferred_parameters: parameters::Pending::default(),
            pending_edit_panel: false,
            selected: [0, 4, 7, 9, 10, 11, 13],
            parameter_values: parameters::State::new(),
            prepare,
            lifecycle,
            timer: slint::Timer::default(),
            generation: 0,
            status: "Unavailable".into(),
        })
    });
}

pub(crate) fn status() -> String {
    REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| {
                if let Some(detail) = r.status.strip_prefix("Unavailable: ") {
                    crate::i18n::detail("Unavailable: {0}", detail)
                } else {
                    crate::i18n::tr(&r.status)
                }
            })
            .unwrap_or_else(|| crate::i18n::tr("Unavailable"))
    })
}
pub(crate) fn set_status(value: String) {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.status = value;
        }
    });
}
pub(crate) fn is_open() -> bool {
    REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.toolbar_state.main.get().is_some())
    })
}

pub(crate) fn refresh_language() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            choice::close(r, choice::CloseReason::OwnerRetired);
            if !r.fonts.names().is_empty() {
                r.reset_font_defaults();
            }
            if let Some(panel) = r.toolbar_state.panel.get()
                && !panel.get_menu()
            {
                panel.set_values(r.values(panel.get_tool() as usize).clone());
            }
            if r.toolbar_state.panel.get().is_some_and(|p| p.get_menu()) {
                r.close_panel();
            } else if let Some(panel) = r.toolbar_state.panel.get() {
                panel.set_heading(crate::i18n::tr(NAMES[panel.get_tool() as usize]).into());
                panel.set_language_revision(panel.get_language_revision().wrapping_add(1));
            }
        }
    });
}

pub(crate) fn apply_theme() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow().as_ref() {
            if let Some(w) = r.toolbar_state.main.get() {
                crate::theme::apply(w);
            }
            if let Some(w) = r.toolbar_state.panel.get() {
                crate::theme::apply(w);
            }
            if let Some(menu) = &r.toolbar_state.menus.current {
                crate::theme::apply(&menu.window);
            }
        }
    });
}

fn sync_history_controls(r: &Registry) {
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_can_undo(r.session.can_undo());
    }
    if let Some(panel) = r.toolbar_state.panel.get() {
        panel.set_can_clear(r.session.has_content());
    }
}

mod text_workflow;
use text_workflow::*;

mod canvas_lifecycle;
use canvas_lifecycle::*;

mod parameter_workflow;
use parameter_workflow::*;

mod focus_workflow;
use focus_workflow::*;

mod canvas_input;
use canvas_input::*;

mod render_workflow;
use render_workflow::*;

mod toolbar_workflow;
use toolbar_workflow::*;

#[cfg(test)]
mod tests;

pub(crate) mod export_render;
pub(crate) mod fingerprint;
pub(crate) mod path;
pub(crate) mod render;
pub(crate) mod scene;
pub(crate) mod sequence;
pub(crate) mod signature;
pub(crate) mod sticker;
pub(crate) mod text;

mod state;
use state::*;
mod lifecycle;
#[cfg(test)]
use lifecycle::escape;
use lifecycle::{backspace_polyline, escape_from, finish_polyline};
pub(crate) use lifecycle::{close, shutdown, toggle};
