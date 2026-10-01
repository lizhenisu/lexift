//! Owns the ephemeral annotation preview windows and per-tool UI values.
use crate::bridge::dispatch_pointer_input as dispatch;
use crate::{
    AnnotationCanvas, AnnotationPanel, AnnotationTextInput, AnnotationToolbar, AnnotationValues,
    AnnotationWatermarkInput,
    bridge::{PassiveWindowPreparation, PopupPointerInput, WindowLifecycleCallbacks},
    placement,
};
use lexift_core::domain::{
    annotation::{
        Bounds, Endpoint, Extra, Hit, Kind, MagnifierConnector, Object, SequenceAnnotation,
        SequenceFormat, SequenceShape, Session, Style, TextAnnotation, TextBackground, TextOutline,
        Watermark, WatermarkPosition, magnifier_output_for_source,
    },
    geometry::{Point, Rect},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    hash::{Hash, Hasher},
    rc::Rc,
    time::{Duration, Instant},
};

#[path = "annotation_choice.rs"]
mod choice;
#[path = "annotation_color.rs"]
mod color;
#[path = "annotation_cursor.rs"]
mod cursor;

thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

struct Registry {
    main: Option<AnnotationToolbar>,
    panel: Option<AnnotationPanel>,
    text_editor: Option<AnnotationTextInput>,
    text_edit: Option<TextEdit>,
    watermark_input: Option<AnnotationWatermarkInput>,
    watermark_input_revision: u64,
    fonts: Vec<String>,
    choice: Option<choice::Menu>,
    choice_revision: u64,
    canvases: Vec<CanvasLayer>,
    pending_displays: Vec<Rect>,
    canvas_bootstrap: Vec<CanvasBootstrap>,
    bootstrap_cursor: usize,
    annotation_ready: bool,
    cold_open_measured: bool,
    cold_open_timing: Option<ColdOpenTiming>,
    toolbar_owner: Option<usize>,
    session: Session,
    hovered: Option<usize>,
    gesture: Option<Gesture>,
    draft: Option<Object>,
    arrow_pending: Option<ArrowPending>,
    polyline: Option<Object>,
    last_polyline_click: Option<(Instant, (f32, f32))>,
    mode: InteractionMode,
    render_queued: bool,
    magnifier_sync: MagnifierSync,
    magnifier_sync_revision: u64,
    first_magnifier_timing: Option<FirstMagnifierTiming>,
    first_magnifier_measured: bool,
    style_edit_active: bool,
    parameter_dragging: bool,
    style_edit_revision: u64,
    pending_edit_panel: bool,
    gesture_object: Option<Object>,
    selected: [i32; 7],
    values: Vec<AnnotationValues>,
    group: Option<usize>,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    timer: slint::Timer,
    layout_timer: slint::Timer,
    magnifier_timer: slint::Timer,
    generation: u64,
    panel_revision: u64,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CanvasBootstrap {
    Prepare(u8),
    Present,
    Ready,
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

struct CanvasLayer {
    last_pointer: Option<(f32, f32)>,
    corner_cursor_failed: Cell<bool>,
    window: AnnotationCanvas,
    bounds: Rect,
    signature: Option<u64>,
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

#[derive(Clone, Copy)]
enum Gesture {
    PlaceSequence {
        start: (f32, f32),
        index: usize,
    },
    SequenceControl {
        index: usize,
        handle: usize,
    },
    PlacePoint,
    FinishArrow,
    DeleteText,
    ToggleTextArrow {
        index: usize,
    },
    CreateText {
        at: (f32, f32),
        scale: f32,
    },
    Draw {
        start: (f32, f32),
        kind: Kind,
        style: Style,
        start_scale: f32,
        max_distance: f32,
    },
    Move {
        start: (f32, f32),
        initial: Bounds,
        index: usize,
    },
    Resize {
        initial: Bounds,
        index: usize,
        handle: usize,
    },
    TextHandle {
        start: (f32, f32),
        grab: (f32, f32),
        handle: usize,
        index: Option<usize>,
        editing: bool,
        active: bool,
        scale: f32,
    },
    TextSelection {
        anchor: usize,
    },
}

struct TextEdit {
    id: u64,
    selection_anchor: Option<usize>,
    index: Option<usize>,
    at: (f32, f32),
    scale: f32,
    original: TextAnnotation,
    bounds: Bounds,
    laid_out_content: String,
    caret: CaretBlink,
    caret_timer: slint::Timer,
}

/// Canvas feedback has its own clock because the native TextInput is transparent.
struct CaretBlink {
    visible: bool,
}
impl CaretBlink {
    fn reset(&mut self) {
        self.visible = true;
    }
    fn tick(&mut self, held: bool) -> bool {
        let previous = self.visible;
        self.visible = held || !self.visible;
        self.visible != previous
    }
}

fn reset_text_caret(r: &mut Registry) {
    let Some(edit) = &mut r.text_edit else { return };
    edit.caret.reset();
    let (generation, id) = (r.generation, edit.id);
    edit.caret_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || tick_text_caret(generation, id),
    );
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
            .text_editor
            .as_ref()
            .is_some_and(|e| !e.get_preedit().is_empty());
        let edit = r.text_edit.as_mut().unwrap();
        edit.caret
            .tick(composing || edit.selection_anchor.is_some())
    });
    if changed {
        schedule_render(generation);
    }
}

thread_local! { static NEXT_TEXT_EDIT: Cell<u64> = const { Cell::new(0) }; }

fn text_edit_matches(r: &Registry, id: u64) -> bool {
    r.text_edit.as_ref().is_some_and(|edit| edit.id == id)
}
fn editing_sequence(r: &Registry) -> bool {
    r.text_edit
        .as_ref()
        .and_then(|e| e.index)
        .and_then(|i| r.session.objects.get(i))
        .is_some_and(|o| o.kind == Kind::Sequence)
}

#[derive(Clone)]
struct ArrowPending {
    start: (f32, f32),
    style: Style,
    start_scale: f32,
    values: AnnotationValues,
}

impl ArrowPending {
    fn preview(&self, end: (f32, f32)) -> Object {
        arrow_draft(self.start, end, self.style, &self.values)
    }

    fn finish(&self, end: (f32, f32)) -> Option<Object> {
        arrow_endpoint_valid(self.start, end, self.start_scale).then(|| self.preview(end))
    }
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
    "Automatic mosaic",
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

fn defaults(tool: i32) -> AnnotationValues {
    AnnotationValues {
        size: if tool == 7 {
            2
        } else if matches!(tool, 3 | 4 | 11..=13) {
            12
        } else {
            4
        },
        rounding: if tool == 4 { 0 } else { 21 },
        text_size: if tool == 10 { 16 } else { 22 },
        text_outline_width: 1,
        text_bg_opacity: 100,
        text_bg_rounding: 0,
        text_bg_padding: 4,
        strength: if tool == 9 {
            100.
        } else if tool == 2 {
            10.
        } else {
            50.
        },
        start: 1,
        sequence_ratio: 2,
        sequence_guide: true,
        sequence_text_color: "".into(),
        sequence_label: "1".into(),
        shape: if tool == 10 {
            4
        } else {
            i32::from(matches!(tool, 1 | 2))
        },
        zoom: if tool == 7 { 150 } else { 0 },
        head: tool == 5,
        line_end: if matches!(tool, 5 | 10) { 3 } else { 0 },
        mode: 0,
        erase: tool != 2,
        antialias: true,
        color_index: if tool == 4 {
            6
        } else if tool == 2 {
            1
        } else {
            0
        },
        ..Default::default()
    }
}

pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    REGISTRY.with(|slot| {
        *slot.borrow_mut() = Some(Registry {
            main: None,
            panel: None,
            text_editor: None,
            text_edit: None,
            watermark_input: None,
            watermark_input_revision: 0,
            fonts: Vec::new(),
            choice: None,
            choice_revision: 0,
            canvases: Vec::new(),
            pending_displays: Vec::new(),
            canvas_bootstrap: Vec::new(),
            bootstrap_cursor: 0,
            annotation_ready: false,
            cold_open_measured: false,
            cold_open_timing: None,
            toolbar_owner: None,
            session: Session::new(),
            hovered: None,
            gesture: None,
            draft: None,
            arrow_pending: None,
            polyline: None,
            last_polyline_click: None,
            mode: InteractionMode::Tool(0),
            render_queued: false,
            magnifier_sync: MagnifierSync::Cold,
            magnifier_sync_revision: 0,
            first_magnifier_timing: None,
            first_magnifier_measured: false,
            style_edit_active: false,
            parameter_dragging: false,
            style_edit_revision: 0,
            pending_edit_panel: false,
            gesture_object: None,
            selected: [0, 4, 7, 9, 10, 11, 13],
            values: (0..14).map(defaults).collect(),
            group: None,
            prepare,
            lifecycle,
            timer: slint::Timer::default(),
            layout_timer: slint::Timer::default(),
            magnifier_timer: slint::Timer::default(),
            generation: 0,
            panel_revision: 0,
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
    REGISTRY.with(|s| s.borrow().as_ref().is_some_and(|r| r.main.is_some()))
}

fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}

fn next_frame(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::from_millis(16), f);
}

impl Registry {
    fn sync_mode_display(&self) {
        if let Some(main) = &self.main {
            main.set_operate(self.mode.is_mouse());
            main.set_active_group(self.mode.selected_group());
        }
    }

    /// Apply the native input policy before exposing a new mode in the toolbar.
    fn set_mode(&mut self, mode: InteractionMode) -> bool {
        if self.mode == mode {
            self.sync_mode_display();
            return true;
        }
        self.close_watermark_input();
        if self.text_editor.is_some() {
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
        if mode != InteractionMode::Tool(4) && cancel_sequence_placement(self) {
            let generation = self.generation;
            later(move || schedule_render(generation));
        }
        self.mode = mode;
        if mode != InteractionMode::Tool(2) {
            self.arrow_pending = None;
            if matches!(
                self.gesture,
                Some(
                    Gesture::FinishArrow
                        | Gesture::Draw {
                            kind: Kind::Arrow,
                            ..
                        }
                )
            ) {
                self.gesture = None;
            }
            self.polyline = None;
            self.last_polyline_click = None;
            self.draft = None;
        }
        self.hovered = None;
        cursor::refresh(self);
        self.sync_mode_display();
        true
    }

    fn finish_style_edit(&mut self) {
        if self.style_edit_active {
            self.session.finish_drag();
            self.style_edit_active = false;
            self.style_edit_revision = self.style_edit_revision.wrapping_add(1);
            if let Some(main) = &self.main {
                main.set_can_undo(self.session.can_undo());
            }
        }
    }

    /// Invalidate callbacks before releasing the native editor and its input bridge.
    fn close_watermark_input(&mut self) {
        choice::close_watermark(self);
        self.watermark_input_revision = self.watermark_input_revision.wrapping_add(1);
        if let Some(editor) = self.watermark_input.take() {
            let _ = editor.hide();
        }
    }

    fn close_panel(&mut self) {
        self.parameter_dragging = false;
        choice::close(self);
        self.finish_style_edit();
        self.panel_revision = self.panel_revision.wrapping_add(1);
        if let Some(panel) = self.panel.take() {
            if !panel.get_menu() {
                self.values[panel.get_tool() as usize] = panel.get_values();
            }
            (self.lifecycle.set_popup_dismissal)(panel.window(), false);
            let _ = panel.hide();
        }
        self.group = None;
        self.sync_mode_display();
    }
    fn close(&mut self) {
        if let Some(timing) = self.cold_open_timing.take() {
            timing.report("cancelled", self.pending_displays.len());
            self.cold_open_measured = true;
        }
        self.close_panel();
        if let Some(editor) = self.text_editor.take() {
            let _ = editor.hide();
        }
        self.text_edit = None;
        self.close_watermark_input();
        (self.lifecycle.annotation_magnifier_sync)(&[], &[]);
        self.generation = self.generation.wrapping_add(1);
        self.timer.stop();
        self.layout_timer.stop();
        self.magnifier_timer.stop();
        self.gesture = None;
        self.draft = None;
        self.arrow_pending = None;
        self.polyline = None;
        self.last_polyline_click = None;
        self.render_queued = false;
        self.annotation_ready = false;
        self.pending_displays.clear();
        self.canvas_bootstrap.clear();
        self.bootstrap_cursor = 0;
        self.magnifier_sync = MagnifierSync::Cold;
        self.magnifier_sync_revision = self.magnifier_sync_revision.wrapping_add(1);
        self.first_magnifier_timing = None;
        self.first_magnifier_measured = false;
        self.style_edit_active = false;
        self.pending_edit_panel = false;
        self.mode = InteractionMode::Tool(0);
        self.session.clear();
        self.toolbar_owner = None;
        for canvas in self.canvases.drain(..) {
            let _ = canvas.window.hide();
        }
        if let Some(main) = self.main.take() {
            let _ = main.hide();
        }
        self.values = (0..14).map(defaults).collect();
        self.selected = [0, 4, 7, 9, 10, 11, 13];
        self.hovered = None;
    }
}

fn finish_text_edit_registry(r: &mut Registry, commit: bool) {
    let value = r
        .text_editor
        .as_ref()
        .map(|editor| editor.get_value().to_string())
        .unwrap_or_default();
    if let Some(editor) = r.text_editor.take() {
        let _ = editor.hide();
    }
    let Some(edit) = r.text_edit.take() else {
        return;
    };
    edit.caret_timer.stop();
    if !commit {
        return;
    }
    if let Some(index) = edit.index
        && r.session
            .objects
            .get(index)
            .is_some_and(|o| o.kind == Kind::Sequence)
    {
        let mut caption = edit.original;
        caption.content = value;
        r.session.selected = Some(index);
        r.session.update_selected(|object| {
            if let Extra::Sequence(s) = &mut object.extra {
                s.caption = caption;
            }
            crate::annotation_sequence::layout(object);
        });
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
        return;
    }
    if value.trim().is_empty() {
        return;
    }
    let mut text = edit.original;
    text.content = value;
    let bounds = if edit.laid_out_content == text.content {
        edit.bounds
    } else {
        crate::annotation_text::text_bounds(edit.at, &text, edit.scale)
    };
    if let Some(index) = edit.index {
        r.session.selected = Some(index);
        r.session.update_selected(|object| {
            object.bounds = bounds;
            object.extra = Extra::Text(text);
        });
    } else {
        r.session.add(Object {
            bounds,
            kind: Kind::Text,
            style: current_style(r, 8),
            points: Vec::new(),
            extra: Extra::Text(text),
        });
    }
    if let Some(main) = &r.main {
        main.set_can_undo(r.session.can_undo());
    }
}

fn finish_text_edit(generation: u64, commit: bool) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        finish_text_edit_registry(r, commit);
        Some(r.generation)
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
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

fn pointer_text(weak: slint::Weak<AnnotationTextInput>) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if let Some(editor) = weak.upgrade() {
            dispatch(editor.window(), input);
        }
    })
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
        let editor = r.text_editor.as_ref().unwrap();
        let origin = editor.window().position();
        let dpi = editor.window().scale_factor();
        let point = (origin.x as f32 + x * dpi, origin.y as f32 + y * dpi);
        let handle = if editing_sequence(r) {
            None
        } else {
            text_handle_at(&object, point, dpi)
        };
        editor.set_control_hover(
            handle.is_some() || matches!(r.gesture, Some(Gesture::TextHandle { .. })),
        );
        if matches!(r.gesture, Some(Gesture::TextHandle { editing: true, .. })) {
            if event == 1 || event == 2 {
                update_text_handle(r, point);
            }
            if event == 2 {
                finish_text_handle(r, point);
            }
            if event == 3 {
                if let Some(object) = r.gesture_object.take() {
                    apply_editing_object(r, object);
                }
                r.gesture = None;
            }
            let generation = r.generation;
            later(move || schedule_render(generation));
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
            restore_text_focus(r);
        }
        let edit = r.text_edit.as_mut().unwrap();
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
            later(move || schedule_render(generation));
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
        let (Some(editor), Some(edit)) = (&r.text_editor, &mut r.text_edit) else {
            return None;
        };
        let mut text = edit.original.clone();
        text.content = editor_preview(editor).0;
        let b = if let Some(mut object) = edit
            .index
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

/// Bounds the native surface to the desktop, while TextInput keeps full logical layout.
fn place_text_editor(
    editor: &AnnotationTextInput,
    bounds: Bounds,
    scale: f32,
    text: &TextAnnotation,
    canvases: &[CanvasLayer],
) {
    let displays = canvases
        .iter()
        .map(|layer| layer.bounds)
        .collect::<Vec<_>>();
    let viewport = text_editor_viewport(bounds, &displays);
    editor.window().set_position(slint::PhysicalPosition::new(
        viewport.left.floor() as i32,
        viewport.top.floor() as i32,
    ));
    let native_scale = editor.window().scale_factor().max(0.1);
    editor.set_text_size(text.font_size * scale / native_scale);
    editor.set_text_padding(crate::annotation_text::text_padding(text, scale) / native_scale);
    editor.set_content_x((bounds.left - viewport.left.floor()) / native_scale);
    editor.set_content_y((bounds.top - viewport.top.floor()) / native_scale);
    editor.set_layout_width(bounds.width() / native_scale);
    editor.set_layout_height(bounds.height() / native_scale);
    editor.window().set_size(slint::PhysicalSize::new(
        (viewport.right.ceil() - viewport.left.floor()).max(1.) as u32,
        (viewport.bottom.ceil() - viewport.top.floor()).max(1.) as u32,
    ));
}

fn text_editor_viewport(bounds: Bounds, displays: &[Rect]) -> Bounds {
    let Some(first) = displays.first() else {
        return bounds;
    };
    let desktop = displays.iter().fold(*first, |mut area, screen| {
        area.left = area.left.min(screen.left);
        area.top = area.top.min(screen.top);
        area.right = area.right.max(screen.right);
        area.bottom = area.bottom.max(screen.bottom);
        area
    });
    let left = bounds
        .left
        .max(desktop.left as f32)
        .min(desktop.right as f32 - 1.);
    let top = bounds
        .top
        .max(desktop.top as f32)
        .min(desktop.bottom as f32 - 1.);
    Bounds {
        left,
        top,
        right: bounds.right.min(desktop.right as f32).max(left + 1.),
        bottom: bounds.bottom.min(desktop.bottom as f32).max(top + 1.),
    }
}

fn open_text_editor(at: (f32, f32), index: Option<usize>, scale: f32) {
    let clicked = at;
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        finish_text_edit_registry(r, true);
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
        let id = NEXT_TEXT_EDIT.with(|next| {
            let id = next.get().wrapping_add(1);
            next.set(id);
            id
        });
        editor.set_value(original.content.clone().into());
        editor.set_text_size(original.font_size);
        editor.set_font_name(original.font_family.clone().into());
        editor.set_bold(original.bold);
        editor.set_italic(original.italic);
        editor.set_text_padding(crate::annotation_text::text_padding(&original, 1.));
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
        editor.set_ink(slint::Color::from_rgb_u8(color[0], color[1], color[2]));
        editor.on_done(move |_| later(move || finish_matching_text_edit(generation, id, true)));
        editor.on_cancel(move || later(move || finish_matching_text_edit(generation, id, false)));
        editor.on_pointer(move |event, x, y| editor_pointer(generation, id, event, x, y));
        editor.on_content_changed(move |_| {
            later(move || resize_text_editor(generation, id));
        });
        editor.on_caret_changed(move || later(move || resize_text_editor(generation, id)));
        editor.window().on_close_requested(move || {
            later(move || finish_matching_text_edit(generation, id, true));
            slint::CloseRequestResponse::KeepWindowShown
        });
        let b = crate::annotation_text::text_bounds(at, &original, scale);
        place_text_editor(&editor, b, scale, &original, &r.canvases);
        if let Some(object) = index.and_then(|i| r.session.objects.get(i)) {
            let caption = object.sequence_caption();
            let offset =
                crate::annotation_text::caret_at(caption.as_ref().unwrap_or(object), clicked)
                    as i32;
            editor.invoke_place_selection(offset, offset);
        }
        if let Some(owner) = r.canvases.iter().find(|layer| {
            at.0 >= layer.bounds.left as f32
                && at.0 < layer.bounds.right as f32
                && at.1 >= layer.bounds.top as f32
                && at.1 < layer.bounds.bottom as f32
        }) {
            (r.lifecycle.attach_tool_window)(editor.window(), owner.window.window());
        }
        r.text_edit = Some(TextEdit {
            id,
            selection_anchor: None,
            index,
            at,
            scale,
            original,
            bounds: b,
            laid_out_content: editor.get_value().to_string(),
            caret: CaretBlink { visible: true },
            caret_timer: slint::Timer::default(),
        });
        r.text_editor = Some(editor);
        reset_text_caret(r);
        later(move || show_text_editor(generation, id, 0));
    });
    let generation = REGISTRY.with(|slot| slot.borrow().as_ref().map(|r| r.generation));
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

fn show_text_editor(generation: u64, id: u64, attempt: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        if !text_edit_matches(r, id) {
            return;
        }
        let Some(editor) = &r.text_editor else { return };
        match (r.prepare)(editor.window()) {
            PassiveWindowPreparation::Ready => {
                if editor.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        editor.window(),
                        pointer_text(editor.as_weak()),
                    )
                {
                    r.text_editor = None;
                    r.text_edit = None;
                    return;
                }
                if let Some(edit) = &r.text_edit {
                    place_text_editor(editor, edit.bounds, edit.scale, &edit.original, &r.canvases);
                }
                (r.lifecycle.activate_user_requested_window)(editor.window());
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = editor.show();
                let _ = editor.hide();
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_text_editor(generation, id, attempt + 1)
                });
            }
            _ => {
                r.text_editor = None;
                r.text_edit = None;
            }
        }
    });
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
pub(crate) fn escape() {
    if choice::dismiss() {
        return;
    }
    let editing = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref().and_then(|r| {
            if r.text_editor.is_some() {
                Some((r.generation, true, 0))
            } else if r.watermark_input.is_some() {
                Some((r.generation, false, r.watermark_input_revision))
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
        if r.gesture.is_some() {
            cancel_sequence_placement(r);
            r.session.cancel_drag();
            r.gesture = None;
            r.draft = None;
            r.arrow_pending = None;
            r.polyline = None;
            r.last_polyline_click = None;
            refresh_hovered(r);
            cursor::refresh(r);
            return Some(r.generation);
        }
        if r.polyline.take().is_some() {
            r.last_polyline_click = None;
            r.draft = None;
            cursor::refresh(r);
            return Some(r.generation);
        }
        if r.arrow_pending.take().is_some() {
            r.draft = None;
            cursor::refresh(r);
            return Some(r.generation);
        }
        if r.session.selected.is_some() {
            r.session.selected = None;
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
    let has_panel = REGISTRY.with(|s| s.borrow().as_ref().is_some_and(|r| r.panel.is_some()));
    if has_panel {
        let target = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut()?;
            r.close_panel();
            Some((
                r.main.as_ref()?.clone_strong(),
                r.lifecycle.activate_user_requested_window,
            ))
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
        let object = r.polyline.take()?;
        r.last_polyline_click = None;
        r.draft = None;
        if object.points.len() >= if object.style.fill { 3 } else { 2 } {
            r.session.add(object);
        }
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
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
        let polyline = r.polyline.as_mut()?;
        polyline.points.pop();
        if polyline.points.is_empty() {
            r.polyline = None;
            r.draft = None;
        } else {
            polyline.refresh_bounds();
            r.draft = r.polyline.clone();
        }
        r.last_polyline_click = None;
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
        main.on_tool_clicked(|group, menu| later(move || open_panel(group as usize, menu)));
        main.on_operate_requested(|| later(activate_mouse_mode));
        main.on_undo_requested(|| later(undo));
        main.on_delete_requested(|| later(delete_selected));
        main.on_finish_path_requested(|| later(finish_polyline));
        main.on_backspace_requested(|| later(backspace_polyline));
        main.on_finish_requested(|| later(close));
        main.on_escape_requested(|| later(escape));
        main.on_drag_requested(|| {
            later(|| {
                choice::dismiss();
                let target = REGISTRY.with(|s| {
                    let slot = s.borrow();
                    let r = slot.as_ref()?;
                    Some((
                        r.main.as_ref()?.clone_strong(),
                        r.lifecycle.begin_window_drag,
                    ))
                });
                if let Some((main, drag)) = target {
                    drag(main.window());
                    reposition_panel();
                }
            })
        });
        main.window().on_close_requested(|| {
            later(close);
            slint::CloseRequestResponse::KeepWindowShown
        });
        main.window().set_size(slint::LogicalSize::new(650., 54.));
        if let Some(timing) = r.cold_open_timing.as_mut() {
            let elapsed = toolbar_start.elapsed();
            timing.toolbar_create += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
        }
        r.main = Some(main);
        r.sync_mode_display();
        r.generation = r.generation.wrapping_add(1);
        let generation = r.generation;
        let query_start = Instant::now();
        r.pending_displays = (r.lifecycle.annotation_displays)();
        r.session.displays = r.pending_displays.clone();
        if let Some(timing) = r.cold_open_timing.as_mut() {
            let elapsed = query_start.elapsed();
            timing.display_query += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
            timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
        }
        next_frame(move || bootstrap_tick(generation));
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
    states: &[CanvasBootstrap],
    cursor: usize,
) -> BootstrapAction {
    if created < total {
        return BootstrapAction::Create(created);
    }
    for offset in 0..states.len() {
        let index = (cursor + offset) % states.len();
        if states[index] != CanvasBootstrap::Ready {
            return BootstrapAction::Advance(index);
        }
    }
    BootstrapAction::ShowToolbar
}

/// Advance one screen stage per event-loop turn; a pending native window does
/// not hold up preparation of the other monitors.
fn bootstrap_tick(generation: u64) {
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let action = bootstrap_action(
            r.canvases.len(),
            r.pending_displays.len(),
            &r.canvas_bootstrap,
            r.bootstrap_cursor,
        );
        match action {
            BootstrapAction::Create(index) => create_canvas(r, index),
            BootstrapAction::Advance(index) => {
                r.bootstrap_cursor = (index + 1) % r.canvas_bootstrap.len();
                match r.canvas_bootstrap[index] {
                    CanvasBootstrap::Prepare(attempt) => show_canvas(r, index, attempt),
                    CanvasBootstrap::Present => render_initial_canvas(r, index),
                    CanvasBootstrap::Ready => unreachable!(),
                }
            }
            BootstrapAction::ShowToolbar => {
                next_frame(move || show_main(generation, 0));
                return;
            }
        }
        if r.generation == generation {
            if let Some(timing) = r.cold_open_timing.as_mut() {
                timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
            }
            next_frame(move || bootstrap_tick(generation));
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
    window.on_pointer(move |kind, x, y| canvas_pointer(index, kind, x, y));
    window.on_escape_requested(|| later(escape));
    window.on_undo_requested(|| later(undo));
    window.on_delete_requested(|| later(delete_selected));
    window.on_finish_path_requested(|| later(finish_polyline));
    window.on_backspace_requested(|| later(backspace_polyline));
    window.window().on_close_requested(|| {
        later(close);
        slint::CloseRequestResponse::KeepWindowShown
    });
    r.canvases.push(CanvasLayer {
        last_pointer: None,
        corner_cursor_failed: Cell::new(false),
        window,
        bounds,
        signature: None,
    });
    r.canvas_bootstrap.push(CanvasBootstrap::Prepare(0));
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
                pointer_canvas(layer.window.as_weak()),
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
            settle_canvas(layer.window.as_weak(), rect, 2);
            r.canvas_bootstrap[index] = CanvasBootstrap::Present;
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
            r.canvas_bootstrap[index] = CanvasBootstrap::Prepare(attempt + 1);
        }
        _ => r.close(),
    }
}

/// Present one complete blank monitor frame before its canvas accepts drawing.
/// The native layered bitmap is retained here until Windows hit testing of an
/// unpresented Slint canvas has been verified on supported DPI layouts.
fn render_initial_canvas(r: &mut Registry, index: usize) {
    let layer = &r.canvases[index];
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
    let presented = (r.lifecycle.annotation_frame_presenter)(
        layer.window.window(),
        frame.data(),
        frame.width(),
        frame.height(),
    );
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
    r.canvas_bootstrap[index] = CanvasBootstrap::Ready;
}

fn settle_canvas(weak: slint::Weak<AnnotationCanvas>, bounds: Rect, remaining: u8) {
    slint::Timer::single_shot(Duration::from_millis(32), move || {
        let Some(canvas) = weak.upgrade() else { return };
        canvas
            .window()
            .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
        canvas.window().set_size(slint::PhysicalSize::new(
            (bounds.right - bounds.left) as u32,
            (bounds.bottom - bounds.top) as u32,
        ));
        if remaining > 0 {
            settle_canvas(weak, bounds, remaining - 1);
        }
    });
}

fn pointer_canvas(weak: slint::Weak<AnnotationCanvas>) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if let Some(window) = weak.upgrade() {
            if let PopupPointerInput::LeftCancelled { x, y } = input {
                let scale = window.window().scale_factor().max(0.1);
                window.invoke_pointer(3, x / scale, y / scale);
            }
            dispatch(window.window(), input);
        }
    })
}

fn palette(tool: usize) -> [[u8; 3]; 8] {
    if tool == 4 {
        [
            [255, 244, 0],
            [255, 165, 0],
            [70, 235, 235],
            [165, 155, 255],
            [255, 160, 245],
            [255, 255, 255],
            [50, 236, 102],
            [33, 33, 33],
        ]
    } else {
        [
            [217, 71, 43],
            [229, 191, 85],
            [105, 173, 97],
            [89, 143, 223],
            [33, 33, 33],
            [255, 255, 255],
            [50, 236, 102],
            [255, 244, 0],
        ]
    }
}

fn palette_color(palette: [[u8; 3]; 8], index: i32, custom: &str) -> [u8; 3] {
    if index == 8 {
        parse_hex_color(custom).unwrap_or(palette[0])
    } else {
        palette[index.clamp(0, 7) as usize]
    }
}

fn color_to_values(palette: [[u8; 3]; 8], rgb: [u8; 3]) -> (i32, slint::SharedString) {
    (
        palette
            .iter()
            .position(|c| *c == rgb)
            .map_or(8, |i| i as i32),
        color::hex(rgb).into(),
    )
}

fn current_style(r: &Registry, tool: i32) -> Style {
    let v = &r.values[tool as usize];
    let palette = palette(tool as usize);
    let color = if v.color_index == 8 {
        parse_hex_color(&v.custom_color).unwrap_or(palette[0])
    } else {
        palette[v.color_index.clamp(0, 7) as usize]
    };
    Style {
        color,
        width: v.size.max(1) as f32,
        rounding: v.rounding.max(0) as f32,
        fill: v.fill,
        dash: v.style.clamp(0, 4) as u8,
        outline: tool != 2 || v.erase,
    }
}

fn chosen_font(r: &Registry, tool: usize) -> String {
    r.fonts
        .get(r.values[tool].font.max(0) as usize)
        .cloned()
        .unwrap_or_else(|| "Segoe UI".into())
}

fn text_from_values(r: &Registry, content: String) -> TextAnnotation {
    let v = &r.values[8];
    let colors = palette(8);
    TextAnnotation {
        content,
        font_family: chosen_font(r, 8),
        font_size: v.text_size.max(5) as f32,
        bold: v.text_bold,
        italic: v.text_italic,
        outline: v.text_outline.then(|| TextOutline {
            color: palette_color(
                colors,
                v.text_outline_color_index,
                &v.text_outline_custom_color,
            ),
            width: v.text_outline_width.clamp(1, 20) as f32,
        }),
        background: v.text_background.then(|| TextBackground {
            color: palette_color(colors, v.text_bg_color_index, &v.text_bg_custom_color),
            opacity: (v.text_bg_opacity.clamp(0, 100) as f32 * 2.55).round() as u8,
            rounding: v.text_bg_rounding.max(0) as f32,
            padding: v.text_bg_padding.max(0) as f32,
        }),
        rotation: 0.,
        layout_scale: 1.,
        linked_arrow_control: None,
        linked_arrow: None,
    }
}

fn watermark_from_values(r: &Registry, template: String, content: String) -> Watermark {
    Watermark {
        template,
        content,
        font_family: chosen_font(r, 9),
        font_size: r.values[9].text_size.clamp(5, 144) as f32,
        opacity: r.values[9].strength.round().clamp(0., 100.) as u8,
        color: current_style(r, 9).color,
        position: match r.values[9].mode {
            1 => WatermarkPosition::BottomRight,
            2 => WatermarkPosition::BottomLeft,
            3 => WatermarkPosition::TopRight,
            4 => WatermarkPosition::TopLeft,
            5 => WatermarkPosition::TopCenter,
            6 => WatermarkPosition::BottomCenter,
            7 => WatermarkPosition::Center,
            _ => WatermarkPosition::Tile,
        },
    }
}

fn adopt_watermark_style(r: &mut Registry) {
    let Some(watermark) = &r.session.watermark else {
        r.values[9].text = "".into();
        if let Some(panel) = &r.panel
            && panel.get_tool() == 9
            && !panel.get_menu()
        {
            panel.set_values(r.values[9].clone());
        }
        return;
    };
    let values = &mut r.values[9];
    values.text = watermark.template.clone().into();
    values.text_size = watermark.font_size.round() as i32;
    values.strength = watermark.opacity as f32;
    values.mode = match watermark.position {
        WatermarkPosition::Tile => 0,
        WatermarkPosition::BottomRight => 1,
        WatermarkPosition::BottomLeft => 2,
        WatermarkPosition::TopRight => 3,
        WatermarkPosition::TopLeft => 4,
        WatermarkPosition::TopCenter => 5,
        WatermarkPosition::BottomCenter => 6,
        WatermarkPosition::Center => 7,
    };
    values.font = r
        .fonts
        .iter()
        .position(|name| name == &watermark.font_family)
        .unwrap_or(0) as i32;
    if let Some(index) = palette(9)
        .iter()
        .position(|color| color == &watermark.color)
    {
        values.color_index = index as i32;
    } else {
        values.color_index = 8;
        values.custom_color = format!(
            "#{:02X}{:02X}{:02X}",
            watermark.color[0], watermark.color[1], watermark.color[2]
        )
        .into();
    }
    if let Some(panel) = &r.panel
        && panel.get_tool() == 9
        && !panel.get_menu()
    {
        panel.set_values(values.clone());
    }
}

fn toolbar_tools(r: &Registry) -> Vec<i32> {
    let mut selected = r.selected;
    if selected[0] != 2 && r.values[selected[0] as usize].shape == 1 {
        selected[0] = 1;
    }
    selected.to_vec()
}

fn adopt_selected_style(r: &mut Registry, index: usize) -> bool {
    let object = r.session.objects[index].clone();
    let tool = object.kind.tool();
    let group = if tool == 10 {
        4
    } else if tool == 8 {
        3
    } else if tool >= 5 {
        2
    } else {
        usize::from(tool >= 3)
    };
    let palette = palette(tool);
    let values = &mut r.values[tool];
    values.size = object.style.width.round() as i32;
    values.rounding = object.style.rounding.round() as i32;
    values.shape = i32::from(matches!(
        object.kind,
        Kind::Ellipse | Kind::SpotlightEllipse | Kind::HighlightRectangle
    ));
    match &object.extra {
        Extra::Arrow {
            curved,
            head,
            start,
            end,
        } => {
            values.shape = i32::from(*curved);
            values.head = *head;
            values.line_start = endpoint_index(*start);
            values.line_end = endpoint_index(*end);
        }
        Extra::Polyline {
            curved,
            head,
            start,
            end,
        } => {
            values.shape = i32::from(*curved);
            values.head = *head;
            values.line_start = endpoint_index(*start);
            values.line_end = endpoint_index(*end);
        }
        Extra::Magnifier {
            zoom,
            ellipse,
            connector,
            erase_annotations,
            antialias,
            shadow,
            ..
        } => {
            values.shape = i32::from(*ellipse);
            values.zoom = (zoom * 100.).round() as i32;
            values.connector_style = connector_index(*connector);
            values.erase = *erase_annotations;
            values.antialias = *antialias;
            values.shadow = *shadow;
        }
        Extra::Text(text) => {
            values.text = text.content.clone().into();
            text_dimensions_to_values(values, text);
            values.font = r
                .fonts
                .iter()
                .position(|family| family == &text.font_family)
                .unwrap_or(0) as i32;
            values.text_bold = text.bold;
            values.text_italic = text.italic;
            values.text_outline = text.outline.is_some();
            values.text_background = text.background.is_some();
            if let Some(outline) = &text.outline {
                (
                    values.text_outline_color_index,
                    values.text_outline_custom_color,
                ) = color_to_values(palette, outline.color);
            }
            if let Some(background) = &text.background {
                values.text_bg_opacity = (background.opacity as f32 / 2.55).round() as i32;
                (values.text_bg_color_index, values.text_bg_custom_color) =
                    color_to_values(palette, background.color);
            }
        }
        Extra::Sequence(s) => {
            values.text_size = s.size.round() as i32;
            values.shape = s.shape as i32;
            values.format = s.format as i32;
            values.start = r.session.next_sequence;
            values.sequence_ratio = s.caption_ratio as i32;
            values.sequence_guide = s.guide;
            values.sequence_text_color = if s.caption_custom_color {
                color::hex(s.caption_color).into()
            } else {
                "".into()
            };
            values.text_outline = s.caption.outline.is_some();
            if let Some(outline) = &s.caption.outline {
                values.text_outline_width = outline.width.round() as i32;
                let (i, c) = color_to_values(palette, outline.color);
                values.text_outline_color_index = i;
                values.text_outline_custom_color = c;
            }
            values.line_end = endpoint_index(s.endpoint);
            values.font = r
                .fonts
                .iter()
                .position(|f| f == &s.caption.font_family)
                .unwrap_or(0) as i32;
        }
        Extra::None => {}
    }
    values.fill = object.style.fill;
    values.style = object.style.dash as i32;
    if tool != 7 {
        values.erase = object.style.outline;
    }
    values.strength = r.session.spotlight_opacity * 100.;
    if let Some(i) = palette
        .iter()
        .position(|color| color == &object.style.color)
    {
        values.color_index = i as i32;
    } else {
        values.color_index = 8;
        values.custom_color = format!(
            "#{:02X}{:02X}{:02X}",
            object.style.color[0], object.style.color[1], object.style.color[2]
        )
        .into();
    }
    r.selected[group] = tool as i32;
    r.set_mode(InteractionMode::Tool(group));
    if let Some(main) = &r.main {
        main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
    }
    if let Some(panel) = &r.panel
        && !panel.get_menu()
        && panel.get_tool() == tool as i32
    {
        panel.set_values(r.values[tool].clone());
        let [red, green, blue] = object.style.color;
        panel.set_custom_preview(slint::Color::from_rgb_u8(red, green, blue));
    }
    !r.panel
        .as_ref()
        .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == tool as i32)
}

fn parse_hex_color(input: &str) -> Option<[u8; 3]> {
    color::parse_hex(input)
}

fn endpoint(index: i32) -> Endpoint {
    match index {
        1 => Endpoint::OpenArrow,
        2 => Endpoint::Arrow,
        3 => Endpoint::FilledArrow,
        4 => Endpoint::Circle,
        5 => Endpoint::OpenCircle,
        6 => Endpoint::Diamond,
        7 => Endpoint::OpenDiamond,
        8 => Endpoint::Bar,
        _ => Endpoint::None,
    }
}

fn endpoint_index(value: Endpoint) -> i32 {
    match value {
        Endpoint::None => 0,
        Endpoint::OpenArrow => 1,
        Endpoint::Arrow => 2,
        Endpoint::FilledArrow => 3,
        Endpoint::Circle => 4,
        Endpoint::OpenCircle => 5,
        Endpoint::Diamond => 6,
        Endpoint::OpenDiamond => 7,
        Endpoint::Bar => 8,
    }
}

fn magnifier_connector(index: i32) -> MagnifierConnector {
    match index {
        1 => MagnifierConnector::Dot,
        2 => MagnifierConnector::Frame,
        3 => MagnifierConnector::None,
        _ => MagnifierConnector::Plain,
    }
}

fn connector_index(value: MagnifierConnector) -> i32 {
    match value {
        MagnifierConnector::Plain => 0,
        MagnifierConnector::Dot => 1,
        MagnifierConnector::Frame => 2,
        MagnifierConnector::None => 3,
    }
}

fn active_kind(r: &Registry) -> Option<Kind> {
    if r.mode == InteractionMode::Tool(4) {
        return Some(Kind::Sequence);
    }
    if r.mode == InteractionMode::Tool(3) {
        return (r.selected[3] == 8).then_some(Kind::Text);
    }
    if r.mode == InteractionMode::Tool(1) {
        return Some(if r.selected[1] == 3 {
            Kind::Pencil
        } else if r.values[4].shape == 0 {
            Kind::HighlightLine
        } else {
            Kind::HighlightRectangle
        });
    }
    if r.mode == InteractionMode::Tool(2) {
        return Some(match r.selected[2] {
            5 => Kind::Arrow,
            6 => Kind::Polyline,
            _ => Kind::Magnifier,
        });
    }
    match r.selected[0] {
        0 | 1 => Some(if r.values[r.selected[0] as usize].shape == 0 {
            Kind::Rectangle
        } else {
            Kind::Ellipse
        }),
        2 => Some(if r.values[2].shape == 0 {
            Kind::SpotlightRectangle
        } else {
            Kind::SpotlightEllipse
        }),
        _ => None,
    }
}

fn sync_sequence_counter(r: &mut Registry) {
    r.values[10].start = r.session.next_sequence.max(1);
    let format = match r.values[10].format {
        1 => SequenceFormat::Roman,
        2 => SequenceFormat::LowerAlpha,
        3 => SequenceFormat::UpperAlpha,
        4 => SequenceFormat::Chinese,
        _ => SequenceFormat::Decimal,
    };
    r.values[10].sequence_label =
        lexift_core::domain::annotation::sequence_label(r.values[10].start, format).into();
    if let Some(panel) = &r.panel
        && panel.get_tool() == 10
    {
        let mut v = panel.get_values();
        v.start = r.values[10].start;
        v.sequence_label = r.values[10].sequence_label.clone();
        panel.set_values(v);
    }
}
fn update_sequence_style(
    object: &mut Object,
    v: &AnnotationValues,
    fonts: &[String],
    style: Style,
) {
    let Extra::Sequence(s) = &mut object.extra else {
        return;
    };
    s.size = v.text_size.max(5) as f32;
    s.shape = match v.shape {
        0 => SequenceShape::Circle,
        1 => SequenceShape::CircleWhiteBorder,
        2 => SequenceShape::Outline,
        3 => SequenceShape::Plain,
        _ => SequenceShape::Square,
    };
    s.format = match v.format {
        1 => SequenceFormat::Roman,
        2 => SequenceFormat::LowerAlpha,
        3 => SequenceFormat::UpperAlpha,
        4 => SequenceFormat::Chinese,
        _ => SequenceFormat::Decimal,
    };
    s.caption.font_family = fonts
        .get(v.font.max(0) as usize)
        .cloned()
        .unwrap_or_else(|| "Microsoft YaHei".into());
    s.caption.outline = v.text_outline.then(|| TextOutline {
        width: v.text_outline_width.max(1) as f32,
        color: palette_color(
            palette(10),
            v.text_outline_color_index,
            &v.text_outline_custom_color,
        ),
    });
    s.caption_ratio = v.sequence_ratio.clamp(0, 4) as usize;
    s.guide = v.sequence_guide;
    s.caption_color = color::parse_hex(&v.sequence_text_color).unwrap_or(style.color);
    s.caption_custom_color = color::parse_hex(&v.sequence_text_color).is_some();
    s.endpoint = endpoint(v.line_end);
    object.style = style;
    crate::annotation_sequence::layout(object);
}
fn sequence_from_values(r: &Registry, at: (f32, f32), scale: f32) -> Object {
    let v = &r.values[10];
    let mut caption = text_from_values(r, String::new());
    caption.outline = None;
    caption.background = None;
    caption.bold = false;
    caption.italic = false;
    let b = Bounds::from_corners(at, at);
    let mut object = Object {
        kind: Kind::Sequence,
        bounds: b,
        style: current_style(r, 10),
        points: Vec::new(),
        extra: Extra::Sequence(SequenceAnnotation {
            value: r.session.next_sequence.max(1),
            format: SequenceFormat::Decimal,
            shape: SequenceShape::Square,
            size: 16.,
            scale,
            caption,
            caption_bounds: b,
            caption_color: current_style(r, 10).color,
            caption_custom_color: false,
            caption_ratio: 2,
            guide: true,
            endpoint: Endpoint::FilledArrow,
        }),
    };
    update_sequence_style(&mut object, v, &r.fonts, current_style(r, 10));
    object
}

/// The press snapshot includes selection and numbering; no undo entry exists until release.
fn begin_sequence_placement(r: &mut Registry, point: (f32, f32), scale: f32) {
    let object = sequence_from_values(r, point, scale);
    r.session.begin_drag();
    if !r.session.add(object.clone()) {
        r.session.cancel_drag();
        return;
    }
    let index = r.session.objects.len() - 1;
    r.session.selected = None;
    r.gesture_object = Some(object);
    r.gesture = Some(Gesture::PlaceSequence {
        start: point,
        index,
    });
    sync_sequence_counter(r);
}

fn update_sequence_placement(r: &mut Registry, point: (f32, f32)) {
    let Some(Gesture::PlaceSequence { start, index }) = r.gesture else {
        return;
    };
    if let Some(mut object) = r.gesture_object.clone() {
        object.move_by(point.0 - start.0, point.1 - start.1);
        r.session.objects[index] = object;
    }
}

fn cancel_sequence_placement(r: &mut Registry) -> bool {
    if !matches!(r.gesture, Some(Gesture::PlaceSequence { .. })) {
        return false;
    }
    r.session.cancel_drag();
    r.gesture = None;
    r.gesture_object = None;
    sync_sequence_counter(r);
    true
}

/// Integer toolbar values must not quantize scaled geometry on unrelated edits.
fn preserve_text_dimensions(
    updated: &mut TextAnnotation,
    original: &TextAnnotation,
    before: &AnnotationValues,
    after: &AnnotationValues,
) {
    if before.text_size == after.text_size {
        updated.font_size = original.font_size;
    }
    if let (Some(next), Some(old)) = (&mut updated.background, &original.background) {
        if before.text_bg_padding == after.text_bg_padding {
            next.padding = old.padding;
        }
        if before.text_bg_rounding == after.text_bg_rounding {
            next.rounding = old.rounding;
        }
    }
    if let (Some(next), Some(old)) = (&mut updated.outline, &original.outline)
        && before.text_outline_width == after.text_outline_width
    {
        next.width = old.width;
    }
}

fn update_geometry_values(generation: u64, tool: i32, values: AnnotationValues) {
    let edit = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        if !r
            .panel
            .as_ref()
            .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == tool)
        {
            return None;
        }
        if r.values[tool as usize] == values {
            return None;
        }
        if !r.style_edit_active {
            r.session.begin_drag();
            r.style_edit_active = true;
        }
        r.style_edit_revision = r.style_edit_revision.wrapping_add(1);
        let previous_values = r.values[tool as usize].clone();
        r.values[tool as usize] = values;
        if let Some(main) = &r.main {
            main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
        }
        let style = current_style(r, tool);
        if let Some(panel) = &r.panel {
            panel.set_custom_preview(slint::Color::from_rgb_u8(
                style.color[0],
                style.color[1],
                style.color[2],
            ));
        }
        let kind = active_kind(r);
        let edited = r.values[tool as usize].clone();
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
        if let Some(next) = &text_style
            && let Some(edit) = r.text_edit.as_mut()
        {
            let mut updated = next.clone();
            updated.content = edit.original.content.clone();
            updated.rotation = edit.original.rotation;
            updated.linked_arrow = edit.original.linked_arrow;
            updated.linked_arrow_control = edit.original.linked_arrow_control;
            updated.layout_scale = edit.original.layout_scale;
            preserve_text_dimensions(&mut updated, &edit.original, &previous_values, &edited);
            edit.original = updated;
            if let Some(editor) = &r.text_editor {
                editor.set_text_size(edit.original.font_size);
                editor.set_font_name(edit.original.font_family.clone().into());
                editor.set_bold(edit.original.bold);
                editor.set_italic(edit.original.italic);
                editor.set_text_padding(crate::annotation_text::text_padding(&edit.original, 1.));
                editor.set_ink(slint::Color::from_rgb_u8(
                    style.color[0],
                    style.color[1],
                    style.color[2],
                ));
                let mut preview = edit.original.clone();
                preview.content = editor_preview(editor).0;
                let bounds = crate::annotation_text::text_bounds(edit.at, &preview, edit.scale);
                edit.bounds = bounds;
                edit.laid_out_content = preview.content.clone();
                place_text_editor(editor, bounds, edit.scale, &preview, &r.canvases);
            }
        }
        let watermark = if tool == 9 {
            r.session.watermark.as_ref().map(|current| {
                watermark_from_values(r, current.template.clone(), current.content.clone())
            })
        } else {
            None
        };
        if tool == 2 {
            r.session.set_spotlight_opacity(r.values[2].strength / 100.);
        }
        if tool == 10 {
            if edited.start != previous_values.start {
                r.session.next_sequence = edited.start.max(1);
            }
            let fonts = r.fonts.clone();
            r.session.update_selected(|object| {
                if object.kind == Kind::Sequence {
                    update_sequence_style(object, &edited, &fonts, style);
                }
            });
            if let Some(index) = r.text_edit.as_ref().and_then(|e| e.index)
                && let Some(object) = r.session.objects.get(index)
                && let Extra::Sequence(s) = &object.extra
                && let (Some(edit), Some(editor)) = (&mut r.text_edit, &r.text_editor)
            {
                edit.original = s.caption.clone();
                editor.set_text_size(s.caption.font_size);
                editor.set_font_name(s.caption.font_family.clone().into());
                editor.set_ink(slint::Color::from_rgb_u8(
                    s.caption_color[0],
                    s.caption_color[1],
                    s.caption_color[2],
                ));
                let mut preview = object.clone();
                if let Extra::Sequence(s) = &mut preview.extra {
                    s.caption.content = editor_preview(editor).0;
                }
                crate::annotation_sequence::layout(&mut preview);
                let caption = preview.sequence_caption().unwrap();
                let Extra::Text(text) = &caption.extra else {
                    unreachable!()
                };
                edit.bounds = caption.bounds;
                edit.at = (caption.bounds.left, caption.bounds.top);
                edit.laid_out_content = text.content.clone();
                place_text_editor(editor, caption.bounds, edit.scale, text, &r.canvases);
            }
        } else if let Some(kind) = kind {
            r.session.update_selected(|object| {
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
                    object.kind = kind;
                    object.style = style;
                    match kind {
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
                                *erase_annotations = edited.erase;
                                *antialias = edited.antialias;
                                *shadow = edited.shadow;
                            }
                        }
                        Kind::Text => {
                            if let (Extra::Text(existing), Some(updated)) =
                                (&mut object.extra, &text_style)
                            {
                                let content = existing.content.clone();
                                let rotation = existing.rotation;
                                let arrow = existing.linked_arrow;
                                let arrow_control = existing.linked_arrow_control;
                                let layout_scale = existing.layout_scale;
                                let old_text = existing.clone();
                                *existing = updated.clone();
                                existing.content = content;
                                existing.rotation = rotation;
                                existing.linked_arrow = arrow;
                                existing.linked_arrow_control = arrow_control;
                                existing.layout_scale = layout_scale;
                                preserve_text_dimensions(
                                    existing,
                                    &old_text,
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
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
        if tool == 10 {
            sync_sequence_counter(r);
        }
        position_panel(r);
        Some((r.generation, r.style_edit_revision))
    });
    if let Some((generation, revision)) = edit {
        schedule_render(generation);
        slint::Timer::single_shot(Duration::from_millis(300), move || {
            REGISTRY.with(|s| {
                if let Some(r) = s.borrow_mut().as_mut().filter(|r| {
                    r.generation == generation
                        && r.style_edit_revision == revision
                        && !choice::color_dragging(r)
                        && !r.parameter_dragging
                }) {
                    r.finish_style_edit();
                }
            });
        });
    }
}

fn update_draft(
    r: &mut Registry,
    start: (f32, f32),
    point: (f32, f32),
    kind: Kind,
    style: Style,
    scale: f32,
) {
    if kind == Kind::Pencil {
        let object = r.draft.get_or_insert_with(|| Object {
            bounds: Bounds::from_corners(start, start),
            kind,
            style,
            points: vec![start],
            extra: Extra::None,
        });
        object.append_point(point, 0.5 * scale);
    } else if kind == Kind::Arrow {
        r.draft = Some(arrow_draft(start, point, style, &r.values[5]));
    } else {
        let tool = kind.tool();
        let values = &r.values[tool];
        let bounds = Bounds::from_corners(start, point);
        let extra = match kind {
            // The drag previews only the source outline. Sampling starts on release.
            Kind::Magnifier => Extra::None,
            _ => Extra::None,
        };
        let points = match kind {
            Kind::HighlightLine => vec![start, point],
            _ => vec![],
        };
        let draft_kind = if kind == Kind::Magnifier {
            if values.shape == 1 {
                Kind::Ellipse
            } else {
                Kind::Rectangle
            }
        } else {
            kind
        };
        let mut draft_style = style;
        if kind == Kind::Magnifier {
            draft_style.rounding = 0.;
        }
        r.draft = Some(Object {
            bounds,
            kind: draft_kind,
            style: draft_style,
            points,
            extra,
        });
    }
}

fn arrow_draft(
    start: (f32, f32),
    end: (f32, f32),
    style: Style,
    values: &AnnotationValues,
) -> Object {
    Object {
        bounds: Bounds::from_corners(start, end),
        kind: Kind::Arrow,
        style,
        points: vec![start, end],
        extra: Extra::Arrow {
            curved: false,
            head: values.head,
            start: endpoint(values.line_start),
            end: endpoint(values.line_end),
        },
    }
}

fn arrow_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn arrow_endpoint_valid(start: (f32, f32), end: (f32, f32), scale: f32) -> bool {
    arrow_distance(start, end) >= 3. * scale
}

fn arrow_drag_commits(start: (f32, f32), end: (f32, f32), max_distance: f32, scale: f32) -> bool {
    max_distance.max(arrow_distance(start, end)) > 3. * scale
        && arrow_endpoint_valid(start, end, scale)
}

/// Adds a tentative endpoint only after the pointer leaves the confirmed node.
fn polyline_preview(polyline: &Object, point: (f32, f32), scale: f32) -> Object {
    let mut draft = polyline.clone();
    if polyline
        .points
        .last()
        .is_some_and(|last| (point.0 - last.0).hypot(point.1 - last.1) > 2. * scale)
    {
        draft.points.push(point);
        draft.refresh_bounds();
    }
    draft
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
        bounds: source,
        kind: Kind::Magnifier,
        style,
        points: Vec::new(),
        extra: Extra::Magnifier {
            output,
            zoom,
            ellipse: values.shape == 1,
            connector: magnifier_connector(values.connector_style),
            erase_annotations: values.erase,
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
    if let Some(editor) = &r.text_editor {
        editor.invoke_place_selection(anchor.unwrap_or(offset) as i32, offset as i32);
    }
    offset
}

/// Restore the existing edit only after its originating toolbar interaction completes.
fn restore_text_focus(r: &Registry) {
    let (Some(edit), Some(_)) = (&r.text_edit, &r.text_editor) else {
        return;
    };
    let (generation, id, panel_revision, choice_revision) =
        (r.generation, edit.id, r.panel_revision, r.choice_revision);
    next_frame(move || {
        if restore_matching_text_focus(generation, id, panel_revision, choice_revision) {
            schedule_render(generation);
        }
    });
}

fn restore_matching_text_focus(
    generation: u64,
    id: u64,
    panel_revision: u64,
    choice_revision: u64,
) -> bool {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && text_edit_matches(r, id)
                && r.panel_revision == panel_revision
                && r.choice_revision == choice_revision
                && r.choice.is_none()
                && !r.parameter_dragging
                && !r
                    .panel
                    .as_ref()
                    .is_some_and(|p| p.get_parameter_input_active())
        }) else {
            return false;
        };
        let Some(editor) = &r.text_editor else {
            return false;
        };
        editor.invoke_focus_input();
        (r.lifecycle.activate_user_requested_window)(editor.window());
        reset_text_caret(r);
        true
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
    let center = object.edit_handles(scale)[handle];
    r.gesture = Some(Gesture::TextHandle {
        start: point,
        grab: (point.0 - center.0, point.1 - center.1),
        handle,
        index: if editing {
            r.text_edit.as_ref().and_then(|edit| edit.index)
        } else {
            r.session.selected
        },
        editing,
        active: false,
        scale,
    });
    r.gesture_object = Some(object);
}

/// Preserve the grab offset; a control click must not change geometry.
fn update_text_handle(r: &mut Registry, point: (f32, f32)) {
    let Some(Gesture::TextHandle {
        start,
        grab,
        handle,
        index,
        editing,
        active,
        scale,
    }) = r.gesture
    else {
        return;
    };
    if matches!(handle, 1 | 2) {
        return;
    }
    let Some(target) = text_drag_target(start, grab, point, scale, active) else {
        return;
    };
    if !active && !editing {
        r.session.begin_drag();
    }
    r.gesture = Some(Gesture::TextHandle {
        start,
        grab,
        handle,
        index,
        editing,
        active: true,
        scale,
    });
    let Some(mut object) = r.gesture_object.clone() else {
        return;
    };
    object.edit_handle(handle, target, scale);
    if editing {
        apply_editing_object(r, object);
    } else if let Some(index) = index {
        r.session.objects[index] = object;
    }
}

fn text_drag_target(
    start: (f32, f32),
    grab: (f32, f32),
    point: (f32, f32),
    scale: f32,
    active: bool,
) -> Option<(f32, f32)> {
    (active || (point.0 - start.0).hypot(point.1 - start.1) > 3. * scale)
        .then_some((point.0 - grab.0, point.1 - grab.1))
}

fn apply_editing_object(r: &mut Registry, object: Object) {
    let Extra::Text(mut text) = object.extra else {
        return;
    };
    let (Some(edit), Some(editor)) = (&mut r.text_edit, &r.text_editor) else {
        return;
    };
    edit.bounds = object.bounds;
    edit.at = (object.bounds.left, object.bounds.top);
    edit.laid_out_content = text.content.clone();
    place_text_editor(editor, object.bounds, edit.scale, &text, &r.canvases);
    text.content = editor.get_value().to_string();
    edit.original = text;
}

fn text_dimensions_to_values(values: &mut AnnotationValues, text: &TextAnnotation) {
    values.text_size = text.font_size.round() as i32;
    if let Some(background) = &text.background {
        values.text_bg_padding = background.padding.round() as i32;
        values.text_bg_rounding = background.rounding.round() as i32;
    }
    if let Some(outline) = &text.outline {
        values.text_outline_width = outline.width.round() as i32;
    }
}

fn finish_text_handle(r: &mut Registry, point: (f32, f32)) {
    let Some(Gesture::TextHandle {
        handle,
        editing,
        index,
        active,
        scale,
        ..
    }) = r.gesture
    else {
        return;
    };
    if editing
        && matches!(handle, 1 | 2)
        && let Some(mut object) = editing_text_object(r)
        && text_handle_at(&object, point, scale) == Some(handle)
    {
        if handle == 1 {
            finish_text_edit_registry(r, false);
            if let Some(index) = index {
                r.session.selected = Some(index);
                r.session.delete_selected();
            }
        } else {
            object.toggle_text_arrow(scale);
            apply_editing_object(r, object);
        }
    }
    if !editing && active {
        r.session.finish_drag();
        if let Some(index) = index {
            adopt_selected_style(r, index);
        }
    }
    if editing
        && active
        && let Some(edit) = &r.text_edit
    {
        text_dimensions_to_values(&mut r.values[8], &edit.original);
        if let Some(panel) = &r.panel
            && panel.get_tool() == 8
        {
            panel.set_values(r.values[8].clone());
        }
    }
    r.gesture = None;
    r.gesture_object = None;
    if editing {
        restore_text_focus(r);
    }
    cursor::refresh(r);
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
        if event == 4 {
            r.canvases[index].last_pointer = None;
            if r.polyline.is_some() {
                r.draft = r.polyline.clone();
            }
            if r.hovered.take().is_some() {
                return Some((r.generation, false));
            }
            return None;
        }
        r.canvases[index].last_pointer = Some(point);
        let mut open_geometry_panel = false;
        match event {
            0 => {
                if r.text_editor.is_some() {
                    if let Some(object) = editing_text_object(r) {
                        if !editing_sequence(r)
                            && let Some(handle) = text_handle_at(&object, point, scale)
                        {
                            begin_text_handle(r, object, handle, point, scale, true);
                            return Some((r.generation, false));
                        }
                        if editing_body_contains(&object, point) {
                            let anchor = select_editing_text(r, point, None);
                            r.gesture = Some(Gesture::TextSelection { anchor });
                            restore_text_focus(r);
                            return Some((r.generation, false));
                        }
                    }
                    let empty = r
                        .text_editor
                        .as_ref()
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
                r.hovered = None;
                r.finish_style_edit();
                r.gesture_object = None;
                // A pending arrow may end over another object; finish it before hit testing.
                if r.arrow_pending.is_some() {
                    r.gesture = Some(Gesture::FinishArrow);
                } else if r.polyline.is_some()
                    || (r.mode == InteractionMode::Tool(2)
                        && r.selected[2] == 6
                        && r.session.hit(point, scale).is_none())
                {
                    r.session.selected = None;
                    r.gesture = Some(Gesture::PlacePoint);
                } else {
                    match r.session.hit(point, scale) {
                        Some(Hit::Handle(handle)) => {
                            let index = r.session.selected.unwrap();
                            if r.session.objects[index].kind == Kind::Sequence && handle < 5 {
                                r.gesture = Some(Gesture::SequenceControl { index, handle });
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
                                r.gesture = Some(Gesture::ToggleTextArrow { index });
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 1 {
                                r.session.delete_selected();
                                r.gesture = Some(Gesture::DeleteText);
                                return Some((r.generation, false));
                            }
                            let initial = r.session.objects[index].bounds;
                            r.gesture_object = Some(r.session.objects[index].clone());
                            r.session.begin_drag();
                            r.gesture = Some(Gesture::Resize {
                                initial,
                                index,
                                handle,
                            });
                        }
                        Some(Hit::Object(index)) => {
                            r.session.selected = Some(index);
                            r.pending_edit_panel = adopt_selected_style(r, index);
                            let initial = r.session.objects[index].bounds;
                            r.gesture_object = Some(r.session.objects[index].clone());
                            r.session.begin_drag();
                            r.gesture = Some(Gesture::Move {
                                start: point,
                                initial,
                                index,
                            });
                        }
                        None => {
                            r.pending_edit_panel = false;
                            if r.mode == InteractionMode::Tool(4) {
                                begin_sequence_placement(r, point, scale);
                                cursor::refresh(r);
                                return Some((r.generation, false));
                            }
                            r.session.selected = None;
                            if r.mode == InteractionMode::Tool(3) && r.selected[3] == 8 {
                                r.gesture = Some(Gesture::CreateText { at: point, scale });
                            } else if matches!(r.mode, InteractionMode::Tool(0..=2))
                                && let Some(kind) = active_kind(r)
                            {
                                if kind == Kind::Magnifier
                                    && matches!(r.magnifier_sync, MagnifierSync::Pending(_))
                                    && r.draft.is_none()
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
                                r.gesture = Some(Gesture::Draw {
                                    start: point,
                                    kind,
                                    style: current_style(
                                        r,
                                        r.selected[r.mode.selected_group() as usize],
                                    ),
                                    start_scale: scale,
                                    max_distance: 0.,
                                });
                            }
                        }
                    }
                }
            }
            1 => match r.gesture {
                Some(Gesture::PlaceSequence { .. }) => update_sequence_placement(r, point),
                Some(Gesture::SequenceControl { .. }) => {}
                Some(Gesture::DeleteText | Gesture::ToggleTextArrow { .. }) => {}
                Some(Gesture::CreateText { .. }) => {}
                Some(Gesture::PlacePoint) => {
                    if let Some(polyline) = &r.polyline {
                        r.draft = Some(polyline_preview(polyline, point, scale));
                    }
                }
                Some(Gesture::FinishArrow) => {
                    if let Some(pending) = &r.arrow_pending {
                        r.draft = Some(pending.preview(point));
                    }
                }
                Some(Gesture::Draw {
                    start,
                    kind,
                    style,
                    start_scale,
                    max_distance,
                }) => {
                    if kind == Kind::Arrow {
                        r.gesture = Some(Gesture::Draw {
                            start,
                            kind,
                            style,
                            start_scale,
                            max_distance: max_distance.max(arrow_distance(start, point)),
                        });
                    }
                    if kind == Kind::Magnifier
                        && r.magnifier_sync == MagnifierSync::Cold
                        && !r.first_magnifier_measured
                        && r.first_magnifier_timing.is_none()
                    {
                        r.first_magnifier_timing = Some(FirstMagnifierTiming::new());
                    }
                    update_draft(r, start, point, kind, style, scale);
                }
                Some(Gesture::Move {
                    start,
                    initial,
                    index,
                }) => {
                    let _ = initial;
                    if let Some(mut object) = r.gesture_object.clone() {
                        object.move_from(start, point.0 - start.0, point.1 - start.1);
                        r.session.objects[index] = object;
                    }
                }
                Some(Gesture::Resize {
                    initial,
                    index,
                    handle,
                }) => {
                    let _ = initial;
                    if let Some(mut object) = r.gesture_object.clone() {
                        object.edit_handle(handle, point, scale);
                        r.session.objects[index] = object;
                    }
                }
                Some(Gesture::TextHandle { .. }) => {
                    update_text_handle(r, point);
                }
                Some(Gesture::TextSelection { anchor }) => {
                    select_editing_text(r, point, Some(anchor));
                }
                None => {
                    if let Some(pending) = &r.arrow_pending {
                        r.draft = Some(pending.preview(point));
                        return Some((r.generation, false));
                    }
                    if let Some(polyline) = &r.polyline {
                        r.draft = Some(polyline_preview(polyline, point, scale));
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
                if let Some(Gesture::PlaceSequence { index, .. }) = r.gesture {
                    update_sequence_placement(r, point);
                    r.session.selected = Some(index);
                    r.session.finish_drag();
                    r.gesture = None;
                    r.gesture_object = None;
                    if let Some(main) = &r.main {
                        main.set_can_undo(r.session.can_undo());
                    }
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::SequenceControl { index, handle }) = r.gesture {
                    r.gesture = None;
                    if r.session.selected == Some(index)
                        && r.session.hit(point, scale) == Some(Hit::Handle(handle))
                    {
                        if handle == 4 {
                            text_to_open = Some((point, Some(index), scale));
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
                    if let Some(main) = &r.main {
                        main.set_can_undo(r.session.can_undo());
                    }
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::TextSelection { anchor }) = r.gesture {
                    select_editing_text(r, point, Some(anchor));
                    r.gesture = None;
                    restore_text_focus(r);
                    return Some((r.generation, false));
                }
                if matches!(r.gesture, Some(Gesture::TextHandle { .. })) {
                    update_text_handle(r, point);
                    finish_text_handle(r, point);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::ToggleTextArrow { index }) = r.gesture {
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
                if matches!(r.gesture, Some(Gesture::DeleteText)) {
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                if let Some(Gesture::CreateText { at, scale }) = r.gesture.as_ref() {
                    text_to_open = Some((*at, None, *scale));
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                open_geometry_panel = std::mem::take(&mut r.pending_edit_panel);
                if matches!(r.gesture, Some(Gesture::FinishArrow)) {
                    r.gesture = None;
                    if let Some(object) = r
                        .arrow_pending
                        .as_ref()
                        .and_then(|pending| pending.finish(point))
                    {
                        r.session.add(object);
                        r.arrow_pending = None;
                    }
                    r.draft = None;
                } else if matches!(r.gesture, Some(Gesture::PlacePoint)) {
                    r.gesture = None;
                    let double_click = r.last_polyline_click.is_some_and(|(time, last)| {
                        time.elapsed() <= Duration::from_millis(350)
                            && (point.0 - last.0).hypot(point.1 - last.1) <= 6. * scale
                    });
                    if double_click {
                        if let Some(object) = r.polyline.take()
                            && object.points.len() >= if object.style.fill { 3 } else { 2 }
                        {
                            r.session.add(object);
                        }
                        r.draft = None;
                        r.last_polyline_click = None;
                    } else {
                        if r.polyline.is_none() {
                            let values = &r.values[6];
                            r.polyline = Some(Object {
                                bounds: Bounds::from_corners(point, point),
                                kind: Kind::Polyline,
                                style: current_style(r, 6),
                                points: Vec::new(),
                                extra: Extra::Polyline {
                                    curved: values.shape == 1,
                                    head: values.head,
                                    start: endpoint(values.line_start),
                                    end: endpoint(values.line_end),
                                },
                            });
                        }
                        if let Some(polyline) = r.polyline.as_mut() {
                            polyline.points.push(point);
                            polyline.refresh_bounds();
                            r.draft = Some(polyline.clone());
                        }
                        r.last_polyline_click = Some((Instant::now(), point));
                    }
                } else if matches!(r.gesture, Some(Gesture::Draw { .. })) {
                    let Some(Gesture::Draw {
                        start,
                        kind,
                        style,
                        start_scale,
                        max_distance,
                    }) = r.gesture.take()
                    else {
                        unreachable!()
                    };
                    if kind == Kind::Arrow {
                        if arrow_drag_commits(start, point, max_distance, start_scale) {
                            r.session
                                .add(arrow_draft(start, point, style, &r.values[5]));
                            r.draft = None;
                        } else {
                            r.arrow_pending = Some(ArrowPending {
                                start,
                                style,
                                start_scale,
                                values: r.values[5].clone(),
                            });
                            r.draft = None;
                        }
                    } else if kind == Kind::Magnifier {
                        update_draft(r, start, point, kind, style, scale);
                        let _ = r.draft.take();
                        if r.magnifier_sync == MagnifierSync::Cold
                            && !r.first_magnifier_measured
                            && r.first_magnifier_timing.is_none()
                        {
                            r.first_magnifier_timing = Some(FirstMagnifierTiming::new());
                        }
                        if let Some(object) = magnifier_from_source(
                            Bounds::from_corners(start, point),
                            style,
                            &r.values[7],
                        ) {
                            r.session.add(object);
                        }
                    } else {
                        update_draft(r, start, point, kind, style, scale);
                        if let Some(object) = r.draft.take() {
                            r.session.add(object);
                        }
                    }
                } else {
                    if let Some(Gesture::Move { start, index, .. }) = r.gesture
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
                        text_to_open = Some((point, Some(index), scale));
                        open_geometry_panel = false;
                    }
                    r.session.finish_drag();
                    r.gesture = None;
                    if let Some(index) = r.session.selected {
                        adopt_selected_style(r, index);
                    }
                }
                r.gesture_object = None;
            }
            3 => {
                cancel_sequence_placement(r);
                if matches!(r.gesture, Some(Gesture::TextHandle { editing: true, .. }))
                    && let Some(object) = r.gesture_object.take()
                {
                    apply_editing_object(r, object);
                }
                r.hovered = None;
                r.pending_edit_panel = false;
                r.session.cancel_drag();
                r.gesture = None;
                r.draft = None;
                r.arrow_pending = None;
                r.polyline = None;
            }
            _ => return None,
        }
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
        cursor::refresh(r);
        Some((r.generation, open_geometry_panel))
    });
    if let Some((generation, open)) = result {
        schedule_render(generation);
        if let Some((point, index, scale)) = text_to_open {
            open_text_editor(point, index, scale);
        }
        if open {
            later(|| {
                let group = REGISTRY.with(|s| {
                    s.borrow()
                        .as_ref()
                        .map(|r| r.mode.selected_group().max(0) as usize)
                });
                if let Some(group) = group {
                    open_panel(group, false);
                }
            });
        }
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
    let should_schedule = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return false;
        };
        cursor::refresh(r);
        if r.render_queued {
            false
        } else {
            r.render_queued = true;
            true
        }
    });
    if should_schedule {
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            render_canvases(generation)
        });
    }
}

/// Matches TextInput's insertion semantics without adding composition to the model.
fn editor_preview(editor: &AnnotationTextInput) -> (String, std::ops::Range<usize>) {
    crate::annotation_text::composition_preview(
        &editor.get_value(),
        &editor.get_preedit(),
        editor.get_cursor_offset().max(0) as usize,
    )
}

/// Shares the editor's current geometry between painting and cursor hit testing.
fn editing_text_object(r: &Registry) -> Option<Object> {
    let (Some(edit), Some(editor)) = (&r.text_edit, &r.text_editor) else {
        return None;
    };
    let mut visible = edit.original.clone();
    visible.content = editor_preview(editor).0;
    if editor.get_native_visual() {
        visible.content.clear();
        visible.outline = None;
    }
    let mut object = edit
        .index
        .and_then(|index| r.session.objects.get(index))
        .cloned()
        .unwrap_or_else(|| Object {
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

/// Composition and committed text share glyphs; native input only paints selection.
fn visible_session(r: &Registry) -> Session {
    // Rendering needs the visible objects, never the undo snapshots.
    let mut session = Session::new();
    session.objects = r.session.objects.clone();
    session.selected = r.session.selected;
    session.watermark = r.session.watermark.clone();
    session.displays = r.session.displays.clone();
    session.spotlight_opacity = r.session.spotlight_opacity;
    let (Some(edit), Some(visible)) = (&r.text_edit, editing_text_object(r)) else {
        return session;
    };
    if let Some(index) = edit.index {
        if let Some(object) = session.objects.get_mut(index) {
            if let Extra::Sequence(s) = &mut object.extra {
                if let Extra::Text(t) = visible.extra {
                    s.caption = t;
                    s.caption_bounds = visible.bounds;
                }
            } else {
                *object = visible;
            }
            session.selected = Some(index);
        }
    } else {
        session.objects.push(visible);
        session.selected = Some(session.objects.len() - 1);
    }
    session
}

fn render_canvases(generation: u64) {
    let deferred_revision = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        r.render_queued = false;
        if r.canvases.is_empty() {
            return None;
        }
        let views = magnifier_views(&r.session, r.draft.as_ref());
        let drawing_magnifier = matches!(
            r.gesture,
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
        let display_session = visible_session(r);
        let confirmed = r
            .text_editor
            .as_ref()
            .filter(|editor| !editor.get_preedit().is_empty())
            .map(|editor| editor.get_value().to_string());
        let caret = r
            .text_editor
            .as_ref()
            .filter(|editor| !editor.get_native_visual())
            .and_then(|editor| {
                display_session.selected.map(|index| {
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
                        r.text_edit.as_ref().is_some_and(|e| e.caret.visible),
                    )
                })
            });
        let active_canvas = r
            .canvases
            .iter()
            .position(|layer| layer.last_pointer.is_some())
            .unwrap_or(0);
        let canvas_order = std::iter::once(active_canvas)
            .chain((0..r.canvases.len()).filter(|&index| index != active_canvas));
        for index in canvas_order {
            let layer = &mut r.canvases[index];
            let mut signature = render_signature(
                &display_session,
                layer.bounds,
                layer.window.window().scale_factor(),
                r.draft.as_ref(),
                r.hovered,
            );
            if let Some((_, offset, ref preedit, anchor, visible)) = caret {
                signature ^= u64::from(visible).wrapping_mul(0xa24b_aed4_963e_e407);
                signature ^= (offset as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                signature ^= (preedit.start as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
                signature ^= (preedit.len() as u64).wrapping_mul(0x94d0_49bb_1331_11eb);
                signature ^= (anchor as u64).wrapping_mul(0xd6e8_feb8_6659_fd93);
            }
            if layer.signature == Some(signature) {
                continue;
            }
            layer.signature = Some(signature);
            let raster_start = Instant::now();
            if let Some(mut frame) = crate::annotation_render::render_editing(
                &display_session,
                layer.bounds,
                layer.window.window().scale_factor(),
                r.draft.as_ref(),
                r.hovered,
                confirmed.as_deref(),
            ) {
                if let Some((index, offset, ref preedit, anchor, visible)) = caret
                    && let Some(object) = display_session.objects.get(index)
                {
                    let caption = object.sequence_caption();
                    let object = caption.as_ref().unwrap_or(object);
                    if preedit.is_empty() {
                        crate::annotation_text::draw_selection(
                            &mut frame,
                            object,
                            anchor.min(offset)..anchor.max(offset),
                            layer.bounds,
                        );
                    }
                    crate::annotation_text::draw_composition_underline(
                        &mut frame,
                        object,
                        preedit.clone(),
                        layer.bounds,
                        layer.window.window().scale_factor(),
                    );
                    if visible {
                        crate::annotation_text::draw_caret(
                            &mut frame,
                            object,
                            offset,
                            layer.bounds,
                            layer.window.window().scale_factor(),
                            confirmed.as_deref(),
                        );
                    }
                }
                if measure_first_frame && let Some(timing) = r.first_magnifier_timing.as_mut() {
                    timing.raster += raster_start.elapsed();
                }
                let present_start = Instant::now();
                (r.lifecycle.annotation_frame_presenter)(
                    layer.window.window(),
                    frame.data(),
                    frame.width(),
                    frame.height(),
                );
                if measure_first_frame && let Some(timing) = r.first_magnifier_timing.as_mut() {
                    timing.present += present_start.elapsed();
                }
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
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            sync_first_magnifier(generation, revision)
        });
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
    if let Some(main) = &r.main {
        overlay_windows.push(main.window());
    }
    if let Some(panel) = &r.panel {
        overlay_windows.push(panel.window());
    }
    if let Some(choice) = &r.choice {
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
        let views = magnifier_views(&r.session, r.draft.as_ref());
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

fn render_signature(
    session: &Session,
    monitor: Rect,
    scale: f32,
    draft: Option<&Object>,
    hovered: Option<usize>,
) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    scale.to_bits().hash(&mut hash);
    session.spotlight_opacity.to_bits().hash(&mut hash);
    format!("{:?}", session.watermark).hash(&mut hash);
    session
        .objects
        .iter()
        .chain(draft)
        .any(|object| object.kind.is_spotlight())
        .hash(&mut hash);
    for (index, object) in session.objects.iter().chain(draft).enumerate() {
        let b = object.bounds;
        let margin = (object.style.width / 2. + 8.) * scale;
        let output_visible = if matches!(object.extra, Extra::Text(_) | Extra::Sequence(_)) {
            // Rotation and a connector can be visible while the unrotated body is not.
            true
        } else if let Extra::Magnifier { output, .. } = object.extra {
            output.right + margin >= monitor.left as f32
                && output.left - margin <= monitor.right as f32
                && output.bottom + margin >= monitor.top as f32
                && output.top - margin <= monitor.bottom as f32
        } else {
            false
        };
        if !output_visible
            && (b.right + margin < monitor.left as f32
                || b.left - margin > monitor.right as f32
                || b.bottom + margin < monitor.top as f32
                || b.top - margin > monitor.bottom as f32)
        {
            continue;
        }
        index.hash(&mut hash);
        object.kind.hash(&mut hash);
        for value in [
            b.left,
            b.top,
            b.right,
            b.bottom,
            object.style.width,
            object.style.rounding,
        ] {
            value.to_bits().hash(&mut hash);
        }
        for (x, y) in &object.points {
            x.to_bits().hash(&mut hash);
            y.to_bits().hash(&mut hash);
        }
        object.style.color.hash(&mut hash);
        object.style.fill.hash(&mut hash);
        object.style.dash.hash(&mut hash);
        object.style.outline.hash(&mut hash);
        format!("{:?}", object.extra).hash(&mut hash);
    }
    if let Some(index) = session.selected {
        index.hash(&mut hash);
    }
    hovered.hash(&mut hash);
    hash.finish()
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
            r.session.cancel_drag();
            r.gesture = None;
            r.draft = None;
            r.session.selected = None;
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
        if cancel_sequence_placement(r) {
            cursor::refresh(r);
            return Some(r.generation);
        }
        if r.text_editor.is_some() {
            finish_text_edit_registry(r, true);
        }
        r.finish_style_edit();
        if !r.session.undo() {
            return None;
        }
        sync_sequence_counter(r);
        adopt_watermark_style(r);
        refresh_hovered(r);
        if matches!(r.mode, InteractionMode::Tool(0 | 1 | 4))
            && let Some(index) = r.session.selected
        {
            adopt_selected_style(r, index);
        }
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
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
        if r.text_editor.is_some() {
            finish_text_edit_registry(r, false);
        }
        r.session.delete_selected();
        refresh_hovered(r);
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

fn show_main(generation: u64, attempt: u8) {
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(main) = r.main.as_ref() else { return };
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
                    pointer_main(main.as_weak()),
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
                let weak_main = main.as_weak();
                let work_area = Rc::clone(&r.lifecycle.popup_work_area);
                sync_toolbar_owner(r);
                settle_main_position(weak_main, work_area, anchor, 2);
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
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_main(generation, attempt + 1)
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
        if r.session
            .objects
            .iter()
            .chain(r.draft.iter())
            .any(|object| object.kind == Kind::Magnifier)
        {
            (r.lifecycle.annotation_magnifier_refresh)();
        }
    });
}

fn check_display_layout() {
    let changed = REGISTRY.with(|s| {
        let slot = s.borrow();
        let Some(r) = slot.as_ref().filter(|r| r.main.is_some()) else {
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
/// Reconcile the initial anchor using the realized physical size; weak ownership cancels on close.
fn settle_main_position(
    weak: slint::Weak<AnnotationToolbar>,
    work_area: Rc<dyn Fn(Point) -> Option<Rect>>,
    anchor: Point,
    remaining: u8,
) {
    slint::Timer::single_shot(Duration::from_millis(32), move || {
        let Some(main) = weak.upgrade() else { return };
        if let Some(area) = work_area(anchor) {
            let size = main.window().size();
            let gap = (16. * main.window().scale_factor()) as i32;
            let point =
                placement::place_popup(anchor, size.width, size.height, area, gap, gap).position;
            main.window()
                .set_position(slint::PhysicalPosition::new(point.x, point.y));
        }
        if remaining > 0 {
            settle_main_position(weak, work_area, anchor, remaining - 1);
        }
    });
}

fn open_panel(group: usize, menu: bool) {
    if group >= GROUPS.len() {
        return;
    }
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.main.is_none() {
            return;
        }
        if !menu && !r.set_mode(InteractionMode::Tool(group)) {
            return;
        }
        let same = r.group == Some(group) && r.panel.as_ref().is_some_and(|p| p.get_menu() == menu);
        r.close_panel();
        if same {
            return;
        }
        let Ok(panel) = AnnotationPanel::new() else {
            return;
        };
        crate::theme::apply(&panel);
        let tool = r.selected[group];
        if matches!(tool, 8..=10) && r.fonts.is_empty() {
            r.fonts = crate::annotation_text::font_families();
            if let Some(index) = r
                .fonts
                .iter()
                .position(|name| name == "Microsoft YaHei")
                .or_else(|| r.fonts.iter().position(|name| name == "Segoe UI"))
            {
                r.values[8].font = index as i32;
                r.values[9].font = index as i32;
                r.values[10].font = index as i32;
            }
        }
        panel.set_font_families(ModelRc::new(VecModel::from(
            r.fonts.iter().cloned().map(Into::into).collect::<Vec<_>>(),
        )));
        panel.set_tool(tool);
        panel.set_menu(menu);
        panel.set_heading(crate::i18n::tr(NAMES[tool as usize]).into());
        panel.set_values(r.values[tool as usize].clone());
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
        let panel_revision = r.panel_revision;
        if !menu && group <= 4 {
            panel.on_values_changed(move |values| {
                later(move || update_geometry_values(generation, tool, values))
            });
        }
        panel.on_parameter_input_finished(move || {
            later(move || {
                REGISTRY.with(|slot| {
                    if let Some(r) = slot.borrow().as_ref().filter(|r| {
                        r.generation == generation && r.panel_revision == panel_revision
                    }) {
                        restore_text_focus(r);
                    }
                });
            });
        });
        panel.on_watermark_edit_requested(move || later(move || open_watermark_input(generation)));
        panel.on_parameter_dragging(move |active| {
            later(move || {
                REGISTRY.with(|slot| {
                    if let Some(r) = slot.borrow_mut().as_mut().filter(|r| {
                        r.generation == generation && r.panel_revision == panel_revision
                    }) {
                        if active {
                            r.finish_style_edit();
                        }
                        r.parameter_dragging = active;
                        if !active {
                            r.finish_style_edit();
                            restore_text_focus(r);
                        }
                    }
                })
            });
        });
        panel.on_selected(move |tool| {
            later(move || {
                let selected = REGISTRY.with(|s| {
                    let mut slot = s.borrow_mut();
                    let Some(r) = slot.as_mut().filter(|r| {
                        r.panel_revision == panel_revision
                            && r.group == Some(group)
                            && r.panel.as_ref().is_some_and(|panel| panel.get_menu())
                    }) else {
                        return false;
                    };
                    if !r.set_mode(InteractionMode::Tool(group)) {
                        return false;
                    }
                    r.close_panel();
                    if r.selected[group] != tool {
                        r.close_watermark_input();
                        if r.text_editor.is_some() {
                            finish_text_edit_registry(r, true);
                        }
                        r.arrow_pending = None;
                        if matches!(
                            r.gesture,
                            Some(
                                Gesture::FinishArrow
                                    | Gesture::Draw {
                                        kind: Kind::Arrow,
                                        ..
                                    }
                            )
                        ) {
                            r.gesture = None;
                        }
                        r.polyline = None;
                        r.last_polyline_click = None;
                        r.draft = None;
                    }
                    r.selected[group] = tool;
                    if let Some(main) = &r.main {
                        main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
                    }
                    true
                });
                if selected {
                    open_panel(group, false);
                }
            })
        });
        panel.on_choice_requested(move |request| {
            later(move || choice::open(generation, panel_revision, request));
        });
        panel.on_escape_requested(|| later(escape));
        panel.window().on_close_requested(|| {
            later(escape);
            slint::CloseRequestResponse::KeepWindowShown
        });
        r.group = Some(group);
        r.panel = Some(panel);
        later(move || show_panel(generation, panel_revision, 0));
    });
}

fn open_watermark_input(generation: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        r.close_watermark_input();
        let revision = r.watermark_input_revision;
        let Ok(editor) = AnnotationWatermarkInput::new() else {
            return;
        };
        crate::theme::apply(&editor);
        editor.set_value(r.values[9].text.clone());
        editor.on_presets_requested(move |x, y, width, height| {
            later(move || choice::open_watermark(generation, revision, x, y, width, height));
        });
        editor.on_dismiss_presets(move || {
            later(move || choice::dismiss_watermark(generation, revision));
        });
        editor.on_apply(move |value| {
            later(move || apply_watermark(generation, revision, value.to_string()))
        });
        editor.on_cancel(move || later(move || close_watermark_input(generation, revision)));
        editor.window().on_close_requested(move || {
            later(move || close_watermark_input(generation, revision));
            slint::CloseRequestResponse::KeepWindowShown
        });
        let Some(main) = &r.main else { return };
        let origin = main.window().position();
        let anchor = Point {
            x: origin.x,
            y: origin.y,
        };
        let position = if let Some(area) = (r.lifecycle.popup_work_area)(anchor) {
            let x = origin.x.clamp(area.left, area.right - 440);
            let above = origin.y - 285;
            let y = if above >= area.top {
                above
            } else {
                (origin.y + 65).min(area.bottom - 270)
            };
            (x, y)
        } else {
            (origin.x, origin.y - 285)
        };
        editor
            .window()
            .set_position(slint::PhysicalPosition::new(position.0, position.1));
        r.watermark_input = Some(editor);
        later(move || show_watermark_input(generation, revision, 0));
    });
}

fn show_watermark_input(generation: u64, revision: u64, attempt: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.watermark_input_revision == revision)
        else {
            return;
        };
        let Some(editor) = &r.watermark_input else {
            return;
        };
        match (r.prepare)(editor.window()) {
            PassiveWindowPreparation::Ready => {
                let owner_ready = r.main.as_ref().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(editor.window(), main.window())
                });
                if !owner_ready
                    || editor.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        editor.window(),
                        pointer_watermark(editor.as_weak()),
                    )
                {
                    r.close_watermark_input();
                    return;
                }
                (r.lifecycle.activate_user_requested_window)(editor.window());
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = editor.show();
                let _ = editor.hide();
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_watermark_input(generation, revision, attempt + 1)
                });
            }
            _ => {
                r.close_watermark_input();
            }
        }
    });
}

fn pointer_watermark(
    weak: slint::Weak<AnnotationWatermarkInput>,
) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if let Some(editor) = weak.upgrade() {
            choice::watermark_pointer(input);
            dispatch(editor.window(), input);
        }
    })
}

fn close_watermark_input(generation: u64, revision: u64) {
    REGISTRY.with(|slot| {
        if let Some(r) = slot
            .borrow_mut()
            .as_mut()
            .filter(|r| r.generation == generation && r.watermark_input_revision == revision)
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
                && r.watermark_input_revision == revision
                && r.watermark_input.is_some()
        })?;
        r.close_watermark_input();
        r.finish_style_edit();
        r.values[9].text = template.clone().into();
        let content =
            crate::annotation_text::resolve_watermark_template(&template, chrono::Local::now());
        let watermark =
            (!content.trim().is_empty()).then(|| watermark_from_values(r, template, content));
        r.session.set_watermark(watermark);
        if let Some(panel) = &r.panel
            && panel.get_tool() == 9
            && !panel.get_menu()
        {
            panel.set_values(r.values[9].clone());
        }
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
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
            .filter(|r| r.generation == generation && r.panel_revision == panel_revision)
        else {
            return;
        };
        let Some(panel) = r.panel.as_ref() else {
            return;
        };
        match (r.prepare)(panel.window()) {
            PassiveWindowPreparation::Ready => {
                position_panel(r);
                let panel = r.panel.as_ref().unwrap();
                if let Some(main) = &r.main {
                    (r.lifecycle.attach_tool_window)(panel.window(), main.window());
                }
                if panel.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        panel.window(),
                        pointer_panel(panel.as_weak(), generation, panel_revision),
                    )
                {
                    r.close_panel();
                    return;
                }
                (r.lifecycle.set_popup_dismissal)(panel.window(), panel.get_menu());
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = panel.show();
                let _ = panel.hide();
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_panel(generation, panel_revision, attempt + 1)
                });
            }
            _ => r.close_panel(),
        }
    });
}

fn reposition_panel() {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            sync_toolbar_owner(r);
            position_panel(r);
            choice::close_if_moved(r);
        }
    });
}

/// Keep the toolbar owned by the canvas under its physical center so activating
/// the canvas during drawing cannot place its transparent input surface above the controls.
fn sync_toolbar_owner(r: &mut Registry) {
    let Some(main) = r.main.as_ref() else { return };
    let position = main.window().position();
    let size = main.window().size();
    let center = Point {
        x: position.x.saturating_add((size.width / 2) as i32),
        y: position.y.saturating_add((size.height / 2) as i32),
    };
    let owner = canvas_at(r.canvases.iter().map(|canvas| canvas.bounds), center)
        .or(r.toolbar_owner)
        .or_else(|| (!r.canvases.is_empty()).then_some(0));
    let Some(owner) = owner else { return };
    if r.toolbar_owner == Some(owner) {
        return;
    }
    if (r.lifecycle.attach_tool_window)(main.window(), r.canvases[owner].window.window()) {
        r.toolbar_owner = Some(owner);
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

fn position_panel(r: &Registry) {
    let (Some(main), Some(panel)) = (&r.main, &r.panel) else {
        return;
    };
    let p = main.window().position();
    let m = main.window().size();
    let Some(area) = (r.lifecycle.popup_work_area)(Point { x: p.x, y: p.y }) else {
        return;
    };
    let scale = panel.window().scale_factor().max(0.1);
    let available = ((area.right - area.left) as f32 / scale).max(1.);
    let width = if panel.get_menu() {
        240f32.min(available)
    } else if panel.get_tool() == 3 {
        650f32.min(available)
    } else if panel.get_tool() == 4 {
        810f32.min(available)
    } else if panel.get_tool() == 7 {
        1120f32.min(available)
    } else if panel.get_tool() == 8 {
        780f32.min(available)
    } else if panel.get_tool() == 9 {
        1000f32.min(available)
    } else if panel.get_tool() == 10 {
        (if panel.get_values().sequence_expanded {
            1320f32
        } else {
            710f32
        })
        .min(available)
    } else if panel.get_tool() == 6 {
        1030f32.min(available)
    } else if panel.get_tool() == 5 {
        856f32.min(available)
    } else if panel.get_tool() <= 2 || panel.get_tool() <= 6 {
        960f32.min(available)
    } else {
        720f32.min(available)
    };
    let columns = ((width - 24.) / 165.).floor().max(1.) as i32;
    panel.set_columns(columns);
    let height = if panel.get_menu() {
        46. + GROUPS[r.group.unwrap_or(0)].len() as f32 * 42.
    } else if panel.get_tool() <= 4 {
        if width < if panel.get_tool() == 3 { 644. } else { 780. } {
            158.
        } else {
            68.
        }
    } else if panel.get_tool() <= 7 {
        if width < 354. {
            if panel.get_tool() == 7 { 252. } else { 180. }
        } else if width < 644. {
            if panel.get_tool() == 7 { 138. } else { 148. }
        } else if width < if panel.get_tool() == 5 { 856. } else { 890. } {
            if panel.get_tool() == 7 { 138. } else { 98. }
        } else {
            58.
        }
    } else if panel.get_tool() == 8 {
        if width < 774. { 106. } else { 58. }
    } else if panel.get_tool() == 9 {
        if width < 988. { 154. } else { 58. }
    } else if panel.get_tool() == 10 {
        if width < 708. {
            if panel.get_values().sequence_expanded {
                if width < 614. { 214. } else { 166. }
            } else {
                116.
            }
        } else if panel.get_values().sequence_expanded && width < 1302. {
            116.
        } else {
            62.
        }
    } else {
        80. + ((fields(panel.get_tool()).len() as f32 / columns as f32).ceil()) * 62.
    };
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
}

fn panel_position(
    main: Point,
    main_height: u32,
    width: u32,
    height: u32,
    area: Rect,
    gap: i32,
) -> Point {
    let below = main
        .y
        .saturating_add(main_height as i32)
        .saturating_add(gap);
    let y = if below.saturating_add(height as i32) <= area.bottom {
        below
    } else {
        main.y.saturating_sub(height as i32).saturating_sub(gap)
    };
    Point {
        x: main
            .x
            .clamp(area.left, (area.right - width as i32).max(area.left)),
        y: y.clamp(area.top, (area.bottom - height as i32).max(area.top)),
    }
}

fn pointer_main(weak: slint::Weak<AnnotationToolbar>) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if let Some(w) = weak.upgrade() {
            dispatch(w.window(), input);
        }
    })
}
fn pointer_panel(
    weak: slint::Weak<AnnotationPanel>,
    generation: u64,
    panel_revision: u64,
) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if input == PopupPointerInput::DismissRequested {
            later(move || {
                REGISTRY.with(|s| {
                    if let Some(r) = s.borrow_mut().as_mut().filter(|r| {
                        r.generation == generation
                            && r.panel_revision == panel_revision
                            && r.panel.as_ref().is_some_and(|panel| panel.get_menu())
                    }) {
                        if choice::cursor_inside(r) {
                            choice::rearm_panel(r);
                            return;
                        }
                        let on_main = (r.lifecycle.toolbar_cursor_position)()
                            .zip(r.main.as_ref())
                            .is_some_and(|(cursor, main)| {
                                let p = main.window().position();
                                let size = main.window().size();
                                cursor.x >= p.x
                                    && cursor.y >= p.y
                                    && cursor.x < p.x + size.width as i32
                                    && cursor.y < p.y + size.height as i32
                            });
                        let on_panel = (r.lifecycle.toolbar_cursor_position)()
                            .zip(r.panel.as_ref())
                            .is_some_and(|(cursor, panel)| {
                                let p = panel.window().position();
                                let size = panel.window().size();
                                cursor.x >= p.x
                                    && cursor.y >= p.y
                                    && cursor.x < p.x + size.width as i32
                                    && cursor.y < p.y + size.height as i32
                            });
                        // Returning keyboard focus to the editor is not an outside click.
                        if on_main || on_panel {
                            if let Some(panel) = &r.panel {
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
                later(move || {
                    REGISTRY.with(|slot| {
                        if let Some(r) = slot.borrow().as_ref().filter(|r| {
                            r.generation == generation && r.panel_revision == panel_revision
                        }) {
                            restore_text_focus(r);
                        }
                    });
                });
            }
        }
    })
}

pub(crate) fn refresh_language() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            choice::close(r);
            if r.panel.as_ref().is_some_and(|p| p.get_menu()) {
                r.close_panel();
            } else if let Some(panel) = &r.panel {
                panel.set_heading(crate::i18n::tr(NAMES[panel.get_tool() as usize]).into());
                panel.set_language_revision(panel.get_language_revision().wrapping_add(1));
            }
        }
    });
}

pub(crate) fn apply_theme() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow().as_ref() {
            if let Some(w) = &r.main {
                crate::theme::apply(w);
            }
            if let Some(w) = &r.panel {
                crate::theme::apply(w);
            }
            if let Some(menu) = &r.choice {
                crate::theme::apply(&menu.window);
            }
        }
    });
}

#[cfg(test)]
mod tests {
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
            });
            for x in 40..100 {
                canvas_pointer(0, 1, x as f32, 80.);
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert!(r.draft.is_none() && r.session.objects.is_empty());
                    assert!(!r.render_queued);
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
                assert_eq!(r.session.selected, Some(0));
                assert!(r.session.can_undo());
            });
            undo();
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
                    assert!(r.gesture.is_none() && r.gesture_object.is_none());
                    r.mode = InteractionMode::Tool(4);
                });
            }
            canvas_pointer(0, 0, 80., 80.);
            canvas_pointer(0, 2, 80., 80.);
            open_text_editor((-100., 0.), Some(0), dpi);
            let (generation, old_id) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                let edit = r.text_edit.as_ref().unwrap();
                assert!(edit.caret.visible && edit.caret_timer.running());
                assert!(edit.original.content.is_empty());
                (r.generation, edit.id)
            });
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(!r.text_edit.as_ref().unwrap().caret.visible);
                reset_text_caret(r);
                assert!(r.text_edit.as_ref().unwrap().caret.visible);
                r.text_edit.as_mut().unwrap().selection_anchor = Some(0);
            });
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert!(r.text_edit.as_ref().unwrap().caret.visible);
                finish_text_edit_registry(r, false);
            });
            open_text_editor((-100., 0.), Some(0), dpi);
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_ne!(r.text_edit.as_ref().unwrap().id, old_id);
                assert!(r.text_edit.as_ref().unwrap().caret.visible);
                finish_text_edit_registry(r, false);
            });
            tick_text_caret(generation, old_id);
            REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut().unwrap();
                assert_eq!(r.session.objects.len(), 1);
                assert!(r.session.undo());
                assert!(!r.session.can_undo());
                r.close();
            });
        }
        REGISTRY.with(|slot| {
            slot.borrow_mut().take();
        });
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
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main = Some(AnnotationToolbar::new().unwrap());
            r.lifecycle.set_popup_dismissal = Rc::new(|_, enabled| {
                WATCHES.with(|events| events.borrow_mut().push(enabled));
                true
            });
        });
        for dpi in [1., 1.25] {
            for group in [0, 4] {
                open_panel(group, false);
                let (generation, revision, sink) = REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    let panel = r.panel.as_ref().unwrap();
                    panel
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    (
                        r.generation,
                        r.panel_revision,
                        pointer_panel(panel.as_weak(), r.generation, r.panel_revision),
                    )
                });
                WATCHES.with(|events| events.borrow_mut().clear());
                show_panel(generation, revision, 0);
                REGISTRY.with(|slot| choice::rearm_panel(slot.borrow().as_ref().unwrap()));
                WATCHES.with(|events| {
                    assert!(!events.borrow().is_empty());
                    assert!(events.borrow().iter().all(|enabled| !enabled));
                });
                sink(PopupPointerInput::DismissRequested);
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    assert_eq!(r.panel_revision, revision);
                    assert!(r.panel.as_ref().is_some_and(|p| !p.get_menu()));
                });
                // The same button explicitly toggles the panel closed.
                open_panel(group, false);
                REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.is_none()));
                open_panel(group, true);
                let (generation, revision, menu_sink) = REGISTRY.with(|slot| {
                    let slot = slot.borrow();
                    let r = slot.as_ref().unwrap();
                    let panel = r.panel.as_ref().unwrap();
                    (
                        r.generation,
                        r.panel_revision,
                        pointer_panel(panel.as_weak(), r.generation, r.panel_revision),
                    )
                });
                show_panel(generation, revision, 0);
                WATCHES.with(|events| assert_eq!(events.borrow().last(), Some(&true)));
                // An old parameter-window event cannot dismiss its replacement menu.
                sink(PopupPointerInput::DismissRequested);
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.is_some()));
                menu_sink(PopupPointerInput::DismissRequested);
                slint::platform::update_timers_and_animations();
                REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.is_none()));
            }
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
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main = Some(AnnotationToolbar::new().unwrap());
            let mut object = crate::annotation_sequence::tests::badge(1.25);
            crate::annotation_sequence::layout(&mut object);
            assert!(r.session.add(object));
        });
        open_panel(4, false);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let panel = slot.as_ref().unwrap().panel.as_ref().unwrap();
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
        let (generation, edit_id, panel_revision, choice_revision) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let editor = r.text_editor.as_ref().unwrap();
            editor.set_value("ABC".into());
            editor.invoke_place_selection(1, 2);
            r.panel.as_ref().unwrap().set_parameter_input_active(true);
            (
                r.generation,
                r.text_edit.as_ref().unwrap().id,
                r.panel_revision,
                r.choice_revision,
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
                .as_ref()
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
        assert!(restore_matching_text_focus(
            generation,
            edit_id,
            panel_revision,
            choice_revision
        ));
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.text_edit.as_ref().unwrap().id, edit_id);
            assert!(r.text_edit.as_ref().unwrap().caret.visible);
            let editor = r.text_editor.as_ref().unwrap();
            assert_eq!(editor.get_value(), "ABC");
            assert_eq!(
                (editor.get_anchor_offset(), editor.get_cursor_offset()),
                (1, 2)
            );
            editor
                .window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "X".into() });
            assert_eq!(editor.get_value(), "AXC");
            finish_text_edit_registry(r, false);
            assert_eq!(r.session.next_sequence, 8);
            let Extra::Sequence(s) = &r.session.objects[0].extra else {
                unreachable!()
            };
            assert_eq!(s.shape, SequenceShape::Circle);
            assert_eq!(s.format, SequenceFormat::Chinese);
            assert_eq!(s.size, 28.);
            assert_eq!(s.caption.font_size, 52.5);
            let preview = sequence_from_values(r, (500., 200.), 1.25);
            let Extra::Sequence(s) = &preview.extra else {
                unreachable!()
            };
            assert_eq!(
                lexift_core::domain::annotation::sequence_label(s.value, s.format),
                "八"
            );
            r.close();
            assert_eq!(r.session.next_sequence, 1);
            assert_eq!(r.values[10].sequence_label, "1");
            assert!(r.text_editor.is_none() && r.choice.is_none() && r.panel.is_none());
            slot.take();
        });
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
            let sink = pointer_watermark(editor.as_weak());
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
            r.main = Some(AnnotationToolbar::new().unwrap());
            r.generation
        });
        open_watermark_input(generation);
        let old_revision =
            REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().watermark_input_revision);
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.watermark_input.as_ref().unwrap().get_presets_open());
            let menu = &r.choice.as_ref().unwrap().window;
            assert_eq!(menu.get_selected(), 1);
            assert_eq!(menu.get_menu_height(), 404.);
        });
        // Clicking the trigger toggles the independent menu, not the input window.
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            assert!(slot.as_ref().unwrap().choice.is_none());
            assert!(slot.as_ref().unwrap().watermark_input.is_some());
        });
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        open_watermark_input(generation);
        choice::open_watermark(generation, old_revision, 60., 217., 140., 34.);
        choice::dismiss_watermark(generation, old_revision);
        show_watermark_input(generation, old_revision, 0);
        apply_watermark(generation, old_revision, "stale".into());
        close_watermark_input(generation, old_revision);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.watermark_input.is_some());
            assert!(r.choice.is_none());
            assert!(r.session.watermark.is_none());
            assert!(!r.session.can_undo());
            r.close();
            assert!(r.watermark_input.is_none());
            slot.take();
        });
    }

    #[test]
    fn text_resize_ignores_clicks_and_keeps_the_grab_offset() {
        for dpi in [1., 1.25] {
            let start = (-120., 60.);
            let grab = (6. * dpi, -4. * dpi);
            assert!(super::text_drag_target(start, grab, start, dpi, false).is_none());
            assert!(
                super::text_drag_target(start, grab, (start.0 + 3. * dpi, start.1), dpi, false)
                    .is_none()
            );
            let target =
                super::text_drag_target(start, grab, (start.0 + 20., start.1 + 30.), dpi, false)
                    .unwrap();
            assert_eq!(target, (start.0 + 20. - grab.0, start.1 + 30. - grab.1));
            // Returning to the press position after crossing the threshold restores geometry.
            assert_eq!(
                super::text_drag_target(start, grab, start, dpi, true),
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
    fn bootstrap_creates_each_canvas_before_advancing_windows() {
        let states = [CanvasBootstrap::Prepare(0), CanvasBootstrap::Prepare(0)];
        assert_eq!(bootstrap_action(0, 2, &[], 0), BootstrapAction::Create(0));
        assert_eq!(
            bootstrap_action(1, 2, &states[..1], 0),
            BootstrapAction::Create(1)
        );
        assert_eq!(
            bootstrap_action(2, 2, &states, 0),
            BootstrapAction::Advance(0)
        );
    }

    #[test]
    fn bootstrap_skips_ready_screen_and_retries_another() {
        let states = [CanvasBootstrap::Ready, CanvasBootstrap::Prepare(2)];
        assert_eq!(
            bootstrap_action(2, 2, &states, 0),
            BootstrapAction::Advance(1)
        );
        let states = [CanvasBootstrap::Present, CanvasBootstrap::Ready];
        assert_eq!(
            bootstrap_action(2, 2, &states, 1),
            BootstrapAction::Advance(0)
        );
        assert_eq!(
            bootstrap_action(2, 2, &[CanvasBootstrap::Ready; 2], 0),
            BootstrapAction::ShowToolbar
        );
    }

    #[test]
    fn arrow_click_preview_commits_once_and_rejects_a_nearby_endpoint() {
        let pending = ArrowPending {
            start: (-120., 40.),
            style: Style::default(),
            start_scale: 1.25,
            values: defaults(5),
        };
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
