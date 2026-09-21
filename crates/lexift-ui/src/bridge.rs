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
        settings::{Settings, SettingsChange, SettingsFeedback, SettingsField},
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    AppWindow, SettingsToastData, SettingsWindow, TranslationPopup, binding, mapper, placement,
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
}

pub type PopupPointerSink = Rc<dyn Fn(PopupPointerInput)>;
type CompletePassiveWindowShow = Rc<dyn Fn(&slint::Window, PopupPointerSink) -> bool>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveWindowPreparation {
    Ready,
    Pending,
    Failed,
}

pub struct WindowLifecycleCallbacks {
    complete_passive_window_show: CompletePassiveWindowShow,
    activate_user_requested_window: fn(&slint::Window) -> bool,
    begin_window_drag: fn(&slint::Window) -> bool,
    is_foreground_window: fn(&slint::Window) -> bool,
}

impl WindowLifecycleCallbacks {
    pub fn new(
        complete_passive_window_show: impl Fn(&slint::Window, PopupPointerSink) -> bool + 'static,
        activate_user_requested_window: fn(&slint::Window) -> bool,
        begin_window_drag: fn(&slint::Window) -> bool,
        is_foreground_window: fn(&slint::Window) -> bool,
    ) -> Self {
        Self {
            complete_passive_window_show: Rc::new(complete_passive_window_show),
            activate_user_requested_window,
            begin_window_drag,
            is_foreground_window,
        }
    }
}

const POPUP_GAP_PX: i32 = 12;
const WORK_AREA_MARGIN_PX: i32 = 8;
const CREDENTIAL_REVEAL_DURATION: Duration = Duration::from_secs(30);
const SETTINGS_TOAST_SUCCESS_DURATION: Duration = Duration::from_secs(2);
const SETTINGS_TOAST_ERROR_DURATION: Duration = Duration::from_secs(5);
const SETTINGS_DEFAULT_WIDTH: f32 = 820.0;
const SETTINGS_DEFAULT_HEIGHT: f32 = 680.0;
const POPUP_DRAG_FALLBACK_DELAY: Duration = Duration::from_millis(16);
const POPUP_SHOW_RETRY_DELAY: Duration = Duration::from_millis(16);
const POPUP_SHOW_TIMEOUT: Duration = Duration::from_secs(1);

thread_local! {
    static POPUP_REGISTRY: RefCell<Option<PopupRegistry>> = const { RefCell::new(None) };
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupWindowRoute {
    Primary,
    ExistingExtra,
    NewExtra,
}

fn popup_window_route(
    primary_session_id: Option<u64>,
    extra_exists: bool,
    requested_session_id: u64,
) -> PopupWindowRoute {
    if primary_session_id == Some(requested_session_id) {
        PopupWindowRoute::Primary
    } else if extra_exists {
        PopupWindowRoute::ExistingExtra
    } else if primary_session_id == Some(0) {
        PopupWindowRoute::Primary
    } else {
        PopupWindowRoute::NewExtra
    }
}

struct PopupRegistry {
    primary: slint::Weak<TranslationPopup>,
    extras: HashMap<u64, TranslationPopup>,
    states: HashMap<u64, mapper::PopupUiState>,
    work_areas: HashMap<u64, Rect>,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    interacted: HashSet<u64>,
    foreground_seen: HashSet<u64>,
    drag_scheduler: PopupDragScheduler,
    pending_shows: HashMap<u64, PendingPopupShow>,
    next_show_generation: u64,
    prepare_passive_window: fn(&slint::Window) -> PassiveWindowPreparation,
    complete_passive_window_show: CompletePassiveWindowShow,
    begin_window_drag: fn(&slint::Window) -> bool,
    is_foreground_window: fn(&slint::Window) -> bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingPopupShow {
    anchor: Option<Point>,
    work_area: Option<Rect>,
    generation: u64,
    deadline: Instant,
    retried_once: bool,
}

impl PopupRegistry {
    fn window_for(&mut self, session_id: u64) -> Option<TranslationPopup> {
        let primary = self.primary.upgrade();
        let primary_session_id = primary
            .as_ref()
            .map(|window| window.get_session_id().max(0) as u64);
        match popup_window_route(
            primary_session_id,
            self.extras.contains_key(&session_id),
            session_id,
        ) {
            PopupWindowRoute::Primary => return primary,
            PopupWindowRoute::ExistingExtra => {
                return self
                    .extras
                    .get(&session_id)
                    .map(ComponentHandle::clone_strong);
            }
            PopupWindowRoute::NewExtra => {}
        }
        let window = TranslationPopup::new().ok()?;
        if let Some(handler) = &self.handler {
            wire_popup_callbacks(&window, Rc::clone(handler));
            wire_popup_close(&window, Rc::clone(handler));
        }
        self.extras.insert(session_id, window.clone_strong());
        Some(window)
    }

    fn update(&mut self, states: Vec<mapper::PopupUiState>) {
        let previous_states = std::mem::replace(
            &mut self.states,
            states
                .into_iter()
                .map(|state| (state.session_id, state))
                .collect(),
        );
        if let Some(primary) = self.primary.upgrade() {
            let id = primary.get_session_id().max(0) as u64;
            if let Some(state) = self.states.get(&id) {
                apply_popup_preserving_draft(&primary, state, previous_states.get(&id));
                if let Some(work_area) = self.work_areas.get(&id) {
                    clamp_popup_to_work_area(&primary, *work_area);
                }
            }
        }
        for (id, window) in &self.extras {
            if let Some(state) = self.states.get(id) {
                apply_popup_preserving_draft(window, state, previous_states.get(id));
                if let Some(work_area) = self.work_areas.get(id) {
                    clamp_popup_to_work_area(window, *work_area);
                }
            }
        }
    }

    fn show(&mut self, session_id: u64, anchor: Option<Point>, work_area: Option<Rect>) {
        let Some(window) = self.window_for(session_id) else {
            return;
        };
        reset_popup_transient_ui(&window);
        if let Some(state) = self.states.get(&session_id) {
            binding::apply_popup(&window, state);
        } else {
            window.set_session_id(session_id as i32);
        }
        self.interacted.remove(&session_id);
        self.foreground_seen.remove(&session_id);
        self.drag_scheduler.cancel(session_id);
        self.next_show_generation = self.next_show_generation.wrapping_add(1).max(1);
        let generation = self.next_show_generation;
        self.pending_shows.insert(
            session_id,
            PendingPopupShow {
                anchor,
                work_area,
                generation,
                deadline: Instant::now() + POPUP_SHOW_TIMEOUT,
                retried_once: false,
            },
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
                self.finish_show(session_id, window, request.anchor, request.work_area);
            }
            PassiveWindowPreparation::Pending if Instant::now() < request.deadline => {
                let delay = popup_show_retry_delay(request.retried_once);
                if let Some(request) = self.pending_shows.get_mut(&session_id) {
                    request.retried_once = true;
                }
                schedule_popup_show_retry(session_id, generation, delay);
            }
            PassiveWindowPreparation::Pending => {
                self.pending_shows.remove(&session_id);
                let _ = window.hide();
                tracing::error!(
                    session_id,
                    "translation popup native window was not created before the show timeout"
                );
            }
            PassiveWindowPreparation::Failed => {
                self.pending_shows.remove(&session_id);
                let _ = window.hide();
            }
        }
    }

    fn existing_window(&self, session_id: u64) -> Option<TranslationPopup> {
        self.primary
            .upgrade()
            .filter(|window| window.get_session_id().max(0) as u64 == session_id)
            .or_else(|| {
                self.extras
                    .get(&session_id)
                    .map(ComponentHandle::clone_strong)
            })
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
                .filter_map(|(id, _)| {
                    self.primary
                        .upgrade()
                        .filter(|candidate| candidate.get_session_id().max(0) as u64 == *id)
                        .or_else(|| self.extras.get(id).map(ComponentHandle::clone_strong))
                })
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
        }
    }

    fn hide(&mut self, session_id: u64) {
        let mut primary_hidden = false;
        if let Some(primary) = self.primary.upgrade()
            && primary.get_session_id().max(0) as u64 == session_id
        {
            reset_popup_transient_ui(&primary);
            let _ = primary.hide();
            primary.set_session_id(0);
            primary_hidden = true;
        }
        if !primary_hidden && let Some(window) = self.extras.remove(&session_id) {
            reset_popup_transient_ui(&window);
            let _ = window.hide();
        }
        self.states.remove(&session_id);
        self.work_areas.remove(&session_id);
        self.interacted.remove(&session_id);
        self.foreground_seen.remove(&session_id);
        self.drag_scheduler.cancel(session_id);
        self.pending_shows.remove(&session_id);
    }

    fn foreground_lost_sessions(&mut self) -> Vec<u64> {
        let candidates = self
            .interacted
            .iter()
            .copied()
            .filter(|id| self.states.get(id).is_some_and(|state| !state.pinned))
            .collect::<Vec<_>>();
        candidates
            .into_iter()
            .filter(|id| {
                let window = self
                    .primary
                    .upgrade()
                    .filter(|window| window.get_session_id().max(0) as u64 == *id)
                    .or_else(|| self.extras.get(id).map(ComponentHandle::clone_strong));
                let Some(window) = window.filter(|window| window.window().is_visible()) else {
                    return false;
                };
                let is_foreground = (self.is_foreground_window)(window.window());
                should_close_after_foreground_observation(
                    &mut self.foreground_seen,
                    *id,
                    is_foreground,
                )
            })
            .collect()
    }
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

fn should_close_after_foreground_observation(
    foreground_seen: &mut HashSet<u64>,
    session_id: u64,
    is_foreground: bool,
) -> bool {
    if is_foreground {
        foreground_seen.insert(session_id);
        false
    } else {
        foreground_seen.contains(&session_id)
    }
}

fn apply_popup_preserving_draft(
    popup: &TranslationPopup,
    state: &mapper::PopupUiState,
    previous: Option<&mapper::PopupUiState>,
) {
    let draft = popup.get_source_text();
    let preserve_draft = previous.is_some_and(|previous| {
        previous.source == state.source && draft.as_str() != previous.source
    });
    binding::apply_popup(popup, state);
    if preserve_draft {
        popup.set_source_text(draft);
    }
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
        if let Err(error) = popup.window().try_dispatch_event(event) {
            tracing::warn!(%error, "translation popup pointer event could not be dispatched");
        }
    })
}

fn clamp_popup_to_work_area(popup: &TranslationPopup, work_area: Rect) {
    let position = popup.window().position();
    let size = popup.window().size();
    let min_x = work_area.left + WORK_AREA_MARGIN_PX;
    let min_y = work_area.top + WORK_AREA_MARGIN_PX;
    let max_x = (work_area.right - size.width as i32 - WORK_AREA_MARGIN_PX).max(min_x);
    let max_y = (work_area.bottom - size.height as i32 - WORK_AREA_MARGIN_PX).max(min_y);
    let x = position.x.clamp(min_x, max_x);
    let y = position.y.clamp(min_y, max_y);
    if x != position.x || y != position.y {
        popup
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SettingsToastRecord {
    id: i32,
    message: String,
    error: bool,
}

pub struct Ui {
    main: AppWindow,
    popup: TranslationPopup,
    settings: SettingsWindow,
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    background_mode: Rc<Cell<bool>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
    foreground_timer: slint::Timer,
}

impl Ui {
    pub fn new(
        initial_state: &AppState,
        show_selection_demo: bool,
        prepare_passive_window: fn(&slint::Window) -> PassiveWindowPreparation,
        window_lifecycle: WindowLifecycleCallbacks,
    ) -> Result<Self, slint::PlatformError> {
        let main = AppWindow::new()?;
        let popup = TranslationPopup::new()?;
        let settings = SettingsWindow::new()?;
        settings.window().set_size(slint::LogicalSize::new(
            SETTINGS_DEFAULT_WIDTH,
            SETTINGS_DEFAULT_HEIGHT,
        ));
        main.set_show_selection_demo(show_selection_demo);
        binding::apply(&main, &settings, mapper::view_state(initial_state));
        POPUP_REGISTRY.with(|registry| {
            *registry.borrow_mut() = Some(PopupRegistry {
                primary: popup.as_weak(),
                extras: HashMap::new(),
                states: mapper::popup_states(initial_state)
                    .into_iter()
                    .map(|state| (state.session_id, state))
                    .collect(),
                work_areas: HashMap::new(),
                handler: None,
                interacted: HashSet::new(),
                foreground_seen: HashSet::new(),
                drag_scheduler: PopupDragScheduler::default(),
                pending_shows: HashMap::new(),
                next_show_generation: 0,
                prepare_passive_window,
                complete_passive_window_show: Rc::clone(
                    &window_lifecycle.complete_passive_window_show,
                ),
                begin_window_drag: window_lifecycle.begin_window_drag,
                is_foreground_window: window_lifecycle.is_foreground_window,
            });
        });
        Ok(Self {
            main,
            popup,
            settings,
            credential_generation: Arc::new(AtomicU64::new(0)),
            toast_next_id: Arc::new(AtomicU64::new(0)),
            toast_records: Arc::new(Mutex::new(Vec::new())),
            background_mode: Rc::new(Cell::new(false)),
            activate_user_requested_window: window_lifecycle.activate_user_requested_window,
            foreground_timer: slint::Timer::default(),
        })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            main: self.main.as_weak(),
            settings: self.settings.as_weak(),
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
        let main = self.main.as_weak();
        let background_mode = Rc::clone(&self.background_mode);
        let main_close_handler = Rc::clone(&handler);
        self.main.window().on_close_requested(move || {
            match close_policy(background_mode.get()) {
                MainWindowClosePolicy::HideToTray => {
                    if let Some(main) = main.upgrade() {
                        let _ = main.hide();
                    }
                }
                MainWindowClosePolicy::Exit => main_close_handler(AppEvent::ExitRequested),
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        wire_popup_close(&self.popup, Rc::clone(&handler));
        wire_popup_callbacks(&self.popup, Rc::clone(&handler));
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.handler = Some(Rc::clone(&handler));
            }
        });
        self.foreground_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(150),
            move || {
                let (sessions, handler) = POPUP_REGISTRY.with(|registry| {
                    let mut registry = registry.borrow_mut();
                    let Some(registry) = registry.as_mut() else {
                        return (Vec::new(), None);
                    };
                    (
                        registry.foreground_lost_sessions(),
                        registry.handler.as_ref().map(Rc::clone),
                    )
                });
                if let Some(handler) = handler {
                    for id in sessions {
                        handler(AppEvent::PopupClosed {
                            session_id: lexift_core::domain::translation::PopupSessionId::new(id),
                        });
                    }
                }
            },
        );
        let settings = self.settings.as_weak();
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        self.settings.window().on_close_requested(move || {
            if let Some(settings) = settings.upgrade() {
                settings.invoke_reset_settings_view();
                clear_credential_transient(&settings, &credential_generation);
                clear_settings_toasts(&settings, &toast_records);
                let _ = settings.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        let settings = self.settings.as_weak();
        let toast_records = Arc::clone(&self.toast_records);
        self.settings.on_toast_dismiss_requested(move |id| {
            remove_settings_toast(&settings, &toast_records, id);
        });
        self.settings
            .on_hotkey_key_pressed(move |text, control, alt, shift, meta| {
                HotkeyConfig::from_key_event(&text, control, alt, shift, meta)
                    .map(|config| config.to_string().into())
                    .unwrap_or_default()
            });
        let settings_handler = Rc::clone(&handler);
        self.settings.on_hotkey_change_requested(move |hotkey| {
            if let Ok(hotkey) = hotkey.to_string().parse::<HotkeyConfig>() {
                settings_handler(AppEvent::SettingsChangeRequested {
                    change: SettingsChange::Hotkey(hotkey),
                });
            }
        });
        let settings_handler = Rc::clone(&handler);
        self.settings
            .on_launch_at_login_change_requested(move |enabled| {
                settings_handler(AppEvent::SettingsChangeRequested {
                    change: SettingsChange::LaunchAtLogin(enabled),
                });
            });
        let credential_handler = Rc::clone(&handler);
        self.settings.on_credential_save_requested(move |secret| {
            credential_handler(AppEvent::CredentialSaveRequested {
                secret: CredentialSecret::new(secret.to_string()),
            });
        });
        let credential_handler = Rc::clone(&handler);
        self.settings.on_credential_remove_requested(move || {
            credential_handler(AppEvent::CredentialRemoveRequested);
        });
        let settings = self.settings.as_weak();
        let credential_generation = Arc::clone(&self.credential_generation);
        let credential_handler = Rc::clone(&handler);
        self.settings.on_credential_reveal_requested(move || {
            let generation = begin_credential_access(&settings, &credential_generation);
            credential_handler(AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Reveal,
                generation,
            });
        });
        let settings = self.settings.as_weak();
        let credential_generation = Arc::clone(&self.credential_generation);
        let credential_handler = Rc::clone(&handler);
        self.settings.on_credential_edit_requested(move || {
            let generation = begin_credential_access(&settings, &credential_generation);
            credential_handler(AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Edit,
                generation,
            });
        });
        let settings = self.settings.as_weak();
        let credential_generation = Arc::clone(&self.credential_generation);
        let credential_handler = Rc::clone(&handler);
        self.settings.on_credential_copy_requested(move || {
            let generation = begin_credential_access(&settings, &credential_generation);
            credential_handler(AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Copy,
                generation,
            });
        });
        let settings = self.settings.as_weak();
        let credential_generation = Arc::clone(&self.credential_generation);
        self.settings.on_credential_hide_requested(move || {
            if let Some(settings) = settings.upgrade() {
                clear_credential_transient(&settings, &credential_generation);
            }
        });
        let settings = self.settings.as_weak();
        let credential_generation = Arc::clone(&self.credential_generation);
        self.settings.on_credential_edit_cancel_requested(move || {
            if let Some(settings) = settings.upgrade() {
                clear_credential_transient(&settings, &credential_generation);
            }
        });
        let settings = self.settings.as_weak();
        let settings_handler = Rc::clone(&handler);
        self.settings.on_settings_menu_selected(move |index| {
            if let Some(settings) = settings.upgrade() {
                if settings.get_settings_menu_provider_mode() {
                    let provider = ProviderConfig::DeepL;
                    if settings.get_draft_provider_id().as_str() != provider.id() {
                        settings.set_draft_provider_id(provider.id().into());
                        settings_handler(AppEvent::SettingsChangeRequested {
                            change: SettingsChange::Provider(provider),
                        });
                    }
                } else if settings.get_draft_target_index() != index {
                    settings.set_draft_target_index(index);
                    settings_handler(AppEvent::SettingsChangeRequested {
                        change: SettingsChange::TargetLanguage(language_for_index(index)),
                    });
                }
            }
        });
        let selection_handler = Rc::clone(&handler);
        self.main.on_selection_translation_requested(move || {
            selection_handler(AppEvent::SelectionTranslationRequested);
        });
        let input_handler = Rc::clone(&handler);
        self.main.on_input_translation_requested(move |text| {
            input_handler(AppEvent::InputTranslationRequested {
                text: text.to_string(),
            });
        });
        let settings_handler = Rc::clone(&handler);
        self.main.on_settings_window_requested(move || {
            settings_handler(AppEvent::SettingsWindowRequested);
        });
    }

    pub fn set_background_mode(&self, enabled: bool) {
        self.background_mode.set(enabled);
    }

    pub fn run(&self, show_main_window: bool) -> Result<(), slint::PlatformError> {
        if show_main_window {
            self.main.show()?;
        }
        slint::run_event_loop_until_quit()?;
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().take() {
                for window in registry.extras.into_values() {
                    reset_popup_transient_ui(&window);
                    let _ = window.hide();
                }
            }
        });
        reset_popup_transient_ui(&self.popup);
        let _ = self.popup.hide();
        let _ = self.settings.hide();
        self.main.hide()
    }
}

fn popup_session_id(popup: &TranslationPopup) -> lexift_core::domain::translation::PopupSessionId {
    lexift_core::domain::translation::PopupSessionId::new(popup.get_session_id().max(0) as u64)
}

fn reset_popup_transient_ui(popup: &TranslationPopup) {
    popup.set_target_menu_open(false);
    popup.set_target_menu_scroll_y(0.0);
}

fn wire_popup_close(popup: &TranslationPopup, handler: Rc<dyn Fn(AppEvent)>) {
    let weak = popup.as_weak();
    popup.window().on_close_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            reset_popup_transient_ui(&popup);
            let _ = popup.hide();
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

fn wire_popup_callbacks(popup: &TranslationPopup, handler: Rc<dyn Fn(AppEvent)>) {
    let drag_render_notifier_installed = install_popup_drag_render_notifier(popup);
    let weak = popup.as_weak();
    popup.on_interaction_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.interacted.insert(id);
                }
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_drag_requested(move || {
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        let scheduled = POPUP_REGISTRY.with(|registry| {
            registry
                .borrow_mut()
                .as_mut()
                .is_some_and(|registry| registry.drag_scheduler.request(session_id))
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
    let event_handler = Rc::clone(&handler);
    popup.on_translate_requested(move |text, target_index| {
        if let Some(popup) = weak.upgrade() {
            event_handler(AppEvent::PopupTranslationRequested {
                session_id: popup_session_id(&popup),
                text: text.to_string(),
                target_language: language_for_index(target_index),
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_target_language_selected(move |target_index| {
        if let Some(popup) = weak.upgrade() {
            event_handler(AppEvent::PopupTranslationRequested {
                session_id: popup_session_id(&popup),
                text: popup.get_source_text().to_string(),
                target_language: language_for_index(target_index),
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_pin_requested(move |pinned| {
        if let Some(popup) = weak.upgrade() {
            event_handler(AppEvent::PopupPinChanged {
                session_id: popup_session_id(&popup),
                pinned,
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_close_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            reset_popup_transient_ui(&popup);
            let _ = popup.hide();
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
                    let language = popup.get_detected_language();
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
        SettingsFeedback::CredentialSaved => ("API key saved", false),
        SettingsFeedback::CredentialRemoved => ("API key removed", false),
        SettingsFeedback::CredentialCopied => ("Copied", false),
        SettingsFeedback::CredentialOperationFailed => ("Credential operation failed", true),
    }
}

#[derive(Clone)]
pub struct UiHandle {
    main: slint::Weak<AppWindow>,
    settings: slint::Weak<SettingsWindow>,
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

impl UiHandle {
    /// Queues state rendering on the Slint event-loop thread.
    pub fn update(&self, state: AppState) {
        let main = self.main.clone();
        let settings = self.settings.clone();
        let view_state = mapper::view_state(&state);
        let popup_states = mapper::popup_states(&state);
        let _ = slint::invoke_from_event_loop(move || {
            if let (Some(main), Some(settings)) = (main.upgrade(), settings.upgrade()) {
                binding::apply(&main, &settings, view_state);
            }
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.update(popup_states);
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
        let main = self.main.clone();
        let settings = self.settings.clone();
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let activate_user_requested_window = self.activate_user_requested_window;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(settings) = settings.upgrade() {
                settings.invoke_close_settings_menu();
                clear_credential_transient(&settings, &credential_generation);
                clear_settings_toasts(&settings, &toast_records);
            }
            if let Some(main) = main.upgrade() {
                if main.window().is_minimized() {
                    main.window().set_minimized(false);
                }
                let _ = main.show();
                activate_user_requested_window(main.window());
            }
        });
    }

    pub fn show_settings_window(&self, settings: Settings) {
        let window = self.settings.clone();
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window.upgrade() {
                window.invoke_reset_settings_view();
                window.set_draft_target_index(language_index(&settings.target_language));
                window.set_draft_hotkey_label(settings.hotkey.to_string().into());
                window.set_draft_provider_id(settings.provider.id().into());
                window.set_launch_at_login(settings.launch_at_login);
                window.set_hotkey_capturing(false);
                clear_credential_transient(&window, &credential_generation);
                clear_settings_toasts(&window, &toast_records);
                if window.window().is_minimized() {
                    window.window().set_minimized(false);
                }
                let _ = window.show();
            }
        });
    }

    pub fn hide_settings_window(&self) {
        let settings = self.settings.clone();
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(settings) = settings.upgrade() {
                settings.invoke_reset_settings_view();
                clear_credential_transient(&settings, &credential_generation);
                clear_settings_toasts(&settings, &toast_records);
                let _ = settings.hide();
            }
        });
    }

    pub fn clear_credential_draft(&self) {
        let settings = self.settings.clone();
        let credential_generation = Arc::clone(&self.credential_generation);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(settings) = settings.upgrade() {
                clear_credential_transient(&settings, &credential_generation);
            }
        });
    }

    pub fn present_credential_secret(
        &self,
        purpose: CredentialAccessPurpose,
        generation: u64,
        secret: CredentialSecret,
    ) {
        let settings = self.settings.clone();
        let current_generation = Arc::clone(&self.credential_generation);
        let _ = slint::invoke_from_event_loop(move || {
            if !credential_session_is_current(&current_generation, generation) {
                return;
            }
            let Some(settings) = settings.upgrade() else {
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
        let settings = self.settings.clone();
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
            let Some(settings) = settings.upgrade() else {
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
        let settings = self.settings.clone();
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(settings) = settings.upgrade() {
                clear_credential_transient(&settings, &credential_generation);
                clear_settings_toasts(&settings, &toast_records);
            }
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
    use lexift_core::domain::language::Language;

    use super::{
        MainWindowClosePolicy, PopupDragPhase, PopupDragScheduler, PopupWindowRoute,
        SettingsToastRecord, close_policy, credential_session_is_current, language_index,
        popup_show_retry_delay, popup_window_route, remove_settings_toast_record,
        should_close_after_foreground_observation,
    };

    #[test]
    fn pinned_primary_does_not_get_reused_for_a_new_session() {
        assert_eq!(
            popup_window_route(Some(1), false, 2),
            PopupWindowRoute::NewExtra
        );
        assert_eq!(
            popup_window_route(Some(1), false, 1),
            PopupWindowRoute::Primary
        );
    }

    #[test]
    fn existing_extra_wins_over_an_idle_primary() {
        assert_eq!(
            popup_window_route(Some(0), true, 2),
            PopupWindowRoute::ExistingExtra
        );
        assert_eq!(
            popup_window_route(Some(0), false, 3),
            PopupWindowRoute::Primary
        );
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
    fn popup_only_closes_after_it_was_foreground_once() {
        let mut foreground_seen = std::collections::HashSet::new();

        assert!(!should_close_after_foreground_observation(
            &mut foreground_seen,
            7,
            false
        ));
        assert!(!should_close_after_foreground_observation(
            &mut foreground_seen,
            7,
            true
        ));
        assert!(should_close_after_foreground_observation(
            &mut foreground_seen,
            7,
            false
        ));
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
