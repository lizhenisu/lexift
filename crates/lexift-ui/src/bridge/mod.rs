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
        runtime_config::HotkeyConfig,
        settings::{Settings, SettingsChange, SettingsFeedback, SettingsField},
    },
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    AppWindow, PopupCornerMode, PopupLanguageMenuWindow, SettingsToastData, SettingsWindow,
    TranslationPopup, binding, mapper, placement,
};

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
mod clipboard_history;
use clipboard_history::HISTORY;
mod selection_toolbar;
#[cfg(test)]
use selection_toolbar::toolbar_opacity;
use selection_toolbar::{SELECTION_TOOLBAR_REGISTRY, SelectionToolbarRegistry};

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

pub(crate) fn cancel_idle_memory_trim() {
    IDLE_TRIM_GENERATION.with(|generation| generation.borrow_mut().invalidate());
}

pub(crate) fn schedule_idle_memory_trim() {
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
        || HISTORY.with(|s| s.borrow().as_ref().is_some_and(|r| r.window.is_some()))
        || crate::ocr::active()
        || crate::annotation::is_open()
        || crate::tray_menu::is_open()
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

mod app_window;
pub use app_window::WindowRole;
type PrepareAppWindow = Rc<dyn Fn(&slint::Window, WindowRole) -> PassiveWindowPreparation>;

pub struct Ui {
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    background_mode: Rc<Cell<bool>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

fn install_resize_background(
    configure: &ConfigureResizeBackground,
    window: &slint::Window,
    color: slint::Color,
) -> bool {
    (configure)(window, [color.red(), color.green(), color.blue()])
}

/// Invalidate all software-rendered pixels after Windows finishes a native move or resize.
pub(crate) fn repair_window_paint(window: &slint::Window) {
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

fn install_app_window_icon<C: ComponentHandle + 'static>(
    component: &C,
    configure: fn(&slint::Window) -> bool,
) {
    if !configure(component.window()) {
        retry_app_window_icon(component.as_weak(), configure, 20);
    }
}

fn retry_app_window_icon<C: ComponentHandle + 'static>(
    component: slint::Weak<C>,
    configure: fn(&slint::Window) -> bool,
    attempts: u8,
) {
    if attempts == 0 {
        tracing::warn!("native application window icon could not be configured");
        return;
    }
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let Some(component) = component.upgrade() else {
            return;
        };
        if !configure(component.window()) {
            retry_app_window_icon(component.as_weak(), configure, attempts - 1);
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
        crate::annotation::init(prepare_passive_window, window_lifecycle.clone());
        crate::ocr::init(prepare_passive_window, window_lifecycle.clone());
        crate::tray_menu::init(prepare_passive_window, window_lifecycle.clone());
        HISTORY.with(|s| {
            *s.borrow_mut() = Some(clipboard_history::Registry::new(
                prepare_passive_window,
                window_lifecycle.complete_toolbar_show.clone(),
                window_lifecycle.history_dismissal.clone(),
                window_lifecycle.popup_work_area.clone(),
            ));
            if let Some(r) = s.borrow_mut().as_mut() {
                r.begin_resize = window_lifecycle.begin_window_resize.clone();
                r.resize_background = window_lifecycle.configure_resize_background.clone();
                r.configure_corners = window_lifecycle.configure_translation_popup_corners;
                r.paint_repair = window_lifecycle.configure_window_paint_repair.clone();
            }
        });
        SELECTION_TOOLBAR_REGISTRY.with(|registry| {
            *registry.borrow_mut() = Some(SelectionToolbarRegistry {
                window: None,
                selection: None,
                target: None,
                target_current: window_lifecycle.toolbar_target_current.clone(),
                work: None,
                handler: None,
                prepare: prepare_passive_window,
                complete: Rc::clone(&window_lifecycle.complete_toolbar_show),
                dismiss: Rc::clone(&window_lifecycle.set_popup_dismissal),
                work_area: Rc::clone(&window_lifecycle.popup_work_area),
                cursor_position: Rc::clone(&window_lifecycle.toolbar_cursor_position),
                fade_timer: slint::Timer::default(),
                generation: 0,
                core_generation: 0,
                selection_generation: 0,
                history_supported: window_lifecycle.clipboard_history_supported,
            });
        });
        IDLE_TRIM_CALLBACK.with(|callback| {
            callback.set(Some(window_lifecycle.trim_process_working_set));
        });
        cancel_idle_memory_trim();
        let credential_generation = Arc::new(AtomicU64::new(0));
        let toast_next_id = Arc::new(AtomicU64::new(0));
        crate::i18n::select(initial_state.desired_settings.ui_language);
        crate::theme::set_current(initial_state.desired_settings.theme);
        let toast_records = Arc::new(Mutex::new(Vec::new()));
        let background_mode = Rc::new(Cell::new(false));
        APP_WINDOW_REGISTRY.with(|registry| {
            *registry.borrow_mut() = Some(AppWindowRegistry {
                llm_query: window_lifecycle.llm_query.clone(),
                history_supported: window_lifecycle.clipboard_history_supported,
                main: None,
                settings: None,
                configure_resize_background: Rc::clone(
                    &window_lifecycle.configure_resize_background,
                ),
                configure_window_paint_repair: Rc::clone(
                    &window_lifecycle.configure_window_paint_repair,
                ),
                configure_app_window_icon: window_lifecycle.configure_app_window_icon,
                prepare_app_window: Rc::clone(&window_lifecycle.prepare_app_window),
                open_about_link: Rc::clone(&window_lifecycle.open_about_link),
                latest_state: initial_state.clone(),
                show_selection_demo,
                handler: None,
                background_mode: Rc::clone(&background_mode),
                credential_generation: Arc::clone(&credential_generation),
                toast_records: Arc::clone(&toast_records),
                main_show_timer: Default::default(),
                settings_show_timer: Default::default(),
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
        crate::tray_menu::set_handler(Rc::clone(&handler));
        crate::ocr::set_handler(handler.clone());
        crate::annotation::set_handler(handler.clone());
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
                registry.handler = Some(handler.clone());
            }
        });
        HISTORY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                r.handler = Some(handler);
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
                let registry = registry.as_mut().expect("UI registry initialized");
                ensure_main_window(registry)?;
                app_window::schedule(
                    registry,
                    WindowRole::Auxiliary,
                    false,
                    self.activate_user_requested_window,
                    0,
                );
                Ok::<(), slint::PlatformError>(())
            })?;
        }
        slint::run_event_loop_until_quit()?;
        crate::tray_menu::close(false);
        crate::annotation::shutdown();
        crate::ocr::close();
        HISTORY.with(|s| {
            s.borrow_mut().take();
        });
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

#[derive(Clone)]
pub struct UiHandle {
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

impl UiHandle {
    pub fn request_ocr(&self) {
        let _ = slint::invoke_from_event_loop(crate::ocr::request);
    }
    pub fn show_ocr_capture(
        &self,
        generation: u64,
        frames: Arc<Vec<lexift_core::ports::screen_capture::CapturedRegion>>,
    ) {
        let _ = slint::invoke_from_event_loop(move || crate::ocr::show(generation, frames));
    }
    pub fn show_tray_menu(&self, request: lexift_core::ports::tray::TrayMenuRequest) {
        let _ = slint::invoke_from_event_loop(move || crate::tray_menu::show(request));
    }

    pub fn toggle_annotation_toolbar(&self) {
        let requested_at = Instant::now();
        let _ = slint::invoke_from_event_loop(move || {
            if !crate::ocr::screen_busy() {
                crate::annotation::toggle(requested_at);
            }
        });
    }

    /// Queues state rendering on the Slint event-loop thread.
    pub fn update(&self, state: AppState) {
        let theme = state.desired_settings.theme;
        let toolbar_selection = state.toolbar_selection.clone();
        let toolbar_target = state.toolbar_target;
        let core_generation = state.toolbar_generation;
        let history_state = state.clone();
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            let language_changed = crate::i18n::select(state.desired_settings.ui_language);
            crate::ocr::sync(state.ocr.generation, state.ocr.phase);
            crate::annotation::sync_export(&state.annotation_export);
            let view_state = mapper::view_state(&state);
            let popup_states = mapper::popup_states(&state);
            if language_changed {
                crate::tray_menu::close(false);
                crate::annotation::refresh_language();
            }
            crate::theme::set_current(theme);
            apply_window_themes();
            APP_WINDOW_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    if language_changed && let Some(settings) = &registry.settings {
                        settings.invoke_close_settings_menu();
                        settings.invoke_close_app_language_menu();
                        render_settings_toasts(settings, &toast_records);
                    }
                    registry.latest_state = state;
                    if let Some(main) = registry.main.as_ref() {
                        binding::apply_main(main, &view_state);
                    }
                    if let Some(settings) = registry.settings.as_ref() {
                        binding::apply_settings(settings, &view_state);
                        crate::ocr::provider::apply_settings(settings, &registry.latest_state);
                    }
                }
            });
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    if language_changed {
                        registry.close_language_menu(false);
                    }
                    registry.update(popup_states);
                }
            });
            SELECTION_TOOLBAR_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.core_generation = core_generation;
                    registry.sync(toolbar_selection, toolbar_target);
                }
            });
            HISTORY.with(|s| {
                if let Some(r) = s.borrow_mut().as_mut() {
                    r.sync(&history_state);
                }
            });
            schedule_idle_memory_trim();
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

fn apply_window_themes() {
    APP_WINDOW_REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow().as_ref() {
            if let Some(w) = &r.main {
                crate::theme::apply(w);
            }
            if let Some(w) = &r.settings {
                crate::theme::apply(w);
            }
        }
    });
    POPUP_REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow().as_ref() {
            for w in r.windows.values() {
                crate::theme::apply(w);
            }
            if let Some(w) = &r.language_menu {
                crate::theme::apply(w);
            }
        }
    });
    SELECTION_TOOLBAR_REGISTRY.with(|slot| {
        if let Some(w) = slot.borrow().as_ref().and_then(|r| r.window.as_ref()) {
            crate::theme::apply(w);
        }
    });
    HISTORY.with(|s| {
        if let Some(w) = s.borrow().as_ref().and_then(|r| r.window.as_ref()) {
            crate::theme::apply(w);
        }
    });
    crate::annotation::apply_theme();
    crate::tray_menu::apply_theme();
}

pub(crate) fn dispatch_pointer_input(window: &slint::Window, input: PopupPointerInput) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let scale = window.scale_factor().max(0.1);
    let pos = |x, y| slint::LogicalPosition::new(x / scale, y / scale);
    let event = match input {
        PopupPointerInput::RightPressed => return,
        PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
            position: pos(x, y),
        },
        PopupPointerInput::Exited => WindowEvent::PointerExited,
        PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
            position: pos(x, y),
            button: PointerEventButton::Left,
        },
        PopupPointerInput::LeftReleased { x, y } | PopupPointerInput::LeftCancelled { x, y } => {
            WindowEvent::PointerReleased {
                position: pos(x, y),
                button: PointerEventButton::Left,
            }
        }
        PopupPointerInput::Scrolled {
            x,
            y,
            delta_x,
            delta_y,
        } => WindowEvent::PointerScrolled {
            position: pos(x, y),
            delta_x: delta_x / scale,
            delta_y: delta_y / scale,
        },
        _ => return,
    };
    let _ = window.dispatch_event_with_result(event);
}

mod callbacks;
use callbacks::*;
pub use callbacks::{
    PassiveWindowPreparation, PopupDismissCause, PopupPointerInput, PopupPointerSink,
    PopupResizeEdge, WindowLifecycleCallbacks,
};

mod popup;
use popup::*;

mod settings_window;
use settings_window::*;

mod main_window;
use main_window::*;

mod language_menu;
use language_menu::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use popup::{invalidate_popup_surface_cache, measured_popup_metrics};
#[cfg(test)]
pub(crate) use settings_window::complete_credential_copy;

use app_window::{APP_WINDOW_REGISTRY, AppWindowRegistry};
