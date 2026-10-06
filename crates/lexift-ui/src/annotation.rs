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

#[path = "annotation_canvas.rs"]
mod canvas;
#[path = "annotation_choice.rs"]
mod choice;
use canvas::{Bootstrap as CanvasBootstrap, Layer as CanvasLayer};
#[path = "annotation_color.rs"]
mod color;
#[path = "annotation_cursor.rs"]
mod cursor;
#[path = "annotation_drag_timing.rs"]
mod drag_timing;
#[path = "annotation_drawing.rs"]
mod drawing;
#[cfg(test)]
use drawing::arrow_drag_commits;
#[cfg(test)]
use drawing::polyline_preview;
use drawing::{ArrowPending, PendingState, Polyline};
#[path = "annotation_editor.rs"]
mod editor;
#[path = "annotation_erase.rs"]
mod eraser;
#[path = "annotation_tools.rs"]
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
#[path = "annotation_gesture.rs"]
mod gestures;
use gestures::{Action, Active, Gesture};
#[path = "annotation_focus.rs"]
mod focus;
#[path = "annotation_fonts.rs"]
mod fonts;
#[path = "annotation_frame.rs"]
mod frame;
#[path = "annotation_mosaic_session.rs"]
mod mosaic;
#[path = "annotation_numeric.rs"]
mod numeric;
#[path = "annotation_panel.rs"]
mod panel;
#[path = "annotation_parameter_transaction.rs"]
mod parameter_edit;
#[path = "annotation_parameters.rs"]
mod parameters;
use parameters::endpoint;
#[path = "annotation_presenter.rs"]
mod presenter;
#[path = "annotation_watermark.rs"]
mod watermark;
#[path = "annotation_window.rs"]
mod window;
#[cfg(test)]
use parameters::defaults;
use parameters::text_dimensions_to_values;

thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

struct Registry {
    scene_compiler: crate::annotation_scene::SceneCompiler,
    scene_revision: u64,
    native_presenter: presenter::Presenter,
    mosaic: mosaic::State,
    main: window::Slot<AnnotationToolbar>,
    panel: window::Slot<AnnotationPanel>,
    editor: editor::State,
    watermark: window::Slot<AnnotationWatermarkInput>,
    fonts: fonts::Directory,
    focus: focus::Coordinator,
    menus: choice::State,
    canvases: Vec<CanvasLayer>,
    pending_displays: Vec<Rect>,
    bootstrap_cursor: usize,
    annotation_ready: bool,
    cold_open_measured: bool,
    cold_open_timing: Option<ColdOpenTiming>,
    toolbar_owner: Option<(usize, u64, u64)>,
    session: Session,
    hovered: Option<usize>,
    gesture: Option<Active>,
    pending_drawing: PendingState,
    mode: InteractionMode,
    frame_tasks: frame::Tasks,
    magnifier_sync: MagnifierSync,
    magnifier_sync_revision: u64,
    first_magnifier_timing: Option<FirstMagnifierTiming>,
    first_magnifier_measured: bool,
    parameter_edit: parameter_edit::Transaction,
    deferred_parameters: parameters::Pending,
    pending_edit_panel: bool,
    selected: [i32; 7],
    parameter_values: parameters::State,
    group: Option<usize>,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    timer: slint::Timer,
    layout_timer: slint::Timer,
    magnifier_timer: slint::Timer,
    generation: u64,
    status: String,
}

#[derive(Default)]
struct ColdOpenTiming {
    requested_at: Option<Instant>,
    ui_queue: Duration,
    toolbar_create: Duration,
    display_query: Duration,
    canvas_create: Duration,
    canvas_prepare: Duration,
    canvas_show: Duration,
    canvas_native_complete: Duration,
    canvas_retry: Duration,
    raster: Duration,
    native_present: Duration,
    toolbar_prepare: Duration,
    toolbar_place: Duration,
    toolbar_show: Duration,
    toolbar_native_complete: Duration,
    toolbar_finish: Duration,
    toolbar_retry: Duration,
    toolbar_retries: u8,
    per_screen: Vec<CanvasColdTiming>,
    largest_stage: Duration,
    largest_callback: Duration,
}

#[derive(Default)]
struct CanvasColdTiming {
    create: Duration,
    prepare: Duration,
    show: Duration,
    native_complete: Duration,
    retry: Duration,
    retries: u8,
    raster: Duration,
    present: Duration,
}

impl ColdOpenTiming {
    fn report(self, outcome: &'static str, screens: usize) {
        let total = self.requested_at.map_or(Duration::ZERO, |at| at.elapsed());
        let per_screen = self
            .per_screen
            .iter()
            .enumerate()
            .map(|(index, screen)| {
                format!(
                    "{index}:create={} prepare={} show={} native={} retry={}({}) raster={} present={}",
                    screen.create.as_millis(),
                    screen.prepare.as_millis(),
                    screen.show.as_millis(),
                    screen.native_complete.as_millis(),
                    screen.retry.as_millis(),
                    screen.retries,
                    screen.raster.as_millis(),
                    screen.present.as_millis(),
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        tracing::info!(
            outcome,
            screens,
            total_ms = total.as_millis(),
            ui_queue_ms = self.ui_queue.as_millis(),
            toolbar_create_ms = self.toolbar_create.as_millis(),
            display_query_ms = self.display_query.as_millis(),
            canvas_create_ms = self.canvas_create.as_millis(),
            canvas_prepare_ms = self.canvas_prepare.as_millis(),
            canvas_show_ms = self.canvas_show.as_millis(),
            canvas_native_complete_ms = self.canvas_native_complete.as_millis(),
            canvas_retry_ms = self.canvas_retry.as_millis(),
            raster_ms = self.raster.as_millis(),
            native_present_ms = self.native_present.as_millis(),
            toolbar_prepare_ms = self.toolbar_prepare.as_millis(),
            toolbar_place_ms = self.toolbar_place.as_millis(),
            toolbar_show_ms = self.toolbar_show.as_millis(),
            toolbar_native_complete_ms = self.toolbar_native_complete.as_millis(),
            toolbar_finish_ms = self.toolbar_finish.as_millis(),
            toolbar_retry_ms = self.toolbar_retry.as_millis(),
            toolbar_retries = self.toolbar_retries,
            per_screen,
            largest_stage_ms = self.largest_stage.as_millis(),
            largest_callback_ms = self.largest_callback.as_millis(),
            "First annotation open timing"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MagnifierSync {
    Cold,
    Pending(u64),
    Live,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MagnifierFrameAction {
    Clear,
    Defer(u64),
    Sync,
    Wait,
}

fn magnifier_frame_action(
    state: &mut MagnifierSync,
    revision: &mut u64,
    has_views: bool,
) -> MagnifierFrameAction {
    if !has_views {
        *revision = revision.wrapping_add(1);
        *state = MagnifierSync::Cold;
        return MagnifierFrameAction::Clear;
    }
    match state {
        MagnifierSync::Cold => {
            *revision = revision.wrapping_add(1);
            *state = MagnifierSync::Pending(*revision);
            MagnifierFrameAction::Defer(*revision)
        }
        MagnifierSync::Live => MagnifierFrameAction::Sync,
        MagnifierSync::Pending(_) | MagnifierSync::Failed => MagnifierFrameAction::Wait,
    }
}

struct FirstMagnifierTiming {
    first_move: Instant,
    frame_start: Option<Instant>,
    raster: Duration,
    present: Duration,
    border_submitted: Option<Instant>,
}

impl FirstMagnifierTiming {
    fn new() -> Self {
        Self {
            first_move: Instant::now(),
            frame_start: None,
            raster: Duration::ZERO,
            present: Duration::ZERO,
            border_submitted: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InteractionMode {
    Mouse,
    Tool(usize),
}

impl InteractionMode {
    fn is_mouse(self) -> bool {
        matches!(self, Self::Mouse)
    }

    fn selected_group(self) -> i32 {
        match self {
            Self::Mouse => -1,
            Self::Tool(group) => group as i32,
        }
    }
}

fn reset_text_caret(r: &mut Registry) {
    let Some(edit) = r.editor.edit_mut() else {
        return;
    };
    let (generation, id) = (r.generation, edit.id);
    edit.reset_caret(move || tick_text_caret(generation, id));
}

fn tick_text_caret(generation: u64, id: u64) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && text_edit_matches(r, id))
        else {
            return false;
        };
        let composing = r
            .editor
            .window()
            .is_some_and(|e| !e.get_preedit().is_empty());
        let edit = r.editor.edit_mut().unwrap();
        edit.caret
            .tick(composing || edit.selection_anchor.is_some())
    });
    if changed {
        schedule_render(generation);
    }
}

fn text_edit_matches(r: &Registry, id: u64) -> bool {
    r.editor.edit().is_some_and(|edit| edit.id == id)
}
fn editing_sequence(r: &Registry) -> bool {
    r.editor
        .edit()
        .and_then(|e| e.index(&r.session))
        .and_then(|i| r.session.objects.get(i))
        .is_some_and(|o| o.kind == Kind::Sequence)
}

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
            scene_compiler: Default::default(),
            scene_revision: 0,
            native_presenter: Default::default(),
            mosaic: mosaic::State::default(),
            main: window::Slot::default(),
            panel: window::Slot::default(),
            editor: editor::State::default(),
            watermark: window::Slot::default(),
            fonts: fonts::Directory::default(),
            focus: focus::Coordinator::default(),
            menus: choice::State::default(),
            canvases: Vec::new(),
            pending_displays: Vec::new(),
            bootstrap_cursor: 0,
            annotation_ready: false,
            cold_open_measured: false,
            cold_open_timing: None,
            toolbar_owner: None,
            session: {
                let mut session = Session::new();
                session.set_edit_scope(EditScope::Geometry);
                session
            },
            hovered: None,
            gesture: None,
            pending_drawing: PendingState::default(),
            mode: InteractionMode::Tool(0),
            frame_tasks: frame::Tasks::default(),
            magnifier_sync: MagnifierSync::Cold,
            magnifier_sync_revision: 0,
            first_magnifier_timing: None,
            first_magnifier_measured: false,
            parameter_edit: Default::default(),
            deferred_parameters: parameters::Pending::default(),
            pending_edit_panel: false,
            selected: [0, 4, 7, 9, 10, 11, 13],
            parameter_values: parameters::State::new(),
            group: None,
            prepare,
            lifecycle,
            timer: slint::Timer::default(),
            layout_timer: slint::Timer::default(),
            magnifier_timer: slint::Timer::default(),
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
    REGISTRY.with(|s| s.borrow().as_ref().is_some_and(|r| r.main.get().is_some()))
}

impl Registry {
    fn scroll_numeric(
        &mut self,
        target: parameter_edit::Target,
        parameter: numeric::Parameter,
        delta: f32,
    ) {
        let values = self.pending_parameter_values(target);
        if self.parameter_target(target.tool) != target || !parameter.enabled(target.tool, values) {
            return;
        }
        if let Some(value) = parameter.step(parameter.read(values), target.tool, delta) {
            self.apply_menu_edit(target, parameters::Edit::Numeric(parameter, value));
            choice::sync_numeric(self);
        }
    }
    /// Child teardown can synchronously forward native input to any session window.
    /// Keep those callbacks outside the coordinator borrow without retiring owners.
    fn pause_native_pointer_inputs(&self) -> Vec<window::PointerPause> {
        let mut guards = vec![
            self.main.input().pause_pointer(),
            self.panel.input().pause_pointer(),
            self.watermark.input().pause_pointer(),
        ];
        guards.extend(
            self.canvases
                .iter()
                .map(|layer| layer.input().pause_pointer()),
        );
        if let Some(edit) = self.editor.edit() {
            guards.push(edit.pause_pointer());
        }
        guards
    }

    fn parameter_entry_active(&self, tool: i32) -> bool {
        !self.mode.is_mouse()
            && parameters::compatible_tools(
                self.selected[self.mode.selected_group() as usize] as usize,
                tool as usize,
            )
    }
    fn parameter_object(&self, tool: usize) -> Option<ObjectId> {
        self.session
            .selected
            .and_then(|index| self.session.objects.get(index))
            .filter(|object| {
                self.session.can_edit(object)
                    && parameters::compatible_tools(object.kind.tool(), tool)
            })
            .map(|object| object.id)
    }
    fn arrow_pending(&self) -> Option<&ArrowPending> {
        self.pending_drawing.arrow()
    }
    fn polyline(&self) -> Option<&Polyline> {
        self.pending_drawing.polyline()
    }
    fn gesture(&self) -> Option<Gesture> {
        self.gesture.as_ref().map(Active::route)
    }
    #[cfg(test)]
    fn sequence_placement(&self) -> Option<&gestures::SequencePlacement> {
        self.gesture.as_ref().and_then(Active::sequence)
    }
    fn geometry_drag(&self) -> Option<&gestures::GeometryDrag> {
        self.gesture.as_ref().and_then(Active::geometry)
    }
    fn text_handle(&self) -> Option<&gestures::TextHandle> {
        self.gesture.as_ref().and_then(Active::text)
    }
    #[cfg(test)]
    fn drawing_press(&self) -> Option<&drawing::Press> {
        self.gesture.as_ref().and_then(Active::drawing)
    }
    fn values(&self, tool: usize) -> &AnnotationValues {
        if tool == 8
            && let Some(values) = self.editor.draft_values()
        {
            return values;
        }
        self.parameter_values
            .values(tool, self.parameter_object(tool))
    }
    fn parameter_target(&self, tool: i32) -> parameter_edit::Target {
        parameter_edit::Target {
            panel: self.panel.revision(),
            tool,
            object: self.parameter_object(tool as usize),
            editor: self.editor.edit().map(|edit| edit.id),
            watermark: if tool == 9 {
                self.parameter_values.watermark_revision()
            } else {
                0
            },
            projection: if tool != 9 {
                self.parameter_values.projection_revision()
            } else {
                0
            },
        }
    }
    #[cfg(test)]
    fn edit_values(&mut self, tool: usize) -> &mut AnnotationValues {
        if tool == 8 && self.editor.draft_values().is_some() {
            return self.editor.draft_values_mut().unwrap();
        }
        let object = self.parameter_object(tool);
        self.parameter_values.edit(tool, object)
    }
    fn replace_values(&mut self, tool: usize, values: AnnotationValues) {
        if tool == 8
            && let Some(draft) = self.editor.draft_values_mut()
        {
            *draft = values;
            return;
        }
        let object = self.parameter_object(tool);
        self.parameter_values.replace(tool, object, values);
    }
    fn pending_parameter_values(&self, target: parameter_edit::Target) -> &AnnotationValues {
        self.deferred_parameters
            .values(target)
            .unwrap_or_else(|| self.values(target.tool as usize))
    }

    /// Apply a decoded popup result to current parameters. The popup does not
    /// own document transactions, font preferences or the editor's draft.
    fn apply_menu_edit(&mut self, target: parameter_edit::Target, edit: parameters::Edit) {
        if self.parameter_target(target.tool) != target
            || !self
                .panel
                .get()
                .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == target.tool)
        {
            return;
        }
        let tool = target.tool as usize;
        let mut values = self.pending_parameter_values(target).clone();
        edit.apply(&mut values);
        if let parameters::Edit::Choice(parameters::ChoiceTarget::Font, index) = &edit
            && let Some(font_tool) = fonts::Tool::from_index(tool)
        {
            self.fonts.choose(font_tool, *index);
            self.parameter_values.defaults_mut(tool).font = *index;
        }
        let changed = apply_geometry_values(self, target, values);
        if let Some(panel) = self.panel.get() {
            panel.set_values(self.pending_parameter_values(target).clone());
        }
        if changed.is_some() {
            // Document and draft changes are applied; the owned timer renders after this borrow.
            self.queue_render();
        }
    }
    fn queue_render(&mut self) {
        cursor::refresh(self);
        self.frame_tasks
            .queue_render(self.generation, render_canvases);
    }

    fn ensure_fonts(&mut self) {
        if self.fonts.ensure() {
            self.reset_font_defaults();
        }
    }

    fn preferred_font(&self, tool: usize) -> String {
        fonts::Tool::from_index(tool)
            .map(|tool| self.fonts.preferred(tool, crate::i18n::language()))
            .unwrap_or_default()
    }

    /// Compute UI protection from the active document; the directory never owns objects.
    fn reset_font_defaults(&mut self) {
        for tool in 8..=10 {
            let editing = self.editor.edit().is_some_and(|e| {
                if tool == 8 {
                    !editing_sequence(self)
                        && e.index(&self.session).is_none_or(|i| {
                            self.session
                                .objects
                                .get(i)
                                .is_some_and(|o| o.kind == Kind::Text)
                        })
                } else {
                    tool == 10 && editing_sequence(self)
                }
            });
            if editing {
                continue;
            }
            let name = self.preferred_font(tool);
            if let Some(index) = self
                .fonts
                .names()
                .iter()
                .position(|f| f.eq_ignore_ascii_case(&name))
            {
                self.parameter_values.defaults_mut(tool).font = index as i32;
            }
        }
    }

    fn sync_mode_display(&self) {
        if let Some(main) = self.main.get() {
            main.set_operate(self.mode.is_mouse());
            main.set_active_group(self.mode.selected_group());
        }
    }

    /// Keep selection and parameter projection within the active toolbar entry.
    fn sync_edit_scope(&mut self) {
        let scope = if self.mode.is_mouse() {
            EditScope::None
        } else {
            tools::scope(self.selected[self.mode.selected_group() as usize] as usize)
        };
        let previous = self.session.selected;
        let hovered = self.hovered.take();
        self.session.set_edit_scope(scope);
        if previous != self.session.selected {
            deselect_object(self);
        }
        if previous != self.session.selected || hovered.is_some() {
            self.queue_render();
        }
    }

    /// Apply the native input policy before exposing a new mode in the toolbar.
    fn set_mode(&mut self, mode: InteractionMode) -> bool {
        if self.mode == mode {
            self.sync_edit_scope();
            cursor::refresh(self);
            self.sync_mode_display();
            return true;
        }
        cancel_active_gesture(self);
        self.finish_style_edit();
        self.close_watermark_input();
        if self.editor.window().is_some() {
            finish_text_edit_registry(self, true);
        }
        if self.mode.is_mouse() != mode.is_mouse() {
            let mut changed = Vec::new();
            for (index, layer) in self.canvases.iter().enumerate() {
                if !(self.lifecycle.annotation_click_through)(
                    layer.window.window(),
                    mode.is_mouse(),
                ) {
                    for index in changed {
                        let layer: &CanvasLayer = &self.canvases[index];
                        (self.lifecycle.annotation_click_through)(
                            layer.window.window(),
                            self.mode.is_mouse(),
                        );
                    }
                    return false;
                }
                changed.push(index);
            }
        }
        self.mode = mode;
        self.sync_edit_scope();
        if mode != InteractionMode::Tool(5) {
            mosaic::cancel_pending(self);
        }
        self.hovered = None;
        cursor::refresh(self);
        self.sync_mode_display();
        true
    }

    /// A variant change retires the same capture as a group change, including
    /// when set_mode is a no-op because this group was already active.
    fn select_tool_variant(&mut self, group: usize, tool: i32) -> bool {
        if !self.set_mode(InteractionMode::Tool(group)) {
            return false;
        }
        if self.selected[group] != tool {
            cancel_active_gesture(self);
            self.finish_style_edit();
            self.close_watermark_input();
            if self.editor.window().is_some() {
                finish_text_edit_registry(self, true);
            }
        }
        self.close_panel();
        self.selected[group] = tool;
        self.sync_edit_scope();
        cursor::refresh(self);
        if let Some(main) = self.main.get() {
            main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(self))));
        }
        true
    }

    /// Replay accepted toolbar input before geometry projection changes its identity.
    fn flush_deferred_parameters(&mut self) {
        self.replay_deferred_parameters(None);
    }

    /// Cancelling placement restores selection before defaults are replayed. Keep
    /// the accepted default target without rebinding it to the restored object.
    fn replay_deferred_parameters(&mut self, cancelled: Option<Gesture>) {
        if self.gesture().is_some_and(Gesture::blocks_parameter_edits) {
            return;
        }
        for (target, values) in self.deferred_parameters.take() {
            if !self.parameter_entry_active(target.tool) {
                continue;
            }
            let context = parameters::ReplayContext {
                current: self.parameter_target(target.tool),
                panel_tool: self
                    .panel
                    .get()
                    .filter(|panel| !panel.get_menu())
                    .map(|panel| panel.get_tool()),
                bound_object: self.parameter_values.bound_object(),
                cancelled_sequence: matches!(cancelled, Some(Gesture::PlaceSequence)),
            };
            match context.resolve(target) {
                parameters::Replay::Current => {
                    apply_geometry_values(self, target, values);
                }
                parameters::Replay::SequenceDefaults => {
                    apply_parameter_values(self, target, values);
                }
                parameters::Replay::Discard => {}
            }
        }
    }

    /// Finish parameter history before a geometry snapshot takes ownership.
    /// Keeping the handoff here prevents callers from depending on pointer routing.
    fn begin_document_gesture(&mut self) {
        self.finish_style_edit();
        self.session.begin_drag();
    }

    fn finish_style_edit(&mut self) {
        if self.parameter_edit.finish(&mut self.session)
            && let Some(main) = self.main.get()
        {
            main.set_can_undo(self.session.can_undo());
            if let Some(panel) = self.panel.get() {
                panel.set_can_clear(self.session.has_content());
            }
        }
    }

    /// Invalidate callbacks before releasing the native editor and its input bridge.
    fn close_watermark_input(&mut self) {
        // Child hook teardown can emit native input before the owner is destroyed.
        self.watermark.retire_input();
        self.focus.cancel_for(focus::Target::Watermark {
            editor: self.watermark.revision(),
        });
        choice::close_watermark(self);
        self.watermark.clear();
        if self
            .native_presenter
            .exclusions_changed(&capture_exclusions(self))
        {
            self.queue_render();
        }
    }

    fn close_panel(&mut self) {
        // Reject reentrant owner events before dropping child windows and hooks.
        self.panel.retire_input();
        self.deferred_parameters.clear();
        self.focus.cancel();
        self.parameter_edit.set_dragging(false);
        choice::close(self, choice::CloseReason::OwnerRetired);
        self.finish_style_edit();
        if let Some(panel) = self.panel.get()
            && !panel.get_menu()
        {
            let tool = panel.get_tool() as usize;
            if self.parameter_values.binding().get() == self.parameter_object(tool) {
                self.replace_values(tool, panel.get_values());
            }
        }
        self.panel.clear();
        if self
            .native_presenter
            .exclusions_changed(&capture_exclusions(self))
        {
            self.queue_render();
        }
        self.group = None;
        self.sync_mode_display();
        // Closing its menu may request focus; that request belongs to the retired panel.
        self.focus.cancel();
    }
    fn close(&mut self) {
        drag_timing::clear();
        // Invalidate queued input before native teardown can emit callbacks.
        // No parameter replay or editor restoration belongs to a dying session.
        self.generation = self.generation.wrapping_add(1);
        self.main.retire_input();
        self.panel.retire_input();
        self.watermark.retire_input();
        self.editor.retire_input();
        self.menus.retire_input();
        for canvas in &self.canvases {
            canvas.retire_input();
        }
        self.focus.cancel();
        if let Some(active) = self.gesture.take() {
            let _ = active.cancel(&mut self.session);
        }
        self.pending_drawing.cancel();
        self.native_presenter
            .close(self.lifecycle.annotation_renderer.as_ref());
        self.scene_compiler.clear();
        self.scene_revision = 0;
        if let Some(timing) = self.cold_open_timing.take() {
            timing.report("cancelled", self.pending_displays.len());
            self.cold_open_measured = true;
        }
        self.close_panel();
        self.editor.clear();
        self.close_watermark_input();
        (self.lifecycle.annotation_magnifier_sync)(&[], &[]);
        if let Some(capture) = &self.lifecycle.annotation_screen_capture {
            capture.clear();
        }
        self.mosaic = mosaic::State::default();
        crate::annotation_sticker::clear_cache();
        self.timer.stop();
        self.layout_timer.stop();
        self.magnifier_timer.stop();
        self.frame_tasks.clear();
        self.annotation_ready = false;
        self.pending_displays.clear();
        self.bootstrap_cursor = 0;
        self.magnifier_sync = MagnifierSync::Cold;
        self.magnifier_sync_revision = self.magnifier_sync_revision.wrapping_add(1);
        self.first_magnifier_timing = None;
        self.first_magnifier_measured = false;
        self.pending_edit_panel = false;
        self.mode = InteractionMode::Tool(0);
        self.session.clear();
        self.toolbar_owner = None;
        self.canvases.clear();
        self.main.clear();
        self.parameter_values = parameters::State::new();
        if !self.fonts.names().is_empty() {
            self.reset_font_defaults();
        }
        self.selected = [0, 4, 7, 9, 10, 11, 13];
        self.hovered = None;
        self.sync_edit_scope();
    }
}

fn finish_text_edit_registry(r: &mut Registry, commit: bool) {
    r.focus.cancel();
    let Some(closed) = r.editor.take() else {
        return;
    };
    choice::close_if_context_changed(r);
    if let Some(panel) = r
        .panel
        .get()
        .filter(|panel| !panel.get_menu() && panel.get_tool() == 8)
    {
        panel.set_values(r.values(8).clone());
    }
    // Retiring the editor changes the canvas even when no document edit is committed.
    // Every caller, including native preparation failures, must invalidate that projection.
    r.queue_render();
    let target_kind = closed
        .edit
        .index(&r.session)
        .map(|index| r.session.objects[index].kind);
    let Some(change) = closed.resolve(commit, target_kind) else {
        return;
    };
    match change {
        editor::EditChange::Caption { target, text } => {
            let Some(index) = r.session.object_index(target) else {
                return;
            };
            if !r.session.can_edit(&r.session.objects[index]) {
                return;
            }
            r.session.selected = Some(index);
            r.session.update_selected(|object| {
                if let Extra::Sequence(sequence) = &mut object.extra {
                    sequence.caption = text;
                }
                crate::annotation_sequence::layout(object);
            });
            if let Some(main) = r.main.get() {
                main.set_can_undo(r.session.can_undo());
                if let Some(panel) = r.panel.get() {
                    panel.set_can_clear(r.session.has_content());
                }
            }
            return;
        }
        editor::EditChange::Text {
            target,
            text,
            bounds,
        } => {
            let Some(index) = r.session.object_index(target) else {
                return;
            };
            if !r.session.can_edit(&r.session.objects[index]) {
                return;
            }
            r.session.selected = Some(index);
            r.session.update_selected(|object| {
                object.bounds = bounds;
                object.extra = Extra::Text(text);
            });
        }
        editor::EditChange::New(object) => {
            r.session.add(*object);
        }
    }
    restore_parameter_projection(r);
    if let Some(main) = r.main.get() {
        main.set_can_undo(r.session.can_undo());
    }
}

fn finish_text_edit(generation: u64, commit: bool) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(r) = slot.as_mut().filter(|r| r.generation == generation) {
            finish_text_edit_registry(r, commit);
        }
    });
}

fn finish_matching_text_edit(generation: u64, id: u64, commit: bool) {
    if REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|r| r.generation == generation && text_edit_matches(r, id))
    }) {
        finish_text_edit(generation, commit);
    }
}

/// Native and forwarded mouse events enter through the same Slint TouchArea.
fn editor_pointer(generation: u64, id: u64, event: i32, x: f32, y: f32) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && text_edit_matches(r, id))
        else {
            return;
        };
        let Some(object) = editing_text_object(r) else {
            return;
        };
        let editor = r.editor.window().unwrap();
        let origin = editor.window().position();
        let dpi = editor.window().scale_factor();
        let point = (origin.x as f32 + x * dpi, origin.y as f32 + y * dpi);
        let handle = if editing_sequence(r) {
            None
        } else {
            text_handle_at(&object, point, dpi)
        };
        editor.set_control_hover(
            handle.is_some() || matches!(r.gesture(), Some(Gesture::TextHandle { .. })),
        );
        if matches!(r.gesture(), Some(Gesture::TextHandle { editing: true, .. })) {
            if event == 1 || event == 2 {
                update_text_handle(r, point);
            }
            if event == 2 {
                finish_text_handle(r, point);
            }
            if event == 3 {
                cancel_active_gesture(r);
            }
            r.queue_render();
            return;
        }
        if event == 0
            && let Some(handle) = handle
        {
            begin_text_handle(r, object, handle, point, dpi, true);
            return;
        }
        let offset = crate::annotation_text::caret_at(&object, point);
        if event == 0 {
            restore_editor_focus(r);
        }
        let (editor, edit) = r.editor.parts_mut().unwrap();
        let selecting = event == 0 || edit.selection_anchor.is_some();
        match event {
            0 => {
                edit.selection_anchor = Some(offset);
                editor.invoke_place_selection(offset as i32, offset as i32);
            }
            1 | 2 => {
                if let Some(anchor) = edit.selection_anchor {
                    editor.invoke_place_selection(anchor as i32, offset as i32);
                }
                if event == 2 {
                    edit.selection_anchor = None;
                }
            }
            _ => edit.selection_anchor = None,
        }
        if selecting {
            reset_text_caret(r);
            r.queue_render();
        }
    });
}

/// Reads the latest input state: IME commit can change value and preedit together.
fn resize_text_editor(generation: u64, id: u64) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        if !text_edit_matches(r, id) {
            return None;
        }
        let (editor, edit) = r.editor.parts_mut()?;
        let mut text = edit.original.clone();
        text.content = editor_preview(editor).0;
        let b = if let Some(mut object) = edit
            .index(&r.session)
            .and_then(|i| r.session.objects.get(i))
            .filter(|o| o.kind == Kind::Sequence)
            .cloned()
        {
            if let Extra::Sequence(s) = &mut object.extra {
                s.caption = text.clone();
            }
            crate::annotation_sequence::layout(&mut object);
            let b = object.sequence_caption().unwrap().bounds;
            edit.at = (b.left, b.top);
            b
        } else {
            crate::annotation_text::text_bounds(edit.at, &text, edit.scale)
        };
        edit.bounds = b;
        edit.laid_out_content = text.content.clone();
        place_text_editor(editor, b, edit.scale, &text, &r.canvases);
        reset_text_caret(r);
        Some(r.generation)
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
}

/// Canvas owners supply desktop bounds; the editor owns component placement.
fn place_text_editor(
    editor: &AnnotationTextInput,
    bounds: Bounds,
    scale: f32,
    text: &TextAnnotation,
    canvases: &[CanvasLayer],
) {
    let displays: Vec<_> = canvases.iter().map(|layer| layer.bounds).collect();
    editor::place(editor, bounds, scale, text, &displays);
}

#[cfg(test)]
fn open_text_editor_object(generation: u64, at: (f32, f32), target: Option<ObjectId>, scale: f32) {
    open_text_editor_with_settings(generation, at, target, scale, None);
}
fn open_text_editor_with_settings(
    generation: u64,
    at: (f32, f32),
    target: Option<ObjectId>,
    scale: f32,
    creation: Option<AnnotationValues>,
) {
    let clicked = at;
    let origin = current_focus_origin();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let index = if let Some(target) = target {
            let Some(index) = r.session.object_index(target) else {
                return;
            };
            if !r.session.can_edit(&r.session.objects[index]) {
                return;
            }
            if !matches!(r.session.objects[index].kind, Kind::Text | Kind::Sequence) {
                return;
            }
            Some(index)
        } else {
            if !r.parameter_entry_active(8) {
                return;
            }
            None
        };
        finish_text_edit_registry(r, true);
        r.ensure_fonts();
        if index.is_none()
            && let Some(values) = &creation
        {
            r.parameter_values.remember_creation(8, values);
        }
        let scale = index
            .and_then(|i| r.session.objects.get(i))
            .map_or(scale, |o| {
                if let Extra::Sequence(s) = &o.extra {
                    s.scale
                } else {
                    crate::annotation_text::object_scale(o)
                }
            });
        let original = index
            .and_then(|i| r.session.objects.get(i))
            .and_then(|o| {
                if let Extra::Text(text) = &o.extra {
                    Some(text.clone())
                } else if let Extra::Sequence(s) = &o.extra {
                    Some(s.caption.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| text_from_values(r, String::new()));
        let at = index
            .and_then(|i| r.session.objects.get(i))
            .map_or(at, |object| {
                let b = object
                    .sequence_caption()
                    .map_or(object.bounds, |o| o.bounds);
                (b.left, b.top)
            });
        let Ok(editor) = AnnotationTextInput::new() else {
            return;
        };
        crate::theme::apply(&editor);
        let generation = r.generation;
        let id = editor::next_instance();
        editor.set_value(original.content.clone().into());
        let color = index.and_then(|i| r.session.objects.get(i)).map_or(
            current_style(r, 8).color,
            |object| {
                if let Extra::Sequence(s) = &object.extra {
                    s.caption_color
                } else {
                    object.style.color
                }
            },
        );
        editor::configure(&editor, &original, color);
        editor.on_pointer(move |event, x, y| editor_pointer(generation, id, event, x, y));
        let b = crate::annotation_text::text_bounds(at, &original, scale);
        place_text_editor(&editor, b, scale, &original, &r.canvases);
        if let Some(object) = index.and_then(|i| r.session.objects.get(i)) {
            let caption = object.sequence_caption();
            let offset =
                crate::annotation_text::caret_at(caption.as_ref().unwrap_or(object), clicked)
                    as i32;
            editor.invoke_place_selection(offset, offset);
        }
        let edit = TextEdit::new(
            id,
            index.and_then(|i| r.session.objects.get(i)).map(|o| o.id),
            at,
            scale,
            original,
            b,
            editor.get_value().to_string(),
        );
        let values = index.is_none().then(|| {
            let mut values = r.parameter_values.defaults(8).clone();
            text_dimensions_to_values(&mut values, &edit.original);
            values
        });
        edit.bind_input(
            &editor,
            move || resize_text_editor(generation, id),
            move |commit| finish_matching_text_edit(generation, id, commit),
        );
        edit.bind_escape(&editor, move || {
            if REGISTRY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .is_some_and(|r| r.generation == generation && text_edit_matches(r, id))
            }) && !choice::dismiss_from(current_focus_origin())
            {
                finish_matching_text_edit(generation, id, false);
            }
        });
        r.editor.open(editor, edit, values);
        choice::close_if_context_changed(r);
        reset_text_caret(r);
        r.editor
            .edit()
            .unwrap()
            .prepare_after(Duration::ZERO, move || {
                show_text_editor(generation, id, 0, origin)
            });
    });
    let generation = REGISTRY.with(|slot| slot.borrow().as_ref().map(|r| r.generation));
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

#[cfg(test)]
fn open_text_editor(at: (f32, f32), index: Option<usize>, scale: f32) {
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        let target = match index {
            Some(index) => Some(r.session.objects.get(index)?.id),
            None => None,
        };
        Some((r.generation, target))
    });
    if let Some((generation, target)) = request {
        open_text_editor_object(generation, at, target, scale);
    }
}

fn show_text_editor(generation: u64, id: u64, attempt: u8, origin: focus::Origin) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        if !text_edit_matches(r, id) {
            return;
        }
        let Some(editor) = r.editor.window() else {
            return;
        };
        match (r.prepare)(editor.window()) {
            PassiveWindowPreparation::Ready => {
                let edit = r.editor.edit().unwrap();
                if !attach_text_editor_owner(r) {
                    retry_text_owner(r, generation, id, attempt, origin);
                    return;
                }
                place_text_editor(editor, edit.bounds, edit.scale, &edit.original, &r.canvases);
                if editor.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        editor.window(),
                        r.editor.edit().unwrap().pointer_sink(editor),
                    )
                {
                    r.focus.cancel();
                    finish_text_edit_registry(r, false);
                    return;
                }
                if let Some(edit) = r.editor.edit() {
                    place_text_editor(editor, edit.bounds, edit.scale, &edit.original, &r.canvases);
                }
                if !attach_text_editor_owner(r) {
                    let _ = editor.hide();
                    retry_text_owner(r, generation, id, attempt, origin);
                    return;
                }
                if permits_focus_activation(r, origin) {
                    (r.lifecycle.activate_user_requested_window)(editor.window());
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = editor.show();
                let _ = editor.hide();
                r.editor
                    .edit()
                    .unwrap()
                    .prepare_after(Duration::from_millis(16), move || {
                        show_text_editor(generation, id, attempt + 1, origin)
                    });
            }
            _ => {
                r.focus.cancel();
                finish_text_edit_registry(r, false);
            }
        }
    });
}

fn attach_text_editor_owner(r: &Registry) -> bool {
    let (Some(editor), Some(edit)) = (r.editor.window(), r.editor.edit()) else {
        return false;
    };
    let displays: Vec<_> = r.canvases.iter().map(|layer| layer.bounds).collect();
    let viewport = editor::viewport(edit.bounds, &displays);
    // Ownership follows the clipped visible editor, whose original bounds can be outside the canvas.
    r.canvases
        .iter()
        .find(|layer| {
            viewport.left >= layer.bounds.left as f32
                && viewport.left < layer.bounds.right as f32
                && viewport.top >= layer.bounds.top as f32
                && viewport.top < layer.bounds.bottom as f32
        })
        .is_none_or(|owner| {
            (r.lifecycle.attach_tool_window)(editor.window(), owner.window.window())
        })
}

fn retry_text_owner(
    r: &mut Registry,
    generation: u64,
    id: u64,
    attempt: u8,
    origin: focus::Origin,
) {
    if attempt < 20 {
        r.editor
            .edit()
            .unwrap()
            .prepare_after(Duration::from_millis(16), move || {
                show_text_editor(generation, id, attempt + 1, origin)
            });
    } else {
        tracing::warn!("could not attach annotation editor after preparation retries");
        r.focus.cancel();
        finish_text_edit_registry(r, false);
    }
}

pub(crate) fn shutdown() {
    REGISTRY.with(|s| {
        if let Some(mut r) = s.borrow_mut().take() {
            r.close();
        }
    });
}
pub(crate) fn close() {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.close();
        }
    });
    crate::bridge::schedule_idle_memory_trim();
}
#[cfg(test)]
fn escape() {
    escape_from(current_focus_origin());
}

fn escape_from(origin: focus::Origin) {
    if choice::dismiss_from(origin) {
        return;
    }
    let editing = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref().and_then(|r| {
            if r.editor.window().is_some() {
                Some((r.generation, true, 0))
            } else if r.watermark.get().is_some() {
                Some((r.generation, false, r.watermark.revision()))
            } else {
                None
            }
        })
    });
    if let Some((generation, text, revision)) = editing {
        if text {
            finish_text_edit(generation, false);
        } else {
            close_watermark_input(generation, revision);
        }
        return;
    }
    let first = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if !r.mosaic.pending.is_empty() {
            cancel_active_gesture(r);
            mosaic::cancel_pending(r);
            return Some(r.generation);
        }
        if cancel_active_gesture(r) {
            refresh_hovered(r);
            return Some(r.generation);
        }
        if r.session.selected.is_some() {
            deselect_object(r);
            refresh_hovered(r);
            cursor::refresh(r);
            return Some(r.generation);
        }
        None
    });
    if let Some(generation) = first {
        schedule_render(generation);
        return;
    }
    let has_panel = REGISTRY.with(|s| s.borrow().as_ref().is_some_and(|r| r.panel.get().is_some()));
    if has_panel {
        let target = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut()?;
            r.close_panel();
            let main = r.main.get()?.clone_strong();
            permits_focus_activation(r, origin)
                .then_some((main, r.lifecycle.activate_user_requested_window))
        });
        // Escape is an explicit keyboard action; keep the next Escape in this UI session.
        if let Some((main, activate)) = target {
            activate(main.window());
        }
    } else {
        close();
    }
}

fn finish_polyline() {
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if let drawing::Progress::Retired(Some(object)) = r.pending_drawing.finish_polyline()? {
            r.session.add(*object);
        }
        if matches!(r.gesture(), Some(Gesture::PlacePoint)) {
            r.gesture = None;
        }
        sync_history_controls(r);
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

fn backspace_polyline() {
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if matches!(
            r.pending_drawing.backspace_polyline()?,
            drawing::Progress::Retired(_)
        ) && matches!(r.gesture(), Some(Gesture::PlacePoint))
        {
            r.gesture = None;
        }
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

pub(crate) fn toggle(requested_at: Instant) {
    if is_open() {
        close();
        return;
    }
    crate::bridge::cancel_idle_memory_trim();
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if !r.cold_open_measured {
            r.cold_open_timing = Some(ColdOpenTiming {
                requested_at: Some(requested_at),
                ui_queue: requested_at.elapsed(),
                ..Default::default()
            });
        }
        let toolbar_start = Instant::now();
        let Ok(main) = AnnotationToolbar::new() else {
            r.close();
            return;
        };
        crate::theme::apply(&main);
        r.generation = r.generation.wrapping_add(1);
        let generation = r.generation;
        let revision = r.main.open(main);
        let main = r.main.get().unwrap();
        let input = r.main.input();
        main.on_tool_clicked(move |group, menu| {
            later_main(&input, generation, revision, move || {
                open_panel(group as usize, menu)
            })
        });
        let input = r.main.input();
        main.on_operate_requested(move || {
            later_main(&input, generation, revision, activate_mouse_mode)
        });
        let input = r.main.input();
        main.on_undo_requested(move || later_main(&input, generation, revision, undo));
        let input = r.main.input();
        main.on_delete_requested(move || later_main(&input, generation, revision, delete_selected));
        let input = r.main.input();
        main.on_finish_path_requested(move || {
            later_main(&input, generation, revision, finish_polyline)
        });
        let input = r.main.input();
        main.on_backspace_requested(move || {
            later_main(&input, generation, revision, backspace_polyline)
        });
        let input = r.main.input();
        main.on_finish_requested(move || later_main(&input, generation, revision, close));
        let input = r.main.input();
        let lifecycle = r.lifecycle.clone();
        main.on_escape_requested(move || {
            let origin = focus_origin(&lifecycle);
            later_main(&input, generation, revision, move || escape_from(origin));
        });
        let input = r.main.input();
        let lifecycle = r.lifecycle.clone();
        main.on_drag_requested(move || {
            let origin = focus_origin(&lifecycle);
            later_main(&input, generation, revision, move || {
                choice::dismiss_from(origin);
                let target = REGISTRY.with(|s| {
                    let slot = s.borrow();
                    let r = slot.as_ref()?;
                    if !permits_focus_activation(r, origin) {
                        return None;
                    }
                    Some((r.main.get()?.clone_strong(), r.lifecycle.begin_window_drag))
                });
                if let Some((main, drag)) = target {
                    drag(main.window());
                    reposition_panel();
                }
            })
        });
        let input = r.main.input();
        main.window().on_close_requested(move || {
            later_main(&input, generation, revision, close);
            slint::CloseRequestResponse::KeepWindowShown
        });
        main.window().set_size(slint::LogicalSize::new(650., 54.));
        if let Some(timing) = r.cold_open_timing.as_mut() {
            let elapsed = toolbar_start.elapsed();
            timing.toolbar_create += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
        }
        r.sync_mode_display();
        let query_start = Instant::now();
        r.pending_displays = (r.lifecycle.annotation_displays)();
        r.session.displays = r.pending_displays.clone();
        if let Some(timing) = r.cold_open_timing.as_mut() {
            let elapsed = query_start.elapsed();
            timing.display_query += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
            timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
        }
        r.main.prepare_after(Duration::from_millis(16), move || {
            bootstrap_tick(generation, revision)
        });
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootstrapAction {
    Create(usize),
    Advance(usize),
    ShowToolbar,
}

fn bootstrap_action(
    created: usize,
    total: usize,
    states: impl Iterator<Item = CanvasBootstrap>,
    cursor: usize,
) -> BootstrapAction {
    if created < total {
        return BootstrapAction::Create(created);
    }
    let cursor = if created == 0 { 0 } else { cursor % created };
    let mut before_cursor = None;
    for (index, state) in states.enumerate() {
        if state != CanvasBootstrap::Ready {
            if index >= cursor {
                return BootstrapAction::Advance(index);
            }
            before_cursor.get_or_insert(index);
        }
    }
    before_cursor.map_or(BootstrapAction::ShowToolbar, BootstrapAction::Advance)
}

/// Advance one screen stage per event-loop turn; a pending native window does
/// not hold up preparation of the other monitors.
fn bootstrap_tick(generation: u64, revision: u64) {
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.main.revision() == revision)
        else {
            return;
        };
        let action = bootstrap_action(
            r.canvases.len(),
            r.pending_displays.len(),
            r.canvases.iter().map(|canvas| canvas.bootstrap),
            r.bootstrap_cursor,
        );
        match action {
            BootstrapAction::Create(index) => create_canvas(r, index),
            BootstrapAction::Advance(index) => {
                r.bootstrap_cursor = (index + 1) % r.canvases.len();
                match r.canvases[index].bootstrap {
                    CanvasBootstrap::Prepare(attempt) => show_canvas(r, index, attempt),
                    CanvasBootstrap::Present => render_initial_canvas(r, index),
                    CanvasBootstrap::Ready => unreachable!(),
                }
            }
            BootstrapAction::ShowToolbar => {
                let revision = r.main.revision();
                r.main.prepare_after(Duration::from_millis(16), move || {
                    show_main(generation, revision, 0)
                });
                return;
            }
        }
        if r.generation == generation {
            if let Some(timing) = r.cold_open_timing.as_mut() {
                timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
            }
            r.main.prepare_after(Duration::from_millis(16), move || {
                bootstrap_tick(generation, revision)
            });
        }
    });
}

fn create_canvas(r: &mut Registry, index: usize) {
    let bounds = r.pending_displays[index];
    let start = Instant::now();
    let Ok(window) = AnnotationCanvas::new() else {
        r.close();
        return;
    };
    window.set_frame(slint::Image::default());
    let layer = CanvasLayer::new(window, bounds);
    let generation = r.generation;
    let revision = layer.revision();
    let window = &layer.window;
    window.on_pointer(move |kind, x, y| {
        if canvas_current(generation, index, revision) {
            canvas_pointer(index, kind, x, y);
        }
    });
    window.on_brush_size_scrolled(move |delta| {
        if canvas_current(generation, index, revision) {
            brush_size_scrolled(index, delta);
        }
    });
    let input = layer.input();
    let lifecycle = r.lifecycle.clone();
    window.on_escape_requested(move || {
        let origin = focus_origin(&lifecycle);
        later_canvas(&input, generation, index, revision, move || {
            escape_from(origin)
        });
    });
    let input = layer.input();
    window.on_undo_requested(move || later_canvas(&input, generation, index, revision, undo));
    let input = layer.input();
    window.on_delete_requested(move || {
        later_canvas(&input, generation, index, revision, delete_selected)
    });
    let input = layer.input();
    window.on_finish_path_requested(move || {
        later_canvas(&input, generation, index, revision, finish_polyline)
    });
    let input = layer.input();
    window.on_backspace_requested(move || {
        later_canvas(&input, generation, index, revision, backspace_polyline)
    });
    let input = layer.input();
    window.window().on_close_requested(move || {
        later_canvas(&input, generation, index, revision, close);
        slint::CloseRequestResponse::KeepWindowShown
    });
    r.canvases.push(layer);
    if let Some(timing) = r.cold_open_timing.as_mut() {
        let elapsed = start.elapsed();
        timing.canvas_create += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen.push(CanvasColdTiming {
            create: elapsed,
            ..Default::default()
        });
    }
}

fn show_canvas(r: &mut Registry, index: usize, attempt: u8) {
    let layer = &r.canvases[index];
    let stage_start = Instant::now();
    let rect = layer.bounds;
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    layer
        .window
        .window()
        .set_size(slint::PhysicalSize::new(width, height));
    layer
        .window
        .window()
        .set_position(slint::PhysicalPosition::new(rect.left, rect.top));
    let preparation = (r.prepare)(layer.window.window());
    if let Some(timing) = r.cold_open_timing.as_mut() {
        let elapsed = stage_start.elapsed();
        timing.canvas_prepare += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].prepare += elapsed;
    }
    match preparation {
        PassiveWindowPreparation::Ready => {
            let show_start = Instant::now();
            if layer.window.show().is_err() {
                r.close();
                return;
            }
            if let Some(timing) = r.cold_open_timing.as_mut() {
                let elapsed = show_start.elapsed();
                timing.canvas_show += elapsed;
                timing.largest_stage = timing.largest_stage.max(elapsed);
                timing.per_screen[index].show += elapsed;
            }
            let native_start = Instant::now();
            if !(r.lifecycle.complete_passive_window_show)(
                layer.window.window(),
                pointer_canvas(
                    layer.window.as_weak(),
                    r.generation,
                    index,
                    layer.revision(),
                    layer.input(),
                ),
            ) {
                r.close();
                return;
            }
            if let Some(timing) = r.cold_open_timing.as_mut() {
                let elapsed = native_start.elapsed();
                timing.canvas_native_complete += elapsed;
                timing.largest_stage = timing.largest_stage.max(elapsed);
                timing.per_screen[index].native_complete += elapsed;
            }
            (r.lifecycle.annotation_click_through)(layer.window.window(), r.mode.is_mouse());
            settle_canvas(r, index, layer.revision(), 2);
            r.canvases[index].bootstrap = CanvasBootstrap::Present;
        }
        PassiveWindowPreparation::Pending if attempt < 20 => {
            let retry_start = Instant::now();
            let _ = layer.window.show();
            let _ = layer.window.hide();
            if let Some(timing) = r.cold_open_timing.as_mut() {
                let elapsed = retry_start.elapsed();
                timing.canvas_retry += elapsed;
                timing.largest_stage = timing.largest_stage.max(elapsed);
                timing.per_screen[index].retry += elapsed;
                timing.per_screen[index].retries += 1;
            }
            r.canvases[index].bootstrap = CanvasBootstrap::Prepare(attempt + 1);
        }
        _ => r.close(),
    }
}

/// Present one complete blank monitor frame before its canvas accepts drawing.
/// The native layered bitmap is retained here until Windows hit testing of an
/// unpresented Slint canvas has been verified on supported DPI layouts.
fn render_initial_canvas(r: &mut Registry, index: usize) {
    let layer = &mut r.canvases[index];
    let raster_start = Instant::now();
    let Some(frame) = crate::annotation_render::render(
        &r.session,
        layer.bounds,
        layer.window.window().scale_factor(),
        None,
        None,
    ) else {
        r.close();
        return;
    };
    if let Some(timing) = r.cold_open_timing.as_mut() {
        let elapsed = raster_start.elapsed();
        timing.raster += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].raster += elapsed;
    }
    let present_start = Instant::now();
    let presented = layer.presented.submit(frame, |frame| {
        (r.lifecycle.annotation_frame_presenter)(
            layer.window.window(),
            frame.data(),
            frame.width(),
            frame.height(),
        )
    });
    if let Some(timing) = r.cold_open_timing.as_mut() {
        let elapsed = present_start.elapsed();
        timing.native_present += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].present += elapsed;
    }
    if !presented {
        r.close();
        return;
    }
    r.canvases[index].signature = Some(render_signature(
        &r.session,
        r.canvases[index].bounds,
        r.canvases[index].window.window().scale_factor(),
        None,
        None,
    ));
    r.canvases[index].bootstrap = CanvasBootstrap::Ready;
}

fn settle_canvas(r: &Registry, index: usize, revision: u64, remaining: u8) {
    let generation = r.generation;
    r.canvases[index].position_after(Duration::from_millis(32), move || {
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let Some(r) = slot.as_ref().filter(|r| r.generation == generation) else {
                return;
            };
            let Some(layer) = r
                .canvases
                .get(index)
                .filter(|layer| layer.revision() == revision)
            else {
                return;
            };
            let bounds = layer.bounds;
            layer
                .window
                .window()
                .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
            layer.window.window().set_size(slint::PhysicalSize::new(
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
            ));
            if remaining > 0 {
                settle_canvas(r, index, revision, remaining - 1);
            }
        });
    });
}

fn canvas_current(generation: u64, index: usize, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        slot.borrow().as_ref().is_some_and(|r| {
            r.generation == generation
                && r.canvases
                    .get(index)
                    .is_some_and(|layer| layer.revision() == revision)
        })
    })
}

fn later_canvas(
    input: &Rc<window::InputWork>,
    generation: u64,
    index: usize,
    revision: u64,
    action: impl FnOnce() + 'static,
) {
    input.post(move || {
        if canvas_current(generation, index, revision) {
            action();
        }
    });
}

fn pointer_canvas(
    weak: slint::Weak<AnnotationCanvas>,
    generation: u64,
    index: usize,
    revision: u64,
    input_work: Rc<window::InputWork>,
) -> crate::bridge::PopupPointerSink {
    registry_pointer(
        input_work,
        Rc::new(move |input| {
            if canvas_current(generation, index, revision)
                && let Some(window) = weak.upgrade()
            {
                if let PopupPointerInput::LeftCancelled { x, y } = input {
                    let scale = window.window().scale_factor().max(0.1);
                    window.invoke_pointer(3, x / scale, y / scale);
                }
                dispatch(window.window(), input);
            }
        }),
    )
}

fn current_style(r: &Registry, tool: i32) -> Style {
    parameters::style(r.values(tool as usize), tool)
}

fn chosen_font(r: &Registry, tool: usize) -> String {
    r.fonts
        .names()
        .get(r.values(tool).font.max(0) as usize)
        .cloned()
        .unwrap_or_else(|| r.preferred_font(tool))
}

fn text_from_values(r: &Registry, content: String) -> TextAnnotation {
    parameters::text(r.values(8), chosen_font(r, 8), content)
}

fn watermark_from_values(r: &Registry, template: String, content: String) -> Watermark {
    parameters::watermark(r.values(9), chosen_font(r, 9), template, content)
}

fn adopt_watermark_style(r: &mut Registry) {
    let font_index = r
        .session
        .watermark
        .as_ref()
        .and_then(|watermark| {
            r.fonts
                .names()
                .iter()
                .position(|name| name == &watermark.font_family)
        })
        .unwrap_or(0) as i32;
    r.parameter_values
        .project_watermark(r.session.watermark.as_ref(), font_index);
    choice::close_if_context_changed(r);
    if let Some(panel) = r.panel.get()
        && panel.get_tool() == 9
        && !panel.get_menu()
    {
        panel.set_values(r.values(9).clone());
    }
}

fn toolbar_tools(r: &Registry) -> Vec<i32> {
    let mut selected = r.selected;
    if selected[0] != 2 && r.values(selected[0] as usize).shape == 1 {
        selected[0] = 1;
    }
    selected.to_vec()
}

fn capture_creation(r: &mut Registry, tool: usize) -> AnnotationValues {
    if matches!(tool, 8 | 10) {
        r.ensure_fonts();
    }
    let mut values = r.values(tool).clone();
    values.text = Default::default();
    values.sequence_label = Default::default();
    if tool == 10 {
        values.start = r.session.next_sequence;
    }
    r.parameter_values.remember_creation(tool, &values);
    values
}

fn deselect_object(r: &mut Registry) {
    r.session.selected = None;
    r.parameter_values.deselect();
    if let Some(panel) = r.panel.get().filter(|panel| !panel.get_menu()) {
        panel.set_values(r.values(panel.get_tool() as usize).clone());
    }
}

fn project_object_parameters(r: &mut Registry, index: usize) -> [u8; 3] {
    let object = &r.session.objects[index];
    let font_family = match &object.extra {
        Extra::Text(text) => Some(&text.font_family),
        Extra::Sequence(sequence) => Some(&sequence.caption.font_family),
        _ => None,
    };
    let font_index = font_family
        .and_then(|family| r.fonts.names().iter().position(|name| name == family))
        .unwrap_or(0) as i32;
    let preview_color = object.style.color;
    r.parameter_values.project(
        object,
        font_index,
        r.session.next_sequence,
        r.session.spotlight_opacity,
    );
    preview_color
}

fn restore_parameter_projection(r: &mut Registry) {
    if let Some(index) = r.session.selected {
        project_object_parameters(r, index);
    } else {
        r.parameter_values.deselect();
    }
    choice::close_if_context_changed(r);
    if let Some(panel) = r.panel.get().filter(|panel| !panel.get_menu()) {
        let tool = panel.get_tool() as usize;
        r.parameter_values.bind(r.parameter_object(tool));
        panel.set_values(r.values(tool).clone());
    }
}

/// Refill only when a canvas operation changes the panel's target. Merely
/// rebinding would leave stale object values available to the next user edit.
fn refresh_parameter_target(r: &mut Registry) {
    choice::close_if_context_changed(r);
    let Some(panel) = r.panel.get().filter(|panel| !panel.get_menu()) else {
        return;
    };
    let target = r.parameter_object(panel.get_tool() as usize);
    if r.parameter_values.bound_object() != target {
        restore_parameter_projection(r);
    }
}

fn adopt_selected_style(r: &mut Registry, index: usize) -> bool {
    if !r
        .session
        .objects
        .get(index)
        .is_some_and(|object| r.session.can_edit(object))
    {
        return false;
    }
    let object = &r.session.objects[index];
    let tool = object.kind.tool();
    let group = if matches!(tool, 11 | 12) {
        5
    } else if tool == 10 {
        4
    } else if tool == 8 {
        3
    } else if tool >= 5 {
        2
    } else {
        usize::from(tool >= 3)
    };
    let preview_color = project_object_parameters(r, index);
    choice::close_if_context_changed(r);
    r.selected[group] = tool as i32;
    r.set_mode(InteractionMode::Tool(group));
    if let Some(main) = r.main.get() {
        main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
    }
    if let Some(panel) = r.panel.get()
        && !panel.get_menu()
        && panel.get_tool() == tool as i32
    {
        panel.set_values(r.values(tool).clone());
        let [red, green, blue] = preview_color;
        panel.set_custom_preview(slint::Color::from_rgb_u8(red, green, blue));
    }
    !r.panel
        .get()
        .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == tool as i32)
}

fn magnifier_connector(index: i32) -> MagnifierConnector {
    match index {
        1 => MagnifierConnector::Dot,
        2 => MagnifierConnector::Frame,
        3 => MagnifierConnector::None,
        _ => MagnifierConnector::Plain,
    }
}

fn active_kind(r: &Registry) -> Option<Kind> {
    if r.mode.is_mouse() {
        return None;
    }
    let tool = r.selected[r.mode.selected_group() as usize] as usize;
    let capture = r
        .lifecycle
        .annotation_screen_capture
        .as_ref()
        .is_some_and(|p| p.is_available());
    match tools::drawing(tool, r.values(tool).shape, capture) {
        tools::Drawing::Object(kind) => Some(kind),
        _ => None,
    }
}

fn sync_sequence_counter(r: &mut Registry) {
    r.parameter_values.sequence_counter(r.session.next_sequence);
    if let Some(panel) = r.panel.get()
        && panel.get_tool() == 10
    {
        let mut v = panel.get_values();
        v.start = r.values(10).start;
        v.sequence_label = r.values(10).sequence_label.clone();
        panel.set_values(v);
    }
}
fn update_sequence_style(
    object: &mut Object,
    values: &AnnotationValues,
    fonts: &[String],
    style: Style,
) {
    parameters::sequence_style(object, values, fonts, style);
    crate::annotation_sequence::layout(object);
}

fn sequence_from_values(r: &Registry, at: (f32, f32), scale: f32) -> Object {
    let v = r.parameter_values.defaults(10);
    let style = parameters::style(v, 10);
    let mut caption = text_from_values(r, String::new());
    caption.outline = None;
    caption.background = None;
    caption.bold = false;
    caption.italic = false;
    let b = Bounds::from_corners(at, at);
    let mut object = Object {
        erased: Vec::new(),
        id: Default::default(),
        kind: Kind::Sequence,
        bounds: b,
        style,
        points: Vec::new(),
        extra: Extra::Sequence(SequenceAnnotation {
            value: r.session.next_sequence.max(1),
            format: SequenceFormat::Decimal,
            shape: SequenceShape::Square,
            size: 16.,
            scale,
            caption,
            caption_bounds: b,
            caption_color: style.color,
            caption_custom_color: false,
            caption_ratio: 2,
            guide: true,
            endpoint: Endpoint::FilledArrow,
        }),
    };
    update_sequence_style(&mut object, v, r.fonts.names(), style);
    object
}

/// The press snapshot includes selection and numbering; no undo entry exists until release.
fn begin_sequence_placement(r: &mut Registry, point: (f32, f32), scale: f32) {
    capture_creation(r, 10);
    let object = sequence_from_values(r, point, scale);
    r.begin_document_gesture();
    let Some(placement) = gestures::SequencePlacement::insert(&mut r.session, point, object) else {
        return;
    };
    deselect_object(r);
    r.gesture = Some(Active::Sequence(placement));
    sync_sequence_counter(r);
}

fn update_sequence_placement(r: &mut Registry, point: (f32, f32)) {
    if let Some(placement) = r.gesture.as_ref().and_then(Active::sequence) {
        placement.update(&mut r.session, point);
    }
}

fn cancel_sequence_placement(r: &mut Registry) -> bool {
    matches!(r.gesture(), Some(Gesture::PlaceSequence)) && cancel_active_gesture(r)
}

/// Retire an object operation before another document command can change its target.
fn cancel_object_gesture(r: &mut Registry) -> bool {
    matches!(
        r.gesture(),
        Some(
            Gesture::Erase
                | Gesture::PlaceSequence
                | Gesture::Move
                | Gesture::Resize { .. }
                | Gesture::TextHandle { .. }
        )
    ) && cancel_active_gesture(r)
}

/// Roll back the active input operation, leaving the editor and committed history alive.
/// Pending capture validation is owned by the mosaic coordinator, not this gesture.
fn cancel_active_gesture(r: &mut Registry) -> bool {
    let active = r.gesture.take();
    let gesture = active.as_ref().map(Active::route);
    if active.is_none() && r.pending_drawing.is_none() {
        return false;
    }
    if let Some(original) = active.and_then(|active| active.cancel(&mut r.session)) {
        apply_editing_object(r, original);
    }
    if let Some(edit) = r.editor.edit_mut() {
        edit.selection_anchor = None;
    }
    r.replay_deferred_parameters(gesture);

    r.pending_drawing.cancel();

    r.pending_edit_panel = false;
    restore_parameter_projection(r);
    sync_sequence_counter(r);
    cursor::refresh(r);
    true
}

/// Apply one validated request before another command can retire its editor.
/// Property observers may echo the resulting values, but do not own application.
fn apply_geometry_values(
    r: &mut Registry,
    target: parameter_edit::Target,
    values: AnnotationValues,
) -> Option<u64> {
    let tool = target.tool;
    if !r
        .panel
        .get()
        .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == tool)
    {
        return None;
    }
    if r.parameter_target(tool) != target {
        return None;
    }
    if r.gesture().is_some_and(Gesture::blocks_parameter_edits) {
        r.deferred_parameters.accept(target, values);
        return None;
    }
    apply_parameter_values(r, target, values)
}

/// Execute an accepted parameter request using its explicit object/default target.
/// Validation and geometry deferral belong to the input/replay boundary above.
fn apply_parameter_values(
    r: &mut Registry,
    target: parameter_edit::Target,
    values: AnnotationValues,
) -> Option<u64> {
    let generation = r.generation;
    let tool = target.tool;
    if !r.parameter_entry_active(tool)
        || target
            .object
            .is_some_and(|id| r.parameter_object(tool as usize) != Some(id))
    {
        return None;
    }
    let source_object = target.object;
    let previous_values = if tool == 10 {
        r.parameter_values.values(10, source_object)
    } else {
        r.values(tool as usize)
    }
    .clone();
    if previous_values == values {
        return None;
    }
    let token = r.parameter_edit.touch(&mut r.session, target);
    if tool == 8 {
        r.parameter_values.remember_creation(tool as usize, &values);
    }
    let edited = values;
    if tool == 10 {
        r.parameter_values
            .replace(10, source_object, edited.clone());
    } else {
        r.replace_values(tool as usize, edited.clone());
    }
    if let Some(main) = r.main.get() {
        main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
    }
    let style = parameters::style(&edited, tool);
    if let Some(panel) = r.panel.get() {
        panel.set_custom_preview(slint::Color::from_rgb_u8(
            style.color[0],
            style.color[1],
            style.color[2],
        ));
    }
    let kind = active_kind(r);
    let text_scale = r
        .session
        .selected
        .and_then(|index| r.session.objects.get(index))
        .and_then(|object| {
            r.canvases.iter().find(|layer| {
                let center = object.bounds.center();
                center.0 >= layer.bounds.left as f32
                    && center.0 < layer.bounds.right as f32
                    && center.1 >= layer.bounds.top as f32
                    && center.1 < layer.bounds.bottom as f32
            })
        })
        .map_or(1., |layer| layer.window.window().scale_factor().max(0.1));
    let text_style = (tool == 8).then(|| text_from_values(r, String::new()));
    let text_scale = r
        .session
        .selected
        .and_then(|index| r.session.objects.get(index))
        .filter(|object| object.kind == Kind::Text)
        .map_or(text_scale, crate::annotation_text::object_scale);
    let displays: Vec<_> = if r.editor.edit().is_some() && (tool == 8 || tool == 10) {
        r.canvases.iter().map(|layer| layer.bounds).collect()
    } else {
        Vec::new()
    };
    if let Some(next) = &text_style {
        r.editor
            .apply_text_style(next, style.color, &previous_values, &edited, &displays);
    }
    let watermark = if tool == 9 {
        r.session.watermark.as_ref().map(|current| {
            watermark_from_values(r, current.template.clone(), current.content.clone())
        })
    } else {
        None
    };
    if tool == 2 {
        r.session.set_spotlight_opacity(r.values(2).strength / 100.);
    }
    if tool == 10 {
        if edited.start != previous_values.start {
            r.session.next_sequence = edited.start.max(1);
        }
        let fonts = r.fonts.names();
        r.session.update_selected(|object| {
            if Some(object.id) != source_object {
                return;
            }
            if object.kind == Kind::Sequence {
                update_sequence_style(object, &edited, fonts, style);
            }
        });
        if let Some(index) = r.editor.edit().and_then(|edit| edit.index(&r.session))
            && let Some(object) = r.session.objects.get(index)
        {
            r.editor.apply_caption_style(object, &displays);
        }
    } else if let Some(kind) = kind {
        r.session.update_selected(|object| {
            if Some(object.id) != source_object {
                return;
            }
            if object.kind.tool() == kind.tool() {
                if object.kind != kind && kind.tool() == 4 {
                    if kind == Kind::HighlightLine {
                        object.points = vec![
                            (object.bounds.left, object.bounds.top),
                            (object.bounds.right, object.bounds.bottom),
                        ];
                    } else {
                        object.points.clear();
                    }
                }
                if !matches!(kind.tool(), 11 | 12) {
                    object.kind = kind;
                }
                object.style = style;
                match kind {
                    Kind::StickerArea | Kind::StickerBrush => {
                        if let Extra::Sticker(value) = &mut object.extra {
                            value.texture = if edited.mode == 1 {
                                lexift_core::domain::annotation::StickerTexture::Frost
                            } else {
                                lexift_core::domain::annotation::StickerTexture::Glass
                            };
                            value.opacity = edited.strength.round().clamp(0., 100.) as u8;
                            if edited.zoom != previous_values.zoom {
                                value.set_displayed_size(edited.zoom as f32);
                            }
                            if edited.size != previous_values.size
                                && object.kind == Kind::StickerBrush
                            {
                                value.diameter =
                                    edited.size.clamp(1, 100) as f32 * value.creation_scale;
                            }
                        }
                        if object.kind == Kind::StickerBrush {
                            object.refresh_bounds();
                        }
                    }
                    Kind::MosaicArea | Kind::MosaicBrush => {
                        if let Extra::Mosaic(value) = &mut object.extra {
                            value.effect = if edited.mode == 1 {
                                lexift_core::domain::annotation::MosaicEffect::Blur
                            } else {
                                lexift_core::domain::annotation::MosaicEffect::Pixelate
                            };
                            value.strength = edited.strength.round().clamp(0., 100.) as u8;
                            if edited.size != previous_values.size
                                && object.kind == Kind::MosaicBrush
                            {
                                value.diameter =
                                    edited.size.clamp(1, 100) as f32 * value.creation_scale;
                            }
                        }
                        if object.kind == Kind::MosaicBrush {
                            object.refresh_bounds();
                        }
                    }
                    Kind::Arrow => {
                        if let Extra::Arrow {
                            head, start, end, ..
                        } = &mut object.extra
                        {
                            *head = edited.head;
                            *start = endpoint(edited.line_start);
                            *end = endpoint(edited.line_end);
                        }
                    }
                    Kind::Polyline => {
                        object.extra = Extra::Polyline {
                            curved: edited.shape == 1,
                            head: edited.head,
                            start: endpoint(edited.line_start),
                            end: endpoint(edited.line_end),
                        };
                    }
                    Kind::Magnifier => {
                        object.set_magnifier_zoom_centered(edited.zoom as f32 / 100.);
                        if let Extra::Magnifier {
                            ellipse,
                            connector,
                            erase_annotations,
                            antialias,
                            shadow,
                            ..
                        } = &mut object.extra
                        {
                            *ellipse = edited.shape == 1;
                            *connector = magnifier_connector(edited.connector_style);
                            *erase_annotations = !edited.include_annotations;
                            *antialias = edited.antialias;
                            *shadow = edited.shadow;
                        }
                    }
                    Kind::Text => {
                        if let (Extra::Text(existing), Some(updated)) =
                            (&mut object.extra, &text_style)
                        {
                            *existing = parameters::updated_text_style(
                                existing,
                                updated,
                                &previous_values,
                                &edited,
                            );
                            object.bounds = crate::annotation_text::text_bounds(
                                (object.bounds.left, object.bounds.top),
                                existing,
                                text_scale,
                            );
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    if tool == 9
        && let Some(watermark) = watermark
    {
        r.session.set_watermark(Some(watermark));
    }
    if let Some(main) = r.main.get() {
        main.set_can_undo(r.session.can_undo());
    }
    if tool == 10 {
        sync_sequence_counter(r);
    }
    position_panel(r);
    r.parameter_edit.finish_after(token, move || {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut().filter(|r| {
                r.generation == generation
                    && r.parameter_edit
                        .ready_to_finish(token, choice::color_dragging(r))
            }) {
                r.finish_style_edit();
            }
        });
    });
    Some(r.generation)
}

fn update_geometry_patch(
    generation: u64,
    target: parameter_edit::Target,
    patch: parameters::Patch,
) {
    let edit = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        let mut values = r.pending_parameter_values(target).clone();
        patch.apply(&mut values);
        apply_geometry_values(r, target, values)
    });
    if let Some(generation) = edit {
        schedule_render(generation);
    }
}

#[cfg(test)]
fn update_geometry_values(
    generation: u64,
    target: parameter_edit::Target,
    values: AnnotationValues,
) {
    let edit = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        apply_geometry_values(r, target, values)
    });
    if let Some(generation) = edit {
        schedule_render(generation);
    }
}

fn magnifier_from_source(
    source: Bounds,
    style: Style,
    values: &AnnotationValues,
) -> Option<Object> {
    let zoom = (values.zoom as f32 / 100.).clamp(1., 8.);
    if source.width() < 2. || source.height() < 2. {
        return None;
    }
    let output = magnifier_output_for_source(source, zoom);
    Some(Object {
        erased: Vec::new(),
        id: Default::default(),
        bounds: source,
        kind: Kind::Magnifier,
        style,
        points: Vec::new(),
        extra: Extra::Magnifier {
            output,
            zoom,
            ellipse: values.shape == 1,
            connector: magnifier_connector(values.connector_style),
            erase_annotations: !values.include_annotations,
            antialias: values.antialias,
            shadow: values.shadow,
        },
    })
}

fn text_handle_at(object: &Object, point: (f32, f32), scale: f32) -> Option<usize> {
    object
        .edit_handles(scale)
        .iter()
        .position(|p| (p.0 - point.0).abs() <= 9. * scale && (p.1 - point.1).abs() <= 9. * scale)
}

fn editing_body_contains(object: &Object, point: (f32, f32)) -> bool {
    let Extra::Text(text) = &object.extra else {
        return false;
    };
    let local =
        lexift_core::domain::annotation::rotate_text_point(point, object.bounds, -text.rotation);
    local.0 >= object.bounds.left
        && local.0 <= object.bounds.right
        && local.1 >= object.bounds.top
        && local.1 <= object.bounds.bottom
}

fn select_editing_text(r: &Registry, point: (f32, f32), anchor: Option<usize>) -> usize {
    let Some(object) = editing_text_object(r) else {
        return 0;
    };
    let offset = crate::annotation_text::caret_at(&object, point);
    if let Some(editor) = r.editor.window() {
        editor.invoke_place_selection(anchor.unwrap_or(offset) as i32, offset as i32);
    }
    offset
}

/// Restore the existing edit only after its originating toolbar interaction completes.
fn restore_editor_focus(r: &Registry) {
    restore_editor_focus_with_activation(r, focus::Activation::UserRequested);
}

fn focus_origin(lifecycle: &WindowLifecycleCallbacks) -> focus::Origin {
    lifecycle
        .annotation_foreground
        .as_ref()
        .map_or(focus::Origin::Unobserved, |query| {
            focus::Origin::Foreground(query())
        })
}

fn current_focus_origin() -> focus::Origin {
    REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(focus::Origin::Unobserved, |r| focus_origin(&r.lifecycle))
    })
}

fn permits_focus_activation(r: &Registry, origin: focus::Origin) -> bool {
    origin.permits_activation(
        r.lifecycle
            .annotation_foreground
            .as_ref()
            .and_then(|query| query()),
        &capture_exclusions(r),
    )
}

fn restore_editor_focus_with_activation(r: &Registry, activation: focus::Activation) {
    restore_editor_focus_from(r, activation, focus_origin(&r.lifecycle));
}

fn restore_editor_focus_from(r: &Registry, activation: focus::Activation, origin: focus::Origin) {
    if r.watermark.get().is_some() {
        restore_watermark_focus(r, r.watermark.revision());
        return;
    }
    let Some(edit) = r.editor.edit() else { return };
    restore_input_focus(
        r,
        focus::Target::Text {
            editor: edit.id,
            panel: r.panel.revision(),
        },
        activation,
        origin,
    );
}

fn restore_watermark_focus(r: &Registry, revision: u64) {
    restore_input_focus(
        r,
        focus::Target::Watermark { editor: revision },
        focus::Activation::Local,
        focus_origin(&r.lifecycle),
    );
}

fn restore_input_focus(
    r: &Registry,
    target: focus::Target,
    activation: focus::Activation,
    origin: focus::Origin,
) {
    let request = focus::Request {
        session: r.generation,
        target,
        menu: r.menus.revision,
        activation,
    };
    let observer = r.lifecycle.annotation_foreground.clone();
    let owners = capture_exclusions(r);
    r.focus.restore(request, move |mut request| {
        if request.activation == focus::Activation::UserRequested
            && !origin.permits_activation(observer.as_ref().and_then(|query| query()), &owners)
        {
            request.activation = focus::Activation::Local;
        }
        if restore_matching_input_focus(request)
            && matches!(request.target, focus::Target::Text { .. })
        {
            schedule_render(request.session);
        }
    });
}

fn restore_matching_input_focus(request: focus::Request) -> bool {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return false };
        let live = focus::Context {
            session: r.generation,
            panel: r.panel.revision(),
            menu: r.menus.revision,
            text_editor: r.editor.edit().map(|edit| edit.id),
            watermark_editor: r.watermark.get().map(|_| r.watermark.revision()),
            popup_open: r.menus.current.is_some() && !choice::allows_text_focus(r),
            controls_busy: choice::input_busy(r)
                || r.parameter_edit.dragging()
                || r.panel
                    .get()
                    .is_some_and(|p| p.get_parameter_input_active()),
        };
        if !request.matches(&live) {
            return false;
        }
        match request.target {
            focus::Target::Text { .. } => {
                let Some(editor) = r.editor.window() else {
                    return false;
                };
                editor.invoke_focus_input();
                if request.activation == focus::Activation::UserRequested {
                    (r.lifecycle.activate_user_requested_window)(editor.window());
                }
                reset_text_caret(r);
            }
            focus::Target::Watermark { .. } => {
                r.watermark.get().unwrap().invoke_focus_input();
            }
        }
        true
    })
}

#[cfg(test)]
fn restore_matching_text_focus(
    generation: u64,
    id: u64,
    panel_revision: u64,
    choice_revision: u64,
) -> bool {
    restore_matching_input_focus(focus::Request {
        session: generation,
        target: focus::Target::Text {
            editor: id,
            panel: panel_revision,
        },
        menu: choice_revision,
        activation: focus::Activation::UserRequested,
    })
}

fn begin_text_handle(
    r: &mut Registry,
    object: Object,
    handle: usize,
    point: (f32, f32),
    scale: f32,
    editing: bool,
) {
    let target = if editing {
        r.editor.edit().and_then(|edit| edit.object)
    } else {
        r.session
            .selected
            .and_then(|index| r.session.objects.get(index))
            .map(|object| object.id)
    };
    let handle = gestures::TextHandle::new(object, handle, point, scale, editing, target);
    r.gesture = Some(Active::Text(handle));
}

/// Coordinate accepted parameters and history before the handle adopts its press geometry.
fn update_text_handle(r: &mut Registry, point: (f32, f32)) {
    let Some((info, position)) = r.text_handle().and_then(|handle| {
        handle
            .position(point)
            .map(|position| (handle.info(), position))
    }) else {
        return;
    };
    if !info.active {
        let current = if info.editing {
            editing_text_object(r)
        } else {
            info.target
                .and_then(|id| r.session.object_index(id))
                .and_then(|index| r.session.objects.get(index))
                .cloned()
        };
        let Some(current) = current else {
            return;
        };
        r.finish_style_edit();
        if !info.editing {
            r.begin_document_gesture();
        }
        let Some(handle) = r.gesture.as_mut().and_then(Active::text_mut) else {
            return;
        };
        handle.activate(current);
    }
    let Some(object) = r.text_handle().map(|handle| handle.object_at(position)) else {
        return;
    };
    if info.editing {
        apply_editing_object(r, object);
    } else if let Some(target) = info.target {
        gestures::write(&mut r.session, target, object);
    }
}

fn apply_editing_object(r: &mut Registry, object: Object) {
    let Extra::Text(mut text) = object.extra else {
        return;
    };
    let Some((editor, edit)) = r.editor.parts_mut() else {
        return;
    };
    edit.bounds = object.bounds;
    edit.at = (object.bounds.left, object.bounds.top);
    edit.laid_out_content = text.content.clone();
    place_text_editor(editor, object.bounds, edit.scale, &text, &r.canvases);
    text.content = editor.get_value().to_string();
    edit.original = text;
}

fn finish_text_handle(r: &mut Registry, point: (f32, f32)) {
    let Some(operation) = Active::take_text(&mut r.gesture) else {
        return;
    };
    let gestures::TextHandleInfo {
        handle,
        editing,
        target,
        active,
        scale,
    } = operation.info();
    if editing
        && matches!(handle, 1 | 2)
        && let Some(mut object) = editing_text_object(r)
        && text_handle_at(&object, point, scale) == Some(handle)
    {
        if handle == 1 {
            finish_text_edit_registry(r, false);
            if let Some(index) = target.and_then(|id| r.session.object_index(id)) {
                r.session.selected = Some(index);
                r.session.delete_selected();
            }
        } else {
            object.toggle_text_arrow(scale);
            apply_editing_object(r, object);
        }
    }
    operation.finish(&mut r.session);
    r.gesture = None;
    r.flush_deferred_parameters();
    if !editing
        && active
        && let Some(index) = target.and_then(|id| r.session.object_index(id))
    {
        adopt_selected_style(r, index);
    }
    let parameter_object = r.parameter_object(8);
    if editing && active {
        if !r.editor.sync_draft_dimensions()
            && let Some(edit) = r.editor.edit()
        {
            text_dimensions_to_values(r.parameter_values.edit(8, parameter_object), &edit.original);
        }
        if let Some(panel) = r.panel.get()
            && panel.get_tool() == 8
        {
            panel.set_values(r.values(8).clone());
        }
    }
    r.gesture = None;

    if editing {
        restore_editor_focus(r);
    }
    cursor::refresh(r);
}

/// Eraser wheel input changes only the next gesture, never the document or capture.
fn scroll_eraser_size(r: &mut Registry, delta: f32) {
    if !delta.is_finite()
        || delta == 0.
        || r.gesture().is_some()
        || r.mode != InteractionMode::Tool(6)
        || r.values(13).shape != 1
    {
        return;
    }
    let mut values = r.values(13).clone();
    let Some(size) = numeric::Parameter::Size.step(values.size, 13, delta) else {
        return;
    };
    if size == values.size {
        return;
    }
    values.size = size;
    r.replace_values(13, values.clone());
    if let Some(panel) = r
        .panel
        .get()
        .filter(|panel| !panel.get_menu() && panel.get_tool() == 13)
    {
        panel.set_values(values);
    }
    choice::sync_numeric(r);
    cursor::refresh(r);
}

fn brush_size_scrolled(index: usize, delta: f32) {
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if !delta.is_finite()
            || delta == 0.
            || r.gesture().is_some()
            || (!matches!(active_kind(r), Some(Kind::MosaicBrush | Kind::StickerBrush))
                && !(r.mode == InteractionMode::Tool(6) && r.values(13).shape == 1))
            || index >= r.canvases.len()
        {
            return None;
        }
        if r.mode == InteractionMode::Tool(6) {
            scroll_eraser_size(r, delta);
            return None;
        }
        let tool = if active_kind(r) == Some(Kind::StickerBrush) {
            12
        } else {
            11
        };
        let mut values = r.values(tool).clone();
        values.size = (values.size + if delta > 0. { 1 } else { -1 }).clamp(1, 100);
        if let Some(panel) = r
            .panel
            .get()
            .filter(|p| !p.get_menu() && p.get_tool() == tool as i32)
        {
            panel.set_values(values.clone());
            let target = r.parameter_target(tool as i32);
            apply_geometry_values(r, target, values);
        } else {
            r.replace_values(tool, values);
            let size = r.values(tool).size;
            r.session.update_selected(|object| {
                match &mut object.extra {
                    Extra::Mosaic(value) if object.kind == Kind::MosaicBrush => {
                        value.diameter = size as f32 * value.creation_scale
                    }
                    Extra::Sticker(value) if object.kind == Kind::StickerBrush => {
                        value.diameter = size as f32 * value.creation_scale
                    }
                    _ => return,
                }
                object.refresh_bounds();
            });
            cursor::refresh(r);
        }
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

fn canvas_pointer(index: usize, event: i32, x: f32, y: f32) {
    let mut text_to_open = None;
    let result = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if !r.annotation_ready || r.mode.is_mouse() {
            return None;
        }
        let layer = r.canvases.get(index)?;
        let scale = layer.window.window().scale_factor().max(0.1);
        let point = cursor::screen_point(layer.bounds, (x, y), scale);
        let _input_timing =
            drag_timing::pointer(event, r.generation, index, r.session.objects.len());
        if event == 4 {
            r.canvases[index].last_pointer = None;
            let preview_changed = r.pending_drawing.pointer_left();
            if r.hovered.take().is_some() || preview_changed {
                return Some((r.generation, false));
            }
            return None;
        }
        r.canvases[index].last_pointer = Some(point);
        let mut open_geometry_panel = false;
        match event {
            0 => {
                // A second press cannot reuse an unfinished object's document transaction.
                if r.gesture.as_ref().is_some_and(Active::is_manipulation) {
                    cancel_active_gesture(r);
                }
                r.mosaic.gesture_epoch = r.mosaic.gesture_epoch.wrapping_add(1);
                if r.editor.window().is_some() {
                    if let Some(object) = editing_text_object(r) {
                        if !editing_sequence(r)
                            && let Some(handle) = text_handle_at(&object, point, scale)
                        {
                            begin_text_handle(r, object, handle, point, scale, true);
                            return Some((r.generation, false));
                        }
                        if editing_body_contains(&object, point) {
                            let anchor = select_editing_text(r, point, None);
                            r.gesture = Some(Active::Action(Action::TextSelection { anchor }));
                            restore_editor_focus(r);
                            return Some((r.generation, false));
                        }
                    }
                    let empty = r
                        .editor
                        .window()
                        .is_some_and(|editor| editor.get_value().trim().is_empty());
                    let sequence = editing_sequence(r);
                    finish_text_edit_registry(r, true);
                    if empty && !sequence {
                        // Losing focus on an empty draft ends this gesture; the
                        // same click must not start another blank editor.
                        r.gesture = None;
                        return Some((r.generation, false));
                    }
                }
                if r.mode == InteractionMode::Tool(6) {
                    let values = r.values(13).clone();
                    let views = r
                        .canvases
                        .iter()
                        .map(|c| (c.bounds, c.window.window().scale_factor()))
                        .collect::<Vec<_>>();
                    r.begin_document_gesture();
                    deselect_object(r);
                    r.gesture = Some(Active::Erasing(eraser::Press::begin(
                        &mut r.session,
                        point,
                        values.shape == 1,
                        values.size.clamp(1, 100) as f32 * scale,
                        &views,
                    )));
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                r.hovered = None;
                r.finish_style_edit();

                // A pending arrow may end over another object; finish it before hit testing.
                if r.arrow_pending().is_some() {
                    r.gesture = Some(Active::Action(Action::FinishArrow));
                } else if r.polyline().is_some()
                    || (r.mode == InteractionMode::Tool(2)
                        && r.selected[2] == 6
                        && r.session.hit(point, scale).is_none())
                {
                    if r.polyline().is_none() {
                        let values = capture_creation(r, 6);
                        let style = parameters::style(&values, 6);
                        r.pending_drawing.start_polyline(point, style, &values);
                    }
                    deselect_object(r);
                    r.gesture = Some(Active::Action(Action::PlacePoint));
                } else {
                    match r.session.hit(point, scale) {
                        Some(Hit::Handle(handle)) => {
                            let index = r.session.selected.unwrap();
                            if r.session.objects[index].kind == Kind::Sequence && handle < 5 {
                                r.gesture = Some(Active::Action(Action::SequenceControl {
                                    target: r.session.objects[index].id,
                                    handle,
                                }));
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 3 {
                                begin_text_handle(
                                    r,
                                    r.session.objects[index].clone(),
                                    handle,
                                    point,
                                    scale,
                                    false,
                                );
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 2 {
                                r.gesture = Some(Active::Action(Action::ToggleTextArrow {
                                    target: r.session.objects[index].id,
                                }));
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 1 {
                                r.session.delete_selected();
                                refresh_parameter_target(r);
                                r.gesture = Some(Active::Action(Action::DeleteText));
                                return Some((r.generation, false));
                            }
                            r.begin_document_gesture();
                            r.gesture = (gestures::GeometryDrag::resizing(
                                &r.session,
                                r.session.objects[index].id,
                                handle,
                            ))
                            .map(Active::Geometry);
                        }
                        Some(Hit::Object(index)) => {
                            r.session.selected = Some(index);
                            r.pending_edit_panel = adopt_selected_style(r, index);
                            r.begin_document_gesture();
                            r.gesture = (gestures::GeometryDrag::moving(
                                &r.session,
                                r.session.objects[index].id,
                                point,
                            ))
                            .map(Active::Geometry);
                        }
                        None => {
                            r.pending_edit_panel = false;
                            if r.mode == InteractionMode::Tool(4) {
                                begin_sequence_placement(r, point, scale);
                                cursor::refresh(r);
                                return Some((r.generation, false));
                            }
                            let tool = r.selected[r.mode.selected_group() as usize] as usize;
                            let creation = capture_creation(r, tool);
                            deselect_object(r);
                            if r.mode == InteractionMode::Tool(3) && r.selected[3] == 8 {
                                r.gesture = Some(Active::Action(Action::CreateText {
                                    at: point,
                                    scale,
                                    values: Box::new(creation.clone()),
                                }));
                            } else if matches!(r.mode, InteractionMode::Tool(0..=2 | 5))
                                && let Some(kind) = active_kind(r)
                            {
                                if kind == Kind::Magnifier
                                    && matches!(r.magnifier_sync, MagnifierSync::Pending(_))
                                    && gestures::display_draft(&r.gesture, &r.pending_drawing)
                                        .is_none()
                                    && !r
                                        .session
                                        .objects
                                        .iter()
                                        .any(|object| object.kind == Kind::Magnifier)
                                {
                                    // A cancelled preview may still have a queued native
                                    // callback. A new gesture must receive its own border frame.
                                    r.magnifier_sync_revision =
                                        r.magnifier_sync_revision.wrapping_add(1);
                                    r.magnifier_sync = MagnifierSync::Cold;
                                    r.first_magnifier_timing = None;
                                }
                                let style = parameters::style(&creation, kind.tool() as i32);
                                let values = &creation;
                                r.gesture = Some(Active::Drawing(drawing::Press::begin(
                                    point, kind, style, scale, values,
                                )));
                            }
                        }
                    }
                }
            }
            1 => match r.gesture() {
                Some(Gesture::Erase) => {
                    let views = r
                        .canvases
                        .iter()
                        .map(|c| (c.bounds, c.window.window().scale_factor()))
                        .collect::<Vec<_>>();
                    if let Some(Active::Erasing(press)) = &mut r.gesture {
                        press.update(&mut r.session, point, &views);
                    }
                }
                Some(Gesture::PlaceSequence) => update_sequence_placement(r, point),
                Some(Gesture::SequenceControl { .. }) => {}
                Some(Gesture::DeleteText | Gesture::ToggleTextArrow { .. }) => {}
                Some(Gesture::CreateText { .. }) => {}
                Some(Gesture::PlacePoint | Gesture::FinishArrow) => {
                    let expected = if matches!(r.gesture(), Some(Gesture::PlacePoint)) {
                        Kind::Polyline
                    } else {
                        Kind::Arrow
                    };
                    r.pending_drawing
                        .pointer_moved(point, scale, Some(expected));
                }
                Some(Gesture::Draw { kind }) => {
                    if kind == Kind::Magnifier
                        && r.magnifier_sync == MagnifierSync::Cold
                        && !r.first_magnifier_measured
                        && r.first_magnifier_timing.is_none()
                    {
                        r.first_magnifier_timing = Some(FirstMagnifierTiming::new());
                    }
                    if let Some(press) = r.gesture.as_mut().and_then(Active::drawing_mut) {
                        press.update(point, scale);
                    }
                }

                Some(Gesture::Move | Gesture::Resize { .. }) => {
                    if let Some(drag) = r.gesture.as_ref().and_then(Active::geometry) {
                        drag.update(&mut r.session, point, scale);
                    }
                }
                Some(Gesture::TextHandle { .. }) => {
                    update_text_handle(r, point);
                }
                Some(Gesture::TextSelection { anchor }) => {
                    select_editing_text(r, point, Some(anchor));
                }
                None => {
                    if !r.pending_drawing.is_none() {
                        r.pending_drawing.pointer_moved(point, scale, None);
                        return Some((r.generation, false));
                    }
                    let hovered = hovered_geometry_at(&r.session, point, scale);
                    let changed = r.hovered != hovered;
                    r.hovered = hovered;
                    cursor::refresh(r);
                    return changed.then_some((r.generation, false));
                }
            },
            2 => {
                // Native release can arrive after the gesture retired or from another control.
                // It cannot finish a snapshot owned by a continuous parameter operation.
                r.gesture()?;
                if matches!(r.gesture(), Some(Gesture::Erase)) {
                    let views = r
                        .canvases
                        .iter()
                        .map(|c| (c.bounds, c.window.window().scale_factor()))
                        .collect::<Vec<_>>();
                    if let Some(Active::Erasing(mut press)) = r.gesture.take() {
                        press.update(&mut r.session, point, &views);
                    }
                    r.session.finish_drag();
                    r.flush_deferred_parameters();
                    sync_history_controls(r);
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if matches!(r.gesture(), Some(Gesture::PlaceSequence)) {
                    let placement = Active::take_sequence(&mut r.gesture)?;
                    let target = placement.finish(&mut r.session, point);
                    r.gesture = None;
                    r.flush_deferred_parameters();
                    r.session.selected = r.session.object_index(target);
                    if let Some(main) = r.main.get() {
                        main.set_can_undo(r.session.can_undo());
                        if let Some(panel) = r.panel.get() {
                            panel.set_can_clear(r.session.has_content());
                        }
                    }
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::SequenceControl { target, handle }) = r.gesture() {
                    r.gesture = None;
                    let Some(index) = r.session.object_index(target) else {
                        return Some((r.generation, false));
                    };
                    if r.session.selected == Some(index)
                        && r.session.hit(point, scale) == Some(Hit::Handle(handle))
                    {
                        if handle == 4 {
                            text_to_open =
                                Some((point, Some(r.session.objects[index].id), scale, None));
                        } else if handle == 2 {
                            r.session.delete_selected();
                        } else {
                            r.session.update_selected(|object| {
                                if handle == 3 {
                                    object.toggle_text_arrow(scale);
                                } else if let Extra::Sequence(s) = &mut object.extra {
                                    s.value = if handle == 0 {
                                        s.value.saturating_add(1)
                                    } else {
                                        (s.value - 1).max(1)
                                    };
                                }
                                crate::annotation_sequence::layout(object);
                            });
                        }
                    }
                    if let Some(main) = r.main.get() {
                        main.set_can_undo(r.session.can_undo());
                        if let Some(panel) = r.panel.get() {
                            panel.set_can_clear(r.session.has_content());
                        }
                    }
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::TextSelection { anchor }) = r.gesture() {
                    select_editing_text(r, point, Some(anchor));
                    r.gesture = None;
                    restore_editor_focus(r);
                    return Some((r.generation, false));
                }
                if matches!(r.gesture(), Some(Gesture::TextHandle { .. })) {
                    update_text_handle(r, point);
                    finish_text_handle(r, point);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::ToggleTextArrow { target }) = r.gesture() {
                    let Some(index) = r.session.object_index(target) else {
                        r.gesture = None;
                        return Some((r.generation, false));
                    };
                    if r.session.selected == Some(index)
                        && r.session.objects.get(index).is_some()
                        && r.session.hit(point, scale) == Some(Hit::Handle(2))
                    {
                        r.session
                            .update_selected(|object| object.toggle_text_arrow(scale));
                    }
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                if matches!(r.gesture(), Some(Gesture::DeleteText)) {
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                if let Some(Gesture::CreateText { at, scale }) = r.gesture().as_ref() {
                    let values = match &r.gesture {
                        Some(Active::Action(Action::CreateText { values, .. })) => {
                            Some((**values).clone())
                        }
                        _ => None,
                    };
                    text_to_open = Some((*at, None, *scale, values));
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                open_geometry_panel = std::mem::take(&mut r.pending_edit_panel);
                if matches!(r.gesture(), Some(Gesture::FinishArrow)) {
                    r.gesture = None;
                    if let Some(object) = r.pending_drawing.release_arrow(point) {
                        r.session.add(object);
                    }
                } else if matches!(r.gesture(), Some(Gesture::PlacePoint)) {
                    r.gesture = None;
                    let style = current_style(r, 6);
                    let object = r.parameter_object(6);
                    let values = r.parameter_values.values(6, object);
                    if let Some(drawing::Progress::Retired(Some(object))) = r
                        .pending_drawing
                        .place_polyline(point, scale, Instant::now(), style, values)
                    {
                        r.session.add(*object);
                    }
                } else if matches!(r.gesture(), Some(Gesture::Draw { .. })) {
                    let press = Active::take_drawing(&mut r.gesture)?;
                    match press.release(point, scale) {
                        Some(drawing::Release::Add(object)) => {
                            r.session.add(object);
                        }
                        Some(drawing::Release::StageMosaic(object)) => {
                            r.mosaic.stage(&mut r.session, object);
                        }
                        Some(drawing::Release::AwaitArrow(arrow)) => {
                            r.pending_drawing.await_arrow(arrow);
                        }
                        Some(drawing::Release::Magnifier {
                            source,
                            style,
                            values,
                        }) => {
                            if r.magnifier_sync == MagnifierSync::Cold
                                && !r.first_magnifier_measured
                                && r.first_magnifier_timing.is_none()
                            {
                                r.first_magnifier_timing = Some(FirstMagnifierTiming::new());
                            }
                            if let Some(object) = magnifier_from_source(source, style, &values) {
                                r.session.add(object);
                            }
                        }
                        None => {}
                    }
                } else {
                    if let Some(drag) = r.geometry_drag()
                        && let Some(start) = drag.move_start()
                        && let Some(index) = r.session.object_index(drag.target())
                        && (r.session.objects[index].kind == Kind::Text
                            || (r.session.objects[index].kind == Kind::Sequence
                                && r.session.objects[index]
                                    .sequence_caption()
                                    .is_some_and(|o| {
                                        point.0 >= o.bounds.left
                                            && point.0 <= o.bounds.right
                                            && point.1 >= o.bounds.top
                                            && point.1 <= o.bounds.bottom
                                    })))
                        && (point.0 - start.0).hypot(point.1 - start.1) <= 3. * scale
                    {
                        text_to_open =
                            Some((point, Some(r.session.objects[index].id), scale, None));
                        open_geometry_panel = false;
                    }
                    if let Some(drag) = Active::take_geometry(&mut r.gesture) {
                        drag.finish(&mut r.session);
                    }
                    r.gesture = None;
                    r.flush_deferred_parameters();
                    if let Some(index) = r.session.selected {
                        adopt_selected_style(r, index);
                    }
                }
            }
            3 => {
                cancel_active_gesture(r);
                mosaic::cancel_pending(r);
                r.hovered = None;
            }
            _ => return None,
        }
        sync_history_controls(r);
        refresh_parameter_target(r);
        cursor::refresh(r);
        Some((r.generation, open_geometry_panel))
    });
    if let Some((generation, open)) = result {
        schedule_render(generation);
        if let Some((point, target, scale, values)) = text_to_open {
            open_text_editor_with_settings(generation, point, target, scale, values);
        }
        if open {
            queue_geometry_panel(generation, index);
        }
    }
}

fn queue_geometry_panel(generation: u64, index: usize) {
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().filter(|r| r.generation == generation)?;
        let layer = r.canvases.get(index)?;
        let target = r
            .session
            .selected
            .and_then(|selected| r.session.objects.get(selected))
            .map(|object| object.id)?;
        let group = r.mode.selected_group().max(0) as usize;
        Some((layer.input(), layer.revision(), target, group))
    });
    if let Some((input, revision, target, group)) = request {
        input.post(move || {
            let valid = REGISTRY.with(|slot| {
                slot.borrow().as_ref().is_some_and(|r| {
                    r.generation == generation
                        && !r.mode.is_mouse()
                        && r.canvases
                            .get(index)
                            .is_some_and(|layer| layer.revision() == revision)
                        && r.mode.selected_group().max(0) as usize == group
                        && r.session
                            .selected
                            .and_then(|selected| r.session.objects.get(selected))
                            .is_some_and(|object| object.id == target)
                })
            });
            if valid {
                open_panel(group, false);
            }
        });
    }
}

fn hovered_geometry_at(session: &Session, point: (f32, f32), scale: f32) -> Option<usize> {
    match session.hit(point, scale) {
        Some(Hit::Object(index))
            if session.selected != Some(index)
                && matches!(
                    session.objects[index].kind,
                    Kind::Rectangle | Kind::Ellipse | Kind::HighlightRectangle
                ) =>
        {
            Some(index)
        }
        _ => None,
    }
}

fn refresh_hovered(r: &mut Registry) {
    r.hovered = if r.mode.is_mouse() {
        None
    } else {
        r.canvases.iter().find_map(|layer| {
            let point = layer.last_pointer?;
            hovered_geometry_at(
                &r.session,
                point,
                layer.window.window().scale_factor().max(0.1),
            )
        })
    };
}

fn schedule_render(generation: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        r.queue_render();
    });
}

/// Shares the editor's current geometry between painting and cursor hit testing.
fn editing_text_object(r: &Registry) -> Option<Object> {
    let (Some(edit), Some(editor)) = (r.editor.edit(), r.editor.window()) else {
        return None;
    };
    if edit.object.is_some() && edit.index(&r.session).is_none() {
        return None;
    }
    let mut visible = edit.original.clone();
    visible.content = editor_preview(editor).0;
    if editor.get_native_visual() {
        visible.content.clear();
        visible.outline = None;
    }
    let mut object = edit
        .index(&r.session)
        .and_then(|index| r.session.objects.get(index))
        .cloned()
        .unwrap_or_else(|| Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: edit.bounds,
            kind: Kind::Text,
            style: current_style(r, 8),
            points: Vec::new(),
            extra: Extra::None,
        });
    object.bounds = edit.bounds;
    if let Extra::Sequence(s) = &object.extra {
        object.style.color = s.caption_color;
    }
    object.kind = Kind::Text;
    object.extra = Extra::Text(visible);
    Some(object)
}

/// Own only the edited object; every other native scene object stays borrowed.
fn edited_document_object(r: &Registry) -> Option<(Option<usize>, Object)> {
    let edit = r.editor.edit()?;
    let visible = editing_text_object(r)?;
    let index = edit.index(&r.session);
    if let Some(index) = index
        && matches!(r.session.objects[index].extra, Extra::Sequence(_))
    {
        let mut object = r.session.objects[index].clone();
        if let (Extra::Sequence(sequence), Extra::Text(text)) = (&mut object.extra, visible.extra) {
            sequence.caption = text;
            sequence.caption_bounds = visible.bounds;
        }
        return Some((Some(index), object));
    }
    Some((index, visible))
}

fn render_canvases(generation: u64) {
    let _render_timing = drag_timing::Span::new(drag_timing::Phase::Render);
    let deferred_revision = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        r.frame_tasks.cancel_render();
        if r.canvases.is_empty() {
            return None;
        }
        let edited_object = edited_document_object(r);
        let display_document = crate::annotation_scene::Document {
            session: &r.session,
            edit: edited_object
                .as_ref()
                .map(|(index, object)| (*index, object)),
        };
        let confirmed = r
            .editor
            .window()
            .filter(|editor| !editor.get_preedit().is_empty())
            .map(|editor| editor.get_value().to_string());
        let caret = r
            .editor
            .window()
            .filter(|editor| !editor.get_native_visual())
            .and_then(|editor| {
                display_document.selected().map(|index| {
                    let (_, preedit) = editor_preview(editor);
                    let offset = if preedit.is_empty() {
                        editor.get_cursor_offset().max(0) as usize
                    } else {
                        preedit.end
                    };
                    (
                        index,
                        offset,
                        preedit,
                        editor.get_anchor_offset().max(0) as usize,
                        r.editor.edit().is_some_and(|e| e.caret.visible),
                    )
                })
            });
        let editor_overlay =
            caret
                .as_ref()
                .and_then(|(index, offset, preedit, anchor, visible)| {
                    display_document.get(*index).map(|object| {
                        crate::annotation_scene::EditorOverlay {
                            object: object.sequence_caption().unwrap_or_else(|| object.clone()),
                            offset: *offset,
                            anchor: *anchor,
                            preedit: preedit.clone(),
                            visible: *visible,
                            reference: confirmed.clone(),
                        }
                    })
                });
        let active_canvas = r
            .canvases
            .iter()
            .position(|layer| layer.last_pointer.is_some())
            .unwrap_or(0);
        let canvas_order = std::iter::once(active_canvas)
            .chain((0..r.canvases.len()).filter(|&index| index != active_canvas));
        let has_effects = display_document
            .objects()
            .chain(gestures::display_draft(&r.gesture, &r.pending_drawing))
            .any(|object| {
                matches!(
                    object.kind,
                    Kind::MosaicArea | Kind::MosaicBrush | Kind::Magnifier
                )
            });
        let excluded = capture_exclusions(r);
        let native_scene = if has_effects
            && !r
                .native_presenter
                .available(r.lifecycle.annotation_renderer.as_ref())
        {
            None
        } else {
            use lexift_core::ports::annotation_render::{RenderStamp, RenderSurface};
            let surfaces: Option<Vec<_>> = r
                .canvases
                .iter()
                .map(|layer| {
                    Some(RenderSurface {
                        input_window: (r.lifecycle.annotation_window_token)(layer.window.window())?,
                        bounds: layer.bounds,
                        scale: layer.window.window().scale_factor(),
                    })
                })
                .collect();
            surfaces.and_then(|surfaces| {
                let _compile_timing = drag_timing::Span::new(drag_timing::Phase::Compile);
                r.scene_revision = r.scene_revision.wrapping_add(1);
                r.scene_compiler
                    .compile(crate::annotation_scene::SceneInput {
                        stamp: RenderStamp {
                            session: generation,
                            revision: r.scene_revision,
                        },
                        session: &r.session,
                        edit: display_document.edit,
                        surfaces: &surfaces,
                        draft: gestures::display_draft(&r.gesture, &r.pending_drawing),
                        hovered: r.hovered,
                        reference: confirmed.as_deref(),
                        editor: editor_overlay.as_ref(),
                        paused_regions: &[],
                        excluded_windows: &excluded,
                    })
            })
        };
        // Successful native compilation already pruned the same viewport caches.
        // Keep software cleanup for unavailable or invalid native surfaces.
        if native_scene.is_none() {
            r.scene_compiler.retain_viewports(
                &r.canvases
                    .iter()
                    .map(|layer| layer.bounds)
                    .collect::<Vec<_>>(),
            );
        }
        if native_scene.is_none() && r.native_presenter.active {
            r.native_presenter
                .suspend(r.lifecycle.annotation_renderer.as_ref());
        }
        if let Some(scene) = &native_scene
            && r.native_presenter
                .available(r.lifecycle.annotation_renderer.as_ref())
        {
            r.mosaic
                .stop_software(r.lifecycle.annotation_screen_capture.as_ref());
            let port = r.lifecycle.annotation_renderer.as_ref().unwrap();
            let _submit_timing = drag_timing::Span::new(drag_timing::Phase::Submit);
            match r
                .native_presenter
                .submit(port.as_ref(), std::sync::Arc::new(scene.clone()))
            {
                Ok(()) => r.native_presenter.arm(generation, poll_native_renderer),
                Err(error) => {
                    tracing::warn!(%error, "native annotation submission failed; using software");
                    r.native_presenter
                        .fail(r.lifecycle.annotation_renderer.as_ref());
                }
            }
            if r.native_presenter.active {
                let mut cleared = true;
                for layer in &mut r.canvases {
                    if layer.gpu_input_cleared {
                        continue;
                    }
                    let frame = tiny_skia::Pixmap::new(
                        (i64::from(layer.bounds.right) - i64::from(layer.bounds.left)) as u32,
                        (i64::from(layer.bounds.bottom) - i64::from(layer.bounds.top)) as u32,
                    );
                    let success = frame.is_some_and(|frame| {
                        layer.presented.submit(frame, |frame| {
                            (r.lifecycle.annotation_frame_presenter)(
                                layer.window.window(),
                                frame.data(),
                                frame.width(),
                                frame.height(),
                            )
                        })
                    });
                    layer.gpu_input_cleared = success;
                    layer.signature = None;
                    cleared &= success;
                }
                if cleared {
                    return None;
                }
                r.native_presenter
                    .fail(r.lifecycle.annotation_renderer.as_ref());
                tracing::warn!("native annotation input surface handoff failed; using software");
            }
        }
        let native_waiting = native_scene.is_some()
            && r.native_presenter
                .available(r.lifecycle.annotation_renderer.as_ref());
        let (deferred_revision, measure_first_frame) = if native_waiting {
            (None, false)
        } else {
            mosaic::sync(r);
            let views = magnifier_views(
                &r.session,
                gestures::display_draft(&r.gesture, &r.pending_drawing),
            );
            let drawing_magnifier = matches!(
                r.gesture(),
                Some(Gesture::Draw {
                    kind: Kind::Magnifier,
                    ..
                })
            );
            let had_native_state = r.magnifier_sync != MagnifierSync::Cold;
            let deferred_revision = match magnifier_frame_action(
                &mut r.magnifier_sync,
                &mut r.magnifier_sync_revision,
                !views.is_empty(),
            ) {
                MagnifierFrameAction::Clear => {
                    r.frame_tasks.cancel_lens();
                    if !drawing_magnifier {
                        r.first_magnifier_timing = None;
                    }
                    if had_native_state {
                        sync_magnifier_views(r, &views);
                    }
                    None
                }
                MagnifierFrameAction::Defer(revision) => {
                    if !r.first_magnifier_measured {
                        let timing = r
                            .first_magnifier_timing
                            .get_or_insert_with(FirstMagnifierTiming::new);
                        timing.frame_start.get_or_insert_with(Instant::now);
                    }
                    Some(revision)
                }
                MagnifierFrameAction::Sync => {
                    sync_magnifier_views(r, &views);
                    None
                }
                MagnifierFrameAction::Wait => None,
            };
            let measure_first_frame = r
                .first_magnifier_timing
                .as_ref()
                .is_some_and(|timing| timing.border_submitted.is_none());
            if measure_first_frame && let Some(timing) = r.first_magnifier_timing.as_mut() {
                timing.frame_start.get_or_insert_with(Instant::now);
            }
            // A failed first request may discard provisional objects; rebuild
            // only the editor substitution after all document mutations.
            (deferred_revision, measure_first_frame)
        };
        let edited_object = edited_document_object(r);
        let display_document = crate::annotation_scene::Document {
            session: &r.session,
            edit: edited_object
                .as_ref()
                .map(|(index, object)| (*index, object)),
        };
        let signatures = crate::annotation_signature::Frame::new(
            display_document,
            gestures::display_draft(&r.gesture, &r.pending_drawing),
        );
        if !native_waiting {
            let monitors = r
                .canvases
                .iter()
                .map(|canvas| (canvas.bounds, canvas.window.window().scale_factor()));
            r.mosaic.submit(
                &signatures,
                r.gesture().as_ref(),
                r.gesture.as_ref().and_then(Active::geometry),
                r.gesture.as_ref().and_then(Active::drawing),
                monitors,
            );
        }
        let mosaic_effects = r.mosaic.effects.clone();
        let mosaic_preview = r.mosaic.preview.as_ref();
        let mosaic_view =
            mosaic_effects
                .as_deref()
                .map(|frame| crate::annotation_render::mosaic_worker::View {
                    frame,
                    preview: mosaic_preview.as_ref().map(|p| (p.index, &p.original)),
                    remask: mosaic_preview.as_ref().is_some_and(|p| p.remask),
                });
        let mut software_presented = true;
        for index in canvas_order {
            let layer = &mut r.canvases[index];
            if layer.gpu_input_cleared {
                layer.gpu_input_cleared = false;
                layer.signature = None;
            }
            let mut signature = signatures.display(
                layer.bounds,
                layer.window.window().scale_factor(),
                r.hovered,
            );
            if let Some(view) = mosaic_view {
                signature ^= view.raster_signature_with(layer.bounds, &signatures);
            }
            if let Some((_, offset, ref preedit, anchor, visible)) = caret {
                signature ^= u64::from(visible).wrapping_mul(0xa24b_aed4_963e_e407);
                signature ^= (offset as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                signature ^= (preedit.start as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
                signature ^= (preedit.len() as u64).wrapping_mul(0x94d0_49bb_1331_11eb);
                signature ^= (anchor as u64).wrapping_mul(0xd6e8_feb8_6659_fd93);
            }
            if let Some(notice) = &r.mosaic.notice {
                signature ^= notice.revision.wrapping_mul(0x83b4c87f91ec7401);
            }
            if layer.signature == Some(signature) {
                continue;
            }
            layer.signature = Some(signature);
            let raster_start = Instant::now();
            let raster_timing = drag_timing::Span::new(drag_timing::Phase::Raster);
            let frame = if has_effects {
                crate::annotation_render::render_document_with_effects(
                    display_document,
                    layer.bounds,
                    layer.window.window().scale_factor(),
                    gestures::display_draft(&r.gesture, &r.pending_drawing),
                    r.hovered,
                    confirmed.as_deref(),
                    mosaic_view,
                )
            } else if let Some(scene) = &native_scene {
                crate::annotation_scene::compose_document(scene, layer.bounds)
            } else {
                r.scene_compiler
                    .render_scene(crate::annotation_scene::RenderInput {
                        session: &r.session,
                        edit: display_document.edit,
                        viewport: layer.bounds,
                        scale: layer.window.window().scale_factor(),
                        draft: gestures::display_draft(&r.gesture, &r.pending_drawing),
                        hovered: r.hovered,
                        reference: confirmed.as_deref(),
                        editor: editor_overlay.as_ref(),
                    })
            };
            drop(raster_timing);
            if let Some(mut frame) = frame {
                if has_effects && let Some(editor) = &editor_overlay {
                    editor.draw(
                        &mut frame,
                        layer.bounds,
                        layer.window.window().scale_factor(),
                    );
                }
                if measure_first_frame && let Some(timing) = r.first_magnifier_timing.as_mut() {
                    timing.raster += raster_start.elapsed();
                }
                if let (Some(notice), Some(toolbar)) = (&r.mosaic.notice, r.main.get()) {
                    mosaic::draw_notice(
                        &mut frame,
                        notice,
                        toolbar,
                        layer.bounds,
                        layer.window.window().scale_factor(),
                    );
                }
                let present_start = Instant::now();
                let submit_timing = drag_timing::Span::new(drag_timing::Phase::Submit);
                if !layer.presented.submit(frame, |frame| {
                    (r.lifecycle.annotation_frame_presenter)(
                        layer.window.window(),
                        frame.data(),
                        frame.width(),
                        frame.height(),
                    )
                }) {
                    // Keep the previous successful pixels and allow this render
                    // signature to retry after a transient native failure.
                    layer.signature = None;
                    software_presented = false;
                }
                drop(submit_timing);
                if measure_first_frame && let Some(timing) = r.first_magnifier_timing.as_mut() {
                    timing.present += present_start.elapsed();
                }
            } else {
                layer.signature = None;
                software_presented = false;
            }
        }
        if r.native_presenter.retaining() {
            let monitors: Vec<_> = r.canvases.iter().map(|c| c.bounds).collect();
            let needs_mosaic = display_document
                .objects()
                .chain(gestures::display_draft(&r.gesture, &r.pending_drawing))
                .any(|object| {
                    matches!(&object.extra, Extra::Mosaic(settings) if settings.strength > 0)
                        && !crate::annotation_render::mosaic::work_regions(object, &monitors)
                            .is_empty()
                });
            let needs_lens = !magnifier_views(
                &r.session,
                gestures::display_draft(&r.gesture, &r.pending_drawing),
            )
            .is_empty();
            if software_presented
                && (!needs_mosaic || r.mosaic.handoff_ready())
                && (!needs_lens || r.magnifier_sync == MagnifierSync::Live)
            {
                r.native_presenter
                    .finish_software_handoff(r.lifecycle.annotation_renderer.as_ref());
            } else {
                r.native_presenter.arm(generation, retry_software_handoff);
            }
        }
        if measure_first_frame && let Some(timing) = r.first_magnifier_timing.as_mut() {
            timing.border_submitted = Some(Instant::now());
        }
        deferred_revision
    });
    if let Some(revision) = deferred_revision {
        // Returning to the event loop lets the compositor show the border
        // before the cold native Magnification API initialization begins.
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            if let Some(r) = slot.as_ref().filter(|r| {
                deferred_sync_is_current(r.generation, r.magnifier_sync, generation, revision)
            }) {
                r.frame_tasks
                    .defer_lens(generation, revision, sync_first_magnifier);
            }
        });
    }
}

fn retry_software_handoff(generation: u64) {
    render_canvases(generation);
}

fn poll_native_renderer(generation: u64) {
    let _confirm_timing = drag_timing::Span::new(drag_timing::Phase::Confirm);
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return false;
        };
        let Some(port) = r.lifecycle.annotation_renderer.as_ref() else {
            return false;
        };
        match r.native_presenter.poll(port.as_ref()) {
            Ok(presenter::Poll::Waiting) => {
                r.native_presenter.arm(generation, poll_native_renderer);
                false
            }
            Ok(presenter::Poll::Ready { activated, pending }) => {
                r.mosaic
                    .confirm_native(r.native_presenter.confirmed_objects());
                if pending || r.native_presenter.live() {
                    r.native_presenter.arm(generation, poll_native_renderer);
                }
                activated
            }
            Err(error) => {
                tracing::warn!(%error, "native annotation presentation failed; using software");
                r.native_presenter
                    .fail(r.lifecycle.annotation_renderer.as_ref());
                for layer in &mut r.canvases {
                    layer.signature = None;
                }
                true
            }
        }
    });
    if changed {
        schedule_render(generation);
    }
}

fn sync_magnifier_views(
    r: &mut Registry,
    views: &[lexift_core::ports::magnifier::MagnifierSpec],
) -> bool {
    let mut overlay_windows = r
        .canvases
        .iter()
        .map(|layer| layer.window.window())
        .collect::<Vec<_>>();
    if let Some(main) = r.main.get() {
        overlay_windows.push(main.window());
    }
    if let Some(panel) = r.panel.get() {
        overlay_windows.push(panel.window());
    }
    if let Some(choice) = &r.menus.current {
        overlay_windows.push(choice.window.window());
    }
    let success = (r.lifecycle.annotation_magnifier_sync)(views, &overlay_windows);
    r.magnifier_sync = if views.is_empty() {
        MagnifierSync::Cold
    } else if success {
        MagnifierSync::Live
    } else {
        // A failed native setup may have left a partially created view.
        (r.lifecycle.annotation_magnifier_sync)(&[], &[]);
        MagnifierSync::Failed
    };
    success
}

fn sync_first_magnifier(generation: u64, revision: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            deferred_sync_is_current(r.generation, r.magnifier_sync, generation, revision)
        }) else {
            return;
        };
        let views = magnifier_views(
            &r.session,
            gestures::display_draft(&r.gesture, &r.pending_drawing),
        );
        if views.is_empty() {
            r.first_magnifier_timing = None;
            sync_magnifier_views(r, &[]);
            return;
        }
        let native_start = Instant::now();
        let success = sync_magnifier_views(r, &views);
        let native = native_start.elapsed();
        if let Some(timing) = r.first_magnifier_timing.take() {
            let border = timing
                .border_submitted
                .map(|submitted| submitted.duration_since(timing.first_move));
            tracing::debug!(
                native_ok = success,
                queue_ms = timing
                    .frame_start
                    .map(|started| started.duration_since(timing.first_move).as_millis() as u64),
                raster_ms = timing.raster.as_millis() as u64,
                present_ms = timing.present.as_millis() as u64,
                border_ms = border.map(|elapsed| elapsed.as_millis() as u64),
                native_ms = native.as_millis() as u64,
                "first annotation magnifier frame"
            );
            r.first_magnifier_measured = true;
        }
    });
}

fn deferred_sync_is_current(
    current_generation: u64,
    state: MagnifierSync,
    callback_generation: u64,
    callback_revision: u64,
) -> bool {
    current_generation == callback_generation && state == MagnifierSync::Pending(callback_revision)
}

/// Maps committed magnifiers to stable native window IDs.
fn magnifier_views(
    session: &Session,
    draft: Option<&Object>,
) -> Vec<lexift_core::ports::magnifier::MagnifierSpec> {
    session
        .objects
        .iter()
        .chain(draft)
        .enumerate()
        .filter_map(|(index, object)| {
            let Extra::Magnifier {
                output,
                zoom,
                ellipse,
                antialias,
                ..
            } = object.extra
            else {
                return None;
            };
            Some(lexift_core::ports::magnifier::MagnifierSpec {
                id: index as u64,
                preview: index == session.objects.len(),
                source: object.bounds,
                output,
                zoom,
                ellipse,
                antialias,
            })
        })
        .collect()
}

/// Both render backends exclude annotation UI by exact native identity, not process ownership.
fn capture_exclusions(r: &Registry) -> Vec<lexift_core::ports::magnifier::WindowToken> {
    let mut windows: Vec<_> = r
        .canvases
        .iter()
        .map(|layer| layer.window.window())
        .chain(r.main.get().map(ComponentHandle::window))
        .chain(r.panel.get().map(ComponentHandle::window))
        .chain(r.editor.window().map(ComponentHandle::window))
        .chain(r.watermark.get().map(ComponentHandle::window))
        .chain(r.menus.current.as_ref().map(|menu| menu.window.window()))
        .filter_map(|window| (r.lifecycle.annotation_window_token)(window))
        .collect();
    windows.sort_unstable_by_key(|window| window.0);
    windows.dedup();
    windows
}

fn render_signature<'a>(
    document: impl Into<crate::annotation_scene::Document<'a>>,
    monitor: Rect,
    scale: f32,
    draft: Option<&'a Object>,
    hovered: Option<usize>,
) -> u64 {
    crate::annotation_signature::Frame::new(document, draft).display(monitor, scale, hovered)
}

#[cfg(test)]
fn document_signature<'a>(
    document: impl Into<crate::annotation_scene::Document<'a>>,
    monitor: Rect,
    scale: f32,
    draft: Option<&'a Object>,
    replacement: Option<(usize, &Object)>,
) -> u64 {
    let frame = crate::annotation_signature::Frame::new(document, draft);
    let replacement = replacement
        .map(|(index, object)| crate::annotation_signature::Replacement::new(index, object.into()));
    frame.document_signature(monitor, scale, replacement.as_ref())
}

fn activate_mouse_mode() {
    let generation = REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            if r.mode == InteractionMode::Mouse {
                r.close_panel();
                return None;
            }
            if !r.set_mode(InteractionMode::Mouse) {
                return None;
            }
            r.finish_style_edit();
            r.close_panel();
            // set_mode already retired the active gesture through its owner.
            deselect_object(r);
            return Some(r.generation);
        }
        None
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

fn undo() {
    let generation = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if cancel_object_gesture(r) {
            cursor::refresh(r);
            return Some(r.generation);
        }
        if r.editor.window().is_some() {
            finish_text_edit_registry(r, true);
        }
        r.finish_style_edit();
        let erased = eraser::has_coverage(&r.session);
        if !r.session.undo() {
            return None;
        }
        if erased || eraser::has_coverage(&r.session) {
            // Restoring alpha coverage must retire the accepted native candidate,
            // not leave a pre-undo presentation eligible for composition reuse.
            r.native_presenter
                .suspend(r.lifecycle.annotation_renderer.as_ref());
            for layer in &mut r.canvases {
                layer.gpu_input_cleared = false;
                layer.signature = None;
            }
        }
        restore_parameter_projection(r);
        sync_sequence_counter(r);
        adopt_watermark_style(r);
        refresh_hovered(r);
        sync_history_controls(r);
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}
fn delete_selected() {
    let generation = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if cancel_sequence_placement(r) {
            cursor::refresh(r);
            return Some(r.generation);
        }
        cancel_object_gesture(r);
        if r.editor.window().is_some() {
            finish_text_edit_registry(r, false);
        }
        r.session.delete_selected();
        refresh_parameter_target(r);
        refresh_hovered(r);
        sync_history_controls(r);
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

fn show_main(generation: u64, revision: u64, attempt: u8) {
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.main.revision() == revision)
        else {
            return;
        };
        let Some(main) = r.main.get().map(ComponentHandle::clone_strong) else {
            return;
        };
        let prepare_start = Instant::now();
        let preparation = (r.prepare)(main.window());
        if let Some(timing) = r.cold_open_timing.as_mut() {
            let elapsed = prepare_start.elapsed();
            timing.toolbar_prepare += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
        }
        match preparation {
            PassiveWindowPreparation::Ready => {
                let place_start = Instant::now();
                let anchor = (r.lifecycle.toolbar_cursor_position)().unwrap_or_default();
                if let Some(area) = (r.lifecycle.popup_work_area)(anchor) {
                    let scale = main.window().scale_factor();
                    let width = (650. * scale).min((area.right - area.left) as f32).max(1.);
                    main.window()
                        .set_size(slint::PhysicalSize::new(width as u32, (54. * scale) as u32));
                    let size = main.window().size();
                    let p = placement::place_popup(
                        anchor,
                        size.width,
                        size.height,
                        area,
                        (16. * scale) as i32,
                        (16. * scale) as i32,
                    )
                    .position;
                    main.window()
                        .set_position(slint::PhysicalPosition::new(p.x, p.y));
                }
                if let Some(timing) = r.cold_open_timing.as_mut() {
                    let elapsed = place_start.elapsed();
                    timing.toolbar_place += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                }
                let show_start = Instant::now();
                if !sync_toolbar_owner(r) {
                    retry_main_owner(r, generation, revision, attempt);
                    return;
                }
                if main.show().is_err() {
                    r.close();
                    return;
                }
                if let Some(timing) = r.cold_open_timing.as_mut() {
                    let elapsed = show_start.elapsed();
                    timing.toolbar_show += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                }
                let native_start = Instant::now();
                if !(r.lifecycle.complete_passive_window_show)(
                    main.window(),
                    pointer_main(main.as_weak(), generation, revision, r.main.input()),
                ) {
                    r.close();
                    return;
                }
                if let Some(timing) = r.cold_open_timing.as_mut() {
                    let elapsed = native_start.elapsed();
                    timing.toolbar_native_complete += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                }
                let finish_start = Instant::now();
                r.toolbar_owner = None;
                if !sync_toolbar_owner(r) {
                    let _ = main.hide();
                    retry_main_owner(r, generation, revision, attempt);
                    return;
                }
                settle_main_position(r, generation, revision, anchor, 2);
                r.timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(33),
                    reposition_panel,
                );
                r.layout_timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_secs(1),
                    check_display_layout,
                );
                r.magnifier_timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(66),
                    refresh_magnifiers,
                );
                r.annotation_ready = true;
                r.pending_displays.clear();
                // Build the empty native scene while the canvases are ready,
                // before the first drag has to pay for software/native handoff.
                // Submission remains asynchronous; no capture or UI wait is added.
                if r.native_presenter
                    .available(r.lifecycle.annotation_renderer.as_ref())
                {
                    r.queue_render();
                }
                if let Some(mut timing) = r.cold_open_timing.take() {
                    let elapsed = finish_start.elapsed();
                    timing.toolbar_finish += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                    timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
                    timing.report("ready", r.canvases.len());
                    r.cold_open_measured = true;
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let retry_start = Instant::now();
                let _ = main.show();
                let _ = main.hide();
                if let Some(timing) = r.cold_open_timing.as_mut() {
                    let elapsed = retry_start.elapsed();
                    timing.toolbar_retry += elapsed;
                    timing.toolbar_retries += 1;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                    timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
                }
                r.main.prepare_after(Duration::from_millis(16), move || {
                    show_main(generation, revision, attempt + 1)
                });
            }
            _ => r.close(),
        }
    });
}

fn refresh_magnifiers() {
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let Some(r) = slot.as_ref() else { return };
        if r.native_presenter.in_use() {
            return;
        }
        if r.session
            .objects
            .iter()
            .chain(gestures::display_draft(&r.gesture, &r.pending_drawing))
            .any(|object| object.kind == Kind::Magnifier)
        {
            (r.lifecycle.annotation_magnifier_refresh)();
        }
    });
}

fn check_display_layout() {
    let changed = REGISTRY.with(|s| {
        let slot = s.borrow();
        let Some(r) = slot.as_ref().filter(|r| r.main.get().is_some()) else {
            return false;
        };
        (r.lifecycle.annotation_displays)()
            != r.canvases.iter().map(|c| c.bounds).collect::<Vec<_>>()
    });
    if changed {
        close();
    }
}

/// Winit applies the destination monitor DPI after showing a native window.
/// Reconcile the anchor using realized physical size; the window owns and cancels retries.
fn settle_main_position(
    r: &Registry,
    generation: u64,
    revision: u64,
    anchor: Point,
    remaining: u8,
) {
    r.main.position_after(Duration::from_millis(32), move || {
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let Some(r) = slot
                .as_ref()
                .filter(|r| r.generation == generation && r.main.revision() == revision)
            else {
                return;
            };
            let Some(main) = r.main.get() else { return };
            if let Some(area) = (r.lifecycle.popup_work_area)(anchor) {
                let size = main.window().size();
                let gap = (16. * main.window().scale_factor()) as i32;
                let point = placement::place_popup(anchor, size.width, size.height, area, gap, gap)
                    .position;
                main.window()
                    .set_position(slint::PhysicalPosition::new(point.x, point.y));
            }
            if remaining > 0 {
                settle_main_position(r, generation, revision, anchor, remaining - 1);
            }
        });
    });
}

fn open_panel(group: usize, menu: bool) {
    if group >= GROUPS.len() {
        return;
    }
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.main.get().is_none() {
            return;
        }
        if !menu && !r.set_mode(InteractionMode::Tool(group)) {
            return;
        }
        let same = r.group == Some(group) && r.panel.get().is_some_and(|p| p.get_menu() == menu);
        r.close_panel();
        if same {
            return;
        }
        let Ok(panel) = AnnotationPanel::new() else {
            return;
        };
        crate::theme::apply(&panel);
        let tool = r.selected[group];
        if matches!(tool, 8..=10) {
            r.ensure_fonts();
        }
        if tool == 9 {
            adopt_watermark_style(r);
        }
        panel.set_font_families(ModelRc::new(VecModel::from(
            r.fonts
                .names()
                .iter()
                .cloned()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )));
        panel.set_can_clear(r.session.has_content());
        panel.set_tool(tool);
        panel.set_menu(menu);
        panel.set_heading(crate::i18n::tr(NAMES[tool as usize]).into());
        panel.set_values(r.values(tool as usize).clone());
        panel.set_mosaic_available(
            r.lifecycle
                .annotation_screen_capture
                .as_ref()
                .is_some_and(|p| p.is_available()),
        );
        let [red, green, blue] = current_style(r, tool).color;
        panel.set_custom_preview(slint::Color::from_rgb_u8(red, green, blue));
        panel.set_fields(ModelRc::new(VecModel::from(fields(tool))));
        panel.set_menu_tools(ModelRc::new(VecModel::from(GROUPS[group].to_vec())));
        panel.set_menu_labels(ModelRc::new(VecModel::from(
            GROUPS[group]
                .iter()
                .map(|&t| crate::i18n::tr(NAMES[t as usize]).into())
                .collect::<Vec<_>>(),
        )));
        let generation = r.generation;
        let dismissal = r.lifecycle.set_popup_dismissal.clone();
        let panel_revision = r.panel.open_with_retirement(panel, move |window| {
            dismissal(window, false);
        });
        r.parameter_values.bind(r.parameter_object(tool as usize));
        let panel = r.panel.get().unwrap();
        if !menu && group <= 6 {
            panel.on_numeric_scrolled(move |field, delta| {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    if let Some(r) = slot.as_mut().filter(|r| {
                        r.generation == generation
                            && r.panel.revision() == panel_revision
                            && r.parameter_entry_active(tool)
                    }) && let Some(parameter) = numeric::Parameter::from_field(field)
                    {
                        let target = r.parameter_target(tool);
                        r.scroll_numeric(target, parameter, delta);
                    }
                });
            });
            let input = r.panel.input();
            let layout_input = input.clone();
            panel.on_numeric_layout_changed(move || {
                layout_input.post(move || {
                    REGISTRY.with(|slot| {
                        if let Some(r) = slot.borrow().as_ref().filter(|r| {
                            r.generation == generation && r.panel.revision() == panel_revision
                        }) {
                            position_panel(r);
                        }
                    })
                });
            });
            let observed = parameters::Notifications::new(panel.get_values());
            panel.on_values_changed(move |values| {
                let target = REGISTRY.with(|slot| {
                    let slot = slot.try_borrow().ok()?;
                    let r = slot.as_ref().filter(|r| {
                        r.generation == generation && r.panel.revision() == panel_revision
                    })?;
                    Some(r.parameter_target(tool))
                });
                let patch = observed.observe(values, target.is_some());
                let (Some(target), Some(patch)) = (target, patch) else {
                    return;
                };
                input.post(move || {
                    if panel_current(generation, panel_revision) {
                        update_geometry_patch(generation, target, patch);
                    }
                })
            });
        }
        let lifecycle = r.lifecycle.clone();
        panel::bind_input(
            panel,
            r.panel.input(),
            Rc::new(move || focus_origin(&lifecycle)),
            Rc::new(move |event| apply_panel_input(generation, panel_revision, group, event)),
        );
        r.group = Some(group);
        r.panel.prepare_after(Duration::ZERO, move || {
            show_panel(generation, panel_revision, 0)
        });
    });
}

/// The panel's FIFO owner delivers typed events here; target and window identity
/// remain session decisions rather than component callback responsibilities.
fn apply_panel_input(generation: u64, revision: u64, group: usize, event: panel::Input) {
    match event {
        panel::Input::Finished { confirmed, origin } => REGISTRY.with(|slot| {
            if let Some(r) = slot
                .borrow()
                .as_ref()
                .filter(|r| r.generation == generation && r.panel.revision() == revision)
            {
                restore_editor_focus_from(
                    r,
                    if confirmed {
                        focus::Activation::UserRequested
                    } else {
                        focus::Activation::Local
                    },
                    origin,
                );
            }
        }),
        panel::Input::Watermark(origin) => {
            if panel_current(generation, revision) {
                open_watermark_input_from(generation, origin);
            }
        }
        panel::Input::Dragging { active, origin } => REGISTRY.with(|slot| {
            if let Some(r) = slot
                .borrow_mut()
                .as_mut()
                .filter(|r| r.generation == generation && r.panel.revision() == revision)
            {
                if active {
                    r.finish_style_edit();
                }
                r.parameter_edit.set_dragging(active);
                if !active {
                    r.finish_style_edit();
                    restore_editor_focus_from(r, focus::Activation::UserRequested, origin);
                }
            }
        }),
        panel::Input::Tool(tool) => {
            let selected = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(r) = slot.as_mut().filter(|r| {
                    r.generation == generation
                        && r.panel.revision() == revision
                        && r.group == Some(group)
                        && r.panel.get().is_some_and(|panel| panel.get_menu())
                }) else {
                    return false;
                };
                r.select_tool_variant(group, tool)
            });
            if selected {
                open_panel(group, false);
            }
        }
        panel::Input::StyleChoice { request, origin } => {
            choice::toggle_style(generation, revision, request, origin);
        }
        panel::Input::Choice { request, origin } => {
            choice::open_from(generation, revision, request, origin);
        }
        panel::Input::Escape(origin) => {
            if panel_current(generation, revision) {
                escape_from(origin);
            }
        }
        panel::Input::ClearAll => {
            if panel_current(generation, revision) {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(r) = slot.as_mut() else { return };
                    if r.mode != InteractionMode::Tool(6) {
                        return;
                    }
                    cancel_active_gesture(r);
                    r.finish_style_edit();
                    if r.session.clear_all() {
                        deselect_object(r);
                        r.hovered = None;
                        sync_sequence_counter(r);
                        r.queue_render();
                    }
                    if let Some(panel) = r.panel.get() {
                        panel.set_can_clear(r.session.has_content());
                    }
                    if let Some(main) = r.main.get() {
                        main.set_can_undo(r.session.can_undo());
                    }
                });
            }
        }
        panel::Input::Undo => {
            let current = REGISTRY.with(|slot| {
                slot.borrow().as_ref().is_some_and(|r| {
                    r.generation == generation
                        && r.panel.revision() == revision
                        && r.panel
                            .get()
                            .is_some_and(|p| matches!(p.get_tool(), 11..=13))
                })
            });
            if current {
                undo();
            }
        }
    }
}

#[cfg(test)]
fn open_watermark_input(generation: u64) {
    open_watermark_input_from(generation, current_focus_origin());
}

fn open_watermark_input_from(generation: u64, request_origin: focus::Origin) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        r.ensure_fonts();
        r.close_watermark_input();
        let Some(main) = r.main.get() else { return };
        let origin = main.window().position();
        // Seed the owner monitor before native preparation establishes the editor DPI.
        let Some(editor) = watermark::build_window(watermark::Request {
            value: r.values(9).text.clone(),
            position: Point {
                x: origin.x,
                y: origin.y,
            },
        }) else {
            return;
        };
        let revision = r.watermark.open(editor);
        watermark::bind_input(
            r.watermark.get().unwrap(),
            r.watermark.input(),
            Rc::new(move |event| apply_watermark_input(generation, revision, event)),
        );
        r.watermark.prepare_after(Duration::ZERO, move || {
            show_watermark_input(generation, revision, 0, request_origin)
        });
    });
}

fn apply_watermark_input(generation: u64, revision: u64, event: watermark::Input) {
    match event {
        watermark::Input::Presets {
            x,
            y,
            width,
            height,
        } => {
            choice::open_watermark(generation, revision, x, y, width, height);
        }
        watermark::Input::DismissPresets => choice::dismiss_watermark(generation, revision),
        watermark::Input::Apply(template) => apply_watermark(generation, revision, template),
        watermark::Input::Cancel => close_watermark_input(generation, revision),
    }
}

fn show_watermark_input(generation: u64, revision: u64, attempt: u8, origin: focus::Origin) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.watermark.revision() == revision)
        else {
            return;
        };
        let Some(editor) = r.watermark.get() else {
            return;
        };
        match (r.prepare)(editor.window()) {
            PassiveWindowPreparation::Ready => {
                let owner_ready = r.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(editor.window(), main.window())
                });
                if !owner_ready {
                    retry_watermark_owner(r, generation, revision, attempt, origin);
                    return;
                }
                position_watermark_input(r);
                if editor.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        editor.window(),
                        watermark::pointer_sink(
                            editor.as_weak(),
                            move || {
                                REGISTRY.with(|slot| {
                                    slot.borrow().as_ref().is_some_and(|r| {
                                        r.generation == generation
                                            && r.watermark.revision() == revision
                                            && r.watermark.get().is_some()
                                    })
                                })
                            },
                            r.watermark.input(),
                            choice::watermark_pointer,
                        ),
                    )
                {
                    r.close_watermark_input();
                    return;
                }
                if !r.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(editor.window(), main.window())
                }) {
                    let _ = editor.hide();
                    retry_watermark_owner(r, generation, revision, attempt, origin);
                    return;
                }
                position_watermark_input(r);
                schedule_watermark_position(r, generation, revision, 2);
                if permits_focus_activation(r, origin) {
                    (r.lifecycle.activate_user_requested_window)(editor.window());
                }
                if r.native_presenter
                    .exclusions_changed(&capture_exclusions(r))
                {
                    r.queue_render();
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = editor.show();
                let _ = editor.hide();
                r.watermark
                    .prepare_after(Duration::from_millis(16), move || {
                        show_watermark_input(generation, revision, attempt + 1, origin)
                    });
            }
            _ => {
                r.close_watermark_input();
            }
        }
    });
}

fn position_watermark_input(r: &Registry) {
    let (Some(main), Some(editor)) = (r.main.get(), r.watermark.get()) else {
        return;
    };
    let origin = main.window().position();
    let toolbar_size = main.window().size();
    let toolbar = Rect {
        left: origin.x,
        top: origin.y,
        right: origin
            .x
            .saturating_add(toolbar_size.width.min(i32::MAX as u32) as i32),
        bottom: origin
            .y
            .saturating_add(toolbar_size.height.min(i32::MAX as u32) as i32),
    };
    let area = (r.lifecycle.popup_work_area)(Point {
        x: origin.x,
        y: origin.y,
    });
    let target = watermark::position(
        toolbar,
        editor.window().size(),
        editor.window().scale_factor(),
        area,
    );
    let target = slint::PhysicalPosition::new(target.x, target.y);
    if editor.window().position() != target {
        editor.window().set_position(target);
    }
}

fn retry_watermark_owner(
    r: &mut Registry,
    generation: u64,
    revision: u64,
    attempt: u8,
    origin: focus::Origin,
) {
    if attempt < 20 {
        r.watermark
            .prepare_after(Duration::from_millis(16), move || {
                show_watermark_input(generation, revision, attempt + 1, origin)
            });
    } else {
        tracing::warn!("could not attach watermark editor after preparation retries");
        r.close_watermark_input();
    }
}

fn schedule_watermark_position(r: &Registry, generation: u64, revision: u64, remaining: u8) {
    r.watermark
        .position_after(Duration::from_millis(32), move || {
            settle_watermark_position(generation, revision, remaining);
        });
}

fn settle_watermark_position(generation: u64, revision: u64, remaining: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.watermark.revision() == revision
                && r.watermark.get().is_some()
        }) else {
            return;
        };
        position_watermark_input(r);
        if remaining > 1 {
            schedule_watermark_position(r, generation, revision, remaining - 1);
        }
        // Window replacement changes capture exclusions; movement alone does not.
        if r.native_presenter
            .exclusions_changed(&capture_exclusions(r))
        {
            r.queue_render();
        }
    });
}

fn close_watermark_input(generation: u64, revision: u64) {
    REGISTRY.with(|slot| {
        if let Some(r) = slot
            .borrow_mut()
            .as_mut()
            .filter(|r| r.generation == generation && r.watermark.revision() == revision)
        {
            r.close_watermark_input();
        }
    });
}

fn apply_watermark(generation: u64, revision: u64, template: String) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.watermark.revision() == revision
                && r.watermark.get().is_some()
        })?;
        r.close_watermark_input();
        r.finish_style_edit();
        let content =
            crate::annotation_text::resolve_watermark_template(&template, chrono::Local::now());
        let watermark =
            (!content.trim().is_empty()).then(|| watermark_from_values(r, template, content));
        r.session.set_watermark(watermark);
        adopt_watermark_style(r);
        sync_history_controls(r);
        Some(r.generation)
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
}

fn show_panel(generation: u64, panel_revision: u64, attempt: u8) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.panel.revision() == panel_revision)
        else {
            return;
        };
        let Some(panel) = r.panel.get() else {
            return;
        };
        match (r.prepare)(panel.window()) {
            PassiveWindowPreparation::Ready => {
                if !position_panel(r) {
                    r.close_panel();
                    return;
                }
                let panel = r.panel.get().unwrap();
                if !r.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(panel.window(), main.window())
                }) {
                    retry_panel_owner(r, generation, panel_revision, attempt);
                    return;
                }
                if panel.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        panel.window(),
                        pointer_panel(panel.as_weak(), generation, panel_revision, r.panel.input()),
                    )
                {
                    r.close_panel();
                    return;
                }
                if !r.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(panel.window(), main.window())
                }) {
                    let _ = panel.hide();
                    retry_panel_owner(r, generation, panel_revision, attempt);
                    return;
                }
                (r.lifecycle.set_popup_dismissal)(panel.window(), panel.get_menu());
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = panel.show();
                let _ = panel.hide();
                r.panel.prepare_after(Duration::from_millis(16), move || {
                    show_panel(generation, panel_revision, attempt + 1)
                });
            }
            _ => r.close_panel(),
        }
    });
}

fn reposition_panel() {
    let changed = REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            sync_toolbar_owner(r);
            position_panel(r);
            choice::close_if_context_changed(r);
            if r.native_presenter
                .exclusions_changed(&capture_exclusions(r))
            {
                return Some(r.generation);
            }
        }
        None
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
}

/// Keep the toolbar owned by the canvas under its physical center so activating
/// the canvas during drawing cannot place its transparent input surface above the controls.
fn sync_toolbar_owner(r: &mut Registry) -> bool {
    let Some(main) = r.main.get() else {
        return false;
    };
    let position = main.window().position();
    let size = main.window().size();
    let center = Point {
        x: position.x.saturating_add((size.width / 2) as i32),
        y: position.y.saturating_add((size.height / 2) as i32),
    };
    let owner = canvas_at(r.canvases.iter().map(|canvas| canvas.bounds), center)
        .or(r.toolbar_owner.map(|owner| owner.0))
        .or_else(|| (!r.canvases.is_empty()).then_some(0));
    let Some(owner) = owner else { return false };
    let identity = (owner, r.canvases[owner].revision(), r.main.revision());
    if r.toolbar_owner == Some(identity) {
        return true;
    }
    if (r.lifecycle.attach_tool_window)(main.window(), r.canvases[owner].window.window()) {
        r.toolbar_owner = Some(identity);
        return true;
    }
    false
}

fn retry_main_owner(r: &mut Registry, generation: u64, revision: u64, attempt: u8) {
    if attempt < 20 {
        r.main.prepare_after(Duration::from_millis(16), move || {
            show_main(generation, revision, attempt + 1)
        });
    } else {
        tracing::warn!("could not attach annotation toolbar after preparation retries");
        r.close();
    }
}

fn retry_panel_owner(r: &mut Registry, generation: u64, revision: u64, attempt: u8) {
    if attempt < 20 {
        r.panel.prepare_after(Duration::from_millis(16), move || {
            show_panel(generation, revision, attempt + 1)
        });
    } else {
        tracing::warn!("could not attach annotation panel after preparation retries");
        r.close_panel();
    }
}

fn canvas_at(bounds: impl IntoIterator<Item = Rect>, point: Point) -> Option<usize> {
    bounds.into_iter().position(|bounds| {
        point.x >= bounds.left
            && point.x < bounds.right
            && point.y >= bounds.top
            && point.y < bounds.bottom
    })
}

fn position_panel(r: &Registry) -> bool {
    let (Some(main), Some(panel)) = (r.main.get(), r.panel.get()) else {
        // Ownership preparation decides whether a missing main window can retry.
        return true;
    };
    let p = main.window().position();
    let m = main.window().size();
    let Some(area) = (r.lifecycle.popup_work_area)(Point { x: p.x, y: p.y }) else {
        return !panel.get_menu();
    };
    let scale = panel.window().scale_factor().max(0.1);
    let available = ((area.right - area.left) as f32 / scale).max(1.);
    let width = if panel.get_menu() {
        220f32.min(available)
    } else {
        // Wrapped preferred widths are clamped by Slint; measure the same row without wrapping.
        panel.set_numeric_measuring(true);
        let width = ((panel.get_numeric_content_width() * scale).ceil() / scale).min(available);
        panel.set_numeric_measuring(false);
        width
    };
    if !panel.get_menu() {
        let current = panel.window().size().to_logical(scale);
        panel
            .window()
            .set_size(slint::LogicalSize::new(width, current.height));
    }
    let columns = ((width - 24.) / 165.).floor().max(1.) as i32;
    panel.set_columns(columns);
    let height = if panel.get_menu() {
        46. + GROUPS[r.group.unwrap_or(0)].len() as f32 * 42.
    } else {
        panel.get_numeric_content_height()
    };
    if panel.get_menu() {
        let main_rect = Rect {
            left: p.x,
            top: p.y,
            right: p.x.saturating_add(m.width as i32),
            bottom: p.y.saturating_add(m.height as i32),
        };
        let desired = slint::LogicalSize::new(width, height).to_physical(scale);
        let Some(placed) = placement::place_annotation_popup(
            main_rect,
            (desired.width, desired.height),
            area,
            &[main_rect],
            false,
            (8. * scale).ceil() as i32,
        ) else {
            tracing::warn!("no unobstructed space for annotation tool group");
            return false;
        };
        panel
            .window()
            .set_size(slint::PhysicalSize::new(placed.width, placed.height));
        panel.window().set_position(slint::PhysicalPosition::new(
            placed.position.x,
            placed.position.y,
        ));
        return true;
    }
    let size = slint::LogicalSize::new(width, height.min((area.bottom - area.top) as f32 / scale));
    let physical = size.to_physical(scale);
    if panel.window().size() != physical {
        panel.window().set_size(size);
    }
    let target = panel_position(
        Point { x: p.x, y: p.y },
        m.height,
        physical.width,
        physical.height,
        area,
        (8. * scale) as i32,
    );
    if panel.window().position() != slint::PhysicalPosition::new(target.x, target.y) {
        panel
            .window()
            .set_position(slint::PhysicalPosition::new(target.x, target.y));
    }
    true
}

fn panel_position(
    main: Point,
    main_height: u32,
    width: u32,
    height: u32,
    area: Rect,
    gap: i32,
) -> Point {
    placement::place_toolbar_popup(
        Rect {
            left: main.x,
            right: main.x,
            top: main.y,
            bottom: main
                .y
                .saturating_add(main_height.min(i32::MAX as u32) as i32),
        },
        width,
        height,
        Some(area),
        gap,
        placement::Side::After,
    )
}

fn main_current(generation: u64, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        slot.borrow().as_ref().is_some_and(|r| {
            r.generation == generation && r.main.revision() == revision && r.main.get().is_some()
        })
    })
}

fn later_main(
    input: &Rc<window::InputWork>,
    generation: u64,
    revision: u64,
    action: impl FnOnce() + 'static,
) {
    input.post(move || {
        if main_current(generation, revision) {
            action();
        }
    });
}

/// Native preparation can pump input while a session update owns Registry.
/// Queue only that reentrant delivery on the originating window; retirement and
/// pointer pauses remain checked when the event is replayed.
fn registry_pointer(
    input_work: Rc<window::InputWork>,
    receive: crate::bridge::PopupPointerSink,
) -> crate::bridge::PopupPointerSink {
    input_work.pointer_sink_when_ready(
        Rc::new(|| REGISTRY.with(|slot| slot.try_borrow().is_ok())),
        receive,
    )
}

fn pointer_main(
    weak: slint::Weak<AnnotationToolbar>,
    generation: u64,
    revision: u64,
    input_work: Rc<window::InputWork>,
) -> crate::bridge::PopupPointerSink {
    registry_pointer(
        input_work,
        Rc::new(move |input| {
            if main_current(generation, revision)
                && let Some(w) = weak.upgrade()
            {
                dispatch(w.window(), input);
            }
        }),
    )
}
fn panel_current(generation: u64, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        slot.borrow().as_ref().is_some_and(|r| {
            r.generation == generation && r.panel.revision() == revision && r.panel.get().is_some()
        })
    })
}

fn pointer_panel(
    weak: slint::Weak<AnnotationPanel>,
    generation: u64,
    panel_revision: u64,
    input_work: Rc<window::InputWork>,
) -> crate::bridge::PopupPointerSink {
    registry_pointer(
        input_work.clone(),
        Rc::new(move |input| {
            if !panel_current(generation, panel_revision) {
                return;
            }
            if let PopupPointerInput::DismissRequested { cause } = input {
                input_work.post(move || {
                    REGISTRY.with(|s| {
                        if let Some(r) = s.borrow_mut().as_mut().filter(|r| {
                            r.generation == generation
                                && r.panel.revision() == panel_revision
                                && r.panel.get().is_some_and(|panel| panel.get_menu())
                        }) {
                            if cause == PopupDismissCause::PointerOutside
                                && choice::cursor_inside(r)
                            {
                                choice::rearm_panel(r);
                                return;
                            }
                            let on_main = (r.lifecycle.toolbar_cursor_position)()
                                .zip(r.main.get())
                                .is_some_and(|(cursor, main)| {
                                    let p = main.window().position();
                                    let size = main.window().size();
                                    cursor.x >= p.x
                                        && cursor.y >= p.y
                                        && cursor.x < p.x + size.width as i32
                                        && cursor.y < p.y + size.height as i32
                                });
                            let on_panel = (r.lifecycle.toolbar_cursor_position)()
                                .zip(r.panel.get())
                                .is_some_and(|(cursor, panel)| {
                                    let p = panel.window().position();
                                    let size = panel.window().size();
                                    cursor.x >= p.x
                                        && cursor.y >= p.y
                                        && cursor.x < p.x + size.width as i32
                                        && cursor.y < p.y + size.height as i32
                                });
                            // Returning keyboard focus to the editor is not an outside click.
                            if cause == PopupDismissCause::PointerOutside && (on_main || on_panel) {
                                if let Some(panel) = r.panel.get() {
                                    (r.lifecycle.set_popup_dismissal)(panel.window(), false);
                                    (r.lifecycle.set_popup_dismissal)(panel.window(), true);
                                }
                            } else {
                                r.close_panel();
                            }
                        }
                    })
                });
            } else if let Some(w) = weak.upgrade() {
                let origin = if matches!(input, PopupPointerInput::LeftReleased { .. }) {
                    current_focus_origin()
                } else {
                    focus::Origin::Unobserved
                };
                if let PopupPointerInput::LeftPressed { x, y } = input {
                    let scale = w.window().scale_factor().max(0.1);
                    let (x, y) = (x / scale, y / scale);
                    if x < w.get_parameter_input_x()
                        || y < w.get_parameter_input_y()
                        || x >= w.get_parameter_input_x() + w.get_parameter_input_width()
                        || y >= w.get_parameter_input_y() + w.get_parameter_input_height()
                    {
                        w.invoke_release_parameter_input();
                    }
                }
                choice::panel_pointer(input);
                dispatch(w.window(), input);
                if matches!(input, PopupPointerInput::LeftReleased { .. }) {
                    input_work.post(move || {
                        REGISTRY.with(|slot| {
                            if let Some(r) = slot.borrow().as_ref().filter(|r| {
                                r.generation == generation && r.panel.revision() == panel_revision
                            }) {
                                restore_editor_focus_from(
                                    r,
                                    focus::Activation::UserRequested,
                                    origin,
                                );
                            }
                        });
                    });
                }
            }
        }),
    )
}

pub(crate) fn refresh_language() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            choice::close(r, choice::CloseReason::OwnerRetired);
            if !r.fonts.names().is_empty() {
                r.reset_font_defaults();
            }
            if let Some(panel) = r.panel.get()
                && !panel.get_menu()
            {
                panel.set_values(r.values(panel.get_tool() as usize).clone());
            }
            if r.panel.get().is_some_and(|p| p.get_menu()) {
                r.close_panel();
            } else if let Some(panel) = r.panel.get() {
                panel.set_heading(crate::i18n::tr(NAMES[panel.get_tool() as usize]).into());
                panel.set_language_revision(panel.get_language_revision().wrapping_add(1));
            }
        }
    });
}

pub(crate) fn apply_theme() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow().as_ref() {
            if let Some(w) = r.main.get() {
                crate::theme::apply(w);
            }
            if let Some(w) = r.panel.get() {
                crate::theme::apply(w);
            }
            if let Some(menu) = &r.menus.current {
                crate::theme::apply(&menu.window);
            }
        }
    });
}

fn sync_history_controls(r: &Registry) {
    if let Some(main) = r.main.get() {
        main.set_can_undo(r.session.can_undo());
    }
    if let Some(panel) = r.panel.get() {
        panel.set_can_clear(r.session.has_content());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_pointer_reentry_is_deferred_and_retirement_cancels_replay() {
        use super::*;
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let panel = AnnotationPanel::new().unwrap();
        let main = AnnotationToolbar::new().unwrap();
        let canvas = AnnotationCanvas::new().unwrap();
        for target in 0..3 {
            let input = window::InputWork::new();
            let sink = match target {
                0 => pointer_panel(panel.as_weak(), 0, 0, input.clone()),
                1 => pointer_main(main.as_weak(), 0, 0, input.clone()),
                _ => pointer_canvas(canvas.as_weak(), 0, 0, 0, input.clone()),
            };
            REGISTRY.with(|slot| {
                let _updating_session = slot.borrow_mut();
                sink(PopupPointerInput::LeftPressed { x: 25., y: 25. });
                sink(PopupPointerInput::LeftReleased { x: 25., y: 25. });
                assert!(input.pending());
            });
            slint::platform::update_timers_and_animations();
            assert!(
                !input.pending(),
                "stale window replay must be rejected safely"
            );
            REGISTRY.with(|slot| {
                let _updating_session = slot.borrow_mut();
                sink(PopupPointerInput::LeftPressed { x: 25., y: 25. });
                assert!(input.pending());
                input.retire();
            });
            slint::platform::update_timers_and_animations();
            assert!(input.retired() && !input.pending());
        }
        let work = window::InputWork::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        let received = events.clone();
        let sink = registry_pointer(
            work.clone(),
            Rc::new(move |event| {
                if let PopupPointerInput::Moved { x, .. } = event {
                    received.borrow_mut().push(x);
                }
            }),
        );
        sink(PopupPointerInput::Moved { x: 1., y: 0. });
        assert_eq!(
            *events.borrow(),
            [1.],
            "normal pointer delivery stays immediate"
        );
        REGISTRY.with(|slot| {
            let _updating = slot.borrow_mut();
            sink(PopupPointerInput::Moved { x: 2., y: 0. });
            sink(PopupPointerInput::Moved { x: 3., y: 0. });
            assert_eq!(*events.borrow(), [1.]);
        });
        sink(PopupPointerInput::Moved { x: 3.5, y: 0. });
        assert_eq!(
            *events.borrow(),
            [1.],
            "later input cannot overtake queued input"
        );
        slint::platform::update_timers_and_animations();
        assert_eq!(
            *events.borrow(),
            [1., 2., 3., 3.5],
            "reentrant events keep FIFO order"
        );
        REGISTRY.with(|slot| {
            let _updating = slot.borrow_mut();
            sink(PopupPointerInput::Moved { x: 4., y: 0. });
        });
        work.retire();
        slint::platform::update_timers_and_animations();
        assert_eq!(
            *events.borrow(),
            [1., 2., 3., 3.5],
            "retirement cancels replay"
        );
    }

    #[test]
    fn caret_blinks_resets_and_stays_visible_during_composition_or_selection() {
        let mut caret = super::CaretBlink { visible: true };
        assert!(caret.tick(false));
        assert!(!caret.visible);
        assert!(caret.tick(true));
        assert!(caret.visible);
        assert!(!caret.tick(true));
        assert!(caret.tick(false));
        caret.reset();
        assert!(caret.visible);
    }

    #[test]
    fn interrupted_drawing_cannot_commit_after_tool_switch_escape_or_capture_loss() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for pencil in [false, true] {
                for interruption in 0..5 {
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        r.pending_displays = vec![Rect {
                            left: -500,
                            top: -300,
                            right: 500,
                            bottom: 500,
                        }];
                        create_canvas(r, 0);
                        r.canvases[0]
                            .window
                            .window()
                            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                        r.annotation_ready = true;
                        r.mode = InteractionMode::Tool(if pencil { 1 } else { 0 });
                        r.sync_edit_scope();
                        if pencil {
                            r.selected[1] = 3;
                            r.sync_edit_scope();
                        }
                    });
                    canvas_pointer(0, 0, 80., 80.);
                    canvas_pointer(0, 1, 180., 180.);
                    REGISTRY.with(|slot| {
                        let slot = slot.borrow();
                        let r = slot.as_ref().unwrap();
                        assert!(matches!(r.gesture(), Some(Gesture::Draw { .. })));
                        assert!(gestures::display_draft(&r.gesture, &r.pending_drawing).is_some());
                        assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    });
                    match interruption {
                        0 => REGISTRY.with(|slot| {
                            assert!(
                                slot.borrow_mut()
                                    .as_mut()
                                    .unwrap()
                                    .set_mode(InteractionMode::Tool(2))
                            );
                        }),
                        1 => escape(),
                        2 => canvas_pointer(0, 3, 180., 180.),
                        3 => REGISTRY.with(|slot| {
                            let mut slot = slot.borrow_mut();
                            let r = slot.as_mut().unwrap();
                            assert!(r.select_tool_variant(
                                if pencil { 1 } else { 0 },
                                if pencil { 4 } else { 1 }
                            ));
                        }),
                        _ => REGISTRY.with(|slot| {
                            slot.borrow_mut().as_mut().unwrap().close();
                        }),
                    }
                    // A release from the old capture may arrive after the toolbar action.
                    canvas_pointer(0, 2, 240., 240.);
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(r.gesture().is_none() && r.text_handle().is_none());
                        assert!(r.geometry_drag().is_none());
                        assert!(
                            gestures::display_draft(&r.gesture, &r.pending_drawing).is_none()
                                && r.pending_drawing.is_none()
                        );
                        assert!(r.session.objects.is_empty() && !r.session.can_undo());
                        r.close();
                    });
                }
            }
        }
        // Keyboard completion or removal of the last node retires a pending point press.
        for dpi in [1., 1.25] {
            for complete in [false, true] {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.mode = InteractionMode::Tool(2);
                    r.sync_edit_scope();
                    r.selected[2] = 6;
                    r.sync_edit_scope();
                    r.annotation_ready = true;
                    let window = AnnotationCanvas::new().unwrap();
                    window
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    r.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -500,
                            top: -300,
                            right: 500,
                            bottom: 500,
                        },
                    ));
                });
                canvas_pointer(0, 0, 40., 40.);
                canvas_pointer(0, 2, 40., 40.);
                canvas_pointer(0, 1, 100., 100.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(
                        gestures::display_draft(&r.gesture, &r.pending_drawing)
                            .unwrap()
                            .points
                            .len(),
                        2
                    );
                    assert!(r.hovered.is_none());
                    r.frame_tasks.cancel_render();
                });
                canvas_pointer(0, 4, 100., 100.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(
                        gestures::display_draft(&r.gesture, &r.pending_drawing)
                            .unwrap()
                            .points
                            .len(),
                        1
                    );
                    assert!(
                        r.frame_tasks.render_pending(),
                        "exit must refresh confirmed nodes"
                    );
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    r.frame_tasks.cancel_render();
                });
                canvas_pointer(0, 4, 100., 100.);
                REGISTRY.with(|slot| {
                    assert!(!slot.borrow().as_ref().unwrap().frame_tasks.render_pending());
                });
                if complete {
                    canvas_pointer(0, 0, 80., 80.);
                    canvas_pointer(0, 2, 80., 80.);
                }
                canvas_pointer(0, 0, 140., 120.);
                if complete {
                    finish_polyline();
                } else {
                    backspace_polyline();
                }
                canvas_pointer(0, 2, 140., 120.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(
                        r.gesture().is_none()
                            && r.polyline().is_none()
                            && gestures::display_draft(&r.gesture, &r.pending_drawing).is_none(),
                        "late release must not start another polyline"
                    );
                    assert!(r.drawing_press().is_none());
                    if complete {
                        assert_eq!(r.session.objects.len(), 1);
                        assert_eq!(r.session.objects[0].points.len(), 2);
                        assert!(r.session.undo());
                    }
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    r.close();
                });
            }
        }
        shutdown();
    }

    #[test]
    fn tool_entries_gate_selection_parameters_and_undo_without_auto_switching() {
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let mut object = crate::annotation_sequence::tests::badge(1.);
            object.kind = Kind::Rectangle;
            object.extra = Extra::None;
            object.style.fill = true;
            object.bounds = Bounds::from_corners((-40., -30.), (40., 30.));
            assert!(r.session.add(object));
            let original = r.session.objects[0].clone();
            let target = r.parameter_target(0);
            let mut values = r.values(0).clone();
            values.size = 20;
            assert!(r.select_tool_variant(0, 1));
            assert_eq!(r.session.selected, Some(0), "same family retains selection");
            assert!(r.select_tool_variant(2, 7));
            assert_eq!(r.session.selected, None);
            assert_eq!(r.parameter_object(0), None);
            assert_eq!(r.session.hit(original.bounds.center(), 1.), None);
            assert!(apply_parameter_values(r, target, values).is_none());
            assert_eq!(r.session.objects[0], original);
            assert!(r.select_tool_variant(0, 0));
            assert_eq!(r.session.selected, None);
            assert_eq!(
                r.session.hit(original.bounds.center(), 1.),
                Some(Hit::Object(0))
            );
            r.session.selected = Some(0);
            r.session.update_selected(|object| object.move_by(20., 0.));
            assert!(r.select_tool_variant(2, 7));
        });
        undo();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.mode, InteractionMode::Tool(2));
            assert_eq!(r.selected[2], 7);
            assert_eq!(r.session.selected, None);
        });
        shutdown();
    }

    #[test]
    fn sticker_only_draws_without_capture_workers_or_repeated_static_submissions() {
        use lexift_core::ports::screen_capture::{
            ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
        };
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        struct Capture(Arc<AtomicUsize>, Arc<AtomicBool>);
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                self.1.load(Ordering::Relaxed)
            }
            fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<Arc<ScreenCaptureFrame>>> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(None)
            }
            fn clear(&self) {}
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let captures = Arc::new(AtomicUsize::new(0));
        let available = Arc::new(AtomicBool::new(false));
        let presents = Rc::new(Cell::new(0));
        let calls = presents.clone();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_screen_capture(Some(Arc::new(Capture(
                captures.clone(),
                available.clone(),
            ))))
            .with_annotation_canvas(
                Vec::new,
                |_, _| true,
                move |_, _, _, _| {
                    calls.set(calls.get() + 1);
                    true
                },
            ),
        );
        for dpi in [1., 1.25] {
            available.store(dpi == 1.25, Ordering::Relaxed);
            for shape in [0, 1] {
                for texture in [0, 1] {
                    let generation = REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        r.close();
                        r.pending_displays = vec![Rect {
                            left: -200,
                            top: -200,
                            right: 400,
                            bottom: 400,
                        }];
                        r.main.open(AnnotationToolbar::new().unwrap());
                        create_canvas(r, 0);
                        r.canvases[0]
                            .window
                            .window()
                            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                        r.annotation_ready = true;
                        r.selected[5] = 12;
                        r.set_mode(InteractionMode::Tool(5));
                        r.edit_values(12).shape = shape;
                        r.edit_values(12).mode = texture;
                        r.generation
                    });
                    open_panel(5, false);
                    canvas_pointer(0, 0, 80., 80.);
                    canvas_pointer(0, 1, 180., 170.);
                    canvas_pointer(0, 2, 180., 170.);
                    REGISTRY.with(|slot| {
                        let slot = slot.borrow();
                        let r = slot.as_ref().unwrap();
                        assert_eq!(r.session.objects.len(), 1);
                        assert_eq!(
                            r.session.objects[0].kind,
                            if shape == 0 {
                                Kind::StickerArea
                            } else {
                                Kind::StickerBrush
                            }
                        );
                        assert!(matches!(&r.session.objects[0].extra, Extra::Sticker(s) if s.opacity == 95));
                        assert_eq!(r.session.selected, (shape == 0).then_some(0));
                        if shape == 1 {
                            assert!(r.session.objects[0].edit_handles(dpi).is_empty());
                            assert_eq!(r.parameter_object(12), None);
                        }
                        assert_eq!(r.values(12).strength, 95.);
                        assert_eq!(r.panel.get().unwrap().get_values().strength, 95.);
                        assert!(r.mosaic.is_idle());
                        assert!(!r.native_presenter.active);
                        assert_eq!(r.panel.get().unwrap().get_tool(), 12);
                    });
                    render_canvases(generation);
                    let count = presents.get();
                    for _ in 0..3 {
                        render_canvases(generation);
                    }
                    assert_eq!(
                        presents.get(),
                        count,
                        "static sticker must reuse presented pixels"
                    );
                    assert_eq!(captures.load(Ordering::Relaxed), 0);
                    if shape == 1 {
                        REGISTRY.with(|slot| {
                            let mut slot = slot.borrow_mut();
                            let r = slot.as_mut().unwrap();
                            let before = r.session.objects[0].clone();
                            let mut stale = r.parameter_target(12);
                            stale.object = Some(before.id);
                            let mut values = r.values(12).clone();
                            values.strength = 80.;
                            assert!(apply_parameter_values(r, stale, values.clone()).is_none());
                            assert_eq!(r.values(12).strength, 95.);
                            let target = r.parameter_target(12);
                            assert!(apply_parameter_values(r, target, values).is_some());
                            r.finish_style_edit();
                            assert_eq!(r.session.objects[0], before);
                            assert_eq!(r.values(12).strength, 80.);
                        });
                        // The old trajectory is not an edit target: drawing over it creates a new mark.
                        canvas_pointer(0, 0, 100., 100.);
                        canvas_pointer(0, 1, 160., 150.);
                        canvas_pointer(0, 2, 160., 150.);
                        REGISTRY.with(|slot| {
                            let mut slot = slot.borrow_mut();
                            let r = slot.as_mut().unwrap();
                            assert_eq!(r.session.objects.len(), 2);
                            assert_eq!(r.session.selected, None);
                            assert!(matches!(&r.session.objects[0].extra, Extra::Sticker(s) if s.opacity == 95));
                            assert!(matches!(&r.session.objects[1].extra, Extra::Sticker(s) if s.opacity == 80));
                            assert!(r.session.undo());
                            assert_eq!(r.session.objects.len(), 1);
                            assert_eq!(r.session.selected, None);
                        });
                    }
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(r.session.undo());
                        assert!(r.session.objects.is_empty());
                    });
                }
            }
        }
        shutdown();
    }

    #[test]
    fn creation_captures_displayed_settings_before_deselecting_all_tools() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for (tool, group, kind) in [
                (0, 0, Kind::Rectangle),
                (1, 0, Kind::Ellipse),
                (2, 0, Kind::SpotlightRectangle),
                (4, 1, Kind::HighlightLine),
                (5, 2, Kind::Arrow),
                (6, 2, Kind::Polyline),
                (7, 2, Kind::Magnifier),
                (8, 3, Kind::Text),
                (10, 4, Kind::Sequence),
                (11, 5, Kind::MosaicArea),
            ] {
                let expected = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close();
                    r.pending_displays = vec![Rect {
                        left: -500,
                        top: -300,
                        right: 500,
                        bottom: 500,
                    }];
                    create_canvas(r, 0);
                    r.canvases[0].window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                    );
                    r.annotation_ready = true;
                    r.selected[group] = tool;
                    r.set_mode(InteractionMode::Tool(group));
                    let object = Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind,
                        bounds: Bounds::from_corners((-450., -250.), (-400., -200.)),
                        points: vec![],
                        style: Style::default(),
                        extra: Extra::None,
                    };
                    r.session.add(object);
                    project_object_parameters(r, 0);
                    // Refill is independent; it is only adopted when creation begins.
                    let mut values = r.values(tool as usize).clone();
                    values.color_index = 8;
                    values.custom_color = "#123456".into();
                    values.size = 19;
                    values.shape = 1;
                    values.style = 1;
                    values.line_start = 2;
                    values.line_end = 3;
                    values.zoom = 3;
                    values.connector_style = 2;
                    values.text_size = 35;
                    values.format = 2;
                    values.mode = 1;
                    values.strength = 73.;
                    *r.parameter_values
                        .edit(tool as usize, r.session.objects.first().map(|o| o.id)) =
                        values.clone();
                    values.text = Default::default();
                    values.sequence_label = Default::default();
                    if tool == 10 {
                        values.start = r.session.next_sequence;
                    }
                    values
                });
                canvas_pointer(0, 0, 700. / dpi, 450. / dpi);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    let mut actual = r.parameter_values.defaults(tool as usize).clone();
                    if tool == 10 {
                        actual.start = expected.start;
                        actual.sequence_label = Default::default();
                    }
                    assert_eq!(actual, expected, "tool {tool} DPI {dpi}");
                    if tool != 10 {
                        assert_eq!(r.session.selected, None);
                    }
                    if tool != 10 {
                        assert_eq!(r.values(tool as usize), &expected);
                    }
                    assert_eq!(r.session.objects[0].style, Style::default());
                    cancel_active_gesture(r);
                    let mut actual = r.parameter_values.defaults(tool as usize).clone();
                    if tool == 10 {
                        actual.start = expected.start;
                        actual.sequence_label = Default::default();
                    }
                    assert_eq!(actual, expected);
                });
            }
        }
        shutdown();
    }

    #[test]
    fn highlighter_rectangle_mode_survives_deselection_drawing_and_cancellation() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for explicit in [true, false] {
                let baseline = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close();
                    r.pending_displays = vec![Rect {
                        left: -500,
                        top: -300,
                        right: 500,
                        bottom: 500,
                    }];
                    create_canvas(r, 0);
                    r.canvases[0].window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                    );
                    r.annotation_ready = true;
                    assert!(r.set_mode(InteractionMode::Tool(1)));
                    let defaults = r.parameter_values.defaults(4).clone();
                    let kind = if explicit {
                        Kind::HighlightLine
                    } else {
                        Kind::HighlightRectangle
                    };
                    r.session.add(Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind,
                        bounds: Bounds::from_corners((-400., -250.), (-350., -200.)),
                        points: if explicit {
                            vec![(-400., -250.), (-350., -200.)]
                        } else {
                            Vec::new()
                        },
                        style: Style {
                            color: [1, 2, 3],
                            width: 37.,
                            ..Style::default()
                        },
                        extra: Extra::None,
                    });
                    project_object_parameters(r, 0);
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(4);
                    panel.set_values(r.values(4).clone());
                    r.panel.open(panel);
                    if explicit {
                        let mut values = r.values(4).clone();
                        values.shape = 1;
                        let target = r.parameter_target(4);
                        assert!(apply_geometry_values(r, target, values).is_some());
                        r.finish_style_edit();
                    }
                    assert_eq!(r.values(4).shape, 1);
                    let baseline = r.session.objects[0].clone();
                    assert_eq!(
                        r.parameter_values.defaults(4).size,
                        if explicit { 37 } else { defaults.size }
                    );
                    assert_eq!(
                        r.parameter_values.defaults(4).color_index,
                        if explicit { 8 } else { defaults.color_index }
                    );
                    baseline
                });
                for n in 0..2 {
                    let x = 300. + n as f32 * 180.;
                    canvas_pointer(0, 0, x / dpi, 250. / dpi);
                    REGISTRY.with(|slot| {
                        let slot = slot.borrow();
                        let r = slot.as_ref().unwrap();
                        assert_eq!(r.values(4).shape, 1);
                        assert_eq!(r.panel.get().unwrap().get_values().shape, 1);
                        assert!(matches!(
                            r.gesture(),
                            Some(Gesture::Draw {
                                kind: Kind::HighlightRectangle,
                                ..
                            })
                        ));
                    });
                    canvas_pointer(0, 1, (x + 120.) / dpi, 350. / dpi);
                    canvas_pointer(0, 2, (x + 120.) / dpi, 350. / dpi);
                }
                canvas_pointer(0, 0, 750. / dpi, 500. / dpi);
                canvas_pointer(0, 1, 850. / dpi, 600. / dpi);
                canvas_pointer(0, 3, 850. / dpi, 600. / dpi);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(r.session.objects.len(), 3);
                    assert_eq!(r.session.objects[0], baseline);
                    assert!(
                        r.session.objects[1..]
                            .iter()
                            .all(|o| o.kind == Kind::HighlightRectangle)
                    );
                    assert_eq!(r.parameter_values.defaults(4).shape, 1);
                    assert!(r.session.undo());
                    assert_eq!(r.session.objects.len(), 2);
                    assert!(r.session.undo());
                    assert_eq!(r.session.objects, vec![baseline]);
                    restore_parameter_projection(r);
                    assert!(r.set_mode(InteractionMode::Tool(2)));
                    assert!(r.set_mode(InteractionMode::Tool(1)));
                    assert_eq!(active_kind(r), Some(Kind::HighlightRectangle));
                    let mut values = r.values(4).clone();
                    values.shape = 0;
                    let target = r.parameter_target(4);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    r.finish_style_edit();
                });
                canvas_pointer(0, 0, 750. / dpi, 150. / dpi);
                canvas_pointer(0, 1, 850. / dpi, 250. / dpi);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(matches!(
                        r.gesture(),
                        Some(Gesture::Draw {
                            kind: Kind::HighlightLine
                        })
                    ));
                    assert_eq!(r.values(4).shape, 0);
                });
                canvas_pointer(0, 3, 850. / dpi, 250. / dpi);
            }
        }
        shutdown();
    }

    #[test]
    fn new_sequence_adopts_displayed_settings_and_cancel_restores_selected_projection() {
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.mode = InteractionMode::Tool(4);
            r.sync_edit_scope();
            let mut existing = crate::annotation_sequence::tests::badge(1.);
            existing.style.color = [1, 2, 3];
            if let Extra::Sequence(sequence) = &mut existing.extra {
                sequence.size = 90.;
            }
            crate::annotation_sequence::layout(&mut existing);
            assert!(r.session.add(existing));
            let old_id = r.session.objects[0].id;
            let old_next = r.session.next_sequence;
            project_object_parameters(r, 0);
            assert_eq!(r.values(10).text_size, 90);
            let placed = sequence_from_values(r, (100., 100.), 1.25);
            let Extra::Sequence(sequence) = &placed.extra else {
                panic!()
            };
            assert_eq!(
                sequence.size,
                r.parameter_values.defaults(10).text_size as f32
            );
            assert_ne!(placed.style.color, [1, 2, 3]);
            begin_sequence_placement(r, (100., 100.), 1.25);
            assert!(cancel_sequence_placement(r));
            assert_eq!(
                r.session.selected.map(|index| r.session.objects[index].id),
                Some(old_id)
            );
            assert_eq!(r.session.next_sequence, old_next);
            assert_eq!(r.values(10).text_size, 90);
            assert_eq!(r.parameter_values.defaults(10).text_size, 90);
            assert_eq!(r.mode, InteractionMode::Tool(4));
        });
        REGISTRY.with(|slot| {
            slot.borrow_mut().take();
        });
    }

    #[test]
    fn sequence_press_drag_release_and_cancellation_do_not_create_hover_frames() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.pending_displays = vec![Rect {
                    left: -500,
                    top: -300,
                    right: 500,
                    bottom: 500,
                }];
                create_canvas(r, 0);
                r.canvases[0]
                    .window
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                r.annotation_ready = true;
                r.mode = InteractionMode::Tool(4);
                r.sync_edit_scope();
            });
            for x in 40..100 {
                canvas_pointer(0, 1, x as f32, 80.);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(
                        gestures::display_draft(&r.gesture, &r.pending_drawing).is_none()
                            && r.session.objects.is_empty()
                    );
                    assert!(!r.frame_tasks.render_pending());
                    assert_eq!(
                        r.canvases[0].window.get_cursor(),
                        crate::AnnotationCursor::Drawing
                    );
                });
            }
            canvas_pointer(0, 0, 80., 80.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert_eq!(r.session.objects.len(), 1);
                assert_eq!(
                    r.session.objects[0].bounds.center(),
                    (-500. + 80. * dpi, -300. + 80. * dpi)
                );
                assert!(r.sequence_placement().is_some() && r.text_handle().is_none());
                assert_eq!(r.session.next_sequence, 2);
                assert!(r.session.selected.is_none() && !r.session.can_undo());
            });
            canvas_pointer(0, 1, 280., 200.);
            canvas_pointer(0, 2, 300., 210.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert_eq!(
                    r.session.objects[0].bounds.center(),
                    (-500. + 300. * dpi, -300. + 210. * dpi)
                );
                assert!(r.sequence_placement().is_none() && r.text_handle().is_none());
                assert_eq!(r.session.selected, Some(0));
                assert!(r.session.can_undo());
            });
            undo();
            // A new press retires an interrupted placement instead of joining its snapshot.
            canvas_pointer(0, 0, 80., 80.);
            canvas_pointer(0, 1, 200., 180.);
            canvas_pointer(0, 0, 100., 120.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert_eq!(r.session.objects.len(), 1);
                assert_eq!(r.session.next_sequence, 2);
                assert_eq!(
                    r.session.objects[0].bounds.center(),
                    (-500. + 100. * dpi, -300. + 120. * dpi)
                );
            });
            canvas_pointer(0, 3, 100., 120.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(r.session.objects.is_empty() && !r.session.can_undo());
                assert!(r.sequence_placement().is_none());
                assert_eq!(r.session.next_sequence, 1);
            });
            for cancel in 0..4 {
                canvas_pointer(0, 0, 80., 80.);
                canvas_pointer(0, 1, 200., 180.);
                match cancel {
                    0 => escape(),
                    1 => canvas_pointer(0, 3, 200., 180.),
                    2 => {
                        let sink = REGISTRY.with(|slot| {
                            pointer_canvas(
                                slot.borrow().as_ref().unwrap().canvases[0].window.as_weak(),
                                slot.borrow().as_ref().unwrap().generation,
                                0,
                                slot.borrow().as_ref().unwrap().canvases[0].revision(),
                                slot.borrow().as_ref().unwrap().canvases[0].input(),
                            )
                        });
                        sink(PopupPointerInput::LeftCancelled {
                            x: 200. * dpi,
                            y: 180. * dpi,
                        });
                    }
                    _ => REGISTRY.with(|slot| {
                        slot.borrow_mut()
                            .as_mut()
                            .unwrap()
                            .set_mode(InteractionMode::Tool(0));
                    }),
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    assert_eq!(r.session.next_sequence, 1);
                    assert!(r.gesture().is_none() && r.text_handle().is_none());
                    assert!(r.geometry_drag().is_none());
                    assert!(r.sequence_placement().is_none());
                    r.mode = InteractionMode::Tool(4);
                    r.sync_edit_scope();
                });
            }
            canvas_pointer(0, 0, 80., 80.);
            canvas_pointer(0, 2, 80., 80.);
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert_eq!(r.session.objects.len(), 1);
                assert_eq!(r.session.objects[0].kind, Kind::Sequence);
                assert!(r.session.object_index(r.session.objects[0].id).is_some());
            });
            open_text_editor((-100., 0.), Some(0), dpi);
            let (generation, old_id) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                let edit = r.editor.edit().unwrap();
                assert!(edit.caret.visible && edit.caret_running());
                assert!(edit.original.content.is_empty());
                (r.generation, edit.id)
            });
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(!r.editor.edit().unwrap().caret.visible);
                reset_text_caret(r);
                assert!(r.editor.edit().unwrap().caret.visible);
                r.editor.edit_mut().unwrap().selection_anchor = Some(0);
            });
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.editor.edit().unwrap().caret.visible);
                finish_text_edit_registry(r, false);
            });
            open_text_editor((-100., 0.), Some(0), dpi);
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_ne!(r.editor.edit().unwrap().id, old_id);
                assert!(r.editor.edit().unwrap().caret.visible);
                finish_text_edit_registry(r, false);
            });
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.session.objects.len(), 1);
                assert!(r.session.undo());
                assert!(!r.session.can_undo());
                begin_sequence_placement(r, (-100., 0.), dpi);
                assert!(r.sequence_placement().is_some());
                r.close();
                assert!(r.sequence_placement().is_none() && r.gesture().is_none());
            });
        }
        REGISTRY.with(|slot| {
            slot.borrow_mut().take();
        });
    }

    #[test]
    fn software_capture_excludes_annotation_ui_and_reissues_for_replaced_canvas() {
        use lexift_core::ports::{magnifier::WindowToken, screen_capture::*};
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::sync::{Arc, Mutex};
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        struct Capture(Arc<Mutex<Vec<ScreenCaptureRequest>>>);
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, request: ScreenCaptureRequest) -> lexift_core::Result<()> {
                self.0.lock().unwrap().push(request);
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_screen_capture(Some(Arc::new(Capture(requests.clone()))))
            .with_annotation_window_token(|window| {
                Some(WindowToken(window as *const slint::Window as isize))
            }),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let bounds = Rect {
                left: -100,
                top: -100,
                right: 100,
                bottom: 100,
            };
            r.canvases
                .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
            r.main.open(AnnotationToolbar::new().unwrap());
            r.panel.open(AnnotationPanel::new().unwrap());
            r.watermark.open(AnnotationWatermarkInput::new().unwrap());
            r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((-80., -80.), (80., 80.)),
                points: Vec::new(),
                style: Style::default(),
                extra: mosaic::settings(r.values(11), 1.),
            });
            mosaic::sync(r);
            mosaic::sync(r);
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "unchanged capture policy must not reissue"
            );
            let old = r.canvases[0].window.clone_strong();
            let old_token = (r.lifecycle.annotation_window_token)(old.window()).unwrap();
            let expected = capture_exclusions(r);
            assert_eq!(expected.len(), 4);
            assert_eq!(requests.lock().unwrap()[0].excluded_windows, expected);
            assert!(
                requests.lock().unwrap()[0].paused_regions.is_empty(),
                "tool windows must be excluded without freezing mosaic edits"
            );
            r.panel
                .get()
                .unwrap()
                .window()
                .set_position(slint::PhysicalPosition::new(-50, -50));
            mosaic::sync(r);
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "moving an excluded toolbar must not pause capture"
            );
            r.canvases[0] = CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds);
            mosaic::sync(r);
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].regions, requests[1].regions);
            assert_eq!(requests[0].paused_regions, requests[1].paused_regions);
            assert!(!requests[1].excluded_windows.contains(&old_token));
            assert_eq!(requests[1].excluded_windows, capture_exclusions(r));
            assert!(requests[1].revision > requests[0].revision);
        });
        shutdown();
    }

    #[test]
    fn replaced_canvas_at_same_index_rejects_old_input_and_keyboard_callbacks() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let (old, sink, current) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.pending_displays = vec![Rect {
                left: -100,
                top: -100,
                right: 100,
                bottom: 100,
            }];
            create_canvas(r, 0);
            let old = r.canvases[0].window.clone_strong();
            let old_revision = r.canvases[0].revision();
            let input = r.canvases[0].input();
            old.invoke_escape_requested();
            assert!(input.pending());
            let sink = pointer_canvas(old.as_weak(), r.generation, 0, old_revision, input.clone());
            r.canvases.clear();
            sink(PopupPointerInput::LeftCancelled { x: 0., y: 0. });
            assert!(input.retired() && !input.pending());
            old.invoke_undo_requested();
            assert!(!input.pending());
            create_canvas(r, 0);
            assert_ne!(r.canvases[0].revision(), old_revision);
            r.annotation_ready = true;
            (old, sink, r.canvases[0].window.clone_strong())
        });
        old.invoke_pointer(0, 20., 20.);
        old.invoke_pointer(2, 80., 80.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.session.objects.is_empty());
            assert!(r.gesture().is_none());
        });
        current.invoke_pointer(0, 20., 20.);
        sink(PopupPointerInput::LeftCancelled { x: 30., y: 30. });
        old.invoke_undo_requested();
        old.invoke_escape_requested();
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.canvases.len(), 1);
            assert!(r.gesture().is_some());
        });
        current.invoke_pointer(1, 80., 80.);
        current.invoke_pointer(2, 80., 80.);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.session.objects.len(), 1);
            assert!(r.session.undo());
            assert!(!r.session.can_undo());
        });
        shutdown();
    }

    #[test]
    fn unowned_pointer_completion_does_not_finish_or_cancel_parameter_transactions() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for cancel_draft in [false, true] {
                let (baseline, opacity) = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.finish_style_edit();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.selected[0] = 0;
                    r.mode = InteractionMode::Tool(0);
                    r.sync_edit_scope();
                    r.annotation_ready = true;
                    r.canvases.clear();
                    let window = AnnotationCanvas::new().unwrap();
                    window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                    );
                    r.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -100,
                            top: -100,
                            right: 100,
                            bottom: 100,
                        },
                    ));
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind: Kind::Rectangle,
                        bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::None,
                    }));
                    let baseline = r.session.objects[0].clone();
                    let opacity = r.session.spotlight_opacity;
                    let tool = if cancel_draft { 2 } else { 0 };
                    r.selected[0] = tool;
                    r.sync_edit_scope();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(tool);
                    panel.set_values(defaults(tool));
                    r.panel.open(panel);
                    if cancel_draft {
                        r.session.selected = None;
                    }
                    let mut values = r.values(tool as usize).clone();
                    if cancel_draft {
                        values.strength = 28.;
                    } else {
                        parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33])
                            .apply(&mut values);
                    }
                    let target = r.parameter_target(tool);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    (baseline, opacity)
                });
                if cancel_draft {
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        let mut press = drawing::Press::begin(
                            (-50., -50.),
                            Kind::SpotlightRectangle,
                            Style::default(),
                            dpi,
                            &defaults(2),
                        );
                        press.update((40., 30.), dpi);
                        r.gesture = Some(Active::Drawing(press));
                        assert!(cancel_active_gesture(r));
                        assert!(
                            (r.session.spotlight_opacity - 0.28).abs() < 0.0001,
                            "draft cancellation cannot roll back parameter opacity"
                        );
                        r.finish_style_edit();
                        assert!(r.session.undo());
                        assert_eq!(r.session.spotlight_opacity, opacity);
                        assert_eq!(r.session.objects, vec![baseline]);
                    });
                } else {
                    canvas_pointer(0, 2, 30., 40.);
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(r.gesture().is_none());
                        let mut values = r.values(0).clone();
                        parameters::Edit::Color(parameters::ColorTarget::Body, [44, 55, 66])
                            .apply(&mut values);
                        let target = r.parameter_target(0);
                        assert!(apply_geometry_values(r, target, values).is_some());
                        r.finish_style_edit();
                        assert!(r.session.undo());
                        assert_eq!(
                            r.session.objects,
                            vec![baseline],
                            "one undo must restore both continuous updates"
                        );
                    });
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty());
                    assert!(
                        !r.session.can_undo(),
                        "parameter edits remain a single operation separate from creation"
                    );
                });
            }
        }
        shutdown();
    }

    /// Isolated CPU measurements; excludes capture, GPU, native submit and display.
    #[test]
    #[ignore = "run explicitly in Release without concurrent builds for CPU profiling"]
    fn long_trajectory_signature_and_cache_costs() {
        use std::hint::black_box;
        fn p95(mut samples: Vec<Duration>) -> f64 {
            samples.sort_unstable();
            samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1000.
        }
        let viewport = Rect {
            left: -128,
            top: -128,
            right: 128,
            bottom: 128,
        };
        for points in [1_000, 20_000, 100_000] {
            let mut session = Session::new();
            assert!(
                session.add(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::Pencil,
                    bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                    points: (0..points)
                        .map(|i| (
                            -40. + 80. * i as f32 / points as f32,
                            -30. + 60. * i as f32 / points as f32
                        ))
                        .collect(),
                    style: Style::default(),
                    extra: Extra::None,
                })
            );
            let reference = session.objects[0].clone();
            let mut compiler = crate::annotation_scene::SceneCompiler::default();
            black_box(
                compiler
                    .document(&session, viewport, 1., None, None)
                    .unwrap(),
            );
            let mut signature = Vec::new();
            let mut compare = Vec::new();
            let mut copy = Vec::new();
            let mut cached_scene = Vec::new();
            for _ in 0..240 {
                let start = Instant::now();
                black_box(render_signature(
                    black_box(&session),
                    viewport,
                    1.,
                    None,
                    None,
                ));
                signature.push(start.elapsed());
                let start = Instant::now();
                black_box(black_box(&session.objects[0]) == black_box(&reference));
                compare.push(start.elapsed());
                let start = Instant::now();
                drop(black_box(black_box(&session.objects[0]).clone()));
                copy.push(start.elapsed());
                let start = Instant::now();
                black_box(
                    compiler
                        .document(black_box(&session), viewport, 1., None, None)
                        .unwrap(),
                );
                cached_scene.push(start.elapsed());
            }
            eprintln!(
                "trajectory_cpu_scope=one_visible_unchanged_pencil_one_viewport points={points} samples=240 signature_p95_ms={:.6} equality_p95_ms={:.6} clone_drop_p95_ms={:.6} cached_scene_p95_ms={:.6} point_payload_bytes={}",
                p95(signature),
                p95(compare),
                p95(copy),
                p95(cached_scene),
                points * std::mem::size_of::<(f32, f32)>()
            );
        }
    }

    /// Isolates native scene preparation and shared software inputs; no capture or display work.
    #[test]
    #[ignore = "run explicitly in Release without concurrent builds for CPU profiling"]
    fn long_mosaic_native_preparation_costs() {
        use lexift_core::{
            domain::annotation::{MosaicAnnotation, MosaicEffect},
            ports::annotation_render::{RenderStamp, RenderSurface},
        };
        use std::hint::black_box;
        fn p95(mut samples: Vec<Duration>) -> f64 {
            samples.sort_unstable();
            samples[(samples.len() * 95 / 100).min(samples.len() - 1)].as_secs_f64() * 1000.
        }
        let surfaces = [
            RenderSurface {
                input_window: lexift_core::ports::magnifier::WindowToken(1),
                bounds: Rect {
                    left: -512,
                    top: -128,
                    right: 0,
                    bottom: 128,
                },
                scale: 1.,
            },
            RenderSurface {
                input_window: lexift_core::ports::magnifier::WindowToken(2),
                bounds: Rect {
                    left: 0,
                    top: -128,
                    right: 512,
                    bottom: 128,
                },
                scale: 1.25,
            },
        ];
        for points in [1_000, 20_000, 100_000] {
            let mut session = Session::new();
            session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicBrush,
                bounds: Bounds::from_corners((-400., -20.), (400., 20.)),
                points: (0..points)
                    .map(|i| {
                        let x = -400. + 800. * i as f32 / points as f32;
                        (x, (x / 80.).sin() * 20.)
                    })
                    .collect(),
                style: Style::default(),
                extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Blur, 100, 12., 1.25)),
            });
            session.selected = None;
            let mut compiler = crate::annotation_scene::SceneCompiler::default();
            let compile = |compiler: &mut crate::annotation_scene::SceneCompiler, revision| {
                compiler
                    .compile(crate::annotation_scene::SceneInput {
                        stamp: RenderStamp {
                            session: 1,
                            revision,
                        },
                        session: &session,
                        edit: None,
                        surfaces: &surfaces,
                        draft: None,
                        hovered: None,
                        reference: None,
                        editor: None,
                        paused_regions: &[],
                        excluded_windows: &[],
                    })
                    .unwrap()
            };
            black_box(compile(&mut compiler, 0));
            let mut snapshots = crate::annotation_render::mosaic_worker::Snapshot::default();
            black_box(snapshots.get(&session, None));
            let mut native = Vec::new();
            let mut signatures = Vec::new();
            let mut snapshot = Vec::new();
            for revision in 1..=240 {
                let start = Instant::now();
                black_box(compile(&mut compiler, revision));
                native.push(start.elapsed());
                let start = Instant::now();
                let frame = crate::annotation_signature::Frame::new(&session, None);
                for surface in &surfaces {
                    black_box(frame.document_signature(surface.bounds, surface.scale, None));
                    black_box(frame.display(surface.bounds, surface.scale, None));
                }
                assert_eq!(frame.computed_objects(), 1);
                signatures.push(start.elapsed());
                let start = Instant::now();
                black_box(snapshots.get(&session, None));
                snapshot.push(start.elapsed());
            }
            let (buffers, bytes) = compiler.retained_trajectory_payload();
            eprintln!(
                "trajectory_cpu_scope=unchanged_mosaic_brush_two_native_surfaces points={points} samples=240 native_prepare_p95_ms={:.6} four_signatures_p95_ms={:.6} snapshot_reuse_p95_ms={:.6} retained_point_buffers={buffers} retained_point_capacity_bytes={bytes}",
                p95(native),
                p95(signatures),
                p95(snapshot)
            );
        }
    }

    #[test]
    fn geometry_handoff_retires_parameter_idle_without_merging_undo() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for cancel in [false, true] {
                for close_panel in [false, true] {
                    let original = REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        r.close_panel();
                        r.session = Session::new();
                        r.parameter_values = parameters::State::new();
                        r.mode = InteractionMode::Tool(0);
                        r.sync_edit_scope();
                        r.annotation_ready = true;
                        r.canvases.clear();
                        let window = AnnotationCanvas::new().unwrap();
                        window.window().dispatch_event(
                            slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                        );
                        r.canvases.push(CanvasLayer::new(
                            window,
                            Rect {
                                left: -100,
                                top: -100,
                                right: 100,
                                bottom: 100,
                            },
                        ));
                        assert!(r.session.add(Object {
                            erased: Vec::new(),
                            id: Default::default(),
                            kind: Kind::Rectangle,
                            bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                            style: Style::default(),
                            points: Vec::new(),
                            extra: Extra::None,
                        }));
                        let original = r.session.objects[0].clone();
                        let panel = AnnotationPanel::new().unwrap();
                        panel.set_tool(0);
                        r.panel.open(panel);
                        restore_parameter_projection(r);
                        let target = r.parameter_target(0);
                        let mut values = r.values(0).clone();
                        values.size = 25;
                        assert!(apply_geometry_values(r, target, values).is_some());
                        assert!(r.parameter_edit.active());
                        original
                    });
                    // Use the production hit-test/press route, not a manually opened snapshot.
                    canvas_pointer(0, 0, 80. / dpi, 70. / dpi);
                    canvas_pointer(0, 1, 100. / dpi, 80. / dpi);
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(matches!(r.gesture(), Some(Gesture::Move)));
                        assert!(r.geometry_drag().is_some() && r.text_handle().is_none());
                        assert!(!r.parameter_edit.active());
                        if close_panel {
                            r.close_panel();
                        }
                    });
                    clock.set(clock.get() + Duration::from_millis(600));
                    slint::platform::update_timers_and_animations();
                    REGISTRY.with(|slot| {
                        let r = slot.borrow();
                        assert!(matches!(r.as_ref().unwrap().gesture(), Some(Gesture::Move)));
                    });
                    if cancel {
                        REGISTRY.with(|slot| {
                            assert!(cancel_active_gesture(slot.borrow_mut().as_mut().unwrap()));
                        });
                    } else {
                        canvas_pointer(0, 2, 100. / dpi, 80. / dpi);
                    }
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(r.gesture().is_none());
                        assert_eq!(r.session.objects[0].style.width, 25.);
                        if !cancel {
                            assert_ne!(r.session.objects[0].bounds, original.bounds);
                            assert!(
                                r.session.undo(),
                                "movement owns its own undo after idle and panel close"
                            );
                        }
                        assert_eq!(r.session.objects[0].bounds, original.bounds);
                        assert!(
                            r.session.undo(),
                            "earlier parameters remain an independent undo"
                        );
                        assert_eq!(r.session.objects, vec![original]);
                        assert!(r.session.undo());
                        assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    });
                }
            }
        }
        shutdown();
    }

    #[test]
    fn delayed_parameters_wait_for_document_gesture_commit_or_cancel() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for cancel in [false, true] {
                let baseline = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.mode = InteractionMode::Tool(0);
                    r.sync_edit_scope();
                    r.annotation_ready = true;
                    r.canvases.clear();
                    let window = AnnotationCanvas::new().unwrap();
                    window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                    );
                    r.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -100,
                            top: -100,
                            right: 100,
                            bottom: 100,
                        },
                    ));
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: Default::default(),
                        kind: Kind::Rectangle,
                        bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::None,
                    }));
                    let baseline = r.session.objects[0].clone();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(0);
                    r.panel.open(panel);
                    restore_parameter_projection(r);
                    r.session.begin_drag();
                    r.gesture = (gestures::GeometryDrag::moving(&r.session, baseline.id, (0., 0.)))
                        .map(Active::Geometry);
                    let target = r.parameter_target(0);
                    for rgb in [[11, 22, 33], [44, 55, 66]] {
                        let mut values = r.values(0).clone();
                        parameters::Edit::Color(parameters::ColorTarget::Body, rgb)
                            .apply(&mut values);
                        assert!(apply_geometry_values(r, target, values).is_none());
                    }
                    r.apply_menu_edit(
                        target,
                        parameters::Edit::Choice(parameters::ChoiceTarget::Style, 1),
                    );
                    assert_eq!(
                        parameters::style(&r.panel.get().unwrap().get_values(), 0).color,
                        [44, 55, 66]
                    );
                    assert_eq!(r.deferred_parameters.len(), 1);
                    assert!(!r.parameter_edit.active());
                    baseline
                });
                canvas_pointer(0, 1, 120. / dpi, 110. / dpi);
                clock.set(clock.get() + Duration::from_millis(600));
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(r.session.objects[0].style.color, baseline.style.color);
                    r.finish_style_edit();
                    assert!(r.gesture().is_some());
                });
                if cancel {
                    REGISTRY.with(|slot| {
                        assert!(cancel_active_gesture(slot.borrow_mut().as_mut().unwrap()))
                    });
                } else {
                    canvas_pointer(0, 2, 120. / dpi, 110. / dpi);
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.deferred_parameters.is_empty());
                    assert_eq!(r.session.objects[0].style.color, [44, 55, 66]);
                    r.finish_style_edit();
                    assert!(r.session.undo(), "parameter update owns a separate undo");
                    assert_eq!(r.session.objects[0].style, baseline.style);
                    if !cancel {
                        assert_ne!(r.session.objects[0].bounds, baseline.bounds);
                        assert!(
                            r.session.undo(),
                            "movement remains one undo after idle time"
                        );
                    }
                    assert_eq!(r.session.objects, vec![baseline]);
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty());
                    assert!(!r.session.can_undo());
                });
            }
        }
        shutdown();
    }

    #[test]
    fn sequence_placement_parameters_keep_default_target_after_release_or_cancel() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for cancel in [false, true] {
                let (original, target, changed) = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.mode = InteractionMode::Tool(4);
                    r.sync_edit_scope();
                    r.annotation_ready = true;
                    r.canvases.clear();
                    let window = AnnotationCanvas::new().unwrap();
                    window
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    r.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -500,
                            top: -300,
                            right: 500,
                            bottom: 500,
                        },
                    ));
                    let mut existing = crate::annotation_sequence::tests::badge(dpi);
                    crate::annotation_sequence::layout(&mut existing);
                    assert!(r.session.add(existing));
                    let original = r.session.objects[0].clone();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(10);
                    r.panel.open(panel);
                    restore_parameter_projection(r);
                    begin_sequence_placement(r, (-200., -100.), dpi);
                    let target = r.parameter_target(10);
                    assert!(target.object.is_none());
                    let mut changed = r.values(10).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33])
                        .apply(&mut changed);
                    changed.start = 20;
                    assert!(apply_geometry_values(r, target, changed.clone()).is_none());
                    assert_eq!(r.deferred_parameters.len(), 1);
                    assert!(!r.parameter_edit.active());
                    (original, target, changed)
                });
                if cancel {
                    REGISTRY.with(|slot| {
                        assert!(cancel_sequence_placement(
                            slot.borrow_mut().as_mut().unwrap()
                        ));
                    });
                } else {
                    canvas_pointer(0, 2, 320. / dpi, 220. / dpi);
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.deferred_parameters.is_empty());
                    assert_eq!(
                        r.parameter_values.defaults(10).color_index,
                        changed.color_index
                    );
                    assert_eq!(
                        r.parameter_values.defaults(10).custom_color,
                        changed.custom_color
                    );
                    assert_eq!(r.session.next_sequence, 20);
                    assert_eq!(r.session.objects[0], original);
                    if cancel {
                        assert_eq!(r.session.selected, Some(0));
                    } else {
                        assert_eq!(r.session.selected, Some(1));
                        assert_ne!(r.session.objects[1].style.color, [11, 22, 33]);
                    }
                    r.finish_style_edit();
                    assert!(
                        r.session.undo(),
                        "number change owns a separate parameter undo"
                    );
                    assert_eq!(r.session.selected, if cancel { Some(0) } else { None });
                    assert_eq!(r.session.next_sequence, if cancel { 2 } else { 3 });
                    if !cancel {
                        assert!(r.session.undo(), "creation plus placement owns one undo");
                    }
                    assert_eq!(r.session.objects, vec![original]);
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    // An old target must not mutate a reopened parameter panel.
                    r.close_panel();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(10);
                    r.panel.open(panel);
                    let before = r.parameter_values.defaults(10).clone();
                    assert!(apply_geometry_values(r, target, changed).is_none());
                    assert_eq!(r.parameter_values.defaults(10), &before);
                });
            }
        }
        shutdown();
    }

    #[test]
    fn retired_sequence_panel_discards_waiting_requests_before_reopen() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.mode = InteractionMode::Tool(4);
            r.sync_edit_scope();
            let mut original = crate::annotation_sequence::tests::badge(1.25);
            crate::annotation_sequence::layout(&mut original);
            assert!(r.session.add(original));
            let original = r.session.objects[0].clone();
            let panel = AnnotationPanel::new().unwrap();
            panel.set_tool(10);
            r.panel.open(panel);
            restore_parameter_projection(r);
            begin_sequence_placement(r, (-200., -100.), 1.25);
            let retired_target = r.parameter_target(10);
            let mut pending = r.values(10).clone();
            parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33])
                .apply(&mut pending);
            pending.start = 20;
            assert!(apply_geometry_values(r, retired_target, pending.clone()).is_none());
            assert_eq!(r.deferred_parameters.len(), 1);
            r.close_panel();
            assert!(r.deferred_parameters.is_empty());
            let reopened = AnnotationPanel::new().unwrap();
            reopened.set_tool(10);
            r.panel.open(reopened);
            assert!(apply_geometry_values(r, retired_target, pending).is_none());
            assert!(cancel_sequence_placement(r));
            assert_eq!(r.session.objects, vec![original.clone()]);
            assert_eq!(r.session.selected, Some(0));
            assert_eq!(r.session.next_sequence, 2);
            assert_ne!(
                parameters::style(r.parameter_values.defaults(10), 10).color,
                [11, 22, 33]
            );
            assert!(!r.parameter_edit.active());
            assert!(r.session.undo());
            assert!(r.session.objects.is_empty() && !r.session.can_undo());
        });
        shutdown();
    }

    #[test]
    fn brush_wheel_updates_open_panel_parameters_without_feedback_callbacks() {
        use lexift_core::ports::screen_capture::{
            ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
        };
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<std::sync::Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_screen_capture(Some(std::sync::Arc::new(Capture))),
        );
        for dpi in [1., 1.25] {
            for selected in [false, true] {
                let original = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.main.open(AnnotationToolbar::new().unwrap());
                    r.annotation_ready = true;
                    r.mode = InteractionMode::Tool(5);
                    r.sync_edit_scope();
                    r.selected[5] = 11;
                    r.sync_edit_scope();
                    r.edit_values(11).shape = 1;
                    r.canvases.clear();
                    let window = AnnotationCanvas::new().unwrap();
                    window
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    r.canvases.push(CanvasLayer::new(
                        window,
                        Rect {
                            left: -500,
                            top: -300,
                            right: 500,
                            bottom: 500,
                        },
                    ));
                    if selected {
                        let mut object = Object {
                            erased: Vec::new(),
                            id: Default::default(),
                            kind: Kind::MosaicBrush,
                            bounds: Bounds::from_corners((-200., -100.), (-180., -80.)),
                            style: Style::default(),
                            points: vec![(-200., -100.), (-180., -80.)],
                            extra: Extra::Mosaic(
                                lexift_core::domain::annotation::MosaicAnnotation::new(
                                    lexift_core::domain::annotation::MosaicEffect::Blur,
                                    70,
                                    12.,
                                    dpi,
                                ),
                            ),
                        };
                        object.refresh_bounds();
                        assert!(r.session.add(object));
                    }
                    restore_parameter_projection(r);
                    r.session.objects.first().cloned()
                });
                open_panel(5, false);
                brush_size_scrolled(0, 1.);
                brush_size_scrolled(0, 1.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(r.values(11).size, 14);
                    assert_eq!(r.panel.get().unwrap().get_values().size, 14);
                    if let Some(original) = original {
                        let Extra::Mosaic(value) = &r.session.objects[0].extra else {
                            panic!()
                        };
                        assert_eq!(value.diameter, 12. * dpi);
                        assert_eq!(
                            value.effect,
                            lexift_core::domain::annotation::MosaicEffect::Blur
                        );
                        assert_eq!(value.strength, 70);
                        r.finish_style_edit();
                        assert_eq!(r.session.objects, vec![original]);
                        assert_eq!(r.session.selected, None);
                        assert!(r.session.undo());
                    } else {
                        r.finish_style_edit();
                    }
                    assert!(!r.session.can_undo());
                });
            }
        }
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.close_panel();
            r.session = Session::new();
            r.parameter_values = parameters::State::new();
            r.mode = InteractionMode::Tool(6);
            r.sync_edit_scope();
            r.edit_values(13).shape = 1;
        });
        open_panel(6, false);
        let (panel, generation, revision) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            (
                r.panel.get().unwrap().clone_strong(),
                r.generation,
                r.panel.revision(),
            )
        });
        panel.window().set_size(slint::LogicalSize::new(329., 58.));
        panel.show().unwrap();
        let wheel = |x, delta| {
            panel.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(x, 29.),
                delta_x: 0.,
                delta_y: delta,
            });
        };
        wheel(120., 120.); // Icon hit area.
        assert_eq!(panel.get_values().size, 13);
        choice::open(
            generation,
            revision,
            crate::AnnotationChoiceRequest {
                field: -9,
                ..Default::default()
            },
        );
        wheel(162., 120.); // Numeric chip, with the popup open.
        assert_eq!(panel.get_values().size, 14);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.values(13).size, 14);
            assert_eq!(
                r.menus.current.as_ref().unwrap().window.get_numeric_value(),
                14
            );
            assert!(!r.session.can_undo());
        });
        for _ in 0..110 {
            wheel(120., 120.);
        }
        assert_eq!(panel.get_values().size, 100);
        for _ in 0..110 {
            wheel(162., -120.);
        }
        assert_eq!(panel.get_values().size, 1);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let press = eraser::Press::begin(&mut r.session, (0., 0.), true, 1., &[]);
            r.gesture = Some(Active::Erasing(press));
        });
        wheel(120., 120.);
        assert_eq!(
            panel.get_values().size,
            2,
            "wheel updates the next gesture only"
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let Some(Active::Erasing(press)) = &r.gesture else {
                panic!()
            };
            assert_eq!(press.diameter, 1., "current stroke diameter remains frozen");
            r.gesture = None;
            r.session.cancel_drag();
            r.edit_values(13).shape = 0;
            panel.set_values(r.values(13).clone());
        });
        wheel(162., 120.);
        assert_eq!(panel.get_values().size, 1, "rectangle mode disables wheel");
        REGISTRY.with(|slot| assert!(!slot.borrow().as_ref().unwrap().session.can_undo()));
        shutdown();
        panel.invoke_numeric_scrolled(-9, 120.); // A retired panel must be harmless.
    }

    #[test]
    fn queued_panel_field_update_preserves_a_later_menu_field() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.annotation_ready = true;
            assert!(r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::None,
            }));
            restore_parameter_projection(r);
        });
        open_panel(0, false);
        // Capture a toolbar edit, then let a menu update a different field
        // before the deferred toolbar callback reaches the Registry.
        let (panel, target) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            (r.panel.get().unwrap().clone_strong(), r.parameter_target(0))
        });
        let mut values = panel.get_values();
        values.size = 25;
        panel.invoke_values_changed(values);
        REGISTRY.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().apply_menu_edit(
                target,
                parameters::Edit::Color(parameters::ColorTarget::Body, [11, 22, 33]),
            );
        });
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.session.objects[0].style.width, 25.);
            assert_eq!(r.session.objects[0].style.color, [11, 22, 33]);
            r.finish_style_edit();
            assert!(r.session.undo());
            assert_eq!(r.session.objects[0].style, Style::default());
        });
        shutdown();
    }

    #[test]
    fn queued_object_panel_requires_the_originating_canvas_selection_and_session() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        };
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let bounds = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let object = Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-30., -30.), (30., 30.)),
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::None,
        };
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.canvases
                .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
            assert!(r.session.add(object.clone()));
            r.generation
        });
        queue_geometry_panel(generation, 0);
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().session.selected = None);
        advance();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.panel.get().is_none());
            r.session.selected = Some(0);
        });
        queue_geometry_panel(generation, 0);
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().mode = InteractionMode::Mouse);
        advance();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.panel.get().is_none());
            r.mode = InteractionMode::Tool(0);
            r.sync_edit_scope();
        });
        queue_geometry_panel(generation, 0);
        REGISTRY.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().canvases[0] =
                CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds)
        });
        advance();
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_none()));
        queue_geometry_panel(generation, 0);
        advance();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(
                r.panel.get().is_some(),
                "current request should open its panel"
            );
            r.close_panel();
        });
        queue_geometry_panel(generation, 0);
        close();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_ne!(r.generation, generation);
            r.main.open(AnnotationToolbar::new().unwrap());
            r.canvases
                .push(CanvasLayer::new(AnnotationCanvas::new().unwrap(), bounds));
            assert!(r.session.add(object));
        });
        advance();
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_none()));
        shutdown();
    }

    #[test]
    fn retired_toolbar_cannot_operate_or_reposition_a_reopened_session() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let placement_queries = Rc::new(Cell::new(0));
        let queries = placement_queries.clone();
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(move |_| {
                queries.set(queries.get() + 1);
                Some(Rect {
                    left: -1000,
                    top: -1000,
                    right: 1000,
                    bottom: 1000,
                })
            });
        });
        toggle(Instant::now());
        let (generation, revision, old) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            (
                r.generation,
                r.main.revision(),
                r.main.get().unwrap().clone_strong(),
            )
        });
        let retired_input = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().main.input());
        let old_sink = pointer_main(old.as_weak(), generation, revision, retired_input.clone());
        show_main(generation, revision, 0);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            settle_main_position(
                slot.as_ref().unwrap(),
                generation,
                revision,
                Point::default(),
                2,
            );
        });
        old.invoke_tool_clicked(4, false);
        old.invoke_finish_requested();
        assert!(retired_input.pending());
        schedule_render(generation);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            r.frame_tasks
                .defer_lens(generation, 1, sync_first_magnifier);
            assert!(r.frame_tasks.render_pending());
            assert!(r.frame_tasks.lens_pending());
        });
        let teardown_sink = old_sink.clone();
        let teardown_events = Rc::new(Cell::new(0));
        let recorded = teardown_events.clone();
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .annotation_magnifier_sync = Rc::new(move |_, _| {
                recorded.set(recorded.get() + 1);
                teardown_sink(PopupPointerInput::Moved { x: 0., y: 0. });
                false
            });
        });
        close();
        assert_eq!(teardown_events.get(), 1);
        // Native teardown can deliver a late pointer while Registry is mutably borrowed.
        REGISTRY.with(|slot| {
            let _teardown = slot.borrow_mut();
            old_sink(PopupPointerInput::Moved { x: 0., y: 0. });
        });
        assert!(retired_input.retired() && !retired_input.pending());
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(!r.frame_tasks.render_pending());
            assert!(!r.frame_tasks.lens_pending());
        });
        toggle(Instant::now());
        let new_revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().main.revision());
        assert_ne!(revision, new_revision);
        old.window()
            .set_position(slint::PhysicalPosition::new(-100, -200));
        let old_position = old.window().position();
        old.invoke_escape_requested();
        old.invoke_operate_requested();
        old.invoke_undo_requested();
        assert!(!retired_input.pending());
        old_sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
        clock.set(Duration::from_millis(1000));
        slint::platform::update_timers_and_animations();
        assert_eq!(old.window().position(), old_position);
        assert_eq!(
            placement_queries.get(),
            0,
            "retired placement must not query a work area"
        );
        let current = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.main.revision(), new_revision);
            assert!(r.panel.get().is_none());
            assert_eq!(r.mode, InteractionMode::Tool(0));
            assert!(!r.session.can_undo());
            r.main.get().unwrap().clone_strong()
        });
        current.invoke_tool_clicked(4, false);
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_some()));
        shutdown();
    }

    #[test]
    fn owner_close_rejects_reentrant_pointer_input_from_child_menu_teardown() {
        exercise_menu_owner_teardown(true, false);
    }

    #[test]
    fn child_menu_close_rejects_cleanup_input_without_retiring_its_owner() {
        exercise_menu_owner_teardown(false, false);
    }

    #[test]
    fn child_menu_close_rejects_reentrant_input_to_another_live_canvas() {
        exercise_menu_owner_teardown(false, true);
    }

    fn exercise_menu_owner_teardown(close_owner: bool, canvas_target: bool) {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let native_sink = Rc::new(RefCell::new(None::<crate::bridge::PopupPointerSink>));
        let cleanup_events = Rc::new(Cell::new(0));
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            let sink = native_sink.clone();
            let events = cleanup_events.clone();
            r.lifecycle.set_popup_dismissal = Rc::new(move |_, enabled| {
                if !enabled && let Some(sink) = sink.borrow().as_ref() {
                    events.set(events.get() + 1);
                    // Removing the native child hook can synchronously deliver input
                    // to its owner while Registry is still mutably borrowed.
                    sink(PopupPointerInput::LeftPressed { x: 10., y: 10. });
                }
                true
            });
        });
        for watermark_owner in [false, true] {
            let generation = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().generation);
            if watermark_owner {
                open_watermark_input(generation);
            } else {
                open_panel(0, false);
            }
            let (revision, old_input, sink) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                if watermark_owner {
                    let revision = r.watermark.revision();
                    let sink = watermark::pointer_sink(
                        r.watermark.get().unwrap().as_weak(),
                        move || {
                            REGISTRY.with(|slot| {
                                slot.borrow().as_ref().is_some_and(|r| {
                                    r.generation == generation && r.watermark.revision() == revision
                                })
                            })
                        },
                        r.watermark.input(),
                        choice::watermark_pointer,
                    );
                    (revision, r.watermark.input(), sink)
                } else {
                    (
                        r.panel.revision(),
                        r.panel.input(),
                        pointer_panel(
                            r.panel.get().unwrap().as_weak(),
                            generation,
                            r.panel.revision(),
                            r.panel.input(),
                        ),
                    )
                }
            });
            let (target_input, sink) = if canvas_target {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    if r.canvases.is_empty() {
                        r.canvases.push(CanvasLayer::new(
                            AnnotationCanvas::new().unwrap(),
                            Rect {
                                left: 0,
                                top: 0,
                                right: 200,
                                bottom: 120,
                            },
                        ));
                    }
                    let layer = &r.canvases[0];
                    (
                        layer.input(),
                        pointer_canvas(
                            layer.window.as_weak(),
                            generation,
                            0,
                            layer.revision(),
                            layer.input(),
                        ),
                    )
                })
            } else {
                (old_input.clone(), sink)
            };
            let delivered = Rc::new(Cell::new(0));
            let observed = delivered.clone();
            let sink = target_input.pointer_sink(move |event| {
                observed.set(observed.get() + 1);
                sink(event);
            });
            *native_sink.borrow_mut() = Some(sink.clone());
            let queued = Rc::new(Cell::new(0));
            let queued_action = queued.clone();
            old_input.post(move || queued_action.set(queued_action.get() + 1));
            if watermark_owner {
                choice::open_watermark(generation, revision, 0., 0., 180., 34.);
            } else {
                choice::open(
                    generation,
                    revision,
                    crate::AnnotationChoiceRequest {
                        field: 1,
                        options: ModelRc::new(VecModel::from(vec![
                            "Rectangle".into(),
                            "Ellipse".into(),
                        ])),
                        width: 180.,
                        height: 34.,
                        ..Default::default()
                    },
                );
            }
            let before = cleanup_events.get();
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.menus.current.is_some());
                if !close_owner {
                    choice::close(r, choice::CloseReason::InteractionFinished);
                } else if watermark_owner {
                    r.close_watermark_input();
                } else {
                    r.close_panel();
                }
                assert!(r.menus.current.is_none());
            });
            assert!(cleanup_events.get() > before);
            assert_eq!(delivered.get(), 0, "cleanup input must not reach the owner");
            assert_eq!(old_input.retired(), close_owner);
            *native_sink.borrow_mut() = None;
            if close_owner {
                if watermark_owner {
                    open_watermark_input(generation);
                } else {
                    open_panel(0, false);
                }
            }
            sink(PopupPointerInput::LeftPressed { x: 10., y: 10. });
            slint::platform::update_timers_and_animations();
            assert_eq!(queued.get(), usize::from(!close_owner));
            assert_eq!(delivered.get(), usize::from(!close_owner));
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                let (new_revision, new_input) = if watermark_owner {
                    (r.watermark.revision(), r.watermark.input())
                } else {
                    (r.panel.revision(), r.panel.input())
                };
                assert_eq!(new_revision == revision, !close_owner);
                assert!(!new_input.retired());
                assert!(r.menus.current.is_none());
            });
        }
        shutdown();
    }

    #[test]
    fn parameter_panels_ignore_native_dismissal_while_tool_menus_still_close() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        thread_local! { static WATCHES: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) }; }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.lifecycle.set_popup_dismissal = Rc::new(|_, enabled| {
                WATCHES.with(|events| events.borrow_mut().push(enabled));
                true
            });
        });
        for dpi in [1., 1.25] {
            for group in [0, 4] {
                open_panel(group, false);
                let (generation, revision, sink, retired_panel) = REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    let panel = r.panel.get().unwrap();
                    panel
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    (
                        r.generation,
                        r.panel.revision(),
                        pointer_panel(
                            panel.as_weak(),
                            r.generation,
                            r.panel.revision(),
                            r.panel.input(),
                        ),
                        panel.clone_strong(),
                    )
                });
                WATCHES.with(|events| events.borrow_mut().clear());
                show_panel(generation, revision, 0);
                REGISTRY.with(|slot| choice::rearm_panel(slot.borrow().as_ref().unwrap()));
                WATCHES.with(|events| {
                    assert!(!events.borrow().is_empty());
                    assert!(events.borrow().iter().all(|enabled| !enabled));
                });
                for cause in [
                    PopupDismissCause::PointerOutside,
                    PopupDismissCause::ForegroundChanged,
                ] {
                    sink(PopupPointerInput::DismissRequested { cause });
                }
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert_eq!(r.panel.revision(), revision);
                    assert!(r.panel.get().is_some_and(|p| !p.get_menu()));
                });
                // Queue parameter work before retiring the source, then keep the component
                // alive to exercise the identity fence rather than weak-handle expiry.
                let mut old_values = retired_panel.get_values();
                let original_size = old_values.size;
                old_values.size = 91;
                retired_panel.invoke_values_changed(old_values);
                // The same button explicitly toggles the panel closed.
                open_panel(group, false);
                REGISTRY
                    .with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_none()));
                open_panel(group, true);
                let (generation, revision, menu_sink) = REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    let panel = r.panel.get().unwrap();
                    (
                        r.generation,
                        r.panel.revision(),
                        pointer_panel(
                            panel.as_weak(),
                            r.generation,
                            r.panel.revision(),
                            r.panel.input(),
                        ),
                    )
                });
                show_panel(generation, revision, 0);
                WATCHES.with(|events| assert_eq!(events.borrow().last(), Some(&true)));
                // An old parameter-window event cannot dismiss its replacement menu.
                sink(PopupPointerInput::DismissRequested {
                    cause: PopupDismissCause::PointerOutside,
                });
                sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
                retired_panel.invoke_escape_requested();
                retired_panel.invoke_watermark_edit_requested();
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert_eq!(r.panel.revision(), revision);
                    assert!(r.panel.get().is_some_and(|p| p.get_menu()));
                    assert!(r.watermark.get().is_none());
                    assert!(!r.session.can_undo());
                    assert_eq!(
                        r.values(retired_panel.get_tool() as usize).size,
                        original_size
                    );
                });
                menu_sink(PopupPointerInput::DismissRequested {
                    cause: PopupDismissCause::PointerOutside,
                });
                slint::platform::update_timers_and_animations();
                REGISTRY
                    .with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_none()));
                open_panel(group, true);
                let (generation, revision, foreground_sink) = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    let panel = r.panel.get().unwrap();
                    let position = panel.window().position();
                    let sink = pointer_panel(
                        panel.as_weak(),
                        r.generation,
                        r.panel.revision(),
                        r.panel.input(),
                    );
                    r.lifecycle.toolbar_cursor_position = Rc::new(move || {
                        Some(Point {
                            x: position.x + 1,
                            y: position.y + 1,
                        })
                    });
                    (r.generation, r.panel.revision(), sink)
                });
                show_panel(generation, revision, 0);
                foreground_sink(PopupPointerInput::DismissRequested {
                    cause: PopupDismissCause::ForegroundChanged,
                });
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(
                        r.panel.get().is_none(),
                        "foreground dismissal bypasses the pointer guard for tool menus"
                    );
                    r.lifecycle.toolbar_cursor_position = Rc::new(|| None);
                });
            }
        }
        shutdown();
    }

    #[test]
    fn undo_same_object_rejects_queued_parameter_and_menu_edits() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let (generation, identity) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            assert!(r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((-100., 50.), (100., 150.)),
                points: Vec::new(),
                style: Style::default(),
                extra: Extra::None,
            }));
            adopt_selected_style(r, 0);
            (r.generation, r.session.objects[0].id)
        });
        open_panel(0, false);
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let panel = slot.as_ref().unwrap().panel.get().unwrap();
            let mut values = panel.get_values();
            values.size = 11;
            panel.set_values(values.clone());
            panel.invoke_values_changed(values);
        });
        slint::platform::update_timers_and_animations();
        let panel_revision = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.finish_style_edit();
            assert_eq!(r.session.objects[0].style.width, 11.);
            r.panel.revision()
        });
        choice::open(
            generation,
            panel_revision,
            crate::AnnotationChoiceRequest {
                field: 1,
                options: ModelRc::new(VecModel::from(vec!["Rectangle".into(), "Ellipse".into()])),
                width: 180.,
                height: 34.,
                ..Default::default()
            },
        );
        let retained_menu = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let mut stale = r.panel.get().unwrap().get_values();
            stale.size = 80;
            r.panel.get().unwrap().invoke_values_changed(stale);
            r.menus.current.as_ref().unwrap().window.clone_strong()
        });
        retained_menu.invoke_chosen(1);
        undo();
        retained_menu.invoke_chosen(1);
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.session.objects[0].id, identity);
            assert_eq!(r.session.objects[0].kind, Kind::Rectangle);
            assert_eq!(r.session.objects[0].style.width, Style::default().width);
            assert_eq!(
                r.panel.get().unwrap().get_values().size,
                Style::default().width as i32
            );
            assert!(r.menus.current.is_none());
        });
        undo();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.session.objects.is_empty());
            assert!(
                !r.session.can_undo(),
                "retired callbacks added history after undo"
            );
            assert!(r.panel.get().is_some());
        });
        shutdown();
    }

    #[test]
    fn applied_watermark_parameters_and_stale_events_do_not_change_defaults() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.selected[3] = 9;
            r.sync_edit_scope();
            r.generation
        });
        open_panel(3, false);
        let defaults = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .parameter_values
                .defaults(9)
                .clone()
        });
        open_watermark_input(generation);
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        apply_watermark(generation, revision, "甲".into());
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let panel = r.panel.get().unwrap();
            let mut values = panel.get_values();
            values.text_size = 44;
            values.color_index = 8;
            values.custom_color = "#123456".into();
            values.strength = 70.;
            values.mode = 7;
            panel.set_values(values.clone());
            panel.invoke_values_changed(values);
        });
        slint::platform::update_timers_and_animations();
        open_watermark_input(generation);
        let cancelled_revision = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            r.watermark.get().unwrap().set_value("未应用".into());
            r.watermark.revision()
        });
        close_watermark_input(generation, cancelled_revision);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.finish_style_edit();
            let watermark = r.session.watermark.as_ref().unwrap();
            assert_eq!(watermark.template, "甲");
            assert_eq!(watermark.font_size, 44.);
            assert_eq!(watermark.color, [18, 52, 86]);
            assert_eq!(watermark.opacity, 70);
            assert_eq!(r.parameter_values.defaults(9), &defaults);
            let mut stale = r.panel.get().unwrap().get_values();
            stale.text_size = 68;
            r.panel.get().unwrap().invoke_values_changed(stale);
        });
        open_watermark_input(generation);
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        apply_watermark(generation, revision, String::new());
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.session.watermark.is_none());
            assert_eq!(r.values(9), &defaults);
            assert_eq!(r.parameter_values.defaults(9), &defaults);
        });
        undo();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.session.watermark.as_ref().unwrap().font_size, 44.);
            assert_eq!(r.values(9).text_size, 44);
            assert_eq!(r.values(9).custom_color, "#123456");
            assert_eq!(r.parameter_values.defaults(9), &defaults);
        });
        shutdown();
    }

    #[test]
    fn new_text_draft_parameters_are_cancelled_or_committed_and_retained_for_subsequent_drawing() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.selected[3] = 8;
            r.sync_edit_scope();
        });
        for commit in [false, true] {
            open_panel(3, false);
            open_text_editor((-100., 80.), None, 1.25);
            slint::platform::update_timers_and_animations();
            let _defaults = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                let defaults = r.parameter_values.defaults(8).clone();
                let panel = r.panel.get().unwrap();
                let mut values = panel.get_values();
                values.text_size = 58;
                values.color_index = 8;
                values.custom_color = "#102030".into();
                values.text_background = true;
                values.text_bg_padding = 13;
                panel.set_values(values.clone());
                panel.invoke_values_changed(values);
                r.editor.window().unwrap().set_value("中文草稿".into());
                defaults
            });
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.parameter_values.defaults(8).text_size, 58);
                assert!(r.parameter_values.defaults(8).text.is_empty());
                assert_eq!(r.values(8).text_size, 58);
                assert_eq!(r.editor.edit().unwrap().original.font_size, 58.);
                assert_eq!(current_style(r, 8).color, [16, 32, 48]);
                finish_text_edit_registry(r, commit);
                r.finish_style_edit();
                assert_eq!(r.parameter_values.defaults(8).text_size, 58);
                assert!(r.parameter_values.defaults(8).text.is_empty());
                assert!(r.editor.draft_values().is_none());
                if commit {
                    let object = r.session.objects.last().unwrap();
                    assert_eq!(object.style.color, [16, 32, 48]);
                    let Extra::Text(text) = &object.extra else {
                        unreachable!()
                    };
                    assert_eq!(text.content, "中文草稿");
                    assert_eq!(text.font_size, 58.);
                    assert_eq!(text.background.as_ref().unwrap().padding, 13.);
                    assert!(r.session.undo());
                    assert!(
                        !r.session.can_undo(),
                        "draft parameters added an extra undo"
                    );
                } else {
                    assert!(!r.session.can_undo());
                    assert_eq!(r.panel.get().unwrap().get_values().text_size, 58);
                }
                assert!(r.session.objects.is_empty());
                // Closing retains accepted settings without retaining draft contents.
                r.close_panel();
                assert_eq!(r.parameter_values.defaults(8).text_size, 58);
                assert!(r.parameter_values.defaults(8).text.is_empty());
            });
        }
        shutdown();
    }

    #[test]
    fn sequence_panel_parameters_reach_the_session_before_the_panel_closes() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            assert!(r.set_mode(InteractionMode::Tool(4)));
            let mut object = crate::annotation_sequence::tests::badge(1.25);
            crate::annotation_sequence::layout(&mut object);
            assert!(r.session.add(object));
        });
        open_panel(4, false);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let panel = slot.as_ref().unwrap().panel.get().unwrap();
            let mut v = panel.get_values();
            v.shape = 0;
            v.format = 4;
            v.text_size = 28;
            v.start = 8;
            v.sequence_ratio = 4;
            panel.set_values(v.clone());
            panel.invoke_values_changed(v);
        });
        slint::platform::update_timers_and_animations();
        open_text_editor((-100., 80.), Some(0), 1.25);
        slint::platform::update_timers_and_animations();
        let (menu_generation, menu_panel, original_font) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            (r.generation, r.panel.revision(), r.values(10).font)
        });
        choice::open(
            menu_generation,
            menu_panel,
            crate::AnnotationChoiceRequest {
                field: 12,
                options: ModelRc::new(VecModel::from(vec!["First".into(), "Second".into()])),
                width: 180.,
                height: 34.,
                ..Default::default()
            },
        );
        let retired_menu = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .clone_strong()
        });
        let stale_font = if original_font == 0 { 1 } else { 0 };
        retired_menu.invoke_chosen(stale_font);
        let retired_editor = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let id = r.editor.edit().unwrap().id;
            let mut stale = r.panel.get().unwrap().get_values();
            stale.text_size = 70;
            stale.format = 1;
            // Queue through the real callback while Registry is borrowed: its
            // fallback must capture this editor, not the eventual replacement.
            r.panel.get().unwrap().invoke_values_changed(stale);
            finish_text_edit_registry(r, false);
            id
        });
        open_text_editor((-100., 80.), Some(0), 1.25);
        // Keep the retired component alive and deliver another old callback:
        // neither weak-reference expiry nor only the first event may protect us.
        retired_menu.invoke_chosen(stale_font);
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_ne!(r.editor.edit().unwrap().id, retired_editor);
            assert_eq!(r.values(10).text_size, 28);
            assert_eq!(r.values(10).format, 4);
            assert_eq!(r.values(10).font, original_font);
            assert!(r.menus.current.is_none());
            assert!(
                r.panel.get().is_some(),
                "retiring a menu closed the parameter panel"
            );
            assert_eq!(r.editor.edit().unwrap().original.font_size, 52.5);
        });
        let (generation, edit_id, panel_revision, choice_revision) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let editor = r.editor.window().unwrap();
            editor.set_value("ABC".into());
            editor.invoke_place_selection(1, 2);
            r.panel.get().unwrap().set_parameter_input_active(true);
            (
                r.generation,
                r.editor.edit().unwrap().id,
                r.panel.revision(),
                r.menus.revision,
            )
        });
        assert!(!restore_matching_text_focus(
            generation,
            edit_id,
            panel_revision,
            choice_revision
        ));
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            slot.as_ref()
                .unwrap()
                .panel
                .get()
                .unwrap()
                .set_parameter_input_active(false);
        });
        assert!(!restore_matching_text_focus(
            generation,
            edit_id,
            panel_revision,
            choice_revision.wrapping_sub(1)
        ));
        assert!(!restore_matching_text_focus(
            generation,
            edit_id.wrapping_sub(1),
            panel_revision,
            choice_revision
        ));
        assert!(!restore_matching_text_focus(
            generation,
            edit_id,
            panel_revision.wrapping_sub(1),
            choice_revision
        ));
        open_watermark_input(generation);
        assert!(REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.get().is_some()));
        assert!(
            !restore_matching_text_focus(generation, edit_id, panel_revision, choice_revision),
            "an older text restoration cannot preempt the watermark input"
        );
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().close_watermark_input());
        assert!(restore_matching_text_focus(
            generation,
            edit_id,
            panel_revision,
            choice_revision
        ));
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.editor.edit().unwrap().id, edit_id);
            assert!(r.editor.edit().unwrap().caret.visible);
            let editor = r.editor.window().unwrap();
            assert_eq!(editor.get_value(), "ABC");
            assert_eq!(
                (editor.get_anchor_offset(), editor.get_cursor_offset()),
                (1, 2)
            );
            editor
                .window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "X".into() });
            assert_eq!(editor.get_value(), "AXC");
            r.apply_menu_edit(
                r.parameter_target(10),
                parameters::Edit::Color(parameters::ColorTarget::SequenceText, [16, 32, 48]),
            );
            finish_text_edit_registry(r, false);
            assert_eq!(r.session.next_sequence, 8);
            let Extra::Sequence(s) = &r.session.objects[0].extra else {
                unreachable!()
            };
            assert_eq!(s.shape, SequenceShape::Circle);
            assert_eq!(s.format, SequenceFormat::Chinese);
            assert_eq!(
                s.caption_color,
                [16, 32, 48],
                "completing editing lost the accepted menu patch"
            );
            assert_eq!(
                lexift_core::domain::annotation::sequence_label(s.value, s.format),
                "一"
            );
            assert_eq!(s.size, 28.);
            assert_eq!(s.caption.font_size, 52.5);
            let preview = sequence_from_values(r, (500., 200.), 1.25);
            let Extra::Sequence(s) = &preview.extra else {
                unreachable!()
            };
            assert_eq!(
                lexift_core::domain::annotation::sequence_label(s.value, s.format),
                "\u{516b}"
            );
            // Accepted settings persist; the counter remains session-owned.
            assert_eq!(s.format, SequenceFormat::Chinese);
            assert_eq!(s.size, 28.);
            assert_eq!(r.parameter_values.defaults(10).format, 4);
            let retired = r.session.objects[0].id;
            assert_eq!(r.parameter_values.binding().get(), Some(retired));
            let generation = r.generation;
            drop(slot);
            delete_selected();
            let stale = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.session.objects.is_empty());
                assert_eq!(r.parameter_values.bound_object(), None);
                assert_eq!(r.panel.get().unwrap().get_values(), *r.values(10));
                let mut stale = r.panel.get().unwrap().get_values();
                stale.format = 4;
                stale.text_size = 70;
                stale
            });
            update_geometry_values(
                generation,
                parameter_edit::Target {
                    panel: panel_revision,
                    tool: 10,
                    object: Some(retired),
                    editor: None,
                    watermark: 0,
                    projection: 0,
                },
                stale,
            );
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.values(10).format, 4);
            assert_eq!(r.values(10).text_size, 28);
            let mut new_object = sequence_from_values(r, (500., 200.), 1.25);
            let Extra::Sequence(sequence) = &mut new_object.extra else {
                unreachable!()
            };
            sequence.format = SequenceFormat::Roman;
            sequence.size = 41.;
            crate::annotation_sequence::layout(&mut new_object);
            assert!(r.session.add(new_object));
            refresh_parameter_target(r);
            assert_eq!(r.panel.get().unwrap().get_values().format, 1);
            assert_eq!(r.panel.get().unwrap().get_values().text_size, 41);
            assert_eq!(r.parameter_values.defaults(10).format, 4);
            assert_eq!(r.parameter_values.defaults(10).text_size, 28);
            r.close();
            assert_eq!(r.session.next_sequence, 1);
            assert_eq!(r.values(10).sequence_label, "1");
            assert!(
                r.editor.window().is_none() && r.menus.current.is_none() && r.panel.get().is_none()
            );
            slot.take();
        });
    }

    #[test]
    fn ready_canvas_prepares_empty_native_scene_before_drawing_and_retires_it() {
        use lexift_core::ports::annotation_render::{
            AnnotationRenderPort, AnnotationScene, RenderBackend, RenderCapabilities,
            RenderCompletion,
        };
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::sync::{Arc, Mutex};
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        #[derive(Default)]
        struct Port(Mutex<Option<Arc<AnnotationScene>>>);
        impl AnnotationRenderPort for Port {
            fn capabilities(&self) -> RenderCapabilities {
                RenderCapabilities {
                    backend: RenderBackend::Gpu,
                    max_refresh_hz: 60,
                }
            }
            fn submit(&self, scene: Arc<AnnotationScene>) -> lexift_core::Result<()> {
                *self.0.lock().unwrap() = Some(scene);
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<RenderCompletion>> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|scene| RenderCompletion {
                        stamp: scene.stamp,
                        sampled_regions: Arc::from([]),
                        presented: true,
                        wait: None,
                    }))
            }
            fn clear(&self) {
                *self.0.lock().unwrap() = None;
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let port = Arc::new(Port::default());
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_canvas(Vec::new, |_, _| true, |_, _, _, _| true)
            .with_annotation_window_token(|_| Some(lexift_core::ports::magnifier::WindowToken(1)))
            .with_annotation_renderer(Some(port.clone())),
        );
        let (generation, revision) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            let mut canvas = CanvasLayer::new(
                AnnotationCanvas::new().unwrap(),
                Rect {
                    left: -200,
                    top: -100,
                    right: 200,
                    bottom: 100,
                },
            );
            canvas.bootstrap = CanvasBootstrap::Ready;
            r.canvases.push(canvas);
            (r.generation, r.main.revision())
        });
        show_main(generation, revision, 0);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.annotation_ready && r.frame_tasks.render_pending());
            assert!(r.session.objects.is_empty() && !r.session.can_undo());
            assert!(r.mosaic.is_idle());
        });
        render_canvases(generation);
        assert!(port.0.lock().unwrap().as_ref().unwrap().layers.is_empty());
        poll_native_renderer(generation);
        render_canvases(generation);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.native_presenter.active && r.canvases[0].gpu_input_cleared);
            assert!(r.set_mode(InteractionMode::Tool(0)));
        });
        canvas_pointer(0, 0, 50., 50.);
        canvas_pointer(0, 1, 100., 100.);
        render_canvases(generation);
        assert!(!port.0.lock().unwrap().as_ref().unwrap().layers.is_empty());
        canvas_pointer(0, 3, 100., 100.);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.session.objects.is_empty() && !r.session.can_undo());
            r.close();
            assert!(!r.frame_tasks.render_pending());
        });
        assert!(port.0.lock().unwrap().is_none());
        render_canvases(generation); // An old warmup cannot rebuild a closed session.
        assert!(port.0.lock().unwrap().is_none());
        shutdown();
    }

    #[test]
    fn native_completion_hands_off_canvas_and_failure_restores_software_pixels() {
        use lexift_core::ports::annotation_render::{
            AnnotationRenderPort, AnnotationScene, RenderBackend, RenderCapabilities,
            RenderCompletion,
        };
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        #[derive(Default)]
        struct Port {
            scenes: Mutex<Vec<Arc<AnnotationScene>>>,
            completion: Mutex<Option<RenderCompletion>>,
            failed: AtomicBool,
            cleared: AtomicUsize,
        }
        impl AnnotationRenderPort for Port {
            fn capabilities(&self) -> RenderCapabilities {
                RenderCapabilities {
                    backend: RenderBackend::Gpu,
                    max_refresh_hz: 60,
                }
            }
            fn submit(&self, scene: Arc<AnnotationScene>) -> lexift_core::Result<()> {
                self.scenes.lock().unwrap().push(scene);
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<RenderCompletion>> {
                if self.failed.load(Ordering::Relaxed) {
                    return Err(lexift_core::Error::new("device removed"));
                }
                Ok(self.completion.lock().unwrap().clone())
            }
            fn clear(&self) {
                self.cleared.fetch_add(1, Ordering::Relaxed);
                *self.completion.lock().unwrap() = None;
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let port = Arc::new(Port::default());
        let frames = Rc::new(RefCell::new(Vec::new()));
        let received = frames.clone();
        let reject_second = Rc::new(std::cell::Cell::new(false));
        let reject = reject_second.clone();
        let second_window = Rc::new(RefCell::new(None::<slint::Weak<AnnotationCanvas>>));
        let rejected_window = second_window.clone();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_canvas(
                Vec::new,
                |_, _| true,
                move |window, pixels, _, _| {
                    if reject.get()
                        && rejected_window
                            .borrow()
                            .as_ref()
                            .and_then(slint::Weak::upgrade)
                            .is_some_and(|target| std::ptr::eq(window, target.window()))
                    {
                        return false;
                    }
                    received.borrow_mut().push(pixels.to_vec());
                    true
                },
            )
            .with_annotation_window_token(|_| Some(lexift_core::ports::magnifier::WindowToken(1)))
            .with_annotation_renderer(Some(port.clone())),
        );
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.canvases.push(CanvasLayer::new(
                AnnotationCanvas::new().unwrap(),
                Rect {
                    left: -32,
                    top: -32,
                    right: 32,
                    bottom: 32,
                },
            ));
            let second = AnnotationCanvas::new().unwrap();
            *second_window.borrow_mut() = Some(second.as_weak());
            second
                .window()
                .set_position(slint::PhysicalPosition::new(32, -32));
            r.canvases.push(CanvasLayer::new(
                second,
                Rect {
                    left: 32,
                    top: -32,
                    right: 96,
                    bottom: 32,
                },
            ));
            r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((-20., -20.), (80., 20.)),
                points: Vec::new(),
                style: Style::default(),
                extra: Extra::None,
            });
            r.generation
        });
        render_canvases(generation);
        assert_eq!(frames.borrow().len(), 2);
        assert!(
            frames.borrow()[0]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        );
        let stamp = port.scenes.lock().unwrap().last().unwrap().stamp;
        *port.completion.lock().unwrap() = Some(RenderCompletion {
            stamp,
            sampled_regions: Arc::from([]),
            presented: true,
            wait: None,
        });
        poll_native_renderer(generation);
        render_canvases(generation);
        assert_eq!(frames.borrow().len(), 4);
        assert!(
            frames.borrow()[2..]
                .iter()
                .all(|frame| frame.iter().all(|byte| *byte == 0))
        );
        render_canvases(generation);
        assert_eq!(
            frames.borrow().len(),
            4,
            "transparent handoff is not repeated"
        );
        reject_second.set(true);
        port.failed.store(true, Ordering::Relaxed);
        poll_native_renderer(generation);
        assert_eq!(port.cleared.load(Ordering::Relaxed), 0);
        render_canvases(generation);
        assert_eq!(frames.borrow().len(), 5);
        assert_eq!(
            port.cleared.load(Ordering::Relaxed),
            0,
            "first monitor success cannot remove the second monitor's native pixels"
        );
        reject_second.set(false);
        render_canvases(generation);
        assert_eq!(port.cleared.load(Ordering::Relaxed), 1);
        assert_eq!(frames.borrow().len(), 6);
        assert_eq!(frames.borrow()[0], frames.borrow()[4]);
        assert_eq!(frames.borrow()[1], frames.borrow()[5]);
        // Re-enable the adapter to exercise the complete effects route through
        // real UI compilation. The source mock records any CPU readback request.
        struct Capture(AtomicUsize);
        impl lexift_core::ports::screen_capture::ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(
                &self,
                _: lexift_core::ports::screen_capture::ScreenCaptureRequest,
            ) -> lexift_core::Result<()> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            fn latest(
                &self,
            ) -> lexift_core::Result<
                Option<Arc<lexift_core::ports::screen_capture::ScreenCaptureFrame>>,
            > {
                Ok(None)
            }
            fn clear(&self) {}
        }
        let capture = Arc::new(Capture(AtomicUsize::new(0)));
        port.failed.store(false, Ordering::Relaxed);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.native_presenter
                .close(r.lifecycle.annotation_renderer.as_ref());
            r.lifecycle.annotation_screen_capture = Some(capture.clone());
            r.main.open(AnnotationToolbar::new().unwrap());
            r.panel.open(AnnotationPanel::new().unwrap());
            r.mosaic.stage(
                &mut r.session,
                Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::MosaicArea,
                    bounds: Bounds::from_corners((-16., -16.), (16., 16.)),
                    points: Vec::new(),
                    style: Style::default(),
                    extra: Extra::Mosaic(lexift_core::domain::annotation::MosaicAnnotation::new(
                        lexift_core::domain::annotation::MosaicEffect::Blur,
                        50,
                        12.,
                        1.,
                    )),
                },
            );
            r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Magnifier,
                bounds: Bounds::from_corners((-16., -16.), (16., 16.)),
                points: Vec::new(),
                style: Style::default(),
                extra: Extra::Magnifier {
                    output: Bounds::from_corners((-16., -16.), (16., 16.)),
                    zoom: 2.,
                    ellipse: true,
                    connector: MagnifierConnector::None,
                    erase_annotations: false,
                    antialias: true,
                    shadow: false,
                },
            });
        });
        render_canvases(generation);
        let scene = port.scenes.lock().unwrap().last().unwrap().clone();
        assert!(
            scene.paused_regions.is_empty(),
            "native mosaic edits must not freeze when toolbar sampling bounds overlap"
        );
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(&*scene.excluded_windows, capture_exclusions(r));
        });
        assert!(scene.layers.iter().any(|layer| matches!(
            layer,
            lexift_core::ports::annotation_render::RenderLayer::Mosaic(_)
        )));
        assert!(scene.layers.iter().any(|layer| matches!(
            layer,
            lexift_core::ports::annotation_render::RenderLayer::Magnifier(_)
        )));
        assert_eq!(capture.0.load(Ordering::Relaxed), 0);
        *port.completion.lock().unwrap() = Some(RenderCompletion {
            stamp: scene.stamp,
            sampled_regions: Arc::from([]),
            presented: true,
            wait: None,
        });
        poll_native_renderer(generation);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(!r.native_presenter.active && !r.mosaic.pending.is_empty());
        });
        let coverage: Vec<_> = scene
            .layers
            .iter()
            .flat_map(|layer| match layer {
                lexift_core::ports::annotation_render::RenderLayer::Mosaic(layer) => layer
                    .masks
                    .iter()
                    .map(|mask| mask.bounds)
                    .collect::<Vec<_>>(),
                lexift_core::ports::annotation_render::RenderLayer::Magnifier(layer) => {
                    layer.masks.iter().map(|mask| mask.bounds).collect()
                }
                _ => Vec::new(),
            })
            .collect();
        port.completion
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .sampled_regions = coverage.into();
        poll_native_renderer(generation);
        render_canvases(generation);
        assert_eq!(
            capture.0.load(Ordering::Relaxed),
            0,
            "native effects must not request CPU screen frames"
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.native_presenter.active && r.canvases[0].gpu_input_cleared);
            assert!(r.mosaic.pending.is_empty());
            r.session.erase(
                lexift_core::domain::annotation_erase::EraseRegion::Rectangle(
                    Bounds::from_corners((10., 10.), (20., 20.)),
                ),
                &[lexift_core::domain::annotation_erase::EraseTarget::Object(
                    r.session.objects[0].id,
                )],
            );
        });
        let clears = port.cleared.load(Ordering::Relaxed);
        undo();
        assert_eq!(port.cleared.load(Ordering::Relaxed), clears + 1);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(!eraser::has_coverage(&r.session));
            assert!(!r.native_presenter.active);
            assert!(r.canvases.iter().all(|layer| !layer.gpu_input_cleared));
            r.close();
            slot.take();
        });
    }

    #[test]
    fn mosaic_refresh_submits_only_changed_canvas_pixels_and_retries_failure() {
        use crate::annotation_render::mosaic_worker::{Job, Worker};
        use lexift_core::{
            domain::annotation::{MosaicAnnotation, MosaicEffect},
            ports::screen_capture::{CapturedRegion, ScreenCaptureFrame},
        };
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::sync::Arc;
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let submissions = Rc::new(RefCell::new([0usize; 2]));
        let failed_once = Rc::new(Cell::new(false));
        let calls = submissions.clone();
        let failure = failed_once.clone();
        let window_ids = RefCell::new(Vec::new());
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_canvas(
                Vec::new,
                |_, _| true,
                move |window, _, _, _| {
                    // The software window adapter has no native desktop position.
                    let id = window as *const slint::Window as usize;
                    let mut ids = window_ids.borrow_mut();
                    let index = ids
                        .iter()
                        .position(|known| *known == id)
                        .unwrap_or_else(|| {
                            ids.push(id);
                            ids.len() - 1
                        });
                    calls.borrow_mut()[index] += 1;
                    !failure.replace(false)
                },
            ),
        );
        let monitors = [
            Rect {
                left: -64,
                top: 0,
                right: 0,
                bottom: 64,
            },
            Rect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            },
        ];
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.pending_displays = monitors.to_vec();
            r.session.displays = monitors.to_vec();
            for (index, monitor) in monitors.iter().enumerate() {
                create_canvas(r, index);
                r.canvases[index]
                    .window
                    .window()
                    .set_position(slint::PhysicalPosition::new(monitor.left, 0));
                render_initial_canvas(r, index);
            }
            r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((8., 8.), (24., 24.)),
                points: vec![],
                style: Style::default(),
                extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 50, 12., 1.)),
            });
            r.session.selected = None;
            r.generation
        });
        assert_eq!(*submissions.borrow(), [1, 1]);
        let mut worker = Worker::default();
        let mut publish = |serial, color: [u8; 4]| {
            let session = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let source = &slot.as_ref().unwrap().session;
                let mut snapshot = Session::new();
                snapshot.objects = source.objects.clone();
                snapshot.displays = source.displays.clone();
                snapshot
            });
            worker.submit(Job {
                key: serial,
                context: 1,
                session: Arc::new(session),
                provisional_index: None,
                monitors: monitors.iter().map(|m| (*m, 1.)).collect(),
                capture: Arc::new(ScreenCaptureFrame {
                    revision: 1,
                    serial,
                    regions: vec![CapturedRegion {
                        bounds: Rect {
                            left: -80,
                            top: -80,
                            right: 144,
                            bottom: 144,
                        },
                        pixels: color.repeat(224 * 224),
                    }],
                }),
            });
            let started = Instant::now();
            let frame = loop {
                if let Some(frame) = worker.latest(1).filter(|f| f.serial == serial) {
                    break frame;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "effect worker timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            };
            REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().mosaic.effects = Some(frame));
            render_canvases(generation);
        };
        publish(1, [20, 100, 180, 255]);
        let raster_signatures = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .canvases
                .iter()
                .map(|c| c.signature)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            *submissions.borrow(),
            [1, 2],
            "blank monitor must retain its initial frame"
        );
        publish(2, [20, 100, 180, 255]);
        REGISTRY.with(|slot| {
            assert_eq!(
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .canvases
                    .iter()
                    .map(|c| c.signature)
                    .collect::<Vec<_>>(),
                raster_signatures,
                "same pixels with a new serial must retain both raster signatures"
            )
        });
        assert_eq!(
            *submissions.borrow(),
            [1, 2],
            "new effect serial alone must not submit identical pixels"
        );
        failed_once.set(true);
        publish(3, [190, 30, 60, 255]);
        assert_eq!(*submissions.borrow(), [1, 3]);
        render_canvases(generation);
        assert_eq!(
            *submissions.borrow(),
            [1, 4],
            "failed presentation must retry the same render signature"
        );
        render_canvases(generation);
        assert_eq!(*submissions.borrow(), [1, 4]);
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().session.selected = Some(0));
        render_canvases(generation);
        assert_eq!(
            *submissions.borrow(),
            [1, 5],
            "selection handles must still update"
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.session.objects.clear();
            r.session.selected = None;
            r.mosaic.effects = None;
        });
        render_canvases(generation);
        assert_eq!(
            *submissions.borrow(),
            [1, 6],
            "removing an effect must clear its old pixels"
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.as_mut().unwrap().close();
            slot.take();
        });
    }

    #[test]
    fn mosaic_cancellation_and_long_slider_drag_preserve_document_and_panel() {
        use lexift_core::ports::screen_capture::{
            ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
        };
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<std::sync::Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_screen_capture(Some(std::sync::Arc::new(Capture))),
        );
        for dpi in [1., 1.25] {
            for shape in [0, 1] {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.main.open(AnnotationToolbar::new().unwrap());
                    r.pending_displays = vec![Rect {
                        left: -500,
                        top: -300,
                        right: 500,
                        bottom: 500,
                    }];
                    create_canvas(r, 0);
                    r.canvases[0]
                        .window
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    r.annotation_ready = true;
                    r.mode = InteractionMode::Tool(5);
                    r.sync_edit_scope();
                    r.selected[5] = 11;
                    r.sync_edit_scope();
                    r.edit_values(11).shape = shape;
                    r.edit_values(11).strength = 0.;
                });
                open_panel(5, false);
                for cancel in 0..4 {
                    canvas_pointer(0, 0, 80., 80.);
                    canvas_pointer(0, 1, 200., 180.);
                    match cancel {
                        0 => escape(),
                        1 => canvas_pointer(0, 3, 200., 180.),
                        2 => {
                            let sink = REGISTRY.with(|slot| {
                                pointer_canvas(
                                    slot.borrow().as_ref().unwrap().canvases[0].window.as_weak(),
                                    slot.borrow().as_ref().unwrap().generation,
                                    0,
                                    slot.borrow().as_ref().unwrap().canvases[0].revision(),
                                    slot.borrow().as_ref().unwrap().canvases[0].input(),
                                )
                            });
                            sink(PopupPointerInput::LeftCancelled {
                                x: 200. * dpi,
                                y: 180. * dpi,
                            });
                        }
                        _ => REGISTRY.with(|slot| {
                            slot.borrow_mut()
                                .as_mut()
                                .unwrap()
                                .set_mode(InteractionMode::Tool(0));
                        }),
                    }
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert!(
                            gestures::display_draft(&r.gesture, &r.pending_drawing).is_none()
                                && r.gesture().is_none()
                        );
                        assert!(r.session.objects.is_empty() && !r.session.can_undo());
                        assert!(r.mosaic.pending.is_empty());
                        r.mode = InteractionMode::Tool(5);
                        r.sync_edit_scope();
                    });
                }
                if REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().panel.get().is_none()) {
                    open_panel(5, false);
                }
                canvas_pointer(0, 0, 80., 80.);
                canvas_pointer(0, 1, 200., 180.);
                canvas_pointer(0, 2, 200., 180.);
                let panel = REGISTRY.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .unwrap()
                        .panel
                        .get()
                        .unwrap()
                        .clone_strong()
                });
                panel.invoke_parameter_dragging(true);
                slint::platform::update_timers_and_animations();
                for strength in [20., 40., 80.] {
                    let mut values = panel.get_values();
                    values.strength = strength;
                    panel.set_values(values.clone());
                    panel.invoke_values_changed(values);
                    slint::platform::update_timers_and_animations();
                    std::thread::sleep(Duration::from_millis(350));
                    slint::platform::update_timers_and_animations();
                    REGISTRY.with(|slot| {
                        assert!(slot.borrow().as_ref().unwrap().parameter_edit.active())
                    });
                }
                panel.invoke_parameter_dragging(false);
                slint::platform::update_timers_and_animations();
                if shape == 1 {
                    REGISTRY.with(|slot| {
                        let slot = slot.borrow();
                        let r = slot.as_ref().unwrap();
                        assert_eq!(r.session.selected, None);
                        assert_eq!(r.values(11).strength, 80.);
                        assert!(matches!(&r.session.objects[0].extra,
                            Extra::Mosaic(value) if value.strength == 0));
                    });
                    canvas_pointer(0, 0, 140., 130.);
                    canvas_pointer(0, 1, 250., 250.);
                    REGISTRY.with(|slot| {
                        let slot = slot.borrow();
                        let r = slot.as_ref().unwrap();
                        assert!(r.drawing_press().is_some());
                        assert!(r.geometry_drag().is_none());
                    });
                    canvas_pointer(0, 3, 250., 250.);
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        assert_eq!(r.session.objects.len(), 1);
                        assert_eq!(r.values(11).strength, 80.);
                        assert!(r.panel.get().is_some());
                        assert!(r.session.undo());
                        assert!(r.session.objects.is_empty() && !r.session.can_undo());
                        r.close();
                    });
                    continue;
                }
                let press_undo = || {
                    panel.window().dispatch_event(WindowEvent::KeyPressed {
                        text: slint::platform::Key::Control.into(),
                    });
                    panel
                        .window()
                        .dispatch_event(WindowEvent::KeyPressed { text: "z".into() });
                    panel
                        .window()
                        .dispatch_event(WindowEvent::KeyReleased { text: "z".into() });
                    panel.window().dispatch_event(WindowEvent::KeyReleased {
                        text: slint::platform::Key::Control.into(),
                    });
                    slint::platform::update_timers_and_animations();
                };
                panel.set_parameter_input_active(true);
                press_undo();
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    assert!(matches!(&slot.as_ref().unwrap().session.objects[0].extra,
                        Extra::Mosaic(value) if value.strength == 80));
                });
                panel.set_parameter_input_active(false);
                press_undo();
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(
                        r.panel.get().is_some()
                            && !r.parameter_edit.dragging()
                            && !r.parameter_edit.active()
                    );
                    let Extra::Mosaic(value) = &r.session.objects[0].extra else {
                        panic!()
                    };
                    assert_eq!(
                        value.strength, 0,
                        "one undo must restore the entire slider gesture"
                    );
                    assert_eq!(value.creation_scale, dpi);
                });
                let placed = REGISTRY
                    .with(|slot| slot.borrow().as_ref().unwrap().session.objects[0].clone());
                canvas_pointer(0, 0, 140., 130.);
                canvas_pointer(0, 1, 250., 250.);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(matches!(r.gesture(), Some(Gesture::Move)));
                    assert_ne!(r.session.objects[0].bounds, placed.bounds);
                });
                canvas_pointer(0, 3, 250., 250.);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(
                        r.session.objects[0], placed,
                        "capture loss must restore a moved object"
                    );
                    assert!(
                        r.panel.get().is_some(),
                        "canvas operations must preserve the parameter panel"
                    );
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty() && !r.session.can_undo());
                    r.close();
                });
                // A destroyed panel's queued callback cannot undo a new session.
                let replacement = Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::Rectangle,
                    bounds: Bounds::from_corners((0., 0.), (50., 50.)),
                    points: vec![],
                    style: Style::default(),
                    extra: Extra::None,
                };
                let replacement = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let session = &mut slot.as_mut().unwrap().session;
                    session.add(replacement);
                    session.objects.last().unwrap().clone()
                });
                panel.invoke_undo_requested();
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert_eq!(r.session.objects, vec![replacement]);
                    r.close();
                });
            }
        }
        close();
    }

    #[test]
    fn mosaic_panel_updates_effect_without_converting_existing_geometry() {
        use lexift_core::ports::screen_capture::{
            ScreenCaptureFrame, ScreenCapturePort, ScreenCaptureRequest,
        };
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        struct Capture;
        impl ScreenCapturePort for Capture {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<std::sync::Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_screen_capture(Some(std::sync::Arc::new(Capture))),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            assert!(r.set_mode(InteractionMode::Tool(5)));
            assert!(r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((-100., 20.), (100., 100.)),
                points: Vec::new(),
                style: Style::default(),
                extra: mosaic::settings(r.values(11), 1.25)
            }));
        });
        open_panel(5, false);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let panel = slot.as_ref().unwrap().panel.get().unwrap();
            let mut v = panel.get_values();
            v.mode = 1;
            v.shape = 1;
            v.strength = 80.;
            panel.set_values(v.clone());
            panel.invoke_values_changed(v);
        });
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.session.objects[0].kind, Kind::MosaicArea);
            let Extra::Mosaic(value) = &r.session.objects[0].extra else {
                panic!()
            };
            assert_eq!(
                value.effect,
                lexift_core::domain::annotation::MosaicEffect::Blur
            );
            assert_eq!(value.strength, 80);
            assert_eq!(value.creation_scale, 1.25);
            assert_eq!(active_kind(r), Some(Kind::MosaicBrush));
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let style = current_style(r, 11);
            let values = r.values(11).clone();
            let mut press =
                drawing::Press::begin((-150., 50.), Kind::MosaicBrush, style, 1.25, &values);
            press.update((-50., 100.), 1.);
            r.gesture = Some(Active::Drawing(press));
            let Extra::Mosaic(value) = &gestures::display_draft(&r.gesture, &r.pending_drawing)
                .unwrap()
                .extra
            else {
                panic!()
            };
            assert_eq!(value.creation_scale, 1.25);
            r.finish_style_edit();
            r.session.begin_drag();
            r.session.objects[0].move_by(40., 20.);
            r.session.finish_drag();
            adopt_selected_style(r, 0);
            assert_eq!(r.values(11).strength, 80.);
            assert_eq!(
                r.panel.get().unwrap().get_values().strength,
                80.,
                "mosaic selection must not inherit spotlight opacity"
            );
            r.gesture = None;
            deselect_object(r);
            assert_eq!(active_kind(r), Some(Kind::MosaicBrush));
            assert_eq!(r.values(11).strength, 80.);
            assert_eq!(r.values(11).mode, 1);
            r.session.selected = Some(0);
            adopt_selected_style(r, 0);
            assert_eq!(active_kind(r), Some(Kind::MosaicBrush));
            assert_eq!(r.values(11).strength, 80.);
        });
        close();
    }

    #[test]
    fn sequence_caption_and_connector_invalidate_another_monitors_frame() {
        let mut session = Session::new();
        let mut object = crate::annotation_sequence::tests::badge(1.);
        crate::annotation_sequence::layout(&mut object);
        session.add(object);
        session.selected = None;
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 300,
        };
        let before = render_signature(&session, monitor, 1., None, None);
        if let Extra::Sequence(s) = &mut session.objects[0].extra {
            s.caption.content = "跨屏更新".into();
        }
        crate::annotation_sequence::layout(&mut session.objects[0]);
        assert_ne!(before, render_signature(&session, monitor, 1., None, None));
        let before = render_signature(&session, monitor, 1., None, None);
        session.objects[0].toggle_text_arrow(1.);
        session.objects[0].edit_handle(5, (300., 150.), 1.);
        assert_ne!(before, render_signature(&session, monitor, 1., None, None));
    }

    #[test]
    fn caption_color_inherits_badge_until_explicitly_chosen() {
        let mut object = crate::annotation_sequence::tests::badge(1.);
        let mut values = defaults(10);
        let blue = Style {
            color: [89, 143, 223],
            ..Style::default()
        };
        update_sequence_style(&mut object, &values, &[], blue);
        let Extra::Sequence(s) = &object.extra else {
            unreachable!()
        };
        assert_eq!(s.caption_color, blue.color);
        assert!(!s.caption_custom_color);
        values.sequence_text_color = "#123456".into();
        update_sequence_style(&mut object, &values, &[], Style::default());
        let Extra::Sequence(s) = &object.extra else {
            unreachable!()
        };
        assert_eq!(s.caption_color, [18, 52, 86]);
        assert!(s.caption_custom_color);
        assert_eq!(s.endpoint, Endpoint::FilledArrow);
    }
    #[test]
    fn foreground_dismissal_closes_the_menu_without_activating_the_text_window() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(
            Rc<Cell<Duration>>,
            Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
        );
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
                self.1.borrow_mut().push(window.clone());
                Ok(window)
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        thread_local! { static ACTIVATIONS: Cell<usize> = const { Cell::new(0) }; }
        thread_local! { static PREPARING: Cell<bool> = const { Cell::new(false) }; }

        let clock = Rc::new(Cell::new(Duration::ZERO));
        let windows = Rc::new(RefCell::new(Vec::new()));
        slint::platform::set_platform(Box::new(Clock(clock.clone(), windows.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
        };
        let sinks = Rc::new(RefCell::new(Vec::new()));
        let received = sinks.clone();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                move |_, sink| {
                    received.borrow_mut().push(sink);
                    true
                },
                |_| {
                    ACTIVATIONS.with(|count| count.set(count.get() + 1));
                    true
                },
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.selected[3] = 8;
            r.sync_edit_scope();
        });
        open_panel(3, false);
        open_text_editor((-120., -50.), None, 1.25);
        advance();
        let (generation, panel, editor, id) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let editor = r.editor.window().unwrap().clone_strong();
            editor.set_value("ABC".into());
            editor.invoke_place_selection(1, 2);
            (
                r.generation,
                r.panel.revision(),
                editor,
                r.editor.edit().unwrap().id,
            )
        });
        advance();
        ACTIVATIONS.with(|count| count.set(0));
        let request = crate::AnnotationChoiceRequest {
            field: 1,
            options: ModelRc::new(VecModel::from(vec!["one".into(), "two".into()])),
            ..Default::default()
        };
        choice::open(generation, panel, request.clone());
        advance();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let position = r.menus.current.as_ref().unwrap().window.window().position();
            // Alt+Tab can change foreground while the physical cursor is still on the popup.
            r.lifecycle.toolbar_cursor_position = Rc::new(move || {
                Some(Point {
                    x: position.x + 8,
                    y: position.y + 8,
                })
            });
        });
        let sink = sinks.borrow().last().unwrap().clone();
        sink(PopupPointerInput::DismissRequested {
            cause: PopupDismissCause::ForegroundChanged,
        });
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            0,
            "native external dismissal must not raise the editor"
        );
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.menus.current.is_none() && r.panel.get().is_some());
            assert_eq!(r.editor.edit().unwrap().id, id);
        });
        assert_eq!(
            (editor.get_anchor_offset(), editor.get_cursor_offset()),
            (1, 2)
        );
        editor
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: "X".into() });
        assert_eq!(
            editor.get_value(),
            "AXC",
            "local focus retains the insertion selection"
        );
        choice::open(generation, panel, request);
        advance();
        REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .invoke_chosen(0)
        });
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            1,
            "explicit menu completion still restores native input"
        );
        let parameter_panel = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .panel
                .get()
                .unwrap()
                .clone_strong()
        });
        parameter_panel
            .window()
            .set_size(slint::LogicalSize::new(900., 58.));
        let save_panel_render = |name: &str| {
            let Some(directory) = std::env::var_os("LEXIFT_ANNOTATION_RENDER_DIR") else {
                return;
            };
            let size = parameter_panel.window().size();
            let (width, height) = (size.width as usize, size.height as usize);
            let adapters = windows.borrow();
            let adapter = adapters
                .iter()
                .find(|adapter| std::ptr::eq(adapter.window(), parameter_panel.window()))
                .unwrap();
            let mut pixels = vec![slint::Rgb8Pixel::default(); width * height];
            parameter_panel.window().request_redraw();
            assert!(adapter.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, width);
            }));
            let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
            ppm.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(format!("{name}.ppm")), ppm).unwrap();
        };
        let (generation, panel_revision) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            (r.generation, r.panel.revision())
        });
        choice::open(
            generation,
            panel_revision,
            crate::AnnotationChoiceRequest {
                field: -102,
                ..Default::default()
            },
        );
        advance();
        let numeric = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .clone_strong()
        });
        for event in [
            WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(226., 43.),
                button: slint::platform::PointerEventButton::Left,
            },
            WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(226., 43.),
                button: slint::platform::PointerEventButton::Left,
            },
        ] {
            numeric.window().dispatch_event(event);
        }
        advance();
        assert!(
            numeric.get_parameter_input_active(),
            "numeric popup input owns the keyboard"
        );
        let before = ACTIVATIONS.with(Cell::get);
        numeric.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Return.into(),
        });
        numeric.window().dispatch_event(WindowEvent::KeyReleased {
            text: slint::platform::Key::Return.into(),
        });
        advance();
        advance();
        assert!(!numeric.get_parameter_input_active());
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            before + 1,
            "Enter restores input exactly once"
        );
        save_panel_render("numeric-input-blur");

        let foreground = Rc::new(Cell::new(Some(lexift_core::ports::magnifier::WindowToken(
            101,
        ))));
        let observed = foreground.clone();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.lifecycle.annotation_foreground = Some(Rc::new(move || observed.get()));
            r.close_panel();
        });
        open_panel(3, false);
        let observed_panel = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .panel
                .get()
                .unwrap()
                .clone_strong()
        });
        observed_panel.invoke_parameter_input_finished(true);
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(202)));
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            2,
            "a foreground change before queued dispatch must revoke native activation"
        );
        observed_panel.invoke_parameter_input_finished(true);
        advance();
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(303)));
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            2,
            "a foreground change before the focus timer must revoke native activation"
        );
        observed_panel.invoke_parameter_input_finished(true);
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            3,
            "an unchanged foreground still permits the user's explicit completion"
        );
        let current_panel = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().panel.revision());
        let menu_request = crate::AnnotationChoiceRequest {
            field: 1,
            options: ModelRc::new(VecModel::from(vec!["one".into(), "two".into()])),
            ..Default::default()
        };
        choice::open(generation, current_panel, menu_request.clone());
        advance();
        REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .invoke_chosen(0)
        });
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(404)));
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            3,
            "menu callbacks retain the observation made before queued selection"
        );
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .annotation_window_token =
                Rc::new(|_| Some(lexift_core::ports::magnifier::WindowToken(505)))
        });
        choice::open(generation, current_panel, menu_request);
        advance();
        REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .invoke_chosen(0)
        });
        advance();
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(505)));
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            4,
            "retiring a popup may return to an exact annotation owner"
        );
        let toggle_request = crate::AnnotationChoiceRequest {
            field: 1,
            options: ModelRc::new(VecModel::from(vec!["one".into(), "two".into()])),
            ..Default::default()
        };
        choice::open(generation, current_panel, toggle_request.clone());
        advance();
        observed_panel.invoke_choice_requested(toggle_request.clone());
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(606)));
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            4,
            "queued same-button close must retain the triggering foreground"
        );
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().menus.current.is_none()));
        choice::open(generation, current_panel, toggle_request);
        advance();
        observed_panel.invoke_escape_requested();
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(707)));
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            4,
            "queued Escape closes one menu without reauthorizing activation"
        );
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(
                r.menus.current.is_none() && r.editor.window().is_some() && r.panel.get().is_some()
            );
            assert_eq!(r.editor.edit().unwrap().id, id);
        });

        let queries = Rc::new(Cell::new(0));
        let recorded = queries.clone();
        let observed = foreground.clone();
        let panel_input = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.lifecycle.annotation_foreground = Some(Rc::new(move || {
                recorded.set(recorded.get() + 1);
                observed.get()
            }));
            r.panel.input()
        });
        let panel_sink = pointer_panel(
            observed_panel.as_weak(),
            generation,
            current_panel,
            panel_input,
        );
        for _ in 0..100 {
            panel_sink(PopupPointerInput::Moved { x: 8., y: 8. });
        }
        assert_eq!(
            queries.get(),
            0,
            "toolbar hover must not query foreground identity"
        );

        finish_text_edit(generation, false);
        REGISTRY.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().prepare = |_| {
                if PREPARING.with(Cell::get) {
                    PassiveWindowPreparation::Pending
                } else {
                    PassiveWindowPreparation::Ready
                }
            }
        });
        PREPARING.with(|pending| pending.set(true));
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(808)));
        open_text_editor((-120., -50.), None, 1.25);
        advance();
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(909)));
        PREPARING.with(|pending| pending.set(false));
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            4,
            "text preparation retries retain the original opening request"
        );
        finish_text_edit(generation, false);

        PREPARING.with(|pending| pending.set(true));
        observed_panel.invoke_watermark_edit_requested();
        foreground.set(Some(lexift_core::ports::magnifier::WindowToken(1010)));
        advance();
        advance();
        PREPARING.with(|pending| pending.set(false));
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            4,
            "watermark opening retains its source across input dispatch and native preparation"
        );
        let watermark = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        close_watermark_input(generation, watermark);
        observed_panel.invoke_watermark_edit_requested();
        advance();
        advance();
        assert_eq!(
            ACTIVATIONS.with(Cell::get),
            5,
            "an unchanged explicit opening still activates the watermark input"
        );

        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.as_mut().unwrap().close();
            slot.take();
        });
    }

    #[test]
    fn watermark_menu_returns_to_its_owner_and_retirement_cancels_restoration() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
        };
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.generation
        });
        open_watermark_input(generation);
        let returns = Rc::new(Cell::new(0));
        let (revision, retained) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let editor = r.watermark.get().unwrap().clone_strong();
            let count = returns.clone();
            editor.on_focus_input(move || count.set(count.get() + 1));
            (r.watermark.revision(), editor)
        });
        choice::open_watermark(generation, revision, 60., 217., 140., 34.);
        REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .invoke_chosen(0);
        });
        advance();
        advance();
        assert_eq!(
            returns.get(),
            0,
            "closing the popup only schedules a return"
        );
        assert!(retained.get_value().contains("$yyyy-MM-dd HH:mm:ss$"));
        let request = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            focus::Request {
                session: generation,
                target: focus::Target::Watermark { editor: revision },
                menu: r.menus.revision,
                activation: focus::Activation::Local,
            }
        });
        // A watermark return belongs to its input window, not a replaced parameter toolbar.
        let panel = AnnotationPanel::new().unwrap();
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .panel
                .open(panel.clone_strong())
        });
        advance();
        assert_eq!(returns.get(), 1);
        assert!(!restore_matching_input_focus(focus::Request {
            session: generation.wrapping_add(1),
            ..request
        }));
        assert!(!restore_matching_input_focus(focus::Request {
            target: focus::Target::Text {
                editor: revision,
                panel: 0
            },
            ..request
        }));
        choice::open_watermark(generation, revision, 60., 217., 140., 34.);
        assert!(
            !restore_matching_input_focus(request),
            "a new popup owns input"
        );
        choice::open_watermark(generation, revision, 60., 217., 140., 34.);
        assert_eq!(returns.get(), 1);
        panel.set_parameter_input_active(true);
        advance();
        assert_eq!(returns.get(), 1, "the parameter input keeps the keyboard");
        panel.set_parameter_input_active(false);
        REGISTRY.with(|slot| restore_editor_focus(slot.borrow().as_ref().unwrap()));
        advance();
        assert_eq!(
            returns.get(),
            2,
            "finishing the control returns to the active watermark input"
        );
        choice::open_watermark(generation, revision, 60., 217., 140., 34.);
        choice::open_watermark(generation, revision, 60., 217., 140., 34.);
        // Destroy/reopen before the next frame, retaining the old component intentionally.
        open_watermark_input(generation);
        let new_returns = Rc::new(Cell::new(0));
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_ne!(r.watermark.revision(), revision);
            let count = new_returns.clone();
            r.watermark
                .get()
                .unwrap()
                .on_focus_input(move || count.set(count.get() + 1));
        });
        advance();
        assert_eq!(returns.get(), 2);
        assert_eq!(new_returns.get(), 0);
        assert!(!restore_matching_input_focus(request));
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.as_mut().unwrap().close();
            slot.take();
        });
        advance();
        assert_eq!(returns.get(), 2);
        assert_eq!(new_returns.get(), 0);
    }

    #[test]
    fn watermark_open_on_a_work_area_smaller_than_the_input_does_not_panic() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.lifecycle.popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -100,
                    top: -50,
                    right: 200,
                    bottom: 200,
                })
            });
            r.generation
        });
        open_watermark_input(generation);
        let old_revision =
            REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        show_watermark_input(generation, old_revision, 0, focus::Origin::Unobserved);
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().watermark.get().is_some()));
        open_watermark_input(generation);
        let queries = Rc::new(Cell::new(0));
        let counted = queries.clone();
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(move |_| {
                counted.set(counted.get() + 1);
                Some(Rect {
                    left: -100,
                    top: -50,
                    right: 200,
                    bottom: 200,
                })
            });
        });
        settle_watermark_position(generation, old_revision, 2);
        assert_eq!(
            queries.get(),
            0,
            "old DPI settlement must not touch its replacement"
        );
        let current_revision =
            REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        settle_watermark_position(generation, current_revision, 1);
        assert_eq!(queries.get(), 1);
        shutdown();
    }

    #[test]
    fn watermark_pointer_bridge_reaches_controls_and_stale_instances_are_ignored() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        for dpi in [1., 1.25] {
            let editor = AnnotationWatermarkInput::new().unwrap();
            editor
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
            editor
                .window()
                .set_size(slint::LogicalSize::new(440., 270.));
            editor.show().unwrap();
            let applied = Rc::new(RefCell::new(None));
            let applied_sink = applied.clone();
            editor.on_apply(move |value| *applied_sink.borrow_mut() = Some(value.to_string()));
            let cancelled = Rc::new(Cell::new(false));
            let cancel_sink = cancelled.clone();
            editor.on_cancel(move || cancel_sink.set(true));
            let sink = watermark::pointer_sink(
                editor.as_weak(),
                || true,
                window::InputWork::new(),
                choice::watermark_pointer,
            );
            let click = |x: f32, y: f32| {
                sink(PopupPointerInput::Moved {
                    x: x * dpi,
                    y: y * dpi,
                });
                sink(PopupPointerInput::LeftPressed {
                    x: x * dpi,
                    y: y * dpi,
                });
                sink(PopupPointerInput::LeftReleased {
                    x: x * dpi,
                    y: y * dpi,
                });
            };
            click(30., 55.);
            editor.window().dispatch_event(WindowEvent::KeyPressed {
                text: "中文".into(),
            });
            editor.window().dispatch_event(WindowEvent::KeyReleased {
                text: "中文".into(),
            });
            assert_eq!(editor.get_value(), "中文");
            let requested = Rc::new(Cell::new(false));
            let requested_sink = requested.clone();
            editor.on_presets_requested(move |x, y, width, height| {
                assert_eq!((x, y, width, height), (60., 217., 140., 34.));
                requested_sink.set(true);
            });
            click(130., 234.);
            assert!(requested.get());
            assert_eq!(editor.get_selected_preset(), 1);
            editor.invoke_choose_preset(1);
            assert!(editor.get_value().contains("$yyyy/MM/dd HH:mm:ss$"));
            click(300., 234.);
            assert_eq!(
                applied.borrow().as_deref(),
                Some(editor.get_value().as_str())
            );
            click(385., 234.);
            assert!(cancelled.get());
            drop(editor);
            sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
        }
        let sinks = Rc::new(RefCell::new(Vec::<crate::bridge::PopupPointerSink>::new()));
        let received = sinks.clone();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                move |_, sink| {
                    received.borrow_mut().push(sink);
                    true
                },
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            r.generation
        });
        open_watermark_input(generation);
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        let old_revision =
            REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        show_watermark_input(generation, old_revision, 0, focus::Origin::Unobserved);
        let old_sink = sinks.borrow()[0].clone();
        // Retain the old component deliberately: identity protection must not rely on weak failure.
        let retained_old = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .watermark
                .get()
                .unwrap()
                .clone_strong()
        });
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.watermark.get().unwrap().get_presets_open());
            let menu = &r.menus.current.as_ref().unwrap().window;
            assert_eq!(menu.get_selected(), 1);
            assert_eq!(menu.get_menu_height(), 404.);
        });
        // Clicking the trigger toggles the independent menu, not the input window.
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            assert!(slot.as_ref().unwrap().menus.current.is_none());
            assert!(slot.as_ref().unwrap().watermark.get().is_some());
        });
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        let retired_menu = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .clone_strong()
        });
        // Queue a choice before replacement, then deliver more callbacks while
        // the retired window remains alive. Ownership must reject all of them.
        retired_menu.invoke_chosen(0);
        open_watermark_input(generation);
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        choice::dismiss_watermark(generation, old_revision);
        show_watermark_input(generation, old_revision, 0, focus::Origin::Unobserved);
        let current_revision =
            REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark.revision());
        choice::open_watermark(generation, current_revision, 60., 217., 140., 34.);
        REGISTRY.with(|slot| {
            let _replacement = slot.borrow_mut();
            old_sink(PopupPointerInput::LeftPressed { x: 0., y: 0. });
        });
        retired_menu.invoke_chosen(0);
        retired_menu.invoke_cancel();
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.menus.current.is_some());
            let editor = r.watermark.get().unwrap();
            assert!(editor.get_presets_open());
            assert_eq!(editor.get_selected_preset(), 1);
            assert_eq!(editor.get_value(), "");
        });
        drop(retired_menu);
        let current_menu = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .clone_strong()
        });
        current_menu.invoke_chosen(0);
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.watermark.get().unwrap().get_selected_preset(), 0);
            assert!(
                r.watermark
                    .get()
                    .unwrap()
                    .get_value()
                    .contains("$yyyy-MM-dd HH:mm:ss$")
            );
        });
        std::thread::sleep(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.menus.current.is_none());
            let editor = r.watermark.get().unwrap();
            assert!(!editor.get_presets_open());
            let before = editor.get_value().to_string();
            editor.window().dispatch_event(WindowEvent::KeyPressed {
                text: "继续".into(),
            });
            editor.window().dispatch_event(WindowEvent::KeyReleased {
                text: "继续".into(),
            });
            assert!(editor.get_value().contains("继续"));
            assert!(editor.get_value().contains(&before));
        });
        drop(current_menu);
        drop(retained_old);
        apply_watermark(generation, old_revision, "stale".into());
        close_watermark_input(generation, old_revision);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.watermark.get().is_some());
            assert!(r.menus.current.is_none());
            assert!(r.session.watermark.is_none());
            assert!(!r.session.can_undo());
            r.close();
            assert!(r.watermark.get().is_none());
            slot.take();
        });
    }

    #[test]
    fn pending_mosaic_cancel_preserves_unrelated_parameter_undo() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for operation in 0..4 {
                let (baseline, opacity) = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.session = Session::new();
                    r.mosaic = mosaic::State::default();
                    r.parameter_values = parameters::State::new();
                    r.mode = InteractionMode::Tool(0);
                    r.sync_edit_scope();
                    r.selected[0] = 2;
                    r.sync_edit_scope();
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: ObjectId::DRAFT,
                        kind: Kind::Rectangle,
                        bounds: Bounds::from_corners((-120., 30.), (-40., 90.)),
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::None,
                    }));
                    let baseline = r.session.objects[0].clone();
                    let opacity = r.session.spotlight_opacity;
                    r.selected[0] = 2;
                    assert!(r.set_mode(InteractionMode::Tool(0)));
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(2);
                    r.panel.open(panel);
                    let target = r.parameter_target(2);
                    let mut values = r.values(2).clone();
                    values.strength = 28.;
                    assert!(apply_geometry_values(r, target, values).is_some());
                    r.finish_style_edit();
                    assert!(r.set_mode(InteractionMode::Tool(5)));
                    r.mosaic.stage(
                        &mut r.session,
                        Object {
                            erased: Vec::new(),
                            id: ObjectId::DRAFT,
                            kind: Kind::MosaicArea,
                            extra: Extra::Mosaic(
                                lexift_core::domain::annotation::MosaicAnnotation::new(
                                    lexift_core::domain::annotation::MosaicEffect::Blur,
                                    50,
                                    12.,
                                    dpi,
                                ),
                            ),
                            ..baseline.clone()
                        },
                    );
                    assert_eq!(r.mosaic.pending.len(), 1);
                    if operation == 0 {
                        r.session.selected = None;
                    } else {
                        let mask = r.session.objects[1].clone();
                        r.session.begin_drag();
                        r.gesture =
                            (gestures::GeometryDrag::moving(&r.session, mask.id, (-80., 60.)))
                                .map(Active::Geometry);
                        r.session.objects[1].move_by(20., 10.);
                    }

                    (baseline, opacity)
                });
                if operation == 1 {
                    REGISTRY
                        .with(|slot| mosaic::cancel_pending(slot.borrow_mut().as_mut().unwrap()));
                } else if operation == 3 {
                    REGISTRY.with(|slot| {
                        let mut slot = slot.borrow_mut();
                        let r = slot.as_mut().unwrap();
                        r.mode = InteractionMode::Tool(5);
                        r.sync_edit_scope();
                        assert!(r.set_mode(InteractionMode::Tool(0)));
                    });
                } else {
                    escape();
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.mosaic.pending.is_empty());
                    assert!(r.gesture().is_none() && r.text_handle().is_none());
                    assert!(r.geometry_drag().is_none());
                    assert!(r.deferred_parameters.is_empty());
                    assert!(!r.parameter_edit.active());
                    assert_eq!(r.session.objects, vec![baseline.clone()]);
                    assert!((r.session.spotlight_opacity - 0.28).abs() < 0.0001);
                    assert!(
                        r.session.undo(),
                        "unrelated parameter history survives mask rejection"
                    );
                    assert_eq!(r.session.spotlight_opacity, opacity);
                    assert_eq!(r.session.objects, vec![baseline]);
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty());
                    assert!(
                        !r.session.can_undo(),
                        "rejected masks and their movement leave no history"
                    );
                });
            }
        }
        shutdown();
    }

    #[test]
    fn text_handle_activation_preserves_parameters_and_separates_undo() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for cancel in [false, true] {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.mode = InteractionMode::Tool(3);
                    r.sync_edit_scope();
                    r.selected[3] = 8;
                    r.sync_edit_scope();
                    let text = text_from_values(r, "文字".into());
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: ObjectId::DRAFT,
                        kind: Kind::Text,
                        bounds: crate::annotation_text::text_bounds((-120., 60.), &text, dpi),
                        style: current_style(r, 8),
                        points: Vec::new(),
                        extra: Extra::Text(text),
                    }));
                    let baseline = r.session.objects[0].clone();
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(8);
                    r.panel.open(panel);
                    restore_parameter_projection(r);
                    let start = baseline.edit_handles(dpi)[3];
                    begin_text_handle(r, baseline.clone(), 3, start, dpi, false);
                    let mut values = r.values(8).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, [44, 55, 66])
                        .apply(&mut values);
                    let target = r.parameter_target(8);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    let colored = r.session.objects[0].clone();
                    update_text_handle(r, (start.0 + dpi, start.1));
                    assert!(r.parameter_edit.active());
                    assert!(!r.gesture().unwrap().owns_document_transaction());
                    update_text_handle(r, (start.0 + 20. * dpi, start.1 + 20. * dpi));
                    assert!(!r.parameter_edit.active());
                    assert!(r.gesture().unwrap().owns_document_transaction());
                    assert_eq!(r.session.objects[0].style.color, [44, 55, 66]);
                    let end = (start.0 + 40. * dpi, start.1 + 40. * dpi);
                    update_text_handle(r, end);
                    let mut values = r.values(8).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, [77, 88, 99])
                        .apply(&mut values);
                    assert!(apply_geometry_values(r, target, values).is_none());
                    if cancel {
                        assert!(cancel_active_gesture(r));
                    } else {
                        finish_text_handle(r, end);
                    }
                    assert_eq!(r.session.objects[0].style.color, [77, 88, 99]);
                    r.finish_style_edit();
                    assert!(r.session.undo());
                    assert_eq!(r.session.objects[0].style.color, [44, 55, 66]);
                    if !cancel {
                        assert_ne!(r.session.objects[0].bounds, colored.bounds);
                        assert!(r.session.undo());
                    }
                    assert_eq!(r.session.objects, vec![colored]);
                    assert!(r.session.undo());
                    assert_eq!(r.session.objects, vec![baseline]);
                    assert!(r.session.undo());
                    assert!(r.session.objects.is_empty());
                    assert!(!r.session.can_undo());
                });
            }
        }
        shutdown();
    }

    #[test]
    fn draft_text_handle_preserves_parameters_across_activation_and_cancel() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for cancel in [false, true] {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.close_panel();
                    r.editor.clear();
                    r.session = Session::new();
                    r.parameter_values = parameters::State::new();
                    r.mode = InteractionMode::Tool(3);
                    r.sync_edit_scope();
                    r.selected[3] = 8;
                    r.sync_edit_scope();
                    let original = text_from_values(r, "说明文字".into());
                    let at = (-120., 60.);
                    let bounds = crate::annotation_text::text_bounds(at, &original, dpi);
                    let window = AnnotationTextInput::new().unwrap();
                    window.set_value(original.content.clone().into());
                    window.set_font_name(original.font_family.clone().into());
                    window.set_text_size(original.font_size);
                    let edit = TextEdit::new(
                        editor::next_instance(),
                        None,
                        at,
                        dpi,
                        original.clone(),
                        bounds,
                        original.content.clone(),
                    );
                    r.editor
                        .open(window, edit, Some(r.parameter_values.defaults(8).clone()));
                    let panel = AnnotationPanel::new().unwrap();
                    panel.set_tool(8);
                    r.panel.open(panel);
                    // An inactive control press owns no geometry to roll back.
                    let before = editing_text_object(r).unwrap();
                    let press = before.edit_handles(dpi)[3];
                    begin_text_handle(r, before, 3, press, dpi, true);
                    let target = r.parameter_target(8);
                    let mut values = r.values(8).clone();
                    values.text_size += 11;
                    assert!(apply_geometry_values(r, target, values).is_some());
                    let accepted = editing_text_object(r).unwrap();
                    assert!(!r.text_handle().unwrap().info().active);
                    assert!(cancel_active_gesture(r));
                    assert_eq!(
                        editing_text_object(r).unwrap(),
                        accepted,
                        "cancelling an inactive handle must retain accepted typography"
                    );
                    assert!(r.text_handle().is_none() && r.gesture().is_none());
                    let baseline = editing_text_object(r).unwrap();
                    let start = baseline.edit_handles(dpi)[3];
                    begin_text_handle(r, baseline, 3, start, dpi, true);
                    let target = r.parameter_target(8);
                    let mut values = r.values(8).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, [44, 55, 66])
                        .apply(&mut values);
                    assert!(apply_geometry_values(r, target, values).is_some());
                    let colored = editing_text_object(r).unwrap();
                    update_text_handle(r, (start.0 + 20. * dpi, start.1 + 20. * dpi));
                    assert!(!r.parameter_edit.active());
                    assert_eq!(editing_text_object(r).unwrap().style.color, [44, 55, 66]);
                    assert!(r.gesture().unwrap().blocks_parameter_edits());
                    assert!(!r.gesture().unwrap().owns_document_transaction());
                    let mut values = r.values(8).clone();
                    parameters::Edit::Color(parameters::ColorTarget::Body, [77, 88, 99])
                        .apply(&mut values);
                    assert!(apply_geometry_values(r, target, values).is_none());
                    let end = (start.0 + 40. * dpi, start.1 + 40. * dpi);
                    update_text_handle(r, end);
                    assert_eq!(editing_text_object(r).unwrap().style.color, [44, 55, 66]);
                    if cancel {
                        assert!(cancel_active_gesture(r));
                    } else {
                        finish_text_handle(r, end);
                    }
                    let current = editing_text_object(r).unwrap();
                    assert_eq!(current.style.color, [77, 88, 99]);
                    if cancel {
                        assert_eq!(current.bounds, colored.bounds);
                    } else {
                        assert_ne!(current.bounds, colored.bounds);
                    }
                    assert_eq!(r.editor.window().unwrap().get_value().as_str(), "说明文字");
                    assert!(r.deferred_parameters.is_empty());
                    r.finish_style_edit();
                    assert!(r.session.objects.is_empty());
                    assert!(
                        !r.session.can_undo(),
                        "draft geometry and parameters never create document history"
                    );
                });
            }
        }
        shutdown();
    }

    #[test]
    fn text_resize_ignores_clicks_and_keeps_the_grab_offset() {
        for dpi in [1., 1.25] {
            let start = (-120., 60.);
            let grab = (6. * dpi, -4. * dpi);
            assert!(super::gestures::text_drag_target(start, grab, start, dpi, false).is_none());
            assert!(
                super::gestures::text_drag_target(
                    start,
                    grab,
                    (start.0 + 3. * dpi, start.1),
                    dpi,
                    false
                )
                .is_none()
            );
            let target = super::gestures::text_drag_target(
                start,
                grab,
                (start.0 + 20., start.1 + 30.),
                dpi,
                false,
            )
            .unwrap();
            assert_eq!(target, (start.0 + 20. - grab.0, start.1 + 30. - grab.1));
            // Returning to the press position after crossing the threshold restores geometry.
            assert_eq!(
                super::gestures::text_drag_target(start, grab, start, dpi, true),
                Some((start.0 - grab.0, start.1 - grab.1))
            );
        }
    }
    #[test]
    fn color_edits_preserve_fractional_and_scaled_text_dimensions() {
        let original = TextAnnotation {
            content: "文字".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 777.75,
            bold: false,
            italic: false,
            outline: Some(TextOutline {
                color: [0, 0, 0],
                width: 180.5,
            }),
            background: Some(TextBackground {
                color: [255, 255, 255],
                opacity: 200,
                rounding: 400.5,
                padding: 240.5,
            }),
            rotation: 0.,
            layout_scale: 50.,
            linked_arrow: None,
            linked_arrow_control: None,
        };
        let before = defaults(8);
        let mut after = before.clone();
        after.color_index = 1;
        let mut updated = original.clone();
        updated.font_size = 778.;
        updated.outline.as_mut().unwrap().width = 20.;
        updated.background.as_mut().unwrap().padding = 0.;
        updated.background.as_mut().unwrap().rounding = 0.;
        preserve_text_dimensions(&mut updated, &original, &before, &after);
        assert_eq!(updated, original);
    }

    #[test]
    fn editor_viewport_clips_large_logical_bounds_to_negative_desktop() {
        let screens = [
            Rect {
                left: -1920,
                top: -100,
                right: 0,
                bottom: 980,
            },
            Rect {
                left: 0,
                top: 0,
                right: 2560,
                bottom: 1440,
            },
        ];
        let huge = Bounds::from_corners((-3000., -500.), (50_000., 20_000.));
        let viewport = text_editor_viewport(huge, &screens);
        assert_eq!(
            viewport,
            Bounds::from_corners((-1920., -100.), (2560., 1440.))
        );
        let small = Bounds::from_corners((-100., 20.), (50., 60.));
        assert_eq!(text_editor_viewport(small, &screens), small);
    }

    use super::*;

    #[test]
    fn document_commands_retire_object_gestures_without_extra_undo_records() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for resize in [false, true] {
            for command in 0..4 {
                let original = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.session.add(Object {
                        erased: Vec::new(),
                        id: ObjectId::DRAFT,
                        kind: Kind::Rectangle,
                        bounds: Bounds::from_corners((10., 20.), (90., 80.)),
                        style: Style::default(),
                        points: Vec::new(),
                        extra: Extra::None,
                    }));
                    let original = r.session.objects[0].clone();
                    let target = original.id;
                    r.gesture = (if resize {
                        gestures::GeometryDrag::resizing(&r.session, target, 7)
                    } else {
                        gestures::GeometryDrag::moving(&r.session, target, (40., 40.))
                    })
                    .map(Active::Geometry);
                    r.session.begin_drag();
                    let mut changed = original.clone();
                    if resize {
                        changed.edit_handle(7, (150., 120.), 1.);
                    } else {
                        changed.move_by(70., 30.);
                    }
                    assert!(gestures::write(&mut r.session, target, changed));
                    original
                });
                match command {
                    0 => undo(),
                    1 => delete_selected(),
                    2 => REGISTRY.with(|slot| {
                        assert!(
                            slot.borrow_mut()
                                .as_mut()
                                .unwrap()
                                .set_mode(InteractionMode::Tool(1))
                        );
                    }),
                    _ => escape(),
                }
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.gesture().is_none() && r.text_handle().is_none());
                    assert!(r.geometry_drag().is_none());
                    if command == 1 {
                        assert!(r.session.objects.is_empty());
                        assert!(r.session.undo());
                    }
                    assert_eq!(r.session.objects, vec![original]);
                    assert!(r.session.undo());
                    assert!(
                        r.session.objects.is_empty(),
                        "cancellation must not create an undo record"
                    );
                    r.close();
                });
            }
        }
        shutdown();
    }

    #[test]
    fn text_editor_failure_invalidates_preview_and_retires_input() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        thread_local! {
            static PREPARATION: Cell<u8> = const { Cell::new(0) };
        }
        struct TestPlatform(Rc<Cell<Duration>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestPlatform(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        init(
            |_| match PREPARATION.with(Cell::get) {
                0 => PassiveWindowPreparation::Failed,
                1 => PassiveWindowPreparation::Pending,
                _ => PassiveWindowPreparation::Ready,
            },
            WindowLifecycleCallbacks::new(
                |_, _| false,
                |_| panic!("failed editor must not activate"),
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.selected[3] = 8;
            assert!(r.set_mode(InteractionMode::Tool(3)));
        });
        for failure in 0..3 {
            PREPARATION.with(|preparation| preparation.set(failure));
            open_text_editor((-120., -50.), None, 1.25);
            let (generation, id, held) = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                let held = r.editor.window().unwrap().clone_strong();
                held.set_value("待显示草稿".into());
                assert!(editing_text_object(r).is_some());
                // Model a preview that was already presented before the delayed native result.
                r.frame_tasks.cancel_render();
                (r.generation, r.editor.edit().unwrap().id, held)
            });
            show_text_editor(generation, id, 20, focus::Origin::Unobserved);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.editor.window().is_none());
                assert!(editing_text_object(r).is_none());
                assert!(
                    r.frame_tasks.render_pending(),
                    "retiring an editor must repaint a previously displayed preview"
                );
                assert!(r.session.objects.is_empty());
                assert!(!r.session.undo());
                r.frame_tasks.cancel_render();
            });
            held.invoke_content_changed("迟到输入".into());
            held.invoke_done("迟到完成".into());
            show_text_editor(generation, id, 21, focus::Origin::Unobserved);
            clock.set(clock.get() + Duration::from_millis(600));
            slint::platform::update_timers_and_animations();
            REGISTRY.with(|slot| {
                let r = slot.borrow();
                let r = r.as_ref().unwrap();
                assert!(r.editor.window().is_none());
                assert!(!r.frame_tasks.render_pending());
                assert!(r.session.objects.is_empty());
            });
        }
        shutdown();
    }

    #[test]
    fn text_editor_owner_is_attached_after_ready_and_old_retries_are_inert() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        thread_local! {
            static READY: Cell<bool> = const { Cell::new(false) };
            static EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
        }
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let accept_owner = Rc::new(Cell::new(true));
        let accepted = accept_owner.clone();
        init(
            |_| {
                EVENTS.with(|events| events.borrow_mut().push("prepare"));
                if READY.with(Cell::get) {
                    PassiveWindowPreparation::Ready
                } else {
                    PassiveWindowPreparation::Pending
                }
            },
            WindowLifecycleCallbacks::new(
                |_, _| {
                    EVENTS.with(|events| events.borrow_mut().push("bridge"));
                    true
                },
                |_| {
                    EVENTS.with(|events| events.borrow_mut().push("activate"));
                    true
                },
                |_| false,
                |_, _, _| false,
                |_, _| true,
                move |_, _| {
                    EVENTS.with(|events| events.borrow_mut().push("owner"));
                    READY.with(Cell::get) && accepted.get()
                },
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .canvases
                .push(CanvasLayer::new(
                    AnnotationCanvas::new().unwrap(),
                    Rect {
                        left: -200,
                        top: -100,
                        right: 400,
                        bottom: 400,
                    },
                ));
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.selected[3] = 8;
            assert!(r.set_mode(InteractionMode::Tool(3)));
        });
        for scale in [1., 1.25] {
            for at in [(-120., -50.), (-220., -50.)] {
                READY.with(|ready| ready.set(false));
                EVENTS.with(|events| events.borrow_mut().clear());
                open_text_editor(at, None, scale);
                let (generation, old_id) = REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    (r.generation, r.editor.edit().unwrap().id)
                });
                assert!(
                    EVENTS.with(|events| events.borrow().is_empty()),
                    "creating a component must not attach an unprepared native window"
                );
                show_text_editor(generation, old_id, 0, focus::Origin::Unobserved);
                EVENTS.with(|events| assert_eq!(*events.borrow(), ["prepare"]));
                READY.with(|ready| ready.set(true));
                EVENTS.with(|events| events.borrow_mut().clear());
                show_text_editor(generation, old_id, 1, focus::Origin::Unobserved);
                EVENTS.with(|events| {
                    assert_eq!(
                        *events.borrow(),
                        ["prepare", "owner", "bridge", "owner", "activate"]
                    )
                });
                finish_text_edit(generation, false);
                open_text_editor(at, None, scale);
                let new_id =
                    REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().editor.edit().unwrap().id);
                assert_ne!(old_id, new_id);
                EVENTS.with(|events| events.borrow_mut().clear());
                show_text_editor(generation, old_id, 2, focus::Origin::Unobserved);
                show_text_editor(
                    generation.wrapping_add(1),
                    new_id,
                    0,
                    focus::Origin::Unobserved,
                );
                assert!(EVENTS.with(|events| events.borrow().is_empty()));
                accept_owner.set(false);
                show_text_editor(generation, new_id, 0, focus::Origin::Unobserved);
                EVENTS.with(|events| assert_eq!(*events.borrow(), ["prepare", "owner"]));
                REGISTRY.with(|slot| {
                    assert!(slot.borrow().as_ref().unwrap().editor.window().is_some())
                });
                show_text_editor(generation, new_id, 20, focus::Origin::Unobserved);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    assert!(r.editor.window().is_none());
                    assert!(r.session.objects.is_empty());
                    assert!(!r.session.undo());
                });
                accept_owner.set(true);
            }
        }
        shutdown();
    }

    #[test]
    fn text_editor_target_survives_index_changes_and_does_not_resurrect_deleted_objects() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform(Rc<std::cell::Cell<Duration>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(std::cell::Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestPlatform(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        for dpi in [1., 1.25] {
            for sequence in [false, true] {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    r.selected[3] = 8;
                    assert!(r.set_mode(InteractionMode::Tool(if sequence { 4 } else { 3 })));
                    r.ensure_fonts();
                    for i in 0..3 {
                        let object = if sequence {
                            let mut object = crate::annotation_sequence::tests::badge(dpi);
                            crate::annotation_sequence::layout(&mut object);
                            object
                        } else {
                            let text = text_from_values(r, format!("Text {i}"));
                            Object {
                                erased: Vec::new(),
                                id: Default::default(),
                                kind: Kind::Text,
                                bounds: crate::annotation_text::text_bounds(
                                    (-100., 20.),
                                    &text,
                                    dpi,
                                ),
                                style: current_style(r, 8),
                                points: Vec::new(),
                                extra: Extra::Text(text),
                            }
                        };
                        assert!(r.session.add(object));
                    }
                });
                open_text_editor((-100., 20.), Some(1), dpi);
                let (target, last) = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    let target = r.session.objects[1].id;
                    let last = r.session.objects[2].clone();
                    r.editor.window().unwrap().set_value("修改中".into());
                    r.session.selected = Some(0);
                    r.session.delete_selected();
                    assert_eq!(editing_text_object(r).unwrap().id, target);
                    let input = r.editor.window().unwrap();
                    input.invoke_content_changed("修改中".into());
                    input.invoke_caret_changed();
                    input.invoke_done("修改中".into());
                    (target, last)
                });
                clock.set(clock.get() + Duration::from_millis(1));
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(r.editor.edit().is_none());
                    assert_eq!(r.session.objects.len(), 2);
                    assert_eq!(r.session.objects[0].id, target);
                    let content = match &r.session.objects[0].extra {
                        Extra::Text(t) => &t.content,
                        Extra::Sequence(s) => &s.caption.content,
                        _ => unreachable!(),
                    };
                    assert_eq!(content, "修改中");
                    assert_eq!(r.session.objects[1], last);
                });
                open_text_editor((-100., 20.), Some(0), dpi);
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let r = slot.as_mut().unwrap();
                    let before = r.session.objects.clone();
                    r.editor.window().unwrap().set_value("obsolete".into());
                    r.session.selected = Some(0);
                    r.session.delete_selected();
                    assert!(editing_text_object(r).is_none());
                    finish_text_edit_registry(r, true);
                    assert_eq!(r.session.objects, before[1..]);
                    r.session.undo();
                    assert_eq!(
                        r.session.objects, before,
                        "ignored commit must not add an undo record"
                    );
                    r.close();
                });
            }
        }
        shutdown();
    }

    #[test]
    fn bootstrap_creates_each_canvas_before_advancing_windows() {
        let states = [CanvasBootstrap::Prepare(0), CanvasBootstrap::Prepare(0)];
        assert_eq!(
            bootstrap_action(0, 2, std::iter::empty(), 0),
            BootstrapAction::Create(0)
        );
        assert_eq!(
            bootstrap_action(1, 2, states[..1].iter().copied(), 0),
            BootstrapAction::Create(1)
        );
        assert_eq!(
            bootstrap_action(2, 2, states.iter().copied(), 0),
            BootstrapAction::Advance(0)
        );
    }

    #[test]
    fn bootstrap_skips_ready_screen_and_retries_another() {
        let states = [CanvasBootstrap::Ready, CanvasBootstrap::Prepare(2)];
        assert_eq!(
            bootstrap_action(2, 2, states.iter().copied(), 0),
            BootstrapAction::Advance(1)
        );
        let states = [CanvasBootstrap::Present, CanvasBootstrap::Ready];
        assert_eq!(
            bootstrap_action(2, 2, states.iter().copied(), 1),
            BootstrapAction::Advance(0)
        );
        assert_eq!(
            bootstrap_action(2, 2, [CanvasBootstrap::Ready; 2].into_iter(), 0),
            BootstrapAction::ShowToolbar
        );
    }

    #[test]
    fn arrow_click_preview_commits_once_and_rejects_a_nearby_endpoint() {
        let pending = ArrowPending::new((-120., 40.), Style::default(), 1.25, defaults(5));
        let mut session = Session::new();
        let preview = pending.preview((80., -20.));
        assert_eq!(preview.points, vec![(-120., 40.), (80., -20.)]);
        assert!(matches!(preview.extra, Extra::Arrow { curved: false, .. }));
        let frame = crate::annotation_render::render(
            &session,
            Rect {
                left: -250,
                top: -100,
                right: 150,
                bottom: 100,
            },
            1.,
            Some(&preview),
            None,
        )
        .unwrap();
        assert!(frame.pixel(230, 110).unwrap().alpha() > 0);
        assert!(session.objects.is_empty());
        assert!(!session.can_undo());
        assert!(pending.finish((-116.35, 40.)).is_none());
        assert!(pending.finish((-116.25, 40.)).is_some());
        assert!(session.add(pending.finish((80., -20.)).unwrap()));
        assert_eq!(session.selected, Some(0));
        assert_eq!(session.objects[0].points, preview.points);
        assert!(session.undo());
        assert!(session.objects.is_empty());
        assert!(!session.can_undo());
    }

    #[test]
    fn arrow_drag_uses_maximum_excursion_and_release_endpoint() {
        let start = (-200., -100.);
        for scale in [1., 1.25] {
            assert!(!arrow_drag_commits(start, start, 2.9 * scale, scale));
            assert!(arrow_drag_commits(
                start,
                (start.0 + 4. * scale, start.1),
                0.,
                scale,
            ));
            assert!(arrow_drag_commits(
                start,
                (start.0 - 4. * scale, start.1 - 4. * scale),
                0.,
                scale,
            ));
            assert!(!arrow_drag_commits(start, start, 20. * scale, scale));
        }
    }

    #[test]
    fn polyline_preview_stays_at_confirmed_node_until_pointer_leaves_it() {
        let monitor = Rect {
            left: -120,
            top: -100,
            right: 120,
            bottom: 100,
        };
        let endpoint = (50., 0.);
        for curved in [false, true] {
            let mut polyline = Object {
                erased: Vec::new(),
                id: Default::default(),
                bounds: Bounds::from_corners((-80., -80.), endpoint),
                kind: Kind::Polyline,
                style: Style::default(),
                points: vec![(-80., -80.), (-60., 50.), endpoint],
                extra: Extra::Polyline {
                    curved,
                    head: false,
                    start: Endpoint::None,
                    end: Endpoint::None,
                },
            };
            polyline.refresh_bounds();
            for scale in [1., 1.25] {
                let close = (endpoint.0 + 2. * scale, endpoint.1);
                let draft = polyline_preview(&polyline, close, scale);
                assert_eq!(draft, polyline);
                assert_eq!(draft.stroke_points().last().copied(), Some(endpoint));
                let committed = crate::annotation_render::render(
                    &Session::new(),
                    monitor,
                    scale,
                    Some(&polyline),
                    None,
                )
                .unwrap();
                let preview = crate::annotation_render::render(
                    &Session::new(),
                    monitor,
                    scale,
                    Some(&draft),
                    None,
                )
                .unwrap();
                assert_eq!(preview.data(), committed.data());

                let far = (endpoint.0 + 10. * scale, endpoint.1);
                let extended = polyline_preview(&polyline, far, scale);
                assert_eq!(extended.points.last().copied(), Some(far));
                assert_eq!(extended.points.len(), polyline.points.len() + 1);
                assert_eq!(polyline.points.last().copied(), Some(endpoint));
                let moving = crate::annotation_render::render(
                    &Session::new(),
                    monitor,
                    scale,
                    Some(&extended),
                    None,
                )
                .unwrap();
                assert_ne!(moving.data(), committed.data());
            }
        }
    }

    #[test]
    fn geometry_hover_uses_existing_hit_regions_without_selecting_handles() {
        for kind in [Kind::Rectangle, Kind::Ellipse, Kind::HighlightRectangle] {
            let mut session = Session::new();
            let bounds = Bounds::from_corners((20., 20.), (80., 70.));
            assert!(session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind,
                bounds,
                points: Vec::new(),
                style: Style::default(),
                extra: Extra::None,
            }));
            session.selected = None;
            assert_eq!(hovered_geometry_at(&session, (50., 20.), 1.), Some(0));
            assert_eq!(hovered_geometry_at(&session, (5., 5.), 1.), None);
            if kind != Kind::HighlightRectangle {
                assert_eq!(hovered_geometry_at(&session, (50., 45.), 1.), None);
                session.objects[0].style.fill = true;
                assert_eq!(hovered_geometry_at(&session, (50., 45.), 1.), Some(0));
            }
            session.selected = Some(0);
            assert_eq!(hovered_geometry_at(&session, (50., 20.), 1.), None);
        }
    }

    #[test]
    fn magnifier_frame_draft_never_creates_a_native_view() {
        let object = Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: Bounds::from_corners((120., 80.), (220., 160.)),
            kind: Kind::Magnifier,
            style: Style::default(),
            points: vec![],
            extra: Extra::Magnifier {
                output: Bounds::from_corners((260., 80.), (410., 200.)),
                zoom: 1.5,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: true,
                antialias: true,
                shadow: false,
            },
        };
        let mut session = Session::new();
        assert!(session.add(object.clone()));
        let output = Bounds::from_corners((260., 80.), (410., 200.));
        let draft = Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: output,
            kind: Kind::Rectangle,
            style: Style::default(),
            points: vec![],
            extra: Extra::None,
        };
        let preview = magnifier_views(&session, Some(&draft));
        assert_eq!(
            preview
                .iter()
                .map(|view| (view.id, view.preview))
                .collect::<Vec<_>>(),
            vec![(0, false)]
        );
        assert!(session.add(object));
        let committed = magnifier_views(&session, None);
        assert_eq!(committed[1].id, 1);
        assert!(!committed[1].preview);
        assert!(session.undo());
        assert_eq!(magnifier_views(&session, None).len(), 1);
    }

    #[test]
    fn magnifier_commit_centers_output_and_rejects_tiny_sample() {
        let values = AnnotationValues {
            zoom: 150,
            ..defaults(7)
        };
        let source = Bounds::from_corners((-180., -120.), (-30., -30.));
        let object = magnifier_from_source(source, Style::default(), &values).unwrap();
        assert!(
            values.include_annotations,
            "new sessions include annotations by default"
        );
        assert!(matches!(
            object.extra,
            Extra::Magnifier {
                erase_annotations: false,
                ..
            }
        ));
        let mut state = parameters::State::new();
        let mut session = Session::new();
        session.add(object.clone());
        let id = session.objects[0].id;
        for include in [false, true] {
            let mut edited = values.clone();
            edited.include_annotations = include;
            state.replace(7, Some(id), edited.clone());
            state.deselect();
            let next =
                magnifier_from_source(source, Style::default(), state.values(7, None)).unwrap();
            assert!(
                matches!(next.extra, Extra::Magnifier { erase_annotations, .. } if erase_annotations == !include)
            );
            // Re-editing an existing object must project its own value, not the default.
            state.project(&next, 0, 1, 0.5);
            assert_eq!(state.values(7, Some(next.id)).include_annotations, include);
            let before = AnnotationValues {
                include_annotations: !include,
                ..values.clone()
            };
            let mut applied = before.clone();
            parameters::Patch::between(before, edited)
                .unwrap()
                .apply(&mut applied);
            assert_eq!(applied.include_annotations, include);
            assert_eq!(
                session.objects[0].extra, object.extra,
                "default changes must not mutate existing objects"
            );
        }
        assert!(!parameters::defaults(2).erase);
        assert!(parameters::defaults(9).erase);
        assert_eq!(object.bounds, source);
        assert_eq!(object.bounds.handles()[7], (-30., -30.));
        assert!(
            matches!(object.extra, Extra::Magnifier { output, .. } if output == Bounds::from_corners((-217.5, -142.5), (7.5, -7.5)))
        );
        assert!(
            magnifier_from_source(
                Bounds::from_corners((0., 0.), (1., 1.)),
                Style::default(),
                &values,
            )
            .is_none()
        );
    }

    #[test]
    fn deferred_magnifier_sync_ignores_cancelled_and_replaced_gestures() {
        let mut state = MagnifierSync::Cold;
        let mut revision = 0;
        assert_eq!(
            magnifier_frame_action(&mut state, &mut revision, true),
            MagnifierFrameAction::Defer(1)
        );
        assert_eq!(
            magnifier_frame_action(&mut state, &mut revision, true),
            MagnifierFrameAction::Wait
        );
        assert_eq!(
            magnifier_frame_action(&mut state, &mut revision, false),
            MagnifierFrameAction::Clear
        );
        assert_eq!(
            magnifier_frame_action(&mut state, &mut revision, true),
            MagnifierFrameAction::Defer(3)
        );
        assert!(!deferred_sync_is_current(7, state, 7, 1));
        state = MagnifierSync::Live;
        assert_eq!(
            magnifier_frame_action(&mut state, &mut revision, true),
            MagnifierFrameAction::Sync
        );
        state = MagnifierSync::Failed;
        assert_eq!(
            magnifier_frame_action(&mut state, &mut revision, true),
            MagnifierFrameAction::Wait
        );
        assert!(deferred_sync_is_current(7, MagnifierSync::Pending(3), 7, 3));
        assert!(!deferred_sync_is_current(7, MagnifierSync::Cold, 7, 3));
        assert!(!deferred_sync_is_current(
            7,
            MagnifierSync::Pending(4),
            7,
            3
        ));
        assert!(!deferred_sync_is_current(
            8,
            MagnifierSync::Pending(3),
            7,
            3
        ));
    }

    #[test]
    fn panel_owner_failure_retries_without_touching_reopened_instance() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let ready = Rc::new(Cell::new(false));
        let calls = Rc::new(Cell::new(0));
        // Native placement now requires a known work area; provide one in the fixture.
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -2000,
                    top: -1000,
                    right: 2000,
                    bottom: 2000,
                })
            });
        });
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            let (ready, calls) = (ready.clone(), calls.clone());
            r.lifecycle.attach_tool_window = Rc::new(move |_, _| {
                calls.set(calls.get() + 1);
                ready.get()
            });
        });
        open_panel(0, true);
        let (generation, old_revision) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            (r.generation, r.panel.revision())
        });
        show_panel(generation, old_revision, 0);
        assert_eq!(calls.get(), 1);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let panel = slot.as_ref().unwrap().panel.get().unwrap();
            assert!(!panel.window().is_visible());
        });
        ready.set(true);
        show_panel(generation, old_revision, 1);
        assert_eq!(
            calls.get(),
            3,
            "owner must be confirmed before and after show"
        );
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().close_panel());
        open_panel(0, true);
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().panel.revision());
        assert_ne!(revision, old_revision);
        show_panel(generation, old_revision, 20);
        assert_eq!(calls.get(), 3, "old retry must not operate a new component");
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_some()));
        ready.set(false);
        show_panel(generation, revision, 20);
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_none()));
        shutdown();
    }

    #[test]
    fn toolbar_owner_tracks_physical_monitor_boundaries() {
        let monitors = [
            Rect {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080,
            },
            Rect {
                left: 0,
                top: -200,
                right: 2560,
                bottom: 1440,
            },
        ];
        assert_eq!(canvas_at(monitors, Point { x: -1, y: 300 }), Some(0));
        assert_eq!(canvas_at(monitors, Point { x: 0, y: 300 }), Some(1));
        assert_eq!(canvas_at(monitors, Point { x: 100, y: -100 }), Some(1));
        assert_eq!(canvas_at(monitors, Point { x: 3000, y: 300 }), None);
    }
    #[test]
    fn panels_flip_and_clamp_on_negative_monitors() {
        let area = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(
            panel_position(Point { x: -100, y: 1000 }, 54, 720, 204, area, 8),
            Point { x: -720, y: 788 }
        );
    }
    #[test]
    fn independent_tool_defaults_match_preview() {
        assert_eq!(defaults(4).color_index, 6);
        assert_eq!(defaults(7).size, 2);
        assert_eq!(defaults(10).text_size, 16);
        assert_eq!(defaults(0).rounding, 21);
        for group in GROUPS {
            for &tool in group {
                assert!(!fields(tool).is_empty());
            }
        }
    }
}
