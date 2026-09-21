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
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    AppWindow, SettingsToastData, SettingsWindow, TranslationPopup, binding, mapper, placement,
};

pub struct WindowLifecycleCallbacks {
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

impl WindowLifecycleCallbacks {
    pub fn new(activate_user_requested_window: fn(&slint::Window) -> bool) -> Self {
        Self {
            activate_user_requested_window,
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
    prepare_passive_window: fn(&slint::Window) -> bool,
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    background_mode: Rc<Cell<bool>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

impl Ui {
    pub fn new(
        initial_state: &AppState,
        show_selection_demo: bool,
        prepare_passive_window: fn(&slint::Window) -> bool,
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
        binding::apply(&main, &popup, &settings, mapper::view_state(initial_state));
        Ok(Self {
            main,
            popup,
            settings,
            prepare_passive_window,
            credential_generation: Arc::new(AtomicU64::new(0)),
            toast_next_id: Arc::new(AtomicU64::new(0)),
            toast_records: Arc::new(Mutex::new(Vec::new())),
            background_mode: Rc::new(Cell::new(false)),
            activate_user_requested_window: window_lifecycle.activate_user_requested_window,
        })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            main: self.main.as_weak(),
            popup: self.popup.as_weak(),
            settings: self.settings.as_weak(),
            prepare_passive_window: self.prepare_passive_window,
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
        let _ = self.popup.hide();
        let _ = self.settings.hide();
        self.main.hide()
    }
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
    prepare_passive_window: fn(&slint::Window) -> bool,
    credential_generation: Arc<AtomicU64>,
    toast_next_id: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
    activate_user_requested_window: fn(&slint::Window) -> bool,
}

impl UiHandle {
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
        MainWindowClosePolicy, SettingsToastRecord, close_policy, credential_session_is_current,
        language_index, remove_settings_toast_record,
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
