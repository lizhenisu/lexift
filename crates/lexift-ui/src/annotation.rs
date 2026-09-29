//! Owns the ephemeral annotation preview windows and per-tool UI values.
use crate::bridge::dispatch_pointer_input as dispatch;
use crate::{
    AnnotationCanvas, AnnotationPanel, AnnotationToolbar, AnnotationValues,
    bridge::{PassiveWindowPreparation, PopupPointerInput, WindowLifecycleCallbacks},
    placement,
};
use lexift_core::domain::{
    annotation::{
        Bounds, Endpoint, Extra, Hit, Kind, MagnifierConnector, Object, Session, Style,
        magnifier_output_for_source,
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
    choice: Option<choice::Menu>,
    choice_revision: u64,
    canvases: Vec<CanvasLayer>,
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
    PlacePoint,
    FinishArrow,
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
        start: 1,
        strength: if tool == 2 { 10. } else { 50. },
        shape: i32::from(matches!(tool, 1 | 2)),
        zoom: if tool == 7 { 150 } else { 0 },
        head: tool == 5,
        line_end: if tool == 5 { 3 } else { 0 },
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
            choice: None,
            choice_revision: 0,
            canvases: Vec::new(),
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
        self.close_panel();
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
    let first = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if r.gesture.is_some() {
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

pub(crate) fn toggle() {
    if is_open() {
        close();
        return;
    }
    crate::bridge::cancel_idle_memory_trim();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Ok(main) = AnnotationToolbar::new() else {
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
        r.main = Some(main);
        r.sync_mode_display();
        r.generation = r.generation.wrapping_add(1);
        let generation = r.generation;
        for (index, bounds) in (r.lifecycle.annotation_displays)().into_iter().enumerate() {
            if let Ok(window) = AnnotationCanvas::new() {
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
            }
        }
        later(move || show_canvas(generation, 0, 0));
    });
}

fn show_canvas(generation: u64, index: usize, attempt: u8) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(layer) = r.canvases.get(index) else {
            later(move || schedule_render(generation));
            later(move || show_main(generation, 0));
            return;
        };
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
        match (r.prepare)(layer.window.window()) {
            PassiveWindowPreparation::Ready => {
                if layer.window.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        layer.window.window(),
                        pointer_canvas(layer.window.as_weak()),
                    )
                {
                    r.close();
                    return;
                }
                (r.lifecycle.annotation_click_through)(layer.window.window(), r.mode.is_mouse());
                settle_canvas(layer.window.as_weak(), rect, 2);
                let next = index + 1;
                later(move || show_canvas(generation, next, 0));
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = layer.window.show();
                let _ = layer.window.hide();
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_canvas(generation, index, attempt + 1)
                });
            }
            _ => r.close(),
        }
    });
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
    let group = if tool >= 5 { 2 } else { usize::from(tool >= 3) };
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
        if tool == 2 {
            r.session.set_spotlight_opacity(r.values[2].strength / 100.);
        }
        if let Some(kind) = kind {
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
                        _ => {}
                    }
                }
            });
        }
        if let Some(main) = &r.main {
            main.set_can_undo(r.session.can_undo());
        }
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

fn canvas_pointer(index: usize, event: i32, x: f32, y: f32) {
    let result = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if r.mode.is_mouse() {
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
                            r.session.selected = None;
                            if matches!(r.mode, InteractionMode::Tool(0..=2))
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
                } else if let Some(Gesture::Draw {
                    start,
                    kind,
                    style,
                    start_scale,
                    max_distance,
                }) = r.gesture.take()
                {
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
                    r.session.finish_drag();
                    r.gesture = None;
                    if let Some(index) = r.session.selected {
                        adopt_selected_style(r, index);
                    }
                }
                r.gesture_object = None;
            }
            3 => {
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
        let active_canvas = r
            .canvases
            .iter()
            .position(|layer| layer.last_pointer.is_some())
            .unwrap_or(0);
        let canvas_order = std::iter::once(active_canvas)
            .chain((0..r.canvases.len()).filter(|&index| index != active_canvas));
        for index in canvas_order {
            let layer = &mut r.canvases[index];
            let signature = render_signature(
                &r.session,
                layer.bounds,
                layer.window.window().scale_factor(),
                r.draft.as_ref(),
                r.hovered,
            );
            if layer.signature == Some(signature) {
                continue;
            }
            layer.signature = Some(signature);
            let raster_start = Instant::now();
            if let Some(frame) = crate::annotation_render::render(
                &r.session,
                layer.bounds,
                layer.window.window().scale_factor(),
                r.draft.as_ref(),
                r.hovered,
            ) {
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
    session
        .objects
        .iter()
        .chain(draft)
        .any(|object| object.kind.is_spotlight())
        .hash(&mut hash);
    for (index, object) in session.objects.iter().chain(draft).enumerate() {
        let b = object.bounds;
        let margin = (object.style.width / 2. + 8.) * scale;
        let output_visible = if let Extra::Magnifier { output, .. } = object.extra {
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
        r.finish_style_edit();
        if !r.session.undo() {
            return None;
        }
        refresh_hovered(r);
        if matches!(r.mode, InteractionMode::Tool(0 | 1))
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
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(main) = r.main.as_ref() else { return };
        match (r.prepare)(main.window()) {
            PassiveWindowPreparation::Ready => {
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
                if main.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        main.window(),
                        pointer_main(main.as_weak()),
                    )
                {
                    r.close();
                    return;
                }
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
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = main.show();
                let _ = main.hide();
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
        if !menu && group <= 2 {
            panel.on_values_changed(move |values| {
                later(move || update_geometry_values(generation, tool, values))
            });
        }
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
                (r.lifecycle.set_popup_dismissal)(panel.window(), true);
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
                        r.generation == generation && r.panel_revision == panel_revision
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
                        if on_main {
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
            choice::panel_pointer(input);
            dispatch(w.window(), input);
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
    use super::*;

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
