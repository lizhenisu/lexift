use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use lexift_core::ports::credential::{CredentialAccessPurpose, CredentialSecret};
use lexift_core::{
    AppEvent, AppState,
    domain::{
        geometry::{Point, Rect},
        language::Language,
        runtime_config::{HotkeyConfig, ProviderConfig},
        selection::Selection,
        settings::{Settings, SettingsChange, SettingsFeedback, SettingsField},
    },
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{
    AppWindow, PopupCornerMode, PopupLanguageMenuWindow, SelectionToolbarWindow, SettingsToastData,
    SettingsWindow, TranslationPopup, binding, mapper, placement,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PopupPointerInput {
    Moved {
        x: f32,
        y: f32,
    },
    Exited,
    LeftPressed {
        x: f32,
        y: f32,
    },
    LeftReleased {
        x: f32,
        y: f32,
    },
    Scrolled {
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    },
    DismissRequested,
    NativeTopResizeRequested,
    Resized {
        width: f32,
        height: f32,
    },
    ResizeFinished {
        width: f32,
        height: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl PopupResizeEdge {
    fn from_index(index: i32) -> Option<Self> {
        Some(match index {
            0 => Self::Left,
            1 => Self::Right,
            2 => Self::Top,
            3 => Self::Bottom,
            4 => Self::TopLeft,
            5 => Self::TopRight,
            6 => Self::BottomLeft,
            7 => Self::BottomRight,
            _ => return None,
        })
    }

    fn changes_height(self) -> bool {
        !matches!(self, Self::Left | Self::Right)
    }
}

pub type PopupPointerSink = Rc<dyn Fn(PopupPointerInput)>;
type CompletePassiveWindowShow = Rc<dyn Fn(&slint::Window, PopupPointerSink) -> bool>;
type SetPopupDismissal = Rc<dyn Fn(&slint::Window, bool) -> bool>;
type AttachToolWindow = Rc<dyn Fn(&slint::Window, &slint::Window) -> bool>;
type BeginWindowResize = Rc<dyn Fn(&slint::Window, PopupResizeEdge, bool) -> bool>;
type PopupWorkArea = Rc<dyn Fn(Point) -> Option<Rect>>;
type ToolbarCursorPosition = Rc<dyn Fn() -> Option<Point>>;
type ConfigureResizeBackground = Rc<dyn Fn(&slint::Window, [u8; 3]) -> bool>;
type WindowPaintRepair = Rc<dyn Fn()>;
type ConfigureWindowPaintRepair = Rc<dyn Fn(&slint::Window, WindowPaintRepair) -> bool>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveWindowPreparation {
    Ready,
    Pending,
    Failed,
}

pub struct WindowLifecycleCallbacks {
    complete_passive_window_show: CompletePassiveWindowShow,
    complete_toolbar_show: CompletePassiveWindowShow,
    configure_translation_popup_corners: fn(&slint::Window) -> PopupCornerMode,
    activate_user_requested_window: fn(&slint::Window) -> bool,
    begin_window_drag: fn(&slint::Window) -> bool,
    begin_window_resize: BeginWindowResize,
    popup_work_area: PopupWorkArea,
    toolbar_cursor_position: ToolbarCursorPosition,
    set_popup_dismissal: SetPopupDismissal,
    attach_tool_window: AttachToolWindow,
    trim_process_working_set: fn() -> bool,
    configure_resize_background: ConfigureResizeBackground,
    configure_window_paint_repair: ConfigureWindowPaintRepair,
}

impl WindowLifecycleCallbacks {
    pub fn new(
        complete_passive_window_show: impl Fn(&slint::Window, PopupPointerSink) -> bool + 'static,
        activate_user_requested_window: fn(&slint::Window) -> bool,
        begin_window_drag: fn(&slint::Window) -> bool,
        begin_window_resize: impl Fn(&slint::Window, PopupResizeEdge, bool) -> bool + 'static,
        set_popup_dismissal: impl Fn(&slint::Window, bool) -> bool + 'static,
        attach_tool_window: impl Fn(&slint::Window, &slint::Window) -> bool + 'static,
        trim_process_working_set: fn() -> bool,
    ) -> Self {
        Self {
            complete_passive_window_show: Rc::new(complete_passive_window_show),
            complete_toolbar_show: Rc::new(|_, _| true),
            configure_translation_popup_corners: |_| PopupCornerMode::SlintRounded,
            activate_user_requested_window,
            begin_window_drag,
            begin_window_resize: Rc::new(begin_window_resize),
            popup_work_area: Rc::new(|_| None),
            toolbar_cursor_position: Rc::new(|| None),
            set_popup_dismissal: Rc::new(set_popup_dismissal),
            attach_tool_window: Rc::new(attach_tool_window),
            trim_process_working_set,
            configure_resize_background: Rc::new(|_, _| true),
            configure_window_paint_repair: Rc::new(|_, _| true),
        }
    }

    /// Resolves the monitor work area for the Popup's physical window center.
    pub fn with_popup_work_area(mut self, query: impl Fn(Point) -> Option<Rect> + 'static) -> Self {
        self.popup_work_area = Rc::new(query);
        self
    }

    /// Supplies physical cursor coordinates while the selection toolbar is visible.
    pub fn with_toolbar_cursor_position(
        mut self,
        query: impl Fn() -> Option<Point> + 'static,
    ) -> Self {
        self.toolbar_cursor_position = Rc::new(query);
        self
    }

    /// Adds a best-effort, popup-only native surface setup after its HWND exists.
    pub fn with_translation_popup_corners(
        mut self,
        configure: fn(&slint::Window) -> PopupCornerMode,
    ) -> Self {
        self.configure_translation_popup_corners = configure;
        self
    }

    pub fn with_passive_toolbar_interaction(
        mut self,
        complete: impl Fn(&slint::Window, PopupPointerSink) -> bool + 'static,
    ) -> Self {
        self.complete_toolbar_show = Rc::new(complete);
        self
    }

    /// Adds the software renderer's native resize fill.
    pub fn with_resize_background(
        mut self,
        configure: impl Fn(&slint::Window, [u8; 3]) -> bool + 'static,
    ) -> Self {
        self.configure_resize_background = Rc::new(configure);
        self
    }

    /// Adds the software renderer's full repaint after native movement ends.
    pub fn with_window_paint_repair(
        mut self,
        configure: impl Fn(&slint::Window, WindowPaintRepair) -> bool + 'static,
    ) -> Self {
        self.configure_window_paint_repair = Rc::new(configure);
        self
    }
}

const POPUP_GAP_PX: i32 = 12;
const WORK_AREA_MARGIN_PX: i32 = 8;
const CREDENTIAL_REVEAL_DURATION: Duration = Duration::from_secs(30);
const SETTINGS_TOAST_SUCCESS_DURATION: Duration = Duration::from_secs(2);
const SETTINGS_TOAST_ERROR_DURATION: Duration = Duration::from_secs(5);
const SETTINGS_DEFAULT_WIDTH: f32 = 820.0;
const SETTINGS_DEFAULT_HEIGHT: f32 = 680.0;
const RESIZE_BACKGROUND_RETRY_INTERVAL: Duration = Duration::from_millis(16);
const RESIZE_BACKGROUND_RETRY_ATTEMPTS: u32 = 20;
const POPUP_DRAG_FALLBACK_DELAY: Duration = Duration::from_millis(16);
const POPUP_SHOW_RETRY_DELAY: Duration = Duration::from_millis(16);
const POPUP_SHOW_TIMEOUT: Duration = Duration::from_secs(1);
const LANGUAGE_MENU_ROW_HEIGHT: f32 = 40.0;
const LANGUAGE_MENU_VISIBLE_ROWS: usize = 10;
const LANGUAGE_MENU_GAP_PX: i32 = 4;
const POPUP_MIN_WIDTH: f32 = 340.0;
const POPUP_MIN_SOURCE_HEIGHT: f32 = 86.0;
const NO_WINDOW_MEMORY_TRIM_DELAY: Duration = Duration::from_secs(120);
const TOOLBAR_FADE_SAMPLE_INTERVAL: Duration = Duration::from_millis(33);
const TOOLBAR_FADE_START_LOGICAL_PX: f64 = 24.0;
const TOOLBAR_FADE_END_LOGICAL_PX: f64 = 220.0;

thread_local! {
    static POPUP_REGISTRY: RefCell<Option<PopupRegistry>> = const { RefCell::new(None) };
    static IDLE_TRIM_GENERATION: RefCell<IdleTrimGeneration> = RefCell::new(IdleTrimGeneration::default());
    static IDLE_TRIM_CALLBACK: Cell<Option<fn() -> bool>> = const { Cell::new(None) };
}

#[derive(Default)]
struct IdleTrimGeneration(u64);

impl IdleTrimGeneration {
    fn invalidate(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    fn schedule(&mut self) -> u64 {
        self.invalidate();
        self.0
    }

    fn is_current(&self, generation: u64) -> bool {
        self.0 == generation
    }
}

fn cancel_idle_memory_trim() {
    IDLE_TRIM_GENERATION.with(|generation| generation.borrow_mut().invalidate());
}

fn schedule_idle_memory_trim() {
    let generation = IDLE_TRIM_GENERATION.with(|current| current.borrow_mut().schedule());
    slint::Timer::single_shot(NO_WINDOW_MEMORY_TRIM_DELAY, move || {
        let current = IDLE_TRIM_GENERATION.with(|current| current.borrow().is_current(generation));
        if !current || has_live_window_instances() {
            return;
        }
        let Some(trim) = IDLE_TRIM_CALLBACK.with(Cell::get) else {
            return;
        };
        if trim() {
            tracing::info!("trimmed resident memory after two minutes without UI windows");
        } else {
            tracing::warn!("could not trim resident memory after no-window idle period");
        }
    });
}

fn has_live_window_instances() -> bool {
    let app_window_exists = APP_WINDOW_REGISTRY.with(|registry| {
        registry
            .borrow()
            .as_ref()
            .is_some_and(|registry| registry.main.is_some() || registry.settings.is_some())
    });
    app_window_exists
        || SELECTION_TOOLBAR_REGISTRY.with(|registry| {
            registry
                .borrow()
                .as_ref()
                .is_some_and(|r| r.window.is_some())
        })
        || POPUP_REGISTRY.with(|registry| {
            registry.borrow().as_ref().is_some_and(|registry| {
                !registry.windows.is_empty() || registry.language_menu.is_some()
            })
        })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PopupDragPhase {
    #[default]
    Idle,
    WaitingForFrame,
    DispatchQueued,
}

#[derive(Default)]
struct PopupDragScheduler {
    phases: HashMap<u64, PopupDragPhase>,
}

impl PopupDragScheduler {
    fn request(&mut self, session_id: u64) -> bool {
        if self.phase(session_id) != PopupDragPhase::Idle {
            return false;
        }
        self.phases
            .insert(session_id, PopupDragPhase::WaitingForFrame);
        true
    }

    fn frame_rendered(&mut self, session_id: u64) -> bool {
        if self.phase(session_id) != PopupDragPhase::WaitingForFrame {
            return false;
        }
        self.phases
            .insert(session_id, PopupDragPhase::DispatchQueued);
        true
    }

    fn take_dispatch(&mut self, session_id: u64) -> bool {
        if self.phase(session_id) != PopupDragPhase::DispatchQueued {
            return false;
        }
        self.phases.remove(&session_id);
        true
    }

    fn cancel(&mut self, session_id: u64) {
        self.phases.remove(&session_id);
    }

    fn phase(&self, session_id: u64) -> PopupDragPhase {
        self.phases.get(&session_id).copied().unwrap_or_default()
    }
}

struct PopupRegistry {
    language_menu: Option<PopupLanguageMenuWindow>,
    language_menu_owner: Option<LanguageMenuOwner>,
    language_menu_pending: Option<PendingLanguageMenuShow>,
    language_menu_dismissal_watched: bool,
    windows: HashMap<u64, TranslationPopup>,
    states: HashMap<u64, mapper::PopupUiState>,
    work_areas: HashMap<u64, Rect>,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    dismissal_watches: HashSet<u64>,
    drag_scheduler: PopupDragScheduler,
    manual_sizes: HashMap<u64, ManualPopupSize>,
    active_resizes: HashMap<u64, ActivePopupResize>,
    pending_window_size_syncs: HashSet<u64>,
    pending_shows: HashMap<u64, PendingPopupShow>,
    next_show_generation: u64,
    prepare_passive_window: fn(&slint::Window) -> PassiveWindowPreparation,
    configure_translation_popup_corners: fn(&slint::Window) -> PopupCornerMode,
    configure_resize_background: ConfigureResizeBackground,
    configure_window_paint_repair: ConfigureWindowPaintRepair,
    complete_passive_window_show: CompletePassiveWindowShow,
    begin_window_drag: fn(&slint::Window) -> bool,
    begin_window_resize: BeginWindowResize,
    popup_work_area: PopupWorkArea,
    set_popup_dismissal: SetPopupDismissal,
    attach_tool_window: AttachToolWindow,
}

#[derive(Clone, Copy, Debug)]
struct ManualPopupSize {
    width: f32,
    height: f32,
    source_height: f32,
    baseline_source_height: f32,
    baseline_remainder_height: f32,
}

#[derive(Clone, Copy, Debug)]
struct ActivePopupResize {
    edge: PopupResizeEdge,
    base_height: f32,
    base_source_height: f32,
}

fn effective_manual_layout(
    manual: ManualPopupSize,
    auto_height: f32,
    auto_source_height: f32,
) -> (f32, f32, f32) {
    let auto_remainder_height = auto_height - auto_source_height;
    let source_growth = (auto_source_height - manual.baseline_source_height).max(0.0);
    let remainder_growth = (auto_remainder_height - manual.baseline_remainder_height).max(0.0);
    (
        manual.width,
        manual.height + source_growth + remainder_growth,
        manual.source_height + source_growth,
    )
}

fn max_source_card_height(window_height: f32, reserved_height: f32, minimum: f32) -> f32 {
    (window_height - reserved_height).max(minimum)
}

fn clamp_source_card_height(
    source_height: f32,
    window_height: f32,
    reserved_height: f32,
    minimum: f32,
) -> f32 {
    clamp_range(
        source_height,
        minimum,
        max_source_card_height(window_height, reserved_height, minimum),
    )
}

/// Clamps a value into an ordered range without panicking on an inverted or NaN bound.
///
/// The popup's size limits are computed from live layout metrics and the monitor work area, so a
/// maximised window can produce a minimum above the maximum. `f32::clamp` aborts the process in
/// that case (the release profile uses `panic = "abort"`), so every size clamp goes through here.
fn clamp_range(value: f32, min: f32, max: f32) -> f32 {
    if !min.is_finite() || !max.is_finite() {
        return if value.is_finite() { value } else { 0.0 };
    }
    // An inverted range means the derived minimum outgrew the monitor limit; the maximum wins so
    // the window still fits the screen.
    value.clamp(min.min(max), max)
}

/// The popup's minimum height, kept at or below its maximum so the sizing loop stays valid.
fn popup_min_height(requested: f32, max_height: f32) -> f32 {
    let floor = POPUP_MIN_SOURCE_HEIGHT.max(336.0);
    let requested = if requested.is_finite() {
        requested
    } else {
        floor
    };
    let max_height = if max_height.is_finite() {
        max_height
    } else {
        floor
    };
    clamp_range(requested.max(floor), floor, max_height.max(floor))
}

/// The smallest height the popup layout can show on its monitor.
///
/// Everything except the source card has a fixed vertical cost (`reserved_height`), and the source
/// card must keep `min_source_height`, so the layout floor does not depend on the current window
/// height. Deriving it from the live height instead (`height - source + min_source`) made a popup
/// whose source card already sat at its minimum report a minimum equal to its current height, often
/// also equal to the monitor maximum. Slint forwards both bounds to Winit, Winit writes them into
/// `WM_GETMINMAXINFO`, and Windows then refuses every height change — which froze the popup's edges
/// and corners.
fn popup_min_window_height(reserved_height: f32, min_source_height: f32, max_height: f32) -> f32 {
    popup_min_height(reserved_height + min_source_height, max_height)
}

fn has_valid_native_client_size(width: f32, height: f32) -> bool {
    width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0
}

fn manual_size_after_external_resize(
    width: f32,
    height: f32,
    source_height: f32,
    reserved_height: f32,
    minimum_source_height: f32,
    baseline_source_height: f32,
    baseline_remainder_height: f32,
) -> ManualPopupSize {
    ManualPopupSize {
        width,
        height,
        source_height: clamp_source_card_height(
            source_height,
            height,
            reserved_height,
            minimum_source_height,
        ),
        baseline_source_height,
        baseline_remainder_height,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LanguageMenuKind {
    Source,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LanguageMenuOwner {
    session_id: u64,
    kind: LanguageMenuKind,
}

#[derive(Clone, Copy, Debug)]
struct PendingLanguageMenuShow {
    owner: LanguageMenuOwner,
    generation: u64,
    deadline: Instant,
    retried_once: bool,
    native_window_creation_requested: bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingPopupShow {
    anchor: Option<Point>,
    work_area: Option<Rect>,
    generation: u64,
    deadline: Instant,
    retried_once: bool,
    native_window_creation_requested: bool,
}

impl PendingPopupShow {
    fn new(
        anchor: Option<Point>,
        work_area: Option<Rect>,
        generation: u64,
        previous: Option<Self>,
    ) -> Self {
        let (retried_once, native_window_creation_requested) = previous
            .map(|request| {
                (
                    request.retried_once,
                    request.native_window_creation_requested,
                )
            })
            .unwrap_or_default();
        Self {
            anchor,
            work_area,
            generation,
            deadline: Instant::now() + POPUP_SHOW_TIMEOUT,
            retried_once,
            native_window_creation_requested,
        }
    }

    fn request_native_window_creation(&mut self) -> bool {
        if self.native_window_creation_requested {
            return false;
        }
        self.native_window_creation_requested = true;
        true
    }
}

impl PopupRegistry {
    fn window_for(&mut self, session_id: u64) -> Option<TranslationPopup> {
        if let Some(window) = self.windows.get(&session_id) {
            return Some(window.clone_strong());
        }
        let window = TranslationPopup::new().ok()?;
        if let Some(handler) = &self.handler {
            wire_popup_callbacks(&window, Rc::clone(handler));
            wire_popup_close(&window, Rc::clone(handler));
        }
        self.windows.insert(session_id, window.clone_strong());
        Some(window)
    }

    fn open_language_menu(
        &mut self,
        session_id: u64,
        popup: &TranslationPopup,
        kind: LanguageMenuKind,
    ) {
        let owner = LanguageMenuOwner { session_id, kind };
        if self.language_menu_owner == Some(owner)
            && (self
                .language_menu
                .as_ref()
                .is_some_and(|menu| menu.window().is_visible())
                || self.language_menu_pending.is_some())
        {
            self.close_language_menu(true);
            return;
        }
        self.close_language_menu(true);
        let Some(work_area) = self.work_areas.get(&session_id).copied() else {
            return;
        };
        let menu = match self.language_menu.as_ref() {
            Some(menu) => menu.clone_strong(),
            None => {
                let Ok(menu) = PopupLanguageMenuWindow::new() else {
                    tracing::error!(session_id, "popup language menu could not be created");
                    return;
                };
                wire_language_menu_callbacks(&menu);
                self.language_menu = Some(menu.clone_strong());
                menu
            }
        };
        self.disable_dismissal_watch(session_id, popup);
        popup.set_source_menu_open(kind == LanguageMenuKind::Source);
        popup.set_target_menu_open(kind == LanguageMenuKind::Target);
        self.language_menu_owner = Some(owner);
        match kind {
            LanguageMenuKind::Source => {
                menu.set_language_options(popup.get_source_language_options());
                menu.set_selected_index(popup.get_source_index());
            }
            LanguageMenuKind::Target => {
                menu.set_language_options(popup.get_target_language_options());
                menu.set_selected_index(popup.get_target_index());
            }
        }
        menu.set_menu_scroll_y(0.0);
        self.next_show_generation = self.next_show_generation.wrapping_add(1).max(1);
        let generation = self.next_show_generation;
        self.language_menu_pending = Some(PendingLanguageMenuShow {
            owner,
            generation,
            deadline: Instant::now() + POPUP_SHOW_TIMEOUT,
            retried_once: false,
            native_window_creation_requested: false,
        });
        self.try_show_language_menu(generation, work_area);
    }

    fn try_show_language_menu(&mut self, generation: u64, work_area: Rect) {
        let Some(request) = self
            .language_menu_pending
            .filter(|request| request.generation == generation)
        else {
            return;
        };
        let Some(parent) = self
            .existing_window(request.owner.session_id)
            .filter(|window| {
                window.window().is_visible()
                    && match request.owner.kind {
                        LanguageMenuKind::Source => window.get_source_menu_open(),
                        LanguageMenuKind::Target => window.get_target_menu_open(),
                    }
            })
        else {
            self.close_language_menu(false);
            return;
        };
        let Some(menu) = self
            .language_menu
            .as_ref()
            .map(ComponentHandle::clone_strong)
        else {
            self.language_menu_pending = None;
            return;
        };
        match (self.prepare_passive_window)(menu.window()) {
            PassiveWindowPreparation::Ready => {
                self.language_menu_pending = None;
                self.finish_language_menu_show(request.owner, &parent, work_area);
            }
            PassiveWindowPreparation::Pending if Instant::now() < request.deadline => {
                let should_prime = self.language_menu_pending.as_mut().is_some_and(|pending| {
                    if pending.native_window_creation_requested {
                        false
                    } else {
                        pending.native_window_creation_requested = true;
                        true
                    }
                });
                if should_prime && !prime_hidden_language_menu(&menu) {
                    self.close_language_menu(true);
                    return;
                }
                let delay = popup_show_retry_delay(request.retried_once);
                if let Some(pending) = self.language_menu_pending.as_mut() {
                    pending.retried_once = true;
                }
                schedule_language_menu_show_retry(generation, work_area, delay);
            }
            PassiveWindowPreparation::Pending => {
                tracing::error!(
                    session_id = request.owner.session_id,
                    "popup language menu native window was not created before the show timeout"
                );
                self.close_language_menu(true);
            }
            PassiveWindowPreparation::Failed => self.close_language_menu(true),
        }
    }

    fn finish_language_menu_show(
        &mut self,
        owner: LanguageMenuOwner,
        parent: &TranslationPopup,
        work_area: Rect,
    ) {
        self.position_language_menu(parent, owner.kind, work_area);
        let Some(menu) = self
            .language_menu
            .as_ref()
            .map(ComponentHandle::clone_strong)
        else {
            self.close_language_menu(false);
            return;
        };
        if !(self.attach_tool_window)(menu.window(), parent.window()) {
            self.close_language_menu(true);
            return;
        }
        if menu.show().is_err() {
            self.close_language_menu(true);
            return;
        }
        let pointer_sink = language_menu_pointer_sink(menu.as_weak());
        if !(self.complete_passive_window_show)(menu.window(), pointer_sink) {
            self.close_language_menu(true);
            return;
        }
        if (self.set_popup_dismissal)(menu.window(), true) {
            self.language_menu_dismissal_watched = true;
        }
        self.language_menu_owner = Some(owner);
    }

    fn position_language_menu(
        &self,
        parent: &TranslationPopup,
        kind: LanguageMenuKind,
        work_area: Rect,
    ) {
        let scale = parent.window().scale_factor().max(f32::EPSILON);
        let parent_position = parent.window().position();
        let (anchor_x, anchor_y, anchor_width, anchor_height, option_count, selected_index) =
            match kind {
                LanguageMenuKind::Source => (
                    parent.get_source_menu_anchor_x(),
                    parent.get_source_menu_anchor_y(),
                    parent.get_source_menu_anchor_width(),
                    parent.get_source_menu_anchor_height(),
                    parent.get_source_language_options().row_count(),
                    parent.get_source_index(),
                ),
                LanguageMenuKind::Target => (
                    parent.get_target_menu_anchor_x(),
                    parent.get_target_menu_anchor_y(),
                    parent.get_target_menu_anchor_width(),
                    parent.get_target_menu_anchor_height(),
                    parent.get_target_language_options().row_count(),
                    parent.get_target_index(),
                ),
            };
        let anchor_left = parent_position.x + (anchor_x * scale).round() as i32;
        let anchor_top = parent_position.y + (anchor_y * scale).round() as i32;
        let anchor_width = (anchor_width * scale).round().max(1.0) as u32;
        let anchor_height = (anchor_height * scale).round().max(1.0) as u32;
        let visible_rows = option_count.min(LANGUAGE_MENU_VISIBLE_ROWS);
        let desired_height = (visible_rows as f32 * LANGUAGE_MENU_ROW_HEIGHT * scale)
            .round()
            .max(1.0) as u32;
        let placement = placement::place_attached_menu(
            Rect {
                left: anchor_left,
                top: anchor_top,
                right: anchor_left.saturating_add(anchor_width as i32),
                bottom: anchor_top.saturating_add(anchor_height as i32),
            },
            desired_height,
            work_area,
            LANGUAGE_MENU_GAP_PX,
            WORK_AREA_MARGIN_PX,
        );
        let logical_width = placement.width as f32 / scale;
        let logical_height = placement.height as f32 / scale;
        let Some(menu) = &self.language_menu else {
            return;
        };
        menu.set_menu_width(logical_width);
        menu.set_menu_height(logical_height);
        let max_scroll = (option_count as f32 * LANGUAGE_MENU_ROW_HEIGHT - logical_height).max(0.0);
        let selected = selected_index.max(0) as f32;
        menu.set_menu_scroll_y(
            -(((selected * LANGUAGE_MENU_ROW_HEIGHT - logical_height / 2.0
                + LANGUAGE_MENU_ROW_HEIGHT / 2.0)
                .clamp(0.0, max_scroll)
                / 4.0)
                .round()
                * 4.0),
        );
        menu.window().set_position(slint::PhysicalPosition::new(
            placement.position.x,
            placement.position.y,
        ));
        menu.window()
            .set_size(slint::LogicalSize::new(logical_width, logical_height));
    }

    fn close_language_menu(&mut self, restore_parent_watch: bool) {
        self.language_menu_pending = None;
        if let Some(menu) = self.language_menu.take() {
            if self.language_menu_dismissal_watched {
                let _ = (self.set_popup_dismissal)(menu.window(), false);
            }
            self.language_menu_dismissal_watched = false;
            menu.set_menu_scroll_y(0.0);
            let _ = menu.hide();
            slint::Timer::single_shot(Duration::ZERO, move || drop(menu));
        }
        let Some(owner) = self.language_menu_owner.take() else {
            return;
        };
        let parent = self.existing_window(owner.session_id);
        if let Some(parent) = &parent {
            parent.set_source_menu_open(false);
            parent.set_target_menu_open(false);
        }
        if restore_parent_watch
            && let Some(parent) = parent
            && parent.window().is_visible()
            && self
                .states
                .get(&owner.session_id)
                .is_some_and(|state| !state.pinned)
        {
            self.set_dismissal_watch(owner.session_id, &parent, true);
        }
    }

    fn update(&mut self, states: Vec<mapper::PopupUiState>) {
        let previous_states = std::mem::replace(
            &mut self.states,
            states
                .into_iter()
                .map(|state| (state.session_id, state))
                .collect(),
        );
        for (id, window) in &self.windows {
            if let Some(state) = self.states.get(id) {
                let geometry_locked = self.active_resizes.contains_key(id)
                    || self.pending_window_size_syncs.contains(id);
                apply_popup_preserving_draft(
                    window,
                    state,
                    previous_states.get(id),
                    self.manual_sizes.get(id).copied(),
                    geometry_locked,
                );
                if !geometry_locked && let Some(work_area) = self.work_areas.get(id) {
                    apply_popup_resize_bounds(window, *work_area);
                    clamp_popup_to_work_area(window, *work_area);
                }
            }
        }
        let watch_updates = self
            .states
            .iter()
            .filter_map(|(id, state)| {
                self.existing_window(*id)
                    .filter(|window| window.window().is_visible())
                    .map(|window| {
                        (
                            *id,
                            window,
                            !state.pinned
                                && !self
                                    .language_menu_owner
                                    .is_some_and(|owner| owner.session_id == *id),
                        )
                    })
            })
            .collect::<Vec<_>>();
        for (id, window, enabled) in watch_updates {
            self.set_dismissal_watch(id, &window, enabled);
        }
        if self
            .language_menu
            .as_ref()
            .is_some_and(|menu| menu.window().is_visible())
            && let Some(owner) = self.language_menu_owner
            && let Some(parent) = self.existing_window(owner.session_id)
            && let Some(work_area) = self.work_areas.get(&owner.session_id).copied()
        {
            self.position_language_menu(&parent, owner.kind, work_area);
        }
    }

    fn show(&mut self, session_id: u64, anchor: Option<Point>, work_area: Option<Rect>) {
        let Some(window) = self.window_for(session_id) else {
            tracing::error!(session_id, "translation popup could not be created");
            return;
        };
        if self
            .language_menu_owner
            .is_some_and(|owner| owner.session_id == session_id)
        {
            self.close_language_menu(false);
        }
        reset_popup_transient_ui(&window);
        if let Some(state) = self.states.get(&session_id) {
            apply_popup_preserving_draft(
                &window,
                state,
                None,
                self.manual_sizes.get(&session_id).copied(),
                false,
            );
            // A new hotkey capture replaces any unsubmitted text in a reused popup.
            window.set_source_text(state.source.clone().into());
        } else {
            window.set_session_id(session_id as i32);
        }
        self.disable_dismissal_watch(session_id, &window);
        self.drag_scheduler.cancel(session_id);
        self.next_show_generation = self.next_show_generation.wrapping_add(1).max(1);
        let generation = self.next_show_generation;
        let previous_request = self.pending_shows.get(&session_id).copied();
        self.pending_shows.insert(
            session_id,
            PendingPopupShow::new(anchor, work_area, generation, previous_request),
        );
        self.try_show(session_id, generation);
    }

    fn try_show(&mut self, session_id: u64, generation: u64) {
        let Some(request) = self
            .pending_shows
            .get(&session_id)
            .copied()
            .filter(|request| request.generation == generation)
        else {
            return;
        };
        let Some(window) = self.existing_window(session_id) else {
            self.pending_shows.remove(&session_id);
            return;
        };
        match (self.prepare_passive_window)(window.window()) {
            PassiveWindowPreparation::Ready => {
                self.pending_shows.remove(&session_id);
                let corner_mode = (self.configure_translation_popup_corners)(window.window());
                window.set_corner_mode(corner_mode);
                self.install_popup_resize_background(&window);
                self.finish_show(session_id, window, request.anchor, request.work_area);
            }
            PassiveWindowPreparation::Pending if Instant::now() < request.deadline => {
                let delay = popup_show_retry_delay(request.retried_once);
                let request_native_window_creation = self
                    .pending_shows
                    .get_mut(&session_id)
                    .is_some_and(PendingPopupShow::request_native_window_creation);
                if request_native_window_creation && !prime_hidden_popup_window(session_id, &window)
                {
                    self.pending_shows.remove(&session_id);
                    return;
                }
                if let Some(request) = self.pending_shows.get_mut(&session_id) {
                    request.retried_once = true;
                }
                schedule_popup_show_retry(session_id, generation, delay);
            }
            PassiveWindowPreparation::Pending => {
                self.pending_shows.remove(&session_id);
                self.hide(session_id);
                tracing::error!(
                    session_id,
                    "translation popup native window was not created before the show timeout"
                );
            }
            PassiveWindowPreparation::Failed => {
                self.pending_shows.remove(&session_id);
                self.hide(session_id);
            }
        }
    }

    fn existing_window(&self, session_id: u64) -> Option<TranslationPopup> {
        self.windows
            .get(&session_id)
            .map(ComponentHandle::clone_strong)
    }

    fn finish_show(
        &mut self,
        session_id: u64,
        window: TranslationPopup,
        anchor: Option<Point>,
        work_area: Option<Rect>,
    ) {
        if window.window().is_minimized() {
            window.window().set_minimized(false);
        }
        if let (Some(anchor), Some(work_area)) = (anchor, work_area) {
            self.work_areas.insert(session_id, work_area);
            apply_popup_resize_bounds(&window, work_area);
            let size = window.window().size();
            let placement = placement::place_popup(
                anchor,
                size.width,
                size.height,
                work_area,
                POPUP_GAP_PX,
                WORK_AREA_MARGIN_PX,
            );
            let mut x = placement.position.x;
            let mut y = placement.position.y;
            let occupied = self
                .states
                .iter()
                .filter(|(id, state)| **id != session_id && state.pinned)
                .filter_map(|(id, _)| self.windows.get(id).map(ComponentHandle::clone_strong))
                .filter(|candidate| candidate.window().is_visible())
                .map(|candidate| candidate.window().position())
                .collect::<Vec<_>>();
            for _ in 0..occupied.len() {
                if !occupied
                    .iter()
                    .any(|position| (position.x - x).abs() < 24 && (position.y - y).abs() < 24)
                {
                    break;
                }
                x += 24;
                y += 24;
                let min_x = work_area.left + WORK_AREA_MARGIN_PX;
                let min_y = work_area.top + WORK_AREA_MARGIN_PX;
                let max_x = (work_area.right - size.width as i32 - WORK_AREA_MARGIN_PX).max(min_x);
                let max_y =
                    (work_area.bottom - size.height as i32 - WORK_AREA_MARGIN_PX).max(min_y);
                x = x.clamp(min_x, max_x);
                y = y.clamp(min_y, max_y);
            }
            window
                .window()
                .set_position(slint::PhysicalPosition::new(x, y));
        }
        if !window.window().is_visible() && window.show().is_err() {
            return;
        }
        let pointer_sink = popup_pointer_sink(window.as_weak());
        if !(self.complete_passive_window_show)(window.window(), pointer_sink) {
            let _ = window.hide();
            return;
        }
        let enabled = self
            .states
            .get(&session_id)
            .is_some_and(|state| !state.pinned);
        self.set_dismissal_watch(session_id, &window, enabled);
    }

    fn start_resize(&mut self, session_id: u64, window: &TranslationPopup, edge: PopupResizeEdge) {
        if self
            .language_menu_owner
            .is_some_and(|owner| owner.session_id == session_id)
        {
            self.close_language_menu(false);
        }
        self.disable_dismissal_watch(session_id, window);
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let actual_size = window.window().size();
        let height = actual_size.height as f32 / scale;
        let min_source_height = window.get_resize_min_source_height();
        let reserved_height = window.get_resize_layout_reserve_height();
        let source_height = clamp_source_card_height(
            window.get_source_card_height(),
            height,
            reserved_height,
            min_source_height,
        );
        window.set_source_card_height(source_height);
        window.set_resize_min_height(popup_min_window_height(
            reserved_height,
            min_source_height,
            window.get_max_popup_height(),
        ));
        window.set_resize_active(true);
        window.set_resize_changes_height(edge.changes_height());
        window.set_resize_fixed_remainder(height - source_height);
        self.active_resizes.insert(
            session_id,
            ActivePopupResize {
                edge,
                base_height: height,
                base_source_height: source_height,
            },
        );
    }

    fn finish_resize(
        &mut self,
        session_id: u64,
        window: &TranslationPopup,
        width: f32,
        height: f32,
    ) {
        self.refresh_popup_work_area(session_id, window);
        let Some(active) = self.active_resizes.remove(&session_id) else {
            return;
        };
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let actual_size = window.window().size();
        let width = if width > 0.0 {
            width
        } else {
            actual_size.width as f32 / scale
        };
        let height = if height > 0.0 {
            height
        } else {
            actual_size.height as f32 / scale
        };
        let width = clamp_range(width, POPUP_MIN_WIDTH, window.get_max_popup_width());
        let height = clamp_range(
            height,
            window.get_resize_min_height(),
            window.get_max_popup_height(),
        );
        let min_source_height = window.get_resize_min_source_height();
        let reserved_height = window.get_resize_layout_reserve_height();
        let requested_source_height = if active.edge.changes_height() {
            (active.base_source_height + height - active.base_height).max(min_source_height)
        } else {
            active.base_source_height
        };
        let source_height = clamp_source_card_height(
            requested_source_height,
            height,
            reserved_height,
            min_source_height,
        );
        window.set_resize_active(false);
        window.set_resize_changes_height(false);
        window.set_popup_width(width);
        window.set_popup_height(height);
        window.set_source_card_height(source_height);
        window.set_resize_min_height(popup_min_window_height(
            reserved_height,
            min_source_height,
            window.get_max_popup_height(),
        ));
        if let Some(state) = self.states.get(&session_id) {
            let (auto_height, auto_source_height) = mapper::popup_metrics_for_width(
                &state.source,
                &state.translated,
                &state.error,
                width,
            );
            self.manual_sizes.insert(
                session_id,
                ManualPopupSize {
                    width,
                    height,
                    source_height,
                    baseline_source_height: auto_source_height,
                    baseline_remainder_height: auto_height - auto_source_height,
                },
            );
        }
        if let Some(work_area) = self.work_areas.get(&session_id).copied() {
            clamp_popup_to_work_area(window, work_area);
        }
        let enabled = self
            .states
            .get(&session_id)
            .is_some_and(|state| !state.pinned);
        self.set_dismissal_watch(session_id, window, enabled);
    }

    fn refresh_popup_work_area(
        &mut self,
        session_id: u64,
        window: &TranslationPopup,
    ) -> Option<Rect> {
        let position = window.window().position();
        let size = window.window().size();
        let center = Point {
            x: position.x.saturating_add((size.width / 2) as i32),
            y: position.y.saturating_add((size.height / 2) as i32),
        };
        let work_area = (self.popup_work_area)(center)?;
        self.work_areas.insert(session_id, work_area);
        apply_popup_resize_bounds(window, work_area);
        Some(work_area)
    }

    fn install_popup_resize_background(&self, window: &TranslationPopup) {
        if !install_resize_background(
            &self.configure_resize_background,
            window.window(),
            window.get_resize_fallback_color(),
        ) {
            tracing::debug!("translation popup resize background fill could not be installed yet");
        }
        if !(self.configure_window_paint_repair)(window.window(), window_paint_repair(window)) {
            tracing::debug!("translation popup paint repair could not be installed yet");
        }
    }

    fn sync_external_window_size(&mut self, session_id: u64, window: &TranslationPopup) {
        if self.active_resizes.contains_key(&session_id) {
            return;
        }

        let physical_size = window.window().size();
        if !has_valid_native_client_size(physical_size.width as f32, physical_size.height as f32) {
            return;
        }
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let width = physical_size.width as f32 / scale;
        let height = physical_size.height as f32 / scale;
        let minimum_source_height = window.get_resize_min_source_height();
        let reserved_height = window.get_resize_layout_reserve_height();
        let (baseline_source_height, baseline_remainder_height) =
            if let Some(state) = self.states.get(&session_id) {
                let (auto_height, auto_source_height) = mapper::popup_metrics_for_width(
                    &state.source,
                    &state.translated,
                    &state.error,
                    width,
                );
                (auto_source_height, auto_height - auto_source_height)
            } else if let Some(previous) = self.manual_sizes.get(&session_id) {
                (
                    previous.baseline_source_height,
                    previous.baseline_remainder_height,
                )
            } else {
                (0.0, 0.0)
            };
        let manual = manual_size_after_external_resize(
            width,
            height,
            window.get_source_card_height(),
            reserved_height,
            minimum_source_height,
            baseline_source_height,
            baseline_remainder_height,
        );

        // The native window already has this size. Update Slint's preferred
        // dimensions and the saved manual layout so later Core updates cannot
        // restore a stale pre-snap height; never write geometry back to HWND.
        window.set_popup_width(manual.width);
        window.set_popup_height(manual.height);
        window.set_source_card_height(manual.source_height);
        window.set_resize_min_height(popup_min_window_height(
            reserved_height,
            minimum_source_height,
            window.get_max_popup_height(),
        ));

        self.manual_sizes.insert(session_id, manual);
    }

    fn cancel_resize(&mut self, session_id: u64, window: &TranslationPopup) {
        if self.active_resizes.remove(&session_id).is_none() {
            return;
        }
        window.set_resize_active(false);
        window.set_resize_changes_height(false);
        let enabled = self
            .states
            .get(&session_id)
            .is_some_and(|state| !state.pinned);
        self.set_dismissal_watch(session_id, window, enabled);
    }

    fn hide(&mut self, session_id: u64) {
        if self
            .language_menu_owner
            .is_some_and(|owner| owner.session_id == session_id)
        {
            self.close_language_menu(false);
        }
        if let Some(window) = self.windows.remove(&session_id) {
            self.disable_dismissal_watch(session_id, &window);
            reset_popup_transient_ui(&window);
            let _ = window.hide();
            slint::Timer::single_shot(Duration::ZERO, move || drop(window));
        }
        self.states.remove(&session_id);
        self.work_areas.remove(&session_id);
        self.drag_scheduler.cancel(session_id);
        self.active_resizes.remove(&session_id);
        self.pending_window_size_syncs.remove(&session_id);
        self.manual_sizes.remove(&session_id);
        self.pending_shows.remove(&session_id);
        schedule_idle_memory_trim();
    }

    fn set_dismissal_watch(&mut self, session_id: u64, window: &TranslationPopup, enabled: bool) {
        if enabled == self.dismissal_watches.contains(&session_id) {
            return;
        }
        if (self.set_popup_dismissal)(window.window(), enabled) {
            if enabled {
                self.dismissal_watches.insert(session_id);
            } else {
                self.dismissal_watches.remove(&session_id);
            }
        } else if !enabled {
            self.dismissal_watches.remove(&session_id);
        }
    }

    fn disable_dismissal_watch(&mut self, session_id: u64, window: &TranslationPopup) {
        self.set_dismissal_watch(session_id, window, false);
    }
}

fn prime_hidden_popup_window(session_id: u64, window: &TranslationPopup) -> bool {
    if let Err(error) = window.show() {
        tracing::error!(
            session_id,
            %error,
            "translation popup could not request native window creation"
        );
        return false;
    }
    if let Err(error) = window.hide() {
        let _ = window.hide();
        tracing::error!(
            session_id,
            %error,
            "translation popup could not remain hidden during native window creation"
        );
        return false;
    }
    true
}

fn popup_show_retry_delay(retried_once: bool) -> Duration {
    if retried_once {
        POPUP_SHOW_RETRY_DELAY
    } else {
        Duration::ZERO
    }
}

fn schedule_popup_show_retry(session_id: u64, generation: u64, delay: Duration) {
    slint::Timer::single_shot(delay, move || {
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.try_show(session_id, generation);
            }
        });
    });
}

fn prime_hidden_language_menu(menu: &PopupLanguageMenuWindow) -> bool {
    if let Err(error) = menu.show() {
        tracing::error!(%error, "target language menu could not request native window creation");
        return false;
    }
    if let Err(error) = menu.hide() {
        let _ = menu.hide();
        tracing::error!(%error, "target language menu could not remain hidden during native window creation");
        return false;
    }
    true
}

fn schedule_language_menu_show_retry(generation: u64, work_area: Rect, delay: Duration) {
    slint::Timer::single_shot(delay, move || {
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.try_show_language_menu(generation, work_area);
            }
        });
    });
}

fn apply_popup_preserving_draft(
    popup: &TranslationPopup,
    state: &mapper::PopupUiState,
    previous: Option<&mapper::PopupUiState>,
    manual_size: Option<ManualPopupSize>,
    geometry_locked: bool,
) {
    let draft = popup.get_source_text();
    let preserve_draft = previous.is_some_and(|previous| {
        previous.source == state.source && draft.as_str() != previous.source
    });
    if geometry_locked {
        // Core snapshots may arrive before Winit has consumed a native resize.
        // Refresh content, but leave geometry untouched until it is reconciled.
        binding::apply_popup_content(popup, state);
    } else if let Some(manual) = manual_size {
        binding::apply_popup_content(popup, state);
        let (auto_height, auto_source_height) = mapper::popup_metrics_for_width(
            &state.source,
            &state.translated,
            &state.error,
            manual.width,
        );
        let (width, height, source_height) =
            effective_manual_layout(manual, auto_height, auto_source_height);
        popup.set_popup_width(width);
        popup.set_source_card_height(source_height);
        popup.set_popup_height(height);
        popup.set_resize_min_height(popup_min_window_height(
            popup.get_resize_layout_reserve_height(),
            popup.get_resize_min_source_height(),
            popup.get_max_popup_height(),
        ));
    } else {
        binding::apply_popup(popup, state);
    }
    if !geometry_locked {
        let min_source_height = popup.get_resize_min_source_height();
        let height = popup.get_popup_height();
        let reserved_height = popup.get_resize_layout_reserve_height();
        let source_height = clamp_source_card_height(
            popup.get_source_card_height(),
            height,
            reserved_height,
            min_source_height,
        );
        popup.set_source_card_height(source_height);
        popup.set_resize_min_height(popup_min_window_height(
            reserved_height,
            min_source_height,
            popup.get_max_popup_height(),
        ));
    }
    if preserve_draft {
        popup.set_source_text(draft);
    }
    if !geometry_locked {
        popup.window().set_size(slint::LogicalSize::new(
            popup.get_popup_width(),
            popup.get_popup_height(),
        ));
    }
}

fn schedule_popup_external_size_sync(session_id: u64, popup: slint::Weak<TranslationPopup>) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        let mut retry = false;
        POPUP_REGISTRY.with(|registry| {
            let Ok(mut registry) = registry.try_borrow_mut() else {
                retry = true;
                return;
            };
            let Some(registry) = registry.as_mut() else {
                return;
            };
            if !registry.pending_window_size_syncs.remove(&session_id)
                || registry.active_resizes.contains_key(&session_id)
            {
                return;
            }
            let Some(popup) = popup.upgrade().filter(|popup| {
                popup.window().is_visible() && popup.get_session_id().max(0) as u64 == session_id
            }) else {
                return;
            };
            registry.sync_external_window_size(session_id, &popup);
        });
        if retry {
            // A native resize can be reported from inside a registry operation.
            // Retry on the next event-loop turn, after that operation returns.
            schedule_popup_external_size_sync(session_id, popup.clone());
        }
    });
}

fn popup_pointer_sink(popup: slint::Weak<TranslationPopup>) -> PopupPointerSink {
    Rc::new(move |input| {
        let Some(popup) = popup.upgrade() else {
            return;
        };
        let scale = popup.window().scale_factor().max(f32::EPSILON);
        let position = |x: f32, y: f32| slint::LogicalPosition::new(x / scale, y / scale);
        use slint::platform::{PointerEventButton, WindowEvent};
        let event = match input {
            PopupPointerInput::NativeTopResizeRequested => {
                let edge = PopupResizeEdge::Top;
                if let Some(begin_resize) = prepare_popup_resize(&popup, edge)
                    && !begin_resize(popup.window(), edge, true)
                {
                    cancel_popup_resize(&popup);
                }
                return;
            }
            PopupPointerInput::DismissRequested => {
                let session_id = popup.get_session_id().max(0) as u64;
                schedule_popup_dismissal(session_id);
                return;
            }
            PopupPointerInput::Resized { width, height } => {
                let session_id = popup.get_session_id().max(0) as u64;
                if !has_valid_native_client_size(width, height) {
                    return;
                }
                let should_schedule_sync = POPUP_REGISTRY.with(|registry| {
                    // Slint/Winit may synchronously emit WM_SIZE while a
                    // registry operation calls Window::set_size(). That size
                    // is already reflected by the caller, so ignore only that
                    // re-entrant notification instead of panicking across the
                    // native window procedure boundary.
                    let Ok(mut registry) = registry.try_borrow_mut() else {
                        return false;
                    };
                    if let Some(registry) = registry.as_mut() {
                        // A resize owned by this registry is applied once when the
                        // native loop ends; every other size change is external.
                        if registry.active_resizes.contains_key(&session_id) {
                            false
                        } else {
                            registry.pending_window_size_syncs.insert(session_id)
                        }
                    } else {
                        false
                    }
                });
                if should_schedule_sync {
                    schedule_popup_external_size_sync(session_id, popup.as_weak());
                }
                return;
            }
            PopupPointerInput::ResizeFinished { width, height } => {
                let session_id = popup.get_session_id().max(0) as u64;
                let weak = popup.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    let Some(popup) = weak.upgrade().filter(|popup| {
                        popup.window().is_visible()
                            && popup.get_session_id().max(0) as u64 == session_id
                    }) else {
                        return;
                    };
                    let scale = popup.window().scale_factor().max(f32::EPSILON);
                    POPUP_REGISTRY.with(|registry| {
                        let Ok(mut registry) = registry.try_borrow_mut() else {
                            return;
                        };
                        if let Some(registry) = registry.as_mut() {
                            registry.finish_resize(
                                session_id,
                                &popup,
                                width / scale,
                                height / scale,
                            );
                        }
                    });
                });
                return;
            }
            PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
                position: position(x, y),
            },
            PopupPointerInput::Exited => WindowEvent::PointerExited,
            PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::LeftReleased { x, y } => WindowEvent::PointerReleased {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::Scrolled {
                x,
                y,
                delta_x,
                delta_y,
            } => WindowEvent::PointerScrolled {
                position: position(x, y),
                delta_x: delta_x / scale,
                delta_y: delta_y / scale,
            },
        };
        if let Err(error) = popup.window().dispatch_event_with_result(event) {
            tracing::warn!(%error, "translation popup pointer event could not be dispatched");
        }
    })
}

fn language_menu_pointer_sink(menu: slint::Weak<PopupLanguageMenuWindow>) -> PopupPointerSink {
    Rc::new(move |input| {
        let Some(menu) = menu.upgrade() else {
            return;
        };
        if input == PopupPointerInput::DismissRequested {
            schedule_language_menu_dismissal();
            return;
        }
        let scale = menu.window().scale_factor().max(f32::EPSILON);
        let position = |x: f32, y: f32| slint::LogicalPosition::new(x / scale, y / scale);
        use slint::platform::{PointerEventButton, WindowEvent};
        let event = match input {
            PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
                position: position(x, y),
            },
            PopupPointerInput::Exited => WindowEvent::PointerExited,
            PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::LeftReleased { x, y } => WindowEvent::PointerReleased {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::Scrolled {
                x,
                y,
                delta_x,
                delta_y,
            } => WindowEvent::PointerScrolled {
                position: position(x, y),
                delta_x: delta_x / scale,
                delta_y: delta_y / scale,
            },
            PopupPointerInput::DismissRequested | PopupPointerInput::NativeTopResizeRequested => {
                return;
            }
            PopupPointerInput::Resized { .. } | PopupPointerInput::ResizeFinished { .. } => return,
        };
        if let Err(error) = menu.window().dispatch_event_with_result(event) {
            tracing::warn!(%error, "target language menu pointer event could not be dispatched");
        }
    })
}

fn schedule_language_menu_dismissal() {
    slint::Timer::single_shot(Duration::ZERO, move || {
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.close_language_menu(true);
            }
        });
    });
}

fn schedule_popup_dismissal(session_id: u64) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        let handler = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let registry = registry.as_mut()?;
            if registry
                .states
                .get(&session_id)
                .is_none_or(|state| state.pinned)
            {
                return None;
            }
            let handler = registry.handler.as_ref().map(Rc::clone);
            registry.hide(session_id);
            handler
        });
        if let Some(handler) = handler {
            handler(AppEvent::PopupClosed {
                session_id: lexift_core::domain::translation::PopupSessionId::new(session_id),
            });
        }
    });
}

fn clamp_popup_to_work_area(popup: &TranslationPopup, work_area: Rect) {
    let position = popup.window().position();
    let size = popup.window().size();
    let min_x = work_area.left;
    let min_y = work_area.top;
    let max_x = (work_area.right - size.width as i32).max(min_x);
    let max_y = (work_area.bottom - size.height as i32).max(min_y);
    let x = position.x.clamp(min_x, max_x.max(min_x));
    let y = position.y.clamp(min_y, max_y.max(min_y));
    if x != position.x || y != position.y {
        popup
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
}

fn apply_popup_resize_bounds(popup: &TranslationPopup, work_area: Rect) {
    let scale = popup.window().scale_factor().max(f32::EPSILON);
    let width = ((work_area.right - work_area.left).max(1) as f32 / scale).max(POPUP_MIN_WIDTH);
    let height = ((work_area.bottom - work_area.top).max(1) as f32 / scale).max(336.0);
    // The maximum must never fall below the current minimum, or the sizing loop would receive an
    // inverted range.
    popup.set_max_popup_width(width.max(POPUP_MIN_WIDTH));
    popup.set_max_popup_height(height.max(popup.get_resize_min_height()).max(336.0));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SettingsToastRecord {
    id: i32,
    message: String,
    error: bool,
}

pub struct Ui {
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    background_mode: Rc<Cell<bool>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

struct AppWindowRegistry {
    main: Option<AppWindow>,
    settings: Option<SettingsWindow>,
    configure_resize_background: ConfigureResizeBackground,
    configure_window_paint_repair: ConfigureWindowPaintRepair,
    latest_state: AppState,
    show_selection_demo: bool,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    background_mode: Rc<Cell<bool>>,
    credential_generation: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    main_close_generation: u64,
    settings_close_generation: u64,
}

thread_local! {
    static APP_WINDOW_REGISTRY: RefCell<Option<AppWindowRegistry>> = const { RefCell::new(None) };
    static SELECTION_TOOLBAR_REGISTRY: RefCell<Option<SelectionToolbarRegistry>> = const { RefCell::new(None) };
}

struct SelectionToolbarRegistry {
    window: Option<SelectionToolbarWindow>,
    selection: Option<Selection>,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    complete: CompletePassiveWindowShow,
    dismiss: SetPopupDismissal,
    work_area: PopupWorkArea,
    cursor_position: ToolbarCursorPosition,
    fade_timer: slint::Timer,
    generation: u64,
}

impl SelectionToolbarRegistry {
    fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.fade_timer.stop();
        if let Some(window) = self.window.take() {
            let _ = (self.dismiss)(window.window(), false);
            let _ = window.hide();
        }
        self.selection = None;
    }

    fn sync(&mut self, selection: Option<Selection>) {
        if self.selection == selection {
            return;
        }
        self.close();
        let Some(selection) = selection else { return };
        let Ok(window) = SelectionToolbarWindow::new() else {
            return;
        };
        let Some(handler) = self.handler.as_ref() else {
            return;
        };
        let action = Rc::clone(handler);
        window.on_translate_requested(move || action(AppEvent::SelectionToolbarTranslateRequested));
        let action = Rc::clone(handler);
        window.on_copy_requested(move || action(AppEvent::SelectionToolbarCopyRequested));
        let action = Rc::clone(handler);
        window.window().on_close_requested(move || {
            action(AppEvent::SelectionToolbarDismissRequested);
            slint::CloseRequestResponse::KeepWindowShown
        });
        window
            .window()
            .set_size(slint::LogicalSize::new(96.0, 48.0));
        self.selection = Some(selection);
        self.window = Some(window);
        self.generation = self.generation.wrapping_add(1);
        schedule_toolbar_show(self.generation, 0);
    }
}

fn schedule_toolbar_show(generation: u64, attempt: u8) {
    slint::Timer::single_shot(
        if attempt == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(16)
        },
        move || {
            SELECTION_TOOLBAR_REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(registry) = slot
                    .as_mut()
                    .filter(|registry| registry.generation == generation)
                else {
                    return;
                };
                let Some(window) = registry.window.as_ref() else {
                    return;
                };
                match (registry.prepare)(window.window()) {
                    PassiveWindowPreparation::Ready => {
                        if let Some(anchor) = registry
                            .selection
                            .as_ref()
                            .and_then(|selection| selection.anchor)
                            && let Some(area) = (registry.work_area)(anchor)
                        {
                            let size = window.window().size();
                            let point =
                                placement::place_popup(anchor, size.width, size.height, area, 8, 8)
                                    .position;
                            window
                                .window()
                                .set_position(slint::PhysicalPosition::new(point.x, point.y));
                        }
                        if window.show().is_ok() {
                            if !(registry.complete)(
                                window.window(),
                                toolbar_pointer_sink(window.as_weak()),
                            ) {
                                registry.close();
                            } else {
                                let _ = (registry.dismiss)(window.window(), true);
                                registry.fade_timer.start(
                                    slint::TimerMode::Repeated,
                                    TOOLBAR_FADE_SAMPLE_INTERVAL,
                                    update_toolbar_fade,
                                );
                            }
                        } else {
                            registry.close();
                        }
                    }
                    PassiveWindowPreparation::Pending if attempt < 20 => {
                        let _ = window.show();
                        let _ = window.hide();
                        schedule_toolbar_show(generation, attempt + 1);
                    }
                    _ => registry.close(),
                }
            });
        },
    );
}

/// Measures from the cursor to the nearest point on the toolbar in logical pixels.
fn toolbar_opacity(cursor: Point, bounds: Rect, scale_factor: f32) -> f32 {
    let dx = (i64::from(bounds.left) - i64::from(cursor.x))
        .max(0)
        .max(i64::from(cursor.x) - i64::from(bounds.right));
    let dy = (i64::from(bounds.top) - i64::from(cursor.y))
        .max(0)
        .max(i64::from(cursor.y) - i64::from(bounds.bottom));
    let physical_distance = ((dx as f64).hypot(dy as f64)) / f64::from(scale_factor.max(0.1));
    ((TOOLBAR_FADE_END_LOGICAL_PX - physical_distance)
        / (TOOLBAR_FADE_END_LOGICAL_PX - TOOLBAR_FADE_START_LOGICAL_PX))
        .clamp(0.0, 1.0) as f32
}

fn update_toolbar_fade() {
    let dismiss = SELECTION_TOOLBAR_REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let registry = slot.as_mut()?;
        let cursor = (registry.cursor_position)()?;
        let window = registry.window.as_ref()?;
        let position = window.window().position();
        let size = window.window().size();
        let bounds = Rect {
            left: position.x,
            top: position.y,
            right: position.x.saturating_add(size.width as i32),
            bottom: position.y.saturating_add(size.height as i32),
        };
        let opacity = toolbar_opacity(cursor, bounds, window.window().scale_factor());
        if (window.get_fade_opacity() - opacity).abs() > 0.001 {
            window.set_fade_opacity(opacity);
        }
        if opacity > 0.0 {
            return None;
        }
        let handler = registry.handler.as_ref().map(Rc::clone);
        registry.close();
        handler
    });
    if let Some(handler) = dismiss {
        handler(AppEvent::SelectionToolbarDismissRequested);
    }
}

fn toolbar_pointer_sink(weak: slint::Weak<SelectionToolbarWindow>) -> PopupPointerSink {
    Rc::new(move |input| {
        let Some(window) = weak.upgrade() else { return };
        if input == PopupPointerInput::DismissRequested {
            SELECTION_TOOLBAR_REGISTRY.with(|slot| {
                if let Some(registry) = slot.borrow().as_ref()
                    && let Some(handler) = registry.handler.as_ref()
                {
                    handler(AppEvent::SelectionToolbarDismissRequested);
                }
            });
            return;
        }
        use slint::platform::{PointerEventButton, WindowEvent};
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let position = |x: f32, y: f32| slint::LogicalPosition::new(x / scale, y / scale);
        let event = match input {
            PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
                position: position(x, y),
            },
            PopupPointerInput::Exited => WindowEvent::PointerExited,
            PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::LeftReleased { x, y } => WindowEvent::PointerReleased {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            _ => return,
        };
        let _ = window.window().dispatch_event_with_result(event);
    })
}

fn install_resize_background(
    configure: &ConfigureResizeBackground,
    window: &slint::Window,
    color: slint::Color,
) -> bool {
    (configure)(window, [color.red(), color.green(), color.blue()])
}

/// Invalidate all software-rendered pixels after Windows finishes a native move or resize.
fn repair_window_paint(window: &slint::Window) {
    let scale = window.scale_factor().max(f32::EPSILON);
    let logical = window.size().to_logical(scale);
    if logical.width <= 0.0 || logical.height <= 0.0 {
        return;
    }
    let mut region = i_slint_core::partial_renderer::DirtyRegion::default();
    region.add_rect(i_slint_core::lengths::LogicalRect::from_size(
        i_slint_core::lengths::LogicalSize::new(logical.width, logical.height),
    ));
    i_slint_core::window::WindowInner::from_pub(window)
        .window_adapter()
        .renderer()
        .mark_dirty_region(region);
    window.request_redraw();
}

fn window_paint_repair<T: ComponentHandle + 'static>(component: &T) -> WindowPaintRepair {
    let weak = component.as_weak();
    Rc::new(move || {
        let Some(component) = weak
            .upgrade()
            .filter(|component| component.window().is_visible())
        else {
            return;
        };
        repair_window_paint(component.window());
    })
}

fn retry_settings_resize_background(
    configure: ConfigureResizeBackground,
    configure_repair: ConfigureWindowPaintRepair,
    settings: slint::Weak<SettingsWindow>,
    color: slint::Color,
    attempts: u32,
) {
    if attempts == 0 {
        tracing::debug!("settings resize background fill was not installed before retry expired");
        return;
    }
    slint::Timer::single_shot(RESIZE_BACKGROUND_RETRY_INTERVAL, move || {
        let Some(settings) = settings.upgrade() else {
            return;
        };
        if !install_settings_platform_hooks(&configure, &configure_repair, &settings, color) {
            retry_settings_resize_background(
                configure,
                configure_repair,
                settings.as_weak(),
                color,
                attempts - 1,
            );
        }
    });
}

fn install_settings_platform_hooks(
    configure: &ConfigureResizeBackground,
    configure_repair: &ConfigureWindowPaintRepair,
    settings: &SettingsWindow,
    color: slint::Color,
) -> bool {
    let background = install_resize_background(configure, settings.window(), color);
    let repair = configure_repair(settings.window(), window_paint_repair(settings));
    background && repair
}

fn ensure_main_window(registry: &mut AppWindowRegistry) -> Result<AppWindow, slint::PlatformError> {
    if let Some(main) = &registry.main {
        return Ok(main.clone_strong());
    }
    let main = AppWindow::new()?;
    main.set_show_selection_demo(registry.show_selection_demo);
    binding::apply_main(&main, &mapper::view_state(&registry.latest_state));
    if let Some(handler) = registry.handler.clone() {
        wire_main_callbacks(&main, handler, Rc::clone(&registry.background_mode));
    }
    registry.main = Some(main.clone_strong());
    Ok(main)
}

fn ensure_settings_window(
    registry: &mut AppWindowRegistry,
) -> Result<SettingsWindow, slint::PlatformError> {
    if let Some(settings) = &registry.settings {
        return Ok(settings.clone_strong());
    }
    let settings = SettingsWindow::new()?;
    let _ = install_settings_platform_hooks(
        &registry.configure_resize_background,
        &registry.configure_window_paint_repair,
        &settings,
        settings.get_resize_fallback_color(),
    );
    settings.window().set_size(slint::LogicalSize::new(
        SETTINGS_DEFAULT_WIDTH,
        SETTINGS_DEFAULT_HEIGHT,
    ));
    binding::apply_settings(&settings, &mapper::view_state(&registry.latest_state));
    if let Some(handler) = registry.handler.clone() {
        wire_settings_callbacks(
            &settings,
            handler,
            Arc::clone(&registry.credential_generation),
            Arc::clone(&registry.toast_records),
        );
    }
    registry.settings = Some(settings.clone_strong());
    Ok(settings)
}

fn schedule_main_window_destruction() {
    let generation = APP_WINDOW_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.as_mut()?;
        registry.main_close_generation = registry.main_close_generation.wrapping_add(1);
        Some(registry.main_close_generation)
    });
    let Some(generation) = generation else {
        return;
    };
    slint::Timer::single_shot(Duration::ZERO, move || {
        APP_WINDOW_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut()
                && registry.main_close_generation == generation
                && let Some(main) = registry.main.take()
            {
                let _ = main.hide();
                drop(main);
            }
        });
        schedule_idle_memory_trim();
    });
}

fn schedule_settings_window_destruction() {
    let generation = APP_WINDOW_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.as_mut()?;
        registry.settings_close_generation = registry.settings_close_generation.wrapping_add(1);
        Some(registry.settings_close_generation)
    });
    let Some(generation) = generation else {
        return;
    };
    slint::Timer::single_shot(Duration::ZERO, move || {
        APP_WINDOW_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            if let Some(registry) = registry.as_mut()
                && registry.settings_close_generation == generation
                && let Some(settings) = registry.settings.take()
            {
                settings.invoke_reset_settings_view();
                clear_credential_transient(&settings, &registry.credential_generation);
                clear_settings_toasts(&settings, &registry.toast_records);
                let _ = settings.hide();
                drop(settings);
            }
        });
        schedule_idle_memory_trim();
    });
}

fn wire_main_callbacks(
    main: &AppWindow,
    handler: Rc<dyn Fn(AppEvent)>,
    background_mode: Rc<Cell<bool>>,
) {
    let main_close_handler = Rc::clone(&handler);
    main.window().on_close_requested(move || {
        match close_policy(background_mode.get()) {
            MainWindowClosePolicy::HideToTray => {
                schedule_main_window_destruction();
            }
            MainWindowClosePolicy::Exit => main_close_handler(AppEvent::ExitRequested),
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let selection_handler = Rc::clone(&handler);
    main.on_selection_translation_requested(move || {
        selection_handler(AppEvent::SelectionTranslationRequested);
    });
    let input_handler = Rc::clone(&handler);
    main.on_input_translation_requested(move |text| {
        input_handler(AppEvent::InputTranslationRequested {
            text: text.to_string(),
        });
    });
    main.on_settings_window_requested(move || handler(AppEvent::SettingsWindowRequested));
}

fn wire_settings_callbacks(
    settings: &SettingsWindow,
    handler: Rc<dyn Fn(AppEvent)>,
    credential_generation: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
) {
    settings.window().on_close_requested(move || {
        schedule_settings_window_destruction();
        slint::CloseRequestResponse::KeepWindowShown
    });
    let settings_weak = settings.as_weak();
    let toast_records_for_dismiss = Arc::clone(&toast_records);
    settings.on_toast_dismiss_requested(move |id| {
        remove_settings_toast(&settings_weak, &toast_records_for_dismiss, id);
    });
    settings.on_hotkey_key_pressed(move |text, control, alt, shift, meta| {
        HotkeyConfig::from_key_event(&text, control, alt, shift, meta)
            .map(|config| config.to_string().into())
            .unwrap_or_default()
    });
    let settings_handler = Rc::clone(&handler);
    settings.on_hotkey_change_requested(move |hotkey| {
        if let Ok(hotkey) = hotkey.to_string().parse::<HotkeyConfig>() {
            settings_handler(AppEvent::SettingsChangeRequested {
                change: SettingsChange::Hotkey(hotkey),
            });
        }
    });
    let settings_handler = Rc::clone(&handler);
    settings.on_launch_at_login_change_requested(move |enabled| {
        settings_handler(AppEvent::SettingsChangeRequested {
            change: SettingsChange::LaunchAtLogin(enabled),
        });
    });
    let settings_handler = Rc::clone(&handler);
    settings.on_selection_toolbar_change_requested(move |enabled| {
        settings_handler(AppEvent::SettingsChangeRequested {
            change: SettingsChange::SelectionToolbar(enabled),
        });
    });
    let credential_handler = Rc::clone(&handler);
    settings.on_credential_save_requested(move |secret| {
        credential_handler(AppEvent::CredentialSaveRequested {
            secret: CredentialSecret::new(secret.to_string()),
        });
    });
    let credential_handler = Rc::clone(&handler);
    settings.on_credential_remove_requested(move || {
        credential_handler(AppEvent::CredentialRemoveRequested);
    });
    let settings_weak = settings.as_weak();
    let generation = Arc::clone(&credential_generation);
    let credential_handler = Rc::clone(&handler);
    settings.on_credential_reveal_requested(move || {
        let generation = begin_credential_access(&settings_weak, &generation);
        credential_handler(AppEvent::CredentialAccessRequested {
            purpose: CredentialAccessPurpose::Reveal,
            generation,
        });
    });
    let settings_weak = settings.as_weak();
    let generation = Arc::clone(&credential_generation);
    let credential_handler = Rc::clone(&handler);
    settings.on_credential_edit_requested(move || {
        let generation = begin_credential_access(&settings_weak, &generation);
        credential_handler(AppEvent::CredentialAccessRequested {
            purpose: CredentialAccessPurpose::Edit,
            generation,
        });
    });
    let settings_weak = settings.as_weak();
    let generation = Arc::clone(&credential_generation);
    let credential_handler = Rc::clone(&handler);
    settings.on_credential_copy_requested(move || {
        let generation = begin_credential_access(&settings_weak, &generation);
        credential_handler(AppEvent::CredentialAccessRequested {
            purpose: CredentialAccessPurpose::Copy,
            generation,
        });
    });
    let settings_weak = settings.as_weak();
    let generation = Arc::clone(&credential_generation);
    settings.on_credential_hide_requested(move || {
        if let Some(settings) = settings_weak.upgrade() {
            clear_credential_transient(&settings, &generation);
        }
    });
    let settings_weak = settings.as_weak();
    let generation = Arc::clone(&credential_generation);
    settings.on_credential_edit_cancel_requested(move || {
        if let Some(settings) = settings_weak.upgrade() {
            clear_credential_transient(&settings, &generation);
        }
    });
    let settings_weak = settings.as_weak();
    settings.on_settings_menu_selected(move |index| {
        if let Some(settings) = settings_weak.upgrade() {
            if settings.get_settings_menu_provider_mode() {
                let provider = ProviderConfig::DeepL;
                if settings.get_draft_provider_id().as_str() != provider.id() {
                    settings.set_draft_provider_id(provider.id().into());
                    handler(AppEvent::SettingsChangeRequested {
                        change: SettingsChange::Provider(provider),
                    });
                }
            } else if settings.get_draft_target_index() != index {
                settings.set_draft_target_index(index);
                handler(AppEvent::SettingsChangeRequested {
                    change: SettingsChange::TargetLanguage(language_for_index(index)),
                });
            }
        }
    });
}

impl Ui {
    pub fn new(
        initial_state: &AppState,
        show_selection_demo: bool,
        prepare_passive_window: fn(&slint::Window) -> PassiveWindowPreparation,
        window_lifecycle: WindowLifecycleCallbacks,
    ) -> Result<Self, slint::PlatformError> {
        SELECTION_TOOLBAR_REGISTRY.with(|registry| {
            *registry.borrow_mut() = Some(SelectionToolbarRegistry {
                window: None,
                selection: None,
                handler: None,
                prepare: prepare_passive_window,
                complete: Rc::clone(&window_lifecycle.complete_toolbar_show),
                dismiss: Rc::clone(&window_lifecycle.set_popup_dismissal),
                work_area: Rc::clone(&window_lifecycle.popup_work_area),
                cursor_position: Rc::clone(&window_lifecycle.toolbar_cursor_position),
                fade_timer: slint::Timer::default(),
                generation: 0,
            });
        });
        IDLE_TRIM_CALLBACK.with(|callback| {
            callback.set(Some(window_lifecycle.trim_process_working_set));
        });
        cancel_idle_memory_trim();
        let credential_generation = Arc::new(AtomicU64::new(0));
        let toast_next_id = Arc::new(AtomicU64::new(0));
        let toast_records = Arc::new(Mutex::new(Vec::new()));
        let background_mode = Rc::new(Cell::new(false));
        APP_WINDOW_REGISTRY.with(|registry| {
            *registry.borrow_mut() = Some(AppWindowRegistry {
                main: None,
                settings: None,
                configure_resize_background: Rc::clone(
                    &window_lifecycle.configure_resize_background,
                ),
                configure_window_paint_repair: Rc::clone(
                    &window_lifecycle.configure_window_paint_repair,
                ),
                latest_state: initial_state.clone(),
                show_selection_demo,
                handler: None,
                background_mode: Rc::clone(&background_mode),
                credential_generation: Arc::clone(&credential_generation),
                toast_records: Arc::clone(&toast_records),
                main_close_generation: 0,
                settings_close_generation: 0,
            });
        });
        POPUP_REGISTRY.with(|registry| {
            *registry.borrow_mut() = Some(PopupRegistry {
                language_menu: None,
                language_menu_owner: None,
                language_menu_pending: None,
                language_menu_dismissal_watched: false,
                windows: HashMap::new(),
                states: mapper::popup_states(initial_state)
                    .into_iter()
                    .map(|state| (state.session_id, state))
                    .collect(),
                work_areas: HashMap::new(),
                handler: None,
                dismissal_watches: HashSet::new(),
                drag_scheduler: PopupDragScheduler::default(),
                manual_sizes: HashMap::new(),
                active_resizes: HashMap::new(),
                pending_window_size_syncs: HashSet::new(),
                pending_shows: HashMap::new(),
                next_show_generation: 0,
                prepare_passive_window,
                configure_translation_popup_corners: window_lifecycle
                    .configure_translation_popup_corners,
                configure_resize_background: Rc::clone(
                    &window_lifecycle.configure_resize_background,
                ),
                configure_window_paint_repair: Rc::clone(
                    &window_lifecycle.configure_window_paint_repair,
                ),
                complete_passive_window_show: Rc::clone(
                    &window_lifecycle.complete_passive_window_show,
                ),
                begin_window_drag: window_lifecycle.begin_window_drag,
                begin_window_resize: Rc::clone(&window_lifecycle.begin_window_resize),
                popup_work_area: Rc::clone(&window_lifecycle.popup_work_area),
                set_popup_dismissal: Rc::clone(&window_lifecycle.set_popup_dismissal),
                attach_tool_window: Rc::clone(&window_lifecycle.attach_tool_window),
            });
        });
        Ok(Self {
            credential_generation,
            toast_next_id,
            toast_records,
            background_mode,
            activate_user_requested_window: window_lifecycle.activate_user_requested_window,
        })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            credential_generation: Arc::clone(&self.credential_generation),
            toast_next_id: Arc::clone(&self.toast_next_id),
            toast_records: Arc::clone(&self.toast_records),
            activate_user_requested_window: self.activate_user_requested_window,
        }
    }

    pub fn on_event(
        &self,
        handler: impl Fn(AppEvent) + 'static,
        _screen_context: impl Fn() -> Option<(Point, Rect)> + 'static,
    ) {
        let handler: Rc<dyn Fn(AppEvent)> = Rc::new(handler);
        APP_WINDOW_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.handler = Some(Rc::clone(&handler));
                if let Some(main) = registry.main.as_ref() {
                    wire_main_callbacks(
                        main,
                        Rc::clone(&handler),
                        Rc::clone(&self.background_mode),
                    );
                }
                if let Some(settings) = registry.settings.as_ref() {
                    wire_settings_callbacks(
                        settings,
                        Rc::clone(&handler),
                        Arc::clone(&self.credential_generation),
                        Arc::clone(&self.toast_records),
                    );
                }
            }
        });
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.handler = Some(Rc::clone(&handler));
                for popup in registry.windows.values() {
                    wire_popup_close(popup, Rc::clone(&handler));
                    wire_popup_callbacks(popup, Rc::clone(&handler));
                }
            }
        });
        SELECTION_TOOLBAR_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.handler = Some(handler);
            }
        });
    }

    pub fn set_background_mode(&self, enabled: bool) {
        self.background_mode.set(enabled);
    }

    pub fn run(&self, show_main_window: bool) -> Result<(), slint::PlatformError> {
        if show_main_window {
            cancel_idle_memory_trim();
            APP_WINDOW_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                let main = ensure_main_window(registry.as_mut().expect("UI registry initialized"))?;
                main.show()
            })?;
        }
        slint::run_event_loop_until_quit()?;
        SELECTION_TOOLBAR_REGISTRY.with(|registry| {
            if let Some(mut registry) = registry.borrow_mut().take() {
                registry.close();
            }
        });
        POPUP_REGISTRY.with(|registry| {
            if let Some(mut registry) = registry.borrow_mut().take() {
                registry.close_language_menu(false);
                let windows: Vec<_> = registry
                    .windows
                    .iter()
                    .map(|(session_id, window)| (*session_id, window.clone_strong()))
                    .collect();
                for (session_id, window) in windows {
                    registry.disable_dismissal_watch(session_id, &window);
                }
                for window in registry.windows.drain().map(|(_, window)| window) {
                    reset_popup_transient_ui(&window);
                    let _ = window.hide();
                }
            }
        });
        APP_WINDOW_REGISTRY.with(|registry| {
            if let Some(mut registry) = registry.borrow_mut().take() {
                if let Some(settings) = registry.settings.take() {
                    settings.invoke_reset_settings_view();
                    clear_credential_transient(&settings, &registry.credential_generation);
                    clear_settings_toasts(&settings, &registry.toast_records);
                    let _ = settings.hide();
                }
                if let Some(main) = registry.main.take() {
                    let _ = main.hide();
                }
            }
        });
        Ok(())
    }
}

fn popup_session_id(popup: &TranslationPopup) -> lexift_core::domain::translation::PopupSessionId {
    lexift_core::domain::translation::PopupSessionId::new(popup.get_session_id().max(0) as u64)
}

fn reset_popup_transient_ui(popup: &TranslationPopup) {
    popup.set_source_menu_open(false);
    popup.set_target_menu_open(false);
    popup.set_resize_hover(-1);
    popup.set_resize_active(false);
}

fn wire_popup_close(popup: &TranslationPopup, handler: Rc<dyn Fn(AppEvent)>) {
    let weak = popup.as_weak();
    popup.window().on_close_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            reset_popup_transient_ui(&popup);
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.hide(session_id.value());
                } else {
                    let _ = popup.hide();
                }
            });
            handler(AppEvent::PopupClosed { session_id });
        }
        slint::CloseRequestResponse::HideWindow
    });
}

fn install_popup_drag_render_notifier(popup: &TranslationPopup) -> bool {
    let weak = popup.as_weak();
    match popup.window().set_rendering_notifier(move |state, _| {
        if !matches!(state, slint::RenderingState::AfterRendering) {
            return;
        }
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        schedule_popup_drag_dispatch(popup.as_weak(), session_id, Duration::ZERO);
    }) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "translation popup could not synchronize dragging with rendering");
            false
        }
    }
}

fn schedule_popup_drag_dispatch(
    popup: slint::Weak<TranslationPopup>,
    session_id: u64,
    delay: Duration,
) {
    slint::Timer::single_shot(delay, move || {
        // Copy the callback and release the registry borrow before entering the
        // native move loop. ReleaseCapture can synchronously dispatch UI events.
        let begin_window_drag = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let registry = registry.as_mut()?;
            if !registry.drag_scheduler.frame_rendered(session_id)
                || !registry.drag_scheduler.take_dispatch(session_id)
            {
                return None;
            }
            Some(registry.begin_window_drag)
        });
        let Some(popup) = popup.upgrade().filter(|popup| {
            popup.window().is_visible() && popup.get_session_id().max(0) as u64 == session_id
        }) else {
            return;
        };
        if let Some(begin_window_drag) = begin_window_drag {
            begin_window_drag(popup.window());
        }
    });
}

fn wire_language_menu_callbacks(menu: &PopupLanguageMenuWindow) {
    menu.on_language_selected(move |selected_index| {
        let dispatch = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let registry = registry.as_mut()?;
            let owner = registry.language_menu_owner?;
            let popup = registry.existing_window(owner.session_id)?;
            let source = popup.get_source_text().to_string();
            let source_language = match owner.kind {
                LanguageMenuKind::Source => source_language_for_index(selected_index),
                LanguageMenuKind::Target => source_language_for_index(popup.get_source_index()),
            };
            let target_language = match owner.kind {
                LanguageMenuKind::Source => language_for_index(popup.get_target_index()),
                LanguageMenuKind::Target => language_for_index(selected_index),
            };
            let handler = registry.handler.as_ref().map(Rc::clone)?;
            registry.close_language_menu(true);
            Some((
                handler,
                owner.session_id,
                source,
                source_language,
                target_language,
            ))
        });
        if let Some((handler, session_id, source, source_language, target_language)) = dispatch {
            handler(AppEvent::PopupTranslationRequested {
                session_id: lexift_core::domain::translation::PopupSessionId::new(session_id),
                text: source,
                source_language,
                target_language,
            });
        }
    });
    menu.on_dismiss_requested(schedule_language_menu_dismissal);
    menu.window().on_close_requested(move || {
        schedule_language_menu_dismissal();
        slint::CloseRequestResponse::KeepWindowShown
    });
}

fn prepare_popup_resize(
    popup: &TranslationPopup,
    edge: PopupResizeEdge,
) -> Option<BeginWindowResize> {
    let session_id = popup.get_session_id().max(0) as u64;
    POPUP_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.as_mut()?;
        if registry.active_resizes.contains_key(&session_id) {
            return None;
        }
        registry.refresh_popup_work_area(session_id, popup);
        registry.start_resize(session_id, popup, edge);
        Some(Rc::clone(&registry.begin_window_resize))
    })
}

fn cancel_popup_resize(popup: &TranslationPopup) {
    let session_id = popup.get_session_id().max(0) as u64;
    POPUP_REGISTRY.with(|registry| {
        if let Some(registry) = registry.borrow_mut().as_mut() {
            registry.cancel_resize(session_id, popup);
        }
    });
}

fn wire_popup_callbacks(popup: &TranslationPopup, handler: Rc<dyn Fn(AppEvent)>) {
    let drag_render_notifier_installed = install_popup_drag_render_notifier(popup);
    let weak = popup.as_weak();
    popup.on_drag_requested(move || {
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        let scheduled = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let Some(registry) = registry.as_mut() else {
                return false;
            };
            if registry
                .language_menu_owner
                .is_some_and(|owner| owner.session_id == session_id)
            {
                registry.close_language_menu(true);
            }
            registry.drag_scheduler.request(session_id)
        });
        if !scheduled {
            return;
        }
        popup.window().request_redraw();
        if !drag_render_notifier_installed {
            schedule_popup_drag_dispatch(popup.as_weak(), session_id, POPUP_DRAG_FALLBACK_DELAY);
        }
    });

    let weak = popup.as_weak();
    popup.on_resize_requested(move |edge_index| {
        let Some(edge) = PopupResizeEdge::from_index(edge_index) else {
            return;
        };
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let Some(begin_resize) = prepare_popup_resize(&popup, edge) else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        let weak = popup.as_weak();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(popup) = weak.upgrade().filter(|popup| {
                popup.window().is_visible() && popup.get_session_id().max(0) as u64 == session_id
            }) else {
                return;
            };
            if !begin_resize(popup.window(), edge, false) {
                cancel_popup_resize(&popup);
            }
        });
    });

    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_translate_requested(move |text, source_index, target_index| {
        if let Some(popup) = weak.upgrade() {
            event_handler(AppEvent::PopupTranslationRequested {
                session_id: popup_session_id(&popup),
                text: text.to_string(),
                source_language: source_language_for_index(source_index),
                target_language: language_for_index(target_index),
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_source_menu_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.open_language_menu(session_id, &popup, LanguageMenuKind::Source);
                }
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_target_menu_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.open_language_menu(session_id, &popup, LanguageMenuKind::Target);
                }
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_language_menu_dismiss_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut()
                    && registry
                        .language_menu_owner
                        .is_some_and(|owner| owner.session_id == session_id)
                {
                    registry.close_language_menu(true);
                }
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_pin_requested(move |pinned| {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    let enabled = !pinned
                        && !registry
                            .language_menu_owner
                            .is_some_and(|owner| owner.session_id == session_id.value());
                    registry.set_dismissal_watch(session_id.value(), &popup, enabled);
                }
            });
            event_handler(AppEvent::PopupPinChanged { session_id, pinned });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_close_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            reset_popup_transient_ui(&popup);
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.hide(session_id.value());
                } else {
                    let _ = popup.hide();
                }
            });
            event_handler(AppEvent::PopupClosed { session_id });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_copy_requested(move |source| {
        if let Some(popup) = weak.upgrade() {
            let text = if source {
                popup.get_source_text()
            } else {
                popup.get_translated_text()
            };
            event_handler(AppEvent::PopupCopyRequested {
                session_id: popup_session_id(&popup),
                text: text.to_string(),
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_speech_requested(move |source| {
        if let Some(popup) = weak.upgrade() {
            let text = if source {
                popup.get_source_text()
            } else {
                popup.get_translated_text()
            };
            event_handler(AppEvent::PopupSpeechRequested {
                session_id: popup_session_id(&popup),
                source,
                text: text.to_string(),
                language: if source {
                    let selected = popup.get_source_language();
                    let language = if selected.is_empty() {
                        popup.get_detected_language()
                    } else {
                        selected
                    };
                    if language.is_empty() {
                        None
                    } else {
                        Some(Language(language.to_string()))
                    }
                } else {
                    Some(language_for_index(popup.get_target_index()))
                },
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_feedback_dismiss_requested(move || {
        if let Some(popup) = weak.upgrade() {
            handler(AppEvent::PopupFeedbackCleared {
                session_id: popup_session_id(&popup),
            });
        }
    });
}

fn reset_credential_view(settings: &SettingsWindow) {
    settings.set_credential_draft("".into());
    settings.set_credential_transient_secret("".into());
    settings.set_credential_revealed(false);
    settings.set_credential_editing(false);
    settings.set_credential_secret_visible(false);
    settings.set_credential_reveal_dismiss_armed(false);
    settings.set_credential_request_pending(false);
}

fn clear_credential_transient(settings: &SettingsWindow, generation: &AtomicU64) {
    generation.fetch_add(1, Ordering::SeqCst);
    reset_credential_view(settings);
}

fn begin_credential_access(settings: &slint::Weak<SettingsWindow>, generation: &AtomicU64) -> u64 {
    let generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(settings) = settings.upgrade() {
        reset_credential_view(&settings);
        settings.set_credential_request_pending(true);
    }
    generation
}

fn credential_session_is_current(generation: &AtomicU64, expected: u64) -> bool {
    generation.load(Ordering::SeqCst) == expected
}

fn render_settings_toasts(
    settings: &SettingsWindow,
    records: &Arc<Mutex<Vec<SettingsToastRecord>>>,
) {
    let rows = records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|record| SettingsToastData {
            id: record.id,
            message: SharedString::from(record.message.as_str()),
            error: record.error,
        })
        .collect::<Vec<_>>();
    settings.set_toast_items(ModelRc::new(VecModel::from(rows)));
}

fn remove_settings_toast(
    settings: &slint::Weak<SettingsWindow>,
    records: &Arc<Mutex<Vec<SettingsToastRecord>>>,
    id: i32,
) {
    if remove_settings_toast_record(records, id)
        && let Some(settings) = settings.upgrade()
    {
        render_settings_toasts(&settings, records);
    }
}

fn remove_settings_toast_record(records: &Mutex<Vec<SettingsToastRecord>>, id: i32) -> bool {
    let mut records = records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_len = records.len();
    records.retain(|record| record.id != id);
    records.len() != previous_len
}

fn clear_settings_toasts(
    settings: &SettingsWindow,
    records: &Arc<Mutex<Vec<SettingsToastRecord>>>,
) {
    records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    render_settings_toasts(settings, records);
}

fn settings_feedback_content(feedback: SettingsFeedback) -> (&'static str, bool) {
    match feedback {
        SettingsFeedback::SettingsSaved(SettingsField::TargetLanguage) => {
            ("Target language saved", false)
        }
        SettingsFeedback::SettingsSaved(SettingsField::Hotkey) => ("Shortcut saved", false),
        SettingsFeedback::SettingsSaved(SettingsField::Provider) => ("Provider saved", false),
        SettingsFeedback::SettingsSaved(SettingsField::LaunchAtLogin) => {
            ("Startup preference saved", false)
        }
        SettingsFeedback::SettingsSaved(SettingsField::SelectionToolbar) => {
            ("Selection toolbar preference saved", false)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::TargetLanguage) => {
            ("Target language wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::Hotkey) => {
            ("Shortcut wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::Provider) => {
            ("Provider wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::LaunchAtLogin) => {
            ("Startup preference wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::SelectionToolbar) => {
            ("Selection toolbar preference wasn't saved", true)
        }
        SettingsFeedback::CredentialSaved => ("API key saved", false),
        SettingsFeedback::CredentialRemoved => ("API key removed", false),
        SettingsFeedback::CredentialCopied => ("Copied", false),
        SettingsFeedback::CredentialOperationFailed => ("Credential operation failed", true),
    }
}

#[derive(Clone)]
pub struct UiHandle {
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

impl UiHandle {
    /// Queues state rendering on the Slint event-loop thread.
    pub fn update(&self, state: AppState) {
        let toolbar_selection = state.toolbar_selection.clone();
        let view_state = mapper::view_state(&state);
        let popup_states = mapper::popup_states(&state);
        let _ = slint::invoke_from_event_loop(move || {
            APP_WINDOW_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.latest_state = state;
                    if let Some(main) = registry.main.as_ref() {
                        binding::apply_main(main, &view_state);
                    }
                    if let Some(settings) = registry.settings.as_ref() {
                        binding::apply_settings(settings, &view_state);
                    }
                }
            });
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.update(popup_states);
                }
            });
            SELECTION_TOOLBAR_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.sync(toolbar_selection);
                }
            });
        });
    }

    pub fn show_popup(
        &self,
        session_id: lexift_core::domain::translation::PopupSessionId,
        anchor: Option<Point>,
        work_area: Option<Rect>,
    ) {
        let session_id = session_id.value();
        let _ = slint::invoke_from_event_loop(move || {
            cancel_idle_memory_trim();
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.show(session_id, anchor, work_area);
                }
            });
        });
    }

    pub fn hide_popup(&self, session_id: lexift_core::domain::translation::PopupSessionId) {
        let session_id = session_id.value();
        let _ = slint::invoke_from_event_loop(move || {
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.hide(session_id);
                }
            });
        });
    }

    pub fn show_main_window(&self) {
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let activate_user_requested_window = self.activate_user_requested_window;
        let _ = slint::invoke_from_event_loop(move || {
            cancel_idle_memory_trim();
            APP_WINDOW_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                let Some(registry) = registry.as_mut() else {
                    return;
                };
                registry.main_close_generation = registry.main_close_generation.wrapping_add(1);
                if let Some(settings) = registry.settings.as_ref() {
                    settings.invoke_close_settings_menu();
                    clear_credential_transient(settings, &credential_generation);
                    clear_settings_toasts(settings, &toast_records);
                }
                match ensure_main_window(registry) {
                    Ok(main) => {
                        if main.window().is_minimized() {
                            main.window().set_minimized(false);
                        }
                        let _ = main.show();
                        activate_user_requested_window(main.window());
                    }
                    Err(error) => tracing::error!(%error, "main window could not be created"),
                }
            });
        });
    }

    pub fn show_settings_window(&self, settings: Settings) {
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            cancel_idle_memory_trim();
            APP_WINDOW_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                let Some(registry) = registry.as_mut() else {
                    return;
                };
                registry.settings_close_generation =
                    registry.settings_close_generation.wrapping_add(1);
                let window = match ensure_settings_window(registry) {
                    Ok(window) => window,
                    Err(error) => {
                        tracing::error!(%error, "settings window could not be created");
                        return;
                    }
                };
                window.invoke_reset_settings_view();
                window.set_draft_target_index(language_index(&settings.target_language));
                window.set_draft_hotkey_label(settings.hotkey.to_string().into());
                window.set_draft_provider_id(settings.provider.id().into());
                window.set_launch_at_login(settings.launch_at_login);
                window.set_selection_toolbar(settings.selection_toolbar);
                window.set_hotkey_capturing(false);
                clear_credential_transient(&window, &credential_generation);
                clear_settings_toasts(&window, &toast_records);
                if window.window().is_minimized() {
                    window.window().set_minimized(false);
                }
                let _ = window.show();
                let color = window.get_resize_fallback_color();
                if !install_settings_platform_hooks(
                    &registry.configure_resize_background,
                    &registry.configure_window_paint_repair,
                    &window,
                    color,
                ) {
                    retry_settings_resize_background(
                        Rc::clone(&registry.configure_resize_background),
                        Rc::clone(&registry.configure_window_paint_repair),
                        window.as_weak(),
                        color,
                        RESIZE_BACKGROUND_RETRY_ATTEMPTS,
                    );
                }
            });
        });
    }

    pub fn hide_settings_window(&self) {
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            APP_WINDOW_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                if let Some(registry) = registry.as_mut()
                    && let Some(settings) = registry.settings.take()
                {
                    registry.settings_close_generation =
                        registry.settings_close_generation.wrapping_add(1);
                    settings.invoke_reset_settings_view();
                    clear_credential_transient(&settings, &credential_generation);
                    clear_settings_toasts(&settings, &toast_records);
                    let _ = settings.hide();
                    drop(settings);
                }
            });
            schedule_idle_memory_trim();
        });
    }

    pub fn clear_credential_draft(&self) {
        let credential_generation = Arc::clone(&self.credential_generation);
        let _ = slint::invoke_from_event_loop(move || {
            APP_WINDOW_REGISTRY.with(|registry| {
                if let Some(settings) = registry.borrow().as_ref().and_then(|r| r.settings.as_ref())
                {
                    clear_credential_transient(settings, &credential_generation);
                }
            });
        });
    }

    pub fn present_credential_secret(
        &self,
        purpose: CredentialAccessPurpose,
        generation: u64,
        secret: CredentialSecret,
    ) {
        let current_generation = Arc::clone(&self.credential_generation);
        let _ = slint::invoke_from_event_loop(move || {
            if !credential_session_is_current(&current_generation, generation) {
                return;
            }
            let Some(settings) = APP_WINDOW_REGISTRY.with(|registry| {
                registry
                    .borrow()
                    .as_ref()?
                    .settings
                    .as_ref()
                    .map(ComponentHandle::clone_strong)
            }) else {
                return;
            };
            match purpose {
                CredentialAccessPurpose::Reveal => {
                    settings.set_credential_transient_secret(secret.into_inner().into());
                    settings.set_credential_revealed(true);
                    settings.set_credential_editing(false);
                    settings.set_credential_secret_visible(true);
                    settings.set_credential_reveal_dismiss_armed(false);
                    settings.invoke_focus_credential_reveal();

                    let settings_for_arm = settings.as_weak();
                    let generation_for_arm = Arc::clone(&current_generation);
                    slint::Timer::single_shot(Duration::from_millis(50), move || {
                        if credential_session_is_current(&generation_for_arm, generation)
                            && let Some(settings) = settings_for_arm.upgrade()
                        {
                            settings.set_credential_reveal_dismiss_armed(true);
                        }
                    });
                    let settings_for_timeout = settings.as_weak();
                    let generation_for_timeout = Arc::clone(&current_generation);
                    slint::Timer::single_shot(CREDENTIAL_REVEAL_DURATION, move || {
                        if credential_session_is_current(&generation_for_timeout, generation)
                            && let Some(settings) = settings_for_timeout.upgrade()
                        {
                            clear_credential_transient(&settings, &generation_for_timeout);
                        }
                    });
                }
                CredentialAccessPurpose::Edit => {
                    settings.set_credential_draft(secret.into_inner().into());
                    settings.set_credential_revealed(false);
                    settings.set_credential_editing(true);
                    settings.set_credential_secret_visible(false);
                    settings.invoke_focus_credential_edit();
                }
                CredentialAccessPurpose::Copy => {}
            }
        });
    }

    /// Adds an independently dismissible Settings toast.
    pub fn show_settings_feedback(&self, feedback: SettingsFeedback) {
        let toast_records = Arc::clone(&self.toast_records);
        let id = self
            .toast_next_id
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1) as i32;
        let (message, error) = settings_feedback_content(feedback);
        let duration = if error {
            SETTINGS_TOAST_ERROR_DURATION
        } else {
            SETTINGS_TOAST_SUCCESS_DURATION
        };
        let _ = slint::invoke_from_event_loop(move || {
            {
                toast_records
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(SettingsToastRecord {
                        id,
                        message: message.to_owned(),
                        error,
                    });
            }
            let Some(settings) = APP_WINDOW_REGISTRY.with(|registry| {
                registry
                    .borrow()
                    .as_ref()?
                    .settings
                    .as_ref()
                    .map(ComponentHandle::clone_strong)
            }) else {
                return;
            };
            render_settings_toasts(&settings, &toast_records);
            let settings_for_timeout = settings.as_weak();
            let records_for_timeout = Arc::clone(&toast_records);
            slint::Timer::single_shot(duration, move || {
                remove_settings_toast(&settings_for_timeout, &records_for_timeout, id);
            });
        });
    }

    pub fn quit(&self) {
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            APP_WINDOW_REGISTRY.with(|registry| {
                if let Some(settings) = registry.borrow().as_ref().and_then(|r| r.settings.as_ref())
                {
                    clear_credential_transient(settings, &credential_generation);
                    clear_settings_toasts(settings, &toast_records);
                }
            });
            let _ = slint::quit_event_loop();
        });
    }
}

fn language_index(language: &Language) -> i32 {
    match language.0.as_str() {
        "zh-CN" => 0,
        "zh-TW" => 1,
        "en-US" => 2,
        "en-GB" => 3,
        "ja" => 4,
        "ko" => 5,
        "de" => 6,
        "fr" => 7,
        "es" => 8,
        "it" => 9,
        "pt-PT" => 10,
        "pt-BR" => 11,
        _ => 0,
    }
}

fn language_for_index(index: i32) -> Language {
    let code = match index {
        0 => "zh-CN",
        1 => "zh-TW",
        2 => "en-US",
        3 => "en-GB",
        4 => "ja",
        5 => "ko",
        6 => "de",
        7 => "fr",
        8 => "es",
        9 => "it",
        10 => "pt-PT",
        _ => "pt-BR",
    };
    Language(code.into())
}

fn source_language_for_index(index: i32) -> Option<Language> {
    let code = match index {
        0 => return None,
        1 => "en-US",
        2 => "zh-CN",
        3 => "ja",
        4 => "ko",
        5 => "de",
        6 => "fr",
        7 => "es",
        8 => "it",
        9 => "pt-PT",
        _ => return None,
    };
    Some(Language(code.into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainWindowClosePolicy {
    HideToTray,
    Exit,
}

fn close_policy(tray_registered: bool) -> MainWindowClosePolicy {
    if tray_registered {
        MainWindowClosePolicy::HideToTray
    } else {
        MainWindowClosePolicy::Exit
    }
}

#[cfg(test)]
mod tests {
    use lexift_core::domain::{
        geometry::{Point, Rect},
        language::Language,
    };

    use super::{
        IdleTrimGeneration, MainWindowClosePolicy, ManualPopupSize, POPUP_MIN_SOURCE_HEIGHT,
        PendingPopupShow, PopupDragPhase, PopupDragScheduler, SettingsToastRecord, clamp_range,
        clamp_source_card_height, close_policy, credential_session_is_current,
        effective_manual_layout, has_valid_native_client_size, language_index,
        manual_size_after_external_resize, max_source_card_height, popup_min_height,
        popup_min_window_height, popup_show_retry_delay, remove_settings_toast_record,
        source_language_for_index, toolbar_opacity,
    };

    #[test]
    fn toolbar_fade_follows_distance_and_scale() {
        let bounds = Rect {
            left: 100,
            top: 100,
            right: 200,
            bottom: 148,
        };
        assert_eq!(toolbar_opacity(Point { x: 150, y: 120 }, bounds, 1.0), 1.0);
        assert_eq!(toolbar_opacity(Point { x: 224, y: 120 }, bounds, 1.0), 1.0);
        assert!((toolbar_opacity(Point { x: 322, y: 120 }, bounds, 1.0) - 0.5).abs() < 0.001);
        assert_eq!(toolbar_opacity(Point { x: 420, y: 120 }, bounds, 1.0), 0.0);
        assert!((toolbar_opacity(Point { x: 444, y: 120 }, bounds, 2.0) - 0.5).abs() < 0.001);
        assert_eq!(toolbar_opacity(Point { x: 224, y: 120 }, bounds, 1.0), 1.0);
    }

    #[test]
    fn toolbar_fade_handles_negative_screen_coordinates() {
        let bounds = Rect {
            left: -1400,
            top: -800,
            right: -1300,
            bottom: -752,
        };
        assert!(
            (toolbar_opacity(Point { x: -1500, y: -780 }, bounds, 1.0) - 120.0 / 196.0).abs()
                < 0.001
        );
        assert_eq!(
            toolbar_opacity(Point { x: -1620, y: -780 }, bounds, 1.0),
            0.0
        );
    }

    #[test]
    fn size_clamps_survive_an_inverted_or_missing_range() {
        // The maximised-popup case: a minimum height above the monitor's maximum.
        assert_eq!(clamp_range(1156.4, 1156.4, 1152.0), 1152.0);
        assert_eq!(clamp_range(100.0, 1156.4, 1152.0), 1152.0);
        assert_eq!(clamp_range(500.0, 340.0, 1920.0), 500.0);
        assert_eq!(clamp_range(10.0, f32::NAN, 100.0), 10.0);
        assert_eq!(clamp_range(10.0, 0.0, f32::NAN), 10.0);
    }

    #[test]
    fn popup_minimum_height_never_exceeds_the_maximum() {
        // 1152 (work area) - 81.6 (source card) + 86 (minimum source card) = 1156.4.
        assert_eq!(popup_min_height(1156.4, 1152.0), 1152.0);
        assert_eq!(popup_min_height(500.0, 1152.0), 500.0);
        assert_eq!(popup_min_height(100.0, 1152.0), 336.0);
        assert_eq!(popup_min_height(900.0, 200.0), 336.0);
        assert_eq!(popup_min_height(f32::NAN, 1152.0), 336.0);
    }

    #[test]
    fn popup_minimum_height_follows_the_layout_not_the_current_height() {
        // 240 (title, language bar, spacing and result-card minimum) + 86 (source card) = 326,
        // raised to the design floor. It must stay independent of the window height, otherwise a
        // full-height popup with a source card at its minimum would report min == max and Windows
        // would refuse every height change.
        let reserved_height = 240.0;
        let min_source_height = POPUP_MIN_SOURCE_HEIGHT;
        let work_area_height = 1152.0;

        let minimum = popup_min_window_height(reserved_height, min_source_height, work_area_height);
        assert_eq!(minimum, 336.0);
        // The state that froze the corners: a source card at its minimum inside a full-height
        // popup must still leave room to shrink.
        assert!(
            minimum < work_area_height,
            "a full-height popup must stay shrinkable, got {minimum}"
        );
        // A short work area still cannot invert the range.
        assert_eq!(
            popup_min_window_height(reserved_height, min_source_height, 200.0),
            336.0
        );
        assert_eq!(
            popup_min_window_height(reserved_height, 900.0, 1152.0),
            1140.0
        );
        assert_eq!(
            popup_min_window_height(f32::NAN, min_source_height, 1152.0),
            336.0
        );
        assert_eq!(
            popup_min_window_height(reserved_height, min_source_height, f32::INFINITY),
            336.0
        );
    }

    #[test]
    fn idle_working_set_trim_is_invalidated_by_window_reopen_or_new_close() {
        let mut generation = IdleTrimGeneration::default();
        let first = generation.schedule();
        assert!(generation.is_current(first));

        generation.invalidate();
        assert!(!generation.is_current(first));

        let second = generation.schedule();
        let third = generation.schedule();
        assert!(!generation.is_current(second));
        assert!(generation.is_current(third));
    }

    #[test]
    fn manual_popup_size_is_a_floor_and_only_new_content_adds_growth() {
        let manual = ManualPopupSize {
            width: 360.0,
            height: 350.0,
            source_height: 90.0,
            baseline_source_height: 120.0,
            baseline_remainder_height: 240.0,
        };

        assert_eq!(
            effective_manual_layout(manual, 300.0, 100.0),
            (360.0, 350.0, 90.0)
        );
        assert_eq!(
            effective_manual_layout(manual, 410.0, 140.0),
            (360.0, 400.0, 110.0)
        );
    }

    #[test]
    fn popup_source_height_budget_keeps_the_language_and_result_rows_visible() {
        let reserved_height = 240.0;
        let minimum_source_height = POPUP_MIN_SOURCE_HEIGHT;

        assert_eq!(
            max_source_card_height(336.0, reserved_height, minimum_source_height),
            96.0
        );
        assert_eq!(
            clamp_source_card_height(1_000.0, 336.0, reserved_height, minimum_source_height),
            96.0
        );
        assert_eq!(
            clamp_source_card_height(20.0, 336.0, reserved_height, minimum_source_height),
            minimum_source_height
        );
        assert_eq!(
            clamp_source_card_height(1_000.0, 1_000.0, reserved_height, minimum_source_height),
            760.0
        );
    }

    #[test]
    fn restoring_a_snapped_popup_reclamps_the_source_card_into_the_layout_budget() {
        // A Popup snapped to the 1152px work area stores a 912px source card. Dragging its title row
        // makes Windows restore the pre-snap size while the modal move loop blocks the event loop,
        // so `popup.slint` caps the rendered height with the same budget; this is the value the
        // release path then saves for the restored window.
        let reserved_height = 240.0;
        let restored_height = 440.0;

        let clamped = clamp_source_card_height(
            912.0,
            restored_height,
            reserved_height,
            POPUP_MIN_SOURCE_HEIGHT,
        );
        assert_eq!(clamped, 200.0);
        assert_eq!(
            max_source_card_height(restored_height, reserved_height, POPUP_MIN_SOURCE_HEIGHT),
            clamped
        );
        assert!(
            reserved_height + clamped <= restored_height,
            "the source card must leave room for the language row and the result card"
        );
        // The snapped height itself is inside the budget and stays untouched.
        assert_eq!(
            clamp_source_card_height(912.0, 1152.0, reserved_height, POPUP_MIN_SOURCE_HEIGHT),
            912.0
        );
    }

    #[test]
    fn reopening_an_oversized_manual_popup_clamps_its_source_card() {
        let manual = ManualPopupSize {
            width: 420.0,
            height: 1_000.0,
            source_height: 900.0,
            baseline_source_height: 100.0,
            baseline_remainder_height: 200.0,
        };
        let (_, height, source_height) = effective_manual_layout(manual, 300.0, 100.0);

        assert_eq!(height, 1_000.0);
        assert_eq!(
            clamp_source_card_height(source_height, height, 240.0, POPUP_MIN_SOURCE_HEIGHT),
            760.0
        );
    }

    #[test]
    fn restored_popup_size_reconciles_manual_layout_without_old_snap_height() {
        let restored = manual_size_after_external_resize(
            420.0,
            390.0,
            900.0,
            240.0,
            POPUP_MIN_SOURCE_HEIGHT,
            100.0,
            200.0,
        );

        assert_eq!(restored.source_height, 150.0);
        assert_eq!(
            effective_manual_layout(restored, 300.0, 100.0),
            (420.0, 390.0, 150.0)
        );
        assert!(
            restored.height - restored.source_height - 240.0 >= 0.0,
            "the language and result area reserve must remain available after restore"
        );
    }

    #[test]
    fn minimized_or_invalid_native_sizes_are_not_reconciled() {
        assert!(!has_valid_native_client_size(0.0, 390.0));
        assert!(!has_valid_native_client_size(420.0, 0.0));
        assert!(!has_valid_native_client_size(-1.0, 390.0));
        assert!(!has_valid_native_client_size(f32::NAN, 390.0));
        assert!(has_valid_native_client_size(420.0, 390.0));
    }

    #[test]
    fn popup_drag_waits_for_one_frame_and_dispatches_once() {
        let mut scheduler = PopupDragScheduler::default();

        assert_eq!(scheduler.phase(7), PopupDragPhase::Idle);
        assert!(scheduler.request(7));
        assert!(!scheduler.request(7));
        assert_eq!(scheduler.phase(7), PopupDragPhase::WaitingForFrame);
        assert!(scheduler.frame_rendered(7));
        assert!(!scheduler.frame_rendered(7));
        assert_eq!(scheduler.phase(7), PopupDragPhase::DispatchQueued);
        assert!(scheduler.take_dispatch(7));
        assert!(!scheduler.take_dispatch(7));
        assert_eq!(scheduler.phase(7), PopupDragPhase::Idle);
    }

    #[test]
    fn popup_drag_can_be_cancelled_before_rendering() {
        let mut scheduler = PopupDragScheduler::default();

        assert!(scheduler.request(9));
        scheduler.cancel(9);
        assert_eq!(scheduler.phase(9), PopupDragPhase::Idle);
        assert!(!scheduler.frame_rendered(9));
        assert!(!scheduler.take_dispatch(9));
    }

    #[test]
    fn popup_native_window_retry_yields_once_then_uses_frame_intervals() {
        assert_eq!(popup_show_retry_delay(false), std::time::Duration::ZERO);
        assert_eq!(
            popup_show_retry_delay(true),
            std::time::Duration::from_millis(16)
        );
    }

    #[test]
    fn popup_native_window_creation_is_requested_only_once() {
        let mut request = PendingPopupShow::new(None, None, 1, None);

        assert!(request.request_native_window_creation());
        assert!(!request.request_native_window_creation());
    }

    #[test]
    fn popup_source_language_indices_are_reduced_and_auto_detect_is_none() {
        assert_eq!(source_language_for_index(0), None);
        for (index, code) in [
            (1, "en-US"),
            (2, "zh-CN"),
            (3, "ja"),
            (4, "ko"),
            (5, "de"),
            (6, "fr"),
            (7, "es"),
            (8, "it"),
            (9, "pt-PT"),
        ] {
            assert_eq!(
                source_language_for_index(index),
                Some(Language(code.into()))
            );
        }
        assert_eq!(source_language_for_index(10), None);
    }

    #[test]
    fn repeated_popup_show_keeps_the_native_creation_request() {
        let mut first = PendingPopupShow::new(None, None, 1, None);
        assert!(first.request_native_window_creation());
        first.retried_once = true;

        let replacement = PendingPopupShow::new(None, None, 2, Some(first));

        assert_eq!(replacement.generation, 2);
        assert!(replacement.retried_once);
        assert!(replacement.native_window_creation_requested);
    }

    #[test]
    fn settings_toasts_are_independent_instances() {
        let records = std::sync::Mutex::new(vec![
            SettingsToastRecord {
                id: 1,
                message: "Shortcut saved".into(),
                error: false,
            },
            SettingsToastRecord {
                id: 2,
                message: "API key saved".into(),
                error: false,
            },
            SettingsToastRecord {
                id: 3,
                message: "Provider wasn't saved".into(),
                error: true,
            },
        ]);

        assert!(remove_settings_toast_record(&records, 2));
        assert!(!remove_settings_toast_record(&records, 2));
        let records = records.lock().unwrap();
        assert_eq!(
            records.iter().map(|toast| toast.id).collect::<Vec<_>>(),
            [1, 3]
        );
    }

    #[test]
    fn stale_credential_result_cannot_affect_the_current_session() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let generation = AtomicU64::new(3);
        assert!(credential_session_is_current(&generation, 3));
        generation.fetch_add(1, Ordering::SeqCst);
        assert!(!credential_session_is_current(&generation, 3));
    }

    #[test]
    fn main_window_only_hides_when_tray_registration_succeeded() {
        assert_eq!(close_policy(true), MainWindowClosePolicy::HideToTray);
        assert_eq!(close_policy(false), MainWindowClosePolicy::Exit);
    }

    #[test]
    fn maps_canonical_languages_to_selector_indices() {
        for (index, code) in [
            "zh-CN", "zh-TW", "en-US", "en-GB", "ja", "ko", "de", "fr", "es", "it", "pt-PT",
            "pt-BR",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(language_index(&Language(code.into())), index as i32);
        }
    }
}
