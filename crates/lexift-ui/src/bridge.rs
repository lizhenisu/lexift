use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
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
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, SharedString, VecModel};

use crate::{
    AppWindow, LanguageMenuWindow, SettingsToastData, SettingsWindow, TranslationPopup, binding,
    mapper, placement,
};

type ArmWindowContext = Arc<dyn Fn(&slint::Window, &slint::Window) -> bool + Send + Sync>;
type DisarmWindowContext = Arc<dyn Fn() + Send + Sync>;

const POPUP_GAP_PX: i32 = 12;
const WORK_AREA_MARGIN_PX: i32 = 8;
const LANGUAGE_MENU_STABILIZATION: Duration = Duration::from_millis(120);
const LANGUAGE_MENU_MONITOR_INTERVAL: Duration = Duration::from_millis(75);
const CREDENTIAL_REVEAL_DURATION: Duration = Duration::from_secs(30);
const SETTINGS_TOAST_SUCCESS_DURATION: Duration = Duration::from_secs(2);
const SETTINGS_TOAST_ERROR_DURATION: Duration = Duration::from_secs(5);

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
    language_menu: LanguageMenuWindow,
    prepare_passive_window: fn(&slint::Window) -> bool,
    prepare_interactive_window: fn(&slint::Window) -> bool,
    set_transient_window_owner: fn(&slint::Window, &slint::Window) -> bool,
    arm_window_context: ArmWindowContext,
    disarm_window_context: DisarmWindowContext,
    language_menu_monitor: Rc<slint::Timer>,
    language_menu_generation: Arc<AtomicU64>,
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    background_mode: Rc<Cell<bool>>,
}

impl Ui {
    pub fn new(
        initial_state: &AppState,
        show_selection_demo: bool,
        prepare_passive_window: fn(&slint::Window) -> bool,
        prepare_interactive_window: fn(&slint::Window) -> bool,
        set_transient_window_owner: fn(&slint::Window, &slint::Window) -> bool,
        arm_window_context: ArmWindowContext,
        disarm_window_context: DisarmWindowContext,
    ) -> Result<Self, slint::PlatformError> {
        let main = AppWindow::new()?;
        let popup = TranslationPopup::new()?;
        let settings = SettingsWindow::new()?;
        let language_menu = LanguageMenuWindow::new()?;
        main.set_show_selection_demo(show_selection_demo);
        binding::apply(&main, &popup, &settings, mapper::view_state(initial_state));
        Ok(Self {
            main,
            popup,
            settings,
            language_menu,
            prepare_passive_window,
            prepare_interactive_window,
            set_transient_window_owner,
            arm_window_context,
            disarm_window_context,
            language_menu_monitor: Rc::new(slint::Timer::default()),
            language_menu_generation: Arc::new(AtomicU64::new(0)),
            credential_generation: Arc::new(AtomicU64::new(0)),
            toast_next_id: Arc::new(AtomicU64::new(0)),
            toast_records: Arc::new(Mutex::new(Vec::new())),
            background_mode: Rc::new(Cell::new(false)),
        })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            main: self.main.as_weak(),
            popup: self.popup.as_weak(),
            settings: self.settings.as_weak(),
            language_menu: self.language_menu.as_weak(),
            prepare_passive_window: self.prepare_passive_window,
            language_menu_generation: Arc::clone(&self.language_menu_generation),
            credential_generation: Arc::clone(&self.credential_generation),
            toast_next_id: Arc::clone(&self.toast_next_id),
            toast_records: Arc::clone(&self.toast_records),
            disarm_window_context: Arc::clone(&self.disarm_window_context),
        }
    }

    pub fn on_event(
        &self,
        handler: impl Fn(AppEvent) + 'static,
        screen_context: impl Fn() -> Option<(Point, Rect)> + 'static,
    ) {
        let handler = Rc::new(handler);
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
        let close_handler = Rc::clone(&handler);
        self.popup.window().on_close_requested(move || {
            close_handler(AppEvent::PopupHidden);
            slint::CloseRequestResponse::KeepWindowShown
        });
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        let language_menu_monitor = Rc::clone(&self.language_menu_monitor);
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        self.settings.window().on_close_requested(move || {
            close_language_menu(
                &settings,
                &language_menu,
                &language_menu_monitor,
                &language_menu_generation,
                &disarm_window_context,
                LanguageMenuCloseReason::WindowClosed,
            );
            if let Some(settings) = settings.upgrade() {
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
        let language_menu = self.language_menu.as_weak();
        let language_menu_monitor = Rc::clone(&self.language_menu_monitor);
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let arm_window_context = Arc::clone(&self.arm_window_context);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        let prepare_interactive_window = self.prepare_interactive_window;
        let set_transient_window_owner = self.set_transient_window_owner;
        self.settings.on_language_menu_requested(
            move |selector_x, selector_y, selector_width, selector_height, pointer_x, pointer_y| {
                let (Some(settings), Some(language_menu)) =
                    (settings.upgrade(), language_menu.upgrade())
                else {
                    return;
                };
                if settings.get_language_menu_open() {
                    close_language_menu(
                        &settings.as_weak(),
                        &language_menu.as_weak(),
                        &language_menu_monitor,
                        &language_menu_generation,
                        &disarm_window_context,
                        LanguageMenuCloseReason::OutsideClick,
                    );
                    return;
                }
                let opening_generation = begin_language_menu_opening(
                    &settings,
                    &language_menu_monitor,
                    &language_menu_generation,
                );
                let scale = settings.window().scale_factor();
                let window_position = settings.window().position();
                let current_screen = screen_context();
                let cursor = current_screen.map(|context| context.0).unwrap_or(Point {
                    x: window_position.x + ((selector_x + pointer_x) * scale).round() as i32,
                    y: window_position.y + ((selector_y + pointer_y) * scale).round() as i32,
                });
                let work_area = current_screen.map(|context| context.1).unwrap_or(Rect {
                    left: window_position.x,
                    top: window_position.y,
                    right: window_position.x + settings.window().size().width as i32,
                    bottom: window_position.y + settings.window().size().height as i32,
                });
                let geometry = language_menu_geometry(
                    cursor,
                    work_area,
                    SettingsMenuAnchor {
                        width: selector_width,
                        height: selector_height,
                        pointer_x,
                        pointer_y,
                    },
                    scale,
                    if settings.get_settings_menu_provider_mode() {
                        40.0
                    } else {
                        242.0
                    },
                );
                let provider_mode = settings.get_settings_menu_provider_mode();
                language_menu.set_provider_mode(provider_mode);
                language_menu.set_selected_index(if provider_mode {
                    0
                } else {
                    settings.get_draft_target_index()
                });
                language_menu.set_scroll_y(if provider_mode {
                    0.0
                } else {
                    -40.0 * settings.get_draft_target_index().clamp(0, 6) as f32
                });
                language_menu
                    .window()
                    .set_position(PhysicalPosition::new(geometry.left, geometry.top));
                language_menu.window().set_size(PhysicalSize::new(
                    geometry.width as u32,
                    geometry.height as u32,
                ));
                if !set_transient_window_owner(language_menu.window(), settings.window())
                    || !prepare_interactive_window(language_menu.window())
                {
                    close_language_menu(
                        &settings.as_weak(),
                        &language_menu.as_weak(),
                        &language_menu_monitor,
                        &language_menu_generation,
                        &disarm_window_context,
                        LanguageMenuCloseReason::SetupFailed,
                    );
                    return;
                }
                if language_menu.show().is_ok()
                    && set_transient_window_owner(language_menu.window(), settings.window())
                    && prepare_interactive_window(language_menu.window())
                {
                    let initial_position = settings.window().position();
                    let settings_for_monitor = settings.as_weak();
                    let language_menu_for_monitor = language_menu.as_weak();
                    let monitor_for_callback = Rc::downgrade(&language_menu_monitor);
                    let generation_for_callback = Arc::clone(&language_menu_generation);
                    let arm_context_for_callback = Arc::clone(&arm_window_context);
                    let disarm_context_for_callback = Arc::clone(&disarm_window_context);
                    slint::Timer::single_shot(LANGUAGE_MENU_STABILIZATION, move || {
                        if !language_menu_session_is_current(
                            &generation_for_callback,
                            opening_generation,
                        ) {
                            return;
                        }
                        let (Some(settings), Some(language_menu), Some(monitor)) = (
                            settings_for_monitor.upgrade(),
                            language_menu_for_monitor.upgrade(),
                            monitor_for_callback.upgrade(),
                        ) else {
                            generation_for_callback.fetch_add(1, Ordering::SeqCst);
                            return;
                        };
                        let current_position = settings.window().position();
                        let decision = language_menu_opening_decision(
                            settings.get_language_menu_open(),
                            language_menu.window().is_visible(),
                            (initial_position.x, initial_position.y),
                            (current_position.x, current_position.y),
                            settings.window().is_visible(),
                            settings.window().is_minimized(),
                        );
                        match decision {
                            LanguageMenuMonitorDecision::Continue => {
                                if !arm_context_for_callback(
                                    settings.window(),
                                    language_menu.window(),
                                ) {
                                    close_language_menu(
                                        &settings_for_monitor,
                                        &language_menu_for_monitor,
                                        &monitor,
                                        &generation_for_callback,
                                        &disarm_context_for_callback,
                                        LanguageMenuCloseReason::SetupFailed,
                                    );
                                    return;
                                }
                                settings.set_language_menu_dismiss_armed(true);
                            }
                            LanguageMenuMonitorDecision::Stop => {
                                monitor.stop();
                                return;
                            }
                            LanguageMenuMonitorDecision::Close(reason) => {
                                close_language_menu(
                                    &settings_for_monitor,
                                    &language_menu_for_monitor,
                                    &monitor,
                                    &generation_for_callback,
                                    &disarm_context_for_callback,
                                    reason,
                                );
                                return;
                            }
                        }

                        let settings_for_tick = settings_for_monitor.clone();
                        let language_menu_for_tick = language_menu_for_monitor.clone();
                        let monitor_for_tick = Rc::downgrade(&monitor);
                        let generation_for_tick = Arc::clone(&generation_for_callback);
                        let disarm_context_for_tick = Arc::clone(&disarm_context_for_callback);
                        monitor.start(
                            slint::TimerMode::Repeated,
                            LANGUAGE_MENU_MONITOR_INTERVAL,
                            move || {
                                let Some(monitor) = monitor_for_tick.upgrade() else {
                                    return;
                                };
                                if !language_menu_session_is_current(
                                    &generation_for_tick,
                                    opening_generation,
                                ) {
                                    monitor.stop();
                                    return;
                                }
                                let (Some(settings), Some(language_menu)) = (
                                    settings_for_tick.upgrade(),
                                    language_menu_for_tick.upgrade(),
                                ) else {
                                    monitor.stop();
                                    return;
                                };
                                let current_position = settings.window().position();
                                match language_menu_monitor_decision(
                                    settings.get_language_menu_open(),
                                    language_menu.window().is_visible(),
                                    (initial_position.x, initial_position.y),
                                    (current_position.x, current_position.y),
                                    settings.window().is_visible(),
                                    settings.window().is_minimized(),
                                ) {
                                    LanguageMenuMonitorDecision::Continue => {}
                                    LanguageMenuMonitorDecision::Stop => monitor.stop(),
                                    LanguageMenuMonitorDecision::Close(reason) => {
                                        close_language_menu(
                                            &settings_for_tick,
                                            &language_menu_for_tick,
                                            &monitor,
                                            &generation_for_tick,
                                            &disarm_context_for_tick,
                                            reason,
                                        );
                                    }
                                }
                            },
                        );
                    });
                } else {
                    close_language_menu(
                        &settings.as_weak(),
                        &language_menu.as_weak(),
                        &language_menu_monitor,
                        &language_menu_generation,
                        &disarm_window_context,
                        LanguageMenuCloseReason::SetupFailed,
                    );
                }
            },
        );
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        let language_menu_monitor = Rc::clone(&self.language_menu_monitor);
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        self.settings.on_language_menu_dismiss_requested(move || {
            close_language_menu(
                &settings,
                &language_menu,
                &language_menu_monitor,
                &language_menu_generation,
                &disarm_window_context,
                LanguageMenuCloseReason::OutsideClick,
            );
        });
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        let language_menu_monitor = Rc::clone(&self.language_menu_monitor);
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        self.settings
            .on_language_menu_external_dismiss_requested(move || {
                close_language_menu(
                    &settings,
                    &language_menu,
                    &language_menu_monitor,
                    &language_menu_generation,
                    &disarm_window_context,
                    LanguageMenuCloseReason::ExternalInteraction,
                );
            });
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        let language_menu_monitor = Rc::clone(&self.language_menu_monitor);
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        let settings_handler = Rc::clone(&handler);
        self.language_menu.on_selected(move |index| {
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
            close_language_menu(
                &settings,
                &language_menu,
                &language_menu_monitor,
                &language_menu_generation,
                &disarm_window_context,
                LanguageMenuCloseReason::ValueSelected,
            );
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
        let _ = self.popup.hide();
        close_language_menu(
            &self.settings.as_weak(),
            &self.language_menu.as_weak(),
            &self.language_menu_monitor,
            &self.language_menu_generation,
            &self.disarm_window_context,
            LanguageMenuCloseReason::WindowClosed,
        );
        let _ = self.settings.hide();
        self.main.hide()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LanguageMenuMonitorDecision {
    Continue,
    Close(LanguageMenuCloseReason),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LanguageMenuCloseReason {
    Cancelled,
    ExternalInteraction,
    MenuHidden,
    OwnerHidden,
    OwnerMinimized,
    OwnerMoved,
    OutsideClick,
    SetupFailed,
    ValueSelected,
    WindowClosed,
}

fn begin_language_menu_opening(
    settings: &SettingsWindow,
    monitor: &slint::Timer,
    generation: &AtomicU64,
) -> u64 {
    monitor.stop();
    let generation = generation.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    settings.set_language_menu_open(true);
    settings.set_language_menu_dismiss_armed(false);
    generation
}

fn close_language_menu(
    settings: &slint::Weak<SettingsWindow>,
    language_menu: &slint::Weak<LanguageMenuWindow>,
    monitor: &slint::Timer,
    generation: &AtomicU64,
    disarm_window_context: &DisarmWindowContext,
    reason: LanguageMenuCloseReason,
) {
    monitor.stop();
    disarm_window_context();
    reset_language_menu_state(settings, language_menu, generation, reason);
}

fn reset_language_menu_state(
    settings: &slint::Weak<SettingsWindow>,
    language_menu: &slint::Weak<LanguageMenuWindow>,
    generation: &AtomicU64,
    reason: LanguageMenuCloseReason,
) {
    generation.fetch_add(1, Ordering::SeqCst);
    let mut was_open = false;
    if let Some(settings) = settings.upgrade() {
        was_open = settings.get_language_menu_open();
        settings.set_language_menu_open(false);
        settings.set_language_menu_dismiss_armed(false);
    }
    if let Some(language_menu) = language_menu.upgrade() {
        let _ = language_menu.hide();
    }
    if was_open {
        tracing::debug!(reason = ?reason, "language menu closed");
    }
}

fn language_menu_session_is_current(generation: &AtomicU64, expected_generation: u64) -> bool {
    generation.load(Ordering::SeqCst) == expected_generation
}

fn language_menu_monitor_decision(
    menu_open: bool,
    menu_visible: bool,
    initial_position: (i32, i32),
    current_position: (i32, i32),
    settings_visible: bool,
    settings_minimized: bool,
) -> LanguageMenuMonitorDecision {
    language_menu_opening_decision(
        menu_open,
        menu_visible,
        initial_position,
        current_position,
        settings_visible,
        settings_minimized,
    )
}

fn language_menu_opening_decision(
    menu_open: bool,
    menu_visible: bool,
    initial_position: (i32, i32),
    current_position: (i32, i32),
    settings_visible: bool,
    settings_minimized: bool,
) -> LanguageMenuMonitorDecision {
    if !menu_open {
        return LanguageMenuMonitorDecision::Stop;
    }
    if !menu_visible {
        return LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::MenuHidden);
    }
    if initial_position != current_position {
        return LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::OwnerMoved);
    }
    if !settings_visible {
        return LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::OwnerHidden);
    }
    if settings_minimized {
        return LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::OwnerMinimized);
    }
    LanguageMenuMonitorDecision::Continue
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
    popup: slint::Weak<TranslationPopup>,
    settings: slint::Weak<SettingsWindow>,
    language_menu: slint::Weak<LanguageMenuWindow>,
    prepare_passive_window: fn(&slint::Window) -> bool,
    language_menu_generation: Arc<AtomicU64>,
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    disarm_window_context: DisarmWindowContext,
}

impl UiHandle {
    /// Queues closure of an open language menu after native external interaction.
    pub fn dismiss_language_menu(&self) {
        let settings = self.settings.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(settings) = settings.upgrade() {
                settings.invoke_language_menu_external_dismiss_requested();
            }
        });
    }

    /// Queues state rendering on the Slint event-loop thread.
    pub fn update(&self, state: AppState) {
        let main = self.main.clone();
        let popup = self.popup.clone();
        let settings = self.settings.clone();
        let view_state = mapper::view_state(&state);
        let _ = slint::invoke_from_event_loop(move || {
            if let (Some(main), Some(popup), Some(settings)) =
                (main.upgrade(), popup.upgrade(), settings.upgrade())
            {
                binding::apply(&main, &popup, &settings, view_state);
            }
        });
    }

    pub fn show_popup(&self, anchor: Option<Point>, work_area: Option<Rect>) {
        let popup = self.popup.clone();
        let prepare_passive_window = self.prepare_passive_window;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(popup) = popup.upgrade() {
                if !prepare_passive_window(popup.window()) {
                    return;
                }
                // Restore before positioning: Windows restores the previous normal bounds
                // when leaving the minimized state, overriding an earlier position update.
                if popup.window().is_minimized() {
                    popup.window().set_minimized(false);
                }
                if let (Some(anchor), Some(work_area)) = (anchor, work_area) {
                    let size = popup.window().size();
                    let placement = placement::place_popup(
                        anchor,
                        size.width,
                        size.height,
                        work_area,
                        POPUP_GAP_PX,
                        WORK_AREA_MARGIN_PX,
                    );
                    popup.window().set_position(slint::PhysicalPosition::new(
                        placement.position.x,
                        placement.position.y,
                    ));
                    tracing::debug!(
                        horizontal = placement.horizontal.as_str(),
                        vertical = placement.vertical.as_str(),
                        "translation popup positioned"
                    );
                }
                // Repeated selection requests only update an already visible popup.
                // Avoid native show/activation side effects on the source application's focus.
                if !popup.window().is_visible()
                    && popup.show().is_ok()
                    && !prepare_passive_window(popup.window())
                {
                    let _ = popup.hide();
                }
            }
        });
    }

    pub fn hide_popup(&self) {
        let popup = self.popup.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(popup) = popup.upgrade() {
                let _ = popup.hide();
            }
        });
    }

    pub fn show_main_window(&self) {
        let main = self.main.clone();
        let settings = self.settings.clone();
        let language_menu = self.language_menu.clone();
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            reset_language_menu_state(
                &settings,
                &language_menu,
                &language_menu_generation,
                LanguageMenuCloseReason::ExternalInteraction,
            );
            disarm_window_context();
            if let Some(settings) = settings.upgrade() {
                clear_credential_transient(&settings, &credential_generation);
                clear_settings_toasts(&settings, &toast_records);
            }
            if let Some(main) = main.upgrade() {
                if main.window().is_minimized() {
                    main.window().set_minimized(false);
                }
                let _ = main.show();
            }
        });
    }

    pub fn show_settings_window(&self, settings: Settings) {
        let window = self.settings.clone();
        let language_menu = self.language_menu.clone();
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            reset_language_menu_state(
                &window,
                &language_menu,
                &language_menu_generation,
                LanguageMenuCloseReason::Cancelled,
            );
            disarm_window_context();
            if let Some(window) = window.upgrade() {
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
        let language_menu = self.language_menu.clone();
        let language_menu_generation = Arc::clone(&self.language_menu_generation);
        let disarm_window_context = Arc::clone(&self.disarm_window_context);
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            reset_language_menu_state(
                &settings,
                &language_menu,
                &language_menu_generation,
                LanguageMenuCloseReason::WindowClosed,
            );
            disarm_window_context();
            if let Some(settings) = settings.upgrade() {
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
struct LanguageMenuGeometry {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SettingsMenuAnchor {
    width: f32,
    height: f32,
    pointer_x: f32,
    pointer_y: f32,
}

fn language_menu_geometry(
    cursor: Point,
    work_area: Rect,
    anchor: SettingsMenuAnchor,
    scale: f32,
    desired_height: f32,
) -> LanguageMenuGeometry {
    let gap = (2.0 * scale).round() as i32;
    let desired_height = (desired_height * scale).round() as i32;
    let width = ((anchor.width * scale).round() as i32)
        .max(1)
        .min((work_area.right - work_area.left).max(1));
    let selector_left = cursor.x - (anchor.pointer_x * scale).round() as i32;
    let selector_top = cursor.y - (anchor.pointer_y * scale).round() as i32;
    let selector_bottom = selector_top + (anchor.height * scale).round() as i32;
    let below_top = selector_bottom + gap;
    let below_space = (work_area.bottom - below_top).max(1);
    let above_space = (selector_top - gap - work_area.top).max(0);
    let (top, height) = if below_space >= desired_height {
        (below_top, desired_height)
    } else if above_space >= desired_height {
        (selector_top - gap - desired_height, desired_height)
    } else {
        (below_top, below_space)
    };
    let left = selector_left.clamp(work_area.left, work_area.right - width);

    LanguageMenuGeometry {
        left,
        top,
        width,
        height,
    }
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
        LanguageMenuCloseReason, LanguageMenuGeometry, LanguageMenuMonitorDecision,
        MainWindowClosePolicy, SettingsMenuAnchor, SettingsToastRecord, close_policy,
        credential_session_is_current, language_index, language_menu_geometry,
        language_menu_monitor_decision, language_menu_opening_decision,
        language_menu_session_is_current, remove_settings_toast_record,
    };

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
    fn stale_language_menu_generation_cannot_affect_the_current_session() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let generation = AtomicU64::new(7);
        assert!(language_menu_session_is_current(&generation, 7));
        generation.fetch_add(1, Ordering::SeqCst);
        assert!(!language_menu_session_is_current(&generation, 7));
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
    fn language_menu_opening_does_not_depend_on_foreground_context() {
        assert_eq!(
            language_menu_opening_decision(true, true, (10, 20), (10, 20), true, false,),
            LanguageMenuMonitorDecision::Continue
        );
    }

    #[test]
    fn language_menu_stays_open_while_settings_remains_available_and_stationary() {
        assert_eq!(
            language_menu_monitor_decision(true, true, (10, 20), (10, 20), true, false),
            LanguageMenuMonitorDecision::Continue
        );
    }

    #[test]
    fn language_menu_closes_when_settings_moves_or_loses_availability() {
        assert_eq!(
            language_menu_monitor_decision(true, true, (10, 20), (11, 20), true, false),
            LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::OwnerMoved)
        );
        assert_eq!(
            language_menu_monitor_decision(true, true, (10, 20), (10, 20), false, false),
            LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::OwnerHidden)
        );
        assert_eq!(
            language_menu_monitor_decision(true, true, (10, 20), (10, 20), true, true),
            LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::OwnerMinimized)
        );
    }

    #[test]
    fn closed_or_hidden_language_menu_stops_monitoring() {
        assert_eq!(
            language_menu_monitor_decision(false, true, (10, 20), (10, 20), true, false),
            LanguageMenuMonitorDecision::Stop
        );
        assert_eq!(
            language_menu_monitor_decision(true, false, (10, 20), (10, 20), true, false),
            LanguageMenuMonitorDecision::Close(LanguageMenuCloseReason::MenuHidden)
        );
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

    #[test]
    fn language_menu_prefers_below_then_above_then_scrolls_below() {
        let work_area = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        assert_eq!(
            language_menu_geometry(
                Point { x: 150, y: 120 },
                work_area,
                SettingsMenuAnchor {
                    width: 400.0,
                    height: 40.0,
                    pointer_x: 50.0,
                    pointer_y: 20.0,
                },
                1.0,
                242.0,
            ),
            LanguageMenuGeometry {
                left: 100,
                top: 142,
                width: 400,
                height: 242,
            }
        );
        assert_eq!(
            language_menu_geometry(
                Point { x: 150, y: 740 },
                work_area,
                SettingsMenuAnchor {
                    width: 400.0,
                    height: 40.0,
                    pointer_x: 50.0,
                    pointer_y: 20.0,
                },
                1.0,
                242.0,
            )
            .top,
            476
        );
        let constrained = language_menu_geometry(
            Point { x: 150, y: 390 },
            Rect {
                top: 200,
                bottom: 500,
                ..work_area
            },
            SettingsMenuAnchor {
                width: 400.0,
                height: 40.0,
                pointer_x: 50.0,
                pointer_y: 20.0,
            },
            1.0,
            242.0,
        );
        assert_eq!(constrained.top, 412);
        assert_eq!(constrained.height, 88);

        let provider = language_menu_geometry(
            Point { x: 150, y: 120 },
            work_area,
            SettingsMenuAnchor {
                width: 400.0,
                height: 40.0,
                pointer_x: 50.0,
                pointer_y: 20.0,
            },
            1.0,
            40.0,
        );
        assert_eq!(provider.height, 40);
        assert_eq!(provider.top, 142);
    }
}
