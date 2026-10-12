//! State ownership for the annotation coordinator and its view resources.
use super::*;
pub(super) struct RenderState {
    pub(super) scene_compiler: crate::annotation_scene::SceneCompiler,
    pub(super) scene_revision: u64,
    pub(super) native_presenter: presenter::Presenter,
    pub(super) mosaic: mosaic::State,
    pub(super) frame_tasks: frame::Tasks,
    pub(super) magnifier_sync: MagnifierSync,
    pub(super) magnifier_sync_revision: u64,
    pub(super) first_magnifier_timing: Option<FirstMagnifierTiming>,
    pub(super) first_magnifier_measured: bool,
    pub(super) magnifier_timer: slint::Timer,
}
pub(super) struct CanvasState {
    pub(super) canvases: Vec<CanvasLayer>,
    pub(super) pending_displays: Vec<Rect>,
    pub(super) bootstrap_cursor: usize,
    pub(super) annotation_ready: bool,
    pub(super) cold_open_measured: bool,
    pub(super) cold_open_timing: Option<ColdOpenTiming>,
}
pub(super) struct ToolbarState {
    pub(super) main: window::Slot<AnnotationToolbar>,
    pub(super) panel: window::Slot<AnnotationPanel>,
    pub(super) watermark: window::Slot<AnnotationWatermarkInput>,
    pub(super) menus: choice::State,
    pub(super) toolbar_owner: Option<(usize, u64, u64)>,
    pub(super) toolbar_press: Option<toolbar::Drag>,
    pub(super) toolbar_drag: Option<toolbar::Drag>,
    pub(super) panel_preparation: toolbar::Preparation,
    pub(super) group: Option<usize>,
    pub(super) layout_timer: slint::Timer,
}
pub(super) struct Registry {
    pub(super) toolbar_state: ToolbarState,
    pub(super) canvas_state: CanvasState,
    pub(super) render_state: RenderState,
    pub(super) hints: hints::State,
    pub(super) export: export::State,
    pub(super) editor: editor::State,
    pub(super) fonts: fonts::Directory,
    pub(super) focus: focus::Coordinator,
    pub(super) session: Session,
    pub(super) hovered: Option<usize>,
    pub(super) gesture: Option<Active>,
    pub(super) pending_drawing: PendingState,
    pub(super) mode: InteractionMode,
    pub(super) parameter_edit: parameter_edit::Transaction,
    pub(super) deferred_parameters: parameters::Pending,
    pub(super) pending_edit_panel: bool,
    pub(super) selected: [i32; 7],
    pub(super) parameter_values: parameters::State,
    pub(super) prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    pub(super) lifecycle: WindowLifecycleCallbacks,
    pub(super) timer: slint::Timer,
    pub(super) generation: u64,
    pub(super) status: String,
}

#[derive(Default)]
pub(super) struct ColdOpenTiming {
    pub(super) requested_at: Option<Instant>,
    pub(super) ui_queue: Duration,
    pub(super) toolbar_create: Duration,
    pub(super) display_query: Duration,
    pub(super) canvas_create: Duration,
    pub(super) canvas_prepare: Duration,
    pub(super) canvas_show: Duration,
    pub(super) canvas_native_complete: Duration,
    pub(super) canvas_retry: Duration,
    pub(super) raster: Duration,
    pub(super) native_present: Duration,
    pub(super) toolbar_prepare: Duration,
    pub(super) toolbar_place: Duration,
    pub(super) toolbar_show: Duration,
    pub(super) toolbar_native_complete: Duration,
    pub(super) toolbar_finish: Duration,
    pub(super) toolbar_retry: Duration,
    pub(super) toolbar_retries: u8,
    pub(super) per_screen: Vec<CanvasColdTiming>,
    pub(super) largest_stage: Duration,
    pub(super) largest_callback: Duration,
}

#[derive(Default)]
pub(super) struct CanvasColdTiming {
    pub(super) create: Duration,
    pub(super) prepare: Duration,
    pub(super) show: Duration,
    pub(super) native_complete: Duration,
    pub(super) retry: Duration,
    pub(super) retries: u8,
    pub(super) raster: Duration,
    pub(super) present: Duration,
}

impl ColdOpenTiming {
    pub(super) fn report(self, outcome: &'static str, screens: usize) {
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
pub(super) enum MagnifierSync {
    Cold,
    Pending(u64),
    Live,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MagnifierFrameAction {
    Clear,
    Defer(u64),
    Sync,
    Wait,
}

pub(super) fn magnifier_frame_action(
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

pub(super) struct FirstMagnifierTiming {
    pub(super) first_move: Instant,
    pub(super) frame_start: Option<Instant>,
    pub(super) raster: Duration,
    pub(super) present: Duration,
    pub(super) border_submitted: Option<Instant>,
}

impl FirstMagnifierTiming {
    pub(super) fn new() -> Self {
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
pub(super) enum InteractionMode {
    Mouse,
    Tool(usize),
}

impl InteractionMode {
    pub(super) fn is_mouse(self) -> bool {
        matches!(self, Self::Mouse)
    }

    pub(super) fn selected_group(self) -> i32 {
        match self {
            Self::Mouse => -1,
            Self::Tool(group) => group as i32,
        }
    }
}

impl ToolbarState {
    pub(super) fn pause_inputs(&self) -> Vec<window::PointerPause> {
        vec![
            self.main.input().pause_pointer(),
            self.panel.input().pause_pointer(),
            self.watermark.input().pause_pointer(),
        ]
    }
}
impl CanvasState {
    pub(super) fn pause_inputs(&self) -> impl Iterator<Item = window::PointerPause> + '_ {
        self.canvases
            .iter()
            .map(|layer| layer.input().pause_pointer())
    }
}
