//! Settings window lifecycle and view wiring.
use super::*;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettingsToastRecord {
    pub(super) id: i32,
    pub(super) message: String,
    pub(super) error: bool,
}

pub(super) fn retry_settings_resize_background(
    configure: ConfigureResizeBackground,
    configure_repair: ConfigureWindowPaintRepair,
    settings: slint::Weak<SettingsWindow>,
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
        if !install_settings_platform_hooks(
            &configure,
            &configure_repair,
            &settings,
            settings.get_resize_fallback_color(),
        ) {
            retry_settings_resize_background(
                configure,
                configure_repair,
                settings.as_weak(),
                attempts - 1,
            );
        }
    });
}

pub(super) fn install_settings_platform_hooks(
    configure: &ConfigureResizeBackground,
    configure_repair: &ConfigureWindowPaintRepair,
    settings: &SettingsWindow,
    color: slint::Color,
) -> bool {
    let background = install_resize_background(configure, settings.window(), color);
    let repair = configure_repair(settings.window(), window_paint_repair(settings));
    background && repair
}

pub(super) fn schedule_settings_restore_repair(
    settings: &SettingsWindow,
    pending: &Arc<std::sync::atomic::AtomicBool>,
) {
    if pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let pending = pending.clone();
    let _ = settings.as_weak().upgrade_in_event_loop(move |settings| {
        pending.store(false, Ordering::Release);
        if !settings.window().is_visible() || settings.window().is_minimized() {
            return;
        }
        let size = settings.window().size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        tracing::debug!(
            width = size.width,
            height = size.height,
            "Settings restore: full paint repair"
        );
        repair_window_paint(settings.window());
    });
}

pub(super) fn ensure_settings_window(
    registry: &mut AppWindowRegistry,
) -> Result<SettingsWindow, slint::PlatformError> {
    if let Some(settings) = &registry.settings {
        return Ok(settings.clone_strong());
    }
    let settings = SettingsWindow::new()?;
    settings.set_history_supported(registry.history_supported);
    crate::about::wire(&settings, registry.open_about_link.clone());
    // Observe pointer input without consuming it; blank space and editors also
    // end keyboard-only button styling. The weak callback dies with this window.
    {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
        let weak = settings.as_weak();
        let suspended = Cell::new(false);
        let occluded_state = Cell::new(false);
        let restored_redraw = Cell::new(false);
        let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        settings
            .window()
            .on_winit_window_event(move |native, event| {
                if let Some(settings) = weak.upgrade() {
                    if matches!(
                        event,
                        winit::event::WindowEvent::Resized(_)
                            | winit::event::WindowEvent::Occluded(_)
                            | winit::event::WindowEvent::Focused(_)
                            | winit::event::WindowEvent::RedrawRequested
                    ) {
                        let minimized = native.is_minimized();
                        if let winit::event::WindowEvent::Occluded(value) = event {
                            occluded_state.set(*value);
                        }
                        let occluded = occluded_state.get();
                        let size = if let winit::event::WindowEvent::Resized(size) = event {
                            slint::PhysicalSize::new(size.width, size.height)
                        } else {
                            native.size()
                        };
                        if minimized || occluded || size.width == 0 || size.height == 0 {
                            if !suspended.replace(true) {
                                tracing::debug!(
                                    minimized,
                                    occluded,
                                    width = size.width,
                                    height = size.height,
                                    "Settings rendering suspended"
                                );
                            }
                            settings.invoke_clear_settings_hint();
                        } else if suspended.replace(false) {
                            tracing::debug!(
                                width = size.width,
                                height = size.height,
                                "Settings rendering restored"
                            );
                            restored_redraw.set(true);
                            schedule_settings_restore_repair(&settings, &pending);
                        }
                    }
                    if matches!(event, winit::event::WindowEvent::RedrawRequested)
                        && restored_redraw.replace(false)
                    {
                        tracing::debug!("Settings restore: backend redraw event");
                    }
                    match event {
                        winit::event::WindowEvent::MouseInput { .. }
                        | winit::event::WindowEvent::Touch(_) => {
                            settings.invoke_settings_pointer_input()
                        }
                        winit::event::WindowEvent::KeyboardInput { event, .. }
                            if event.state == winit::event::ElementState::Pressed =>
                        {
                            use winit::keyboard::{Key, NamedKey};
                            if matches!(
                                event.logical_key,
                                Key::Named(
                                    NamedKey::Tab
                                        | NamedKey::ArrowUp
                                        | NamedKey::ArrowDown
                                        | NamedKey::ArrowLeft
                                        | NamedKey::ArrowRight
                                        | NamedKey::Home
                                        | NamedKey::End
                                        | NamedKey::Enter
                                        | NamedKey::Space
                                        | NamedKey::Escape
                                )
                            ) {
                                settings.invoke_settings_keyboard_input();
                            }
                        }
                        winit::event::WindowEvent::Focused(false) => {
                            settings.invoke_reset_button_visuals()
                        }
                        _ => {}
                    }
                }
                EventResult::Propagate
            });
    }
    crate::theme::apply(&settings);
    let weak = settings.as_weak();
    let configure = registry.configure_resize_background.clone();
    settings.on_theme_background_changed(move || {
        if let Some(settings) = weak.upgrade() {
            install_resize_background(
                &configure,
                settings.window(),
                settings.get_resize_fallback_color(),
            );
        }
    });
    settings.set_annotation_hotkey_status(crate::annotation::status().into());
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
    crate::ocr::provider::apply_settings(&settings, &registry.latest_state);
    if let Some(handler) = registry.handler.clone() {
        wire_settings_callbacks(
            &settings,
            handler,
            Arc::clone(&registry.credential_generation),
            Arc::clone(&registry.toast_records),
        );
    }
    if let Some(handler) = registry.handler.clone() {
        crate::llm_settings::wire(
            &settings,
            registry.latest_state.desired_settings.llm.clone(),
            registry.llm_query.clone(),
            handler,
        );
    }
    registry.settings = Some(settings.clone_strong());
    Ok(settings)
}

pub(super) fn schedule_settings_window_destruction(settings: &SettingsWindow) {
    let token = settings.get_llm_page_token();
    let generation = APP_WINDOW_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.as_mut()?;
        if registry
            .settings
            .as_ref()
            .is_none_or(|window| window.get_llm_page_token() != token)
        {
            return None;
        }
        registry.settings_show_timer.stop();
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
                && registry
                    .settings
                    .as_ref()
                    .is_some_and(|window| window.get_llm_page_token() == token)
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

pub(super) fn current_history_settings(weak: &slint::Weak<SettingsWindow>) -> bool {
    weak.upgrade().is_some_and(|window| {
        APP_WINDOW_REGISTRY.with(|r| {
            r.borrow()
                .as_ref()
                .and_then(|r| r.settings.as_ref())
                .is_some_and(|current| current.get_llm_page_token() == window.get_llm_page_token())
        })
    })
}

pub(super) fn wire_settings_callbacks(
    settings: &SettingsWindow,
    handler: Rc<dyn Fn(AppEvent)>,
    credential_generation: Arc<AtomicU64>,
    toast_records: Arc<Mutex<Vec<SettingsToastRecord>>>,
) {
    let weak = settings.as_weak();
    settings.window().on_close_requested(move || {
        if let Some(window) = weak.upgrade() {
            if crate::llm_settings::has_unsaved(&window) {
                window.invoke_confirm_settings_close();
            } else {
                schedule_settings_window_destruction(&window);
            }
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = settings.as_weak();
    settings.on_discard_settings_requested(move || {
        if let Some(window) = weak.upgrade() {
            schedule_settings_window_destruction(&window);
        }
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
    let h = handler.clone();
    settings.on_ocr_hotkey_change_requested(move |value| {
        if let Ok(hotkey) = value.to_string().parse::<HotkeyConfig>() {
            h(AppEvent::SettingsChangeRequested {
                change: SettingsChange::OcrHotkey(hotkey),
            });
        }
    });
    crate::ocr::provider::wire_settings(settings, handler.clone());
    let annotation_settings_handler = Rc::clone(&handler);
    settings.on_annotation_hotkey_change_requested(move |hotkey| {
        if let Ok(hotkey) = hotkey.to_string().parse::<HotkeyConfig>() {
            annotation_settings_handler(AppEvent::SettingsChangeRequested {
                change: SettingsChange::AnnotationHotkey(hotkey),
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
    let language_handler = Rc::clone(&handler);
    settings.on_ui_language_selected(move |index| {
        language_handler(AppEvent::SettingsChangeRequested {
            change: SettingsChange::UiLanguage(
                lexift_core::domain::ui_language::UiLanguage::from_index(index),
            ),
        });
    });
    let theme_handler = Rc::clone(&handler);
    settings.on_theme_selected(move |index| {
        theme_handler(AppEvent::SettingsChangeRequested {
            change: SettingsChange::Theme(crate::theme::from_index(index)),
        });
    });
    settings.on_selection_toolbar_change_requested(move |enabled| {
        settings_handler(AppEvent::SettingsChangeRequested {
            change: SettingsChange::SelectionToolbar(enabled),
        });
    });
    let h = handler.clone();
    let weak = settings.as_weak();
    settings.on_history_enabled_changed(move |value| {
        if current_history_settings(&weak) {
            h(AppEvent::SettingsChangeRequested {
                change: SettingsChange::ClipboardHistoryEnabled(value),
            });
        }
    });
    let h = handler.clone();
    let weak = settings.as_weak();
    settings.on_history_limit_changed(move |value| {
        if current_history_settings(&weak) {
            h(AppEvent::SettingsChangeRequested {
                change: SettingsChange::ClipboardHistoryLimit(value.clamp(10, 200) as u16),
            });
        }
    });
    let h = handler.clone();
    let weak = settings.as_weak();
    settings.on_history_clear_requested(move || {
        if current_history_settings(&weak) {
            h(AppEvent::ClipboardHistory(
                lexift_core::domain::clipboard_history::HistoryEvent::Clear,
            ));
        }
    });
    let credential_handler = Rc::clone(&handler);
    let weak = settings.as_weak();
    let generation = credential_generation.clone();
    settings.on_credential_save_requested(move |secret| {
        if let Some(w) = weak.upgrade() {
            if w.get_provider_detail_id() == "paddleocr" {
                return;
            }
            let provider = credential_provider(&w);
            clear_credential_transient(&w, &generation);
            credential_handler(if let Some(provider) = provider {
                AppEvent::LlmCredentialChangeRequested {
                    provider,
                    secret: Some(CredentialSecret::new(secret.to_string())),
                }
            } else {
                AppEvent::CredentialSaveRequested {
                    secret: CredentialSecret::new(secret.to_string()),
                }
            });
        }
    });
    let credential_handler = Rc::clone(&handler);
    let weak = settings.as_weak();
    let generation = credential_generation.clone();
    settings.on_credential_remove_requested(move || {
        if let Some(w) = weak.upgrade() {
            if w.get_provider_detail_id() == "paddleocr" {
                return;
            }
            let provider = credential_provider(&w);
            clear_credential_transient(&w, &generation);
            credential_handler(if let Some(provider) = provider {
                AppEvent::LlmCredentialChangeRequested {
                    provider,
                    secret: None,
                }
            } else {
                AppEvent::CredentialRemoveRequested
            });
        }
    });
    for purpose in [
        CredentialAccessPurpose::Reveal,
        CredentialAccessPurpose::Copy,
        CredentialAccessPurpose::Edit,
    ] {
        let weak = settings.as_weak();
        let generation = credential_generation.clone();
        let send = handler.clone();
        let callback = move || {
            let current = begin_credential_access(&weak, &generation);
            if let Some(w) = weak.upgrade() {
                w.set_credential_copy_active(purpose == CredentialAccessPurpose::Copy);
                if w.get_provider_detail_id() == "paddleocr" {
                    return;
                }
                send(if let Some(provider) = credential_provider(&w) {
                    AppEvent::LlmCredentialAccessRequested {
                        provider,
                        purpose,
                        generation: current,
                    }
                } else {
                    AppEvent::CredentialAccessRequested {
                        purpose,
                        generation: current,
                    }
                });
            }
        };
        match purpose {
            CredentialAccessPurpose::Reveal => settings.on_credential_reveal_requested(callback),
            CredentialAccessPurpose::Copy => settings.on_credential_copy_requested(callback),
            CredentialAccessPurpose::Edit => settings.on_credential_edit_requested(callback),
        }
    }
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
    let weak = settings.as_weak();
    let generation = credential_generation.clone();
    settings.on_credential_edit_blurred(move || {
        let weak = weak.clone();
        let generation = generation.clone();
        let version = generation.load(Ordering::SeqCst);
        slint::Timer::single_shot(Duration::from_millis(50), move || {
            if generation.load(Ordering::SeqCst) == version
                && let Some(w) = weak.upgrade()
                && !w.invoke_credential_edit_focused()
            {
                clear_credential_transient(&w, &generation);
            }
        });
    });
    let settings_weak = settings.as_weak();
    settings.on_settings_menu_selected(move |index| {
        if let Some(settings) = settings_weak.upgrade()
            && settings.get_draft_target_index() != index
        {
            settings.set_draft_target_index(index);
            handler(AppEvent::SettingsChangeRequested {
                change: SettingsChange::TargetLanguage(language_for_index(index)),
            });
        }
    });
}

pub(super) fn reset_credential_view(settings: &SettingsWindow) {
    settings.set_credential_draft("".into());
    settings.set_credential_transient_secret("".into());
    settings.set_credential_revealed(false);
    settings.set_credential_editing(false);
    settings.set_credential_secret_visible(false);
    settings.set_credential_reveal_dismiss_armed(false);
    settings.set_credential_request_pending(false);
    settings.set_credential_copy_active(false);
    settings.set_credential_copied(false);
}

pub(super) fn credential_provider(
    window: &SettingsWindow,
) -> Option<lexift_core::domain::llm::LlmProvider> {
    lexift_core::domain::llm::LlmProvider::ALL
        .into_iter()
        .find(|provider| provider.id() == window.get_provider_detail_id().as_str())
}
pub(super) fn clear_credential_transient(settings: &SettingsWindow, generation: &AtomicU64) {
    generation.fetch_add(1, Ordering::SeqCst);
    reset_credential_view(settings);
}

pub(super) fn begin_credential_access(
    settings: &slint::Weak<SettingsWindow>,
    generation: &AtomicU64,
) -> u64 {
    let generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(settings) = settings.upgrade() {
        reset_credential_view(&settings);
        settings.set_credential_request_pending(true);
    }
    generation
}

/// Copy feedback uses the same lifetime token as secret access, without retaining the window.
pub(crate) fn complete_credential_copy(
    settings: &SettingsWindow,
    current: Arc<AtomicU64>,
    generation: u64,
) {
    if !credential_session_is_current(&current, generation) {
        return;
    }
    settings.set_credential_copy_active(false);
    settings.set_credential_copied(true);
    let weak = settings.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1500), move || {
        if credential_session_is_current(&current, generation)
            && let Some(w) = weak.upgrade()
        {
            w.set_credential_copied(false);
        }
    });
}

pub(super) fn credential_session_is_current(generation: &AtomicU64, expected: u64) -> bool {
    generation.load(Ordering::SeqCst) == expected
}

pub(super) fn render_settings_toasts(
    settings: &SettingsWindow,
    records: &Arc<Mutex<Vec<SettingsToastRecord>>>,
) {
    let rows = records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|record| SettingsToastData {
            id: record.id,
            message: crate::i18n::tr(&record.message).into(),
            error: record.error,
        })
        .collect::<Vec<_>>();
    settings.set_toast_items(ModelRc::new(VecModel::from(rows)));
}

pub(super) fn remove_settings_toast(
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

/// Keep feedback compact while giving refreshed notices a new timeout identity.
pub(super) fn insert_settings_toast_record(
    records: &Mutex<Vec<SettingsToastRecord>>,
    toast: SettingsToastRecord,
) {
    let mut records = records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    records.retain(|previous| {
        if toast.error {
            previous.message != toast.message || previous.error != toast.error
        } else {
            previous.error
        }
    });
    records.push(toast);
    // A short queue keeps rapid actions from covering the whole settings page.
    let excess = records.len().saturating_sub(3);
    records.drain(..excess);
}

pub(super) fn remove_settings_toast_record(
    records: &Mutex<Vec<SettingsToastRecord>>,
    id: i32,
) -> bool {
    let mut records = records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_len = records.len();
    records.retain(|record| record.id != id);
    records.len() != previous_len
}

pub(super) fn clear_settings_toasts(
    settings: &SettingsWindow,
    records: &Arc<Mutex<Vec<SettingsToastRecord>>>,
) {
    records
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    render_settings_toasts(settings, records);
}

pub(super) fn settings_feedback_content(feedback: SettingsFeedback) -> (&'static str, bool) {
    match feedback {
        SettingsFeedback::SettingsSaved(SettingsField::UiLanguage) => {
            ("Interface language saved", false)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::UiLanguage) => {
            ("Could not save interface language", true)
        }
        SettingsFeedback::SettingsSaved(SettingsField::OcrKoreanAuto) => {
            ("Korean recognition setting saved", false)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::OcrKoreanAuto) => {
            ("Korean recognition setting wasn't saved", true)
        }
        SettingsFeedback::SettingsSaved(SettingsField::OcrLanguage) => {
            ("Recognition language saved", false)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::OcrLanguage) => {
            ("Recognition language wasn't saved", true)
        }
        SettingsFeedback::SettingsSaved(SettingsField::Theme) => ("Theme saved", false),
        SettingsFeedback::SettingsSaveFailed(SettingsField::Theme) => {
            ("Could not save theme", true)
        }
        SettingsFeedback::SettingsSaved(SettingsField::TargetLanguage) => {
            ("Target language saved", false)
        }
        SettingsFeedback::SettingsSaved(SettingsField::OcrHotkey | SettingsField::Hotkey) => {
            ("Shortcut saved", false)
        }
        SettingsFeedback::SettingsSaved(SettingsField::AnnotationHotkey) => {
            ("Shortcut saved", false)
        }
        SettingsFeedback::SettingsSaved(
            SettingsField::Provider
            | SettingsField::Ollama
            | SettingsField::Llm
            | SettingsField::TranslationSelection
            | SettingsField::ProviderEnabled(_)
            | SettingsField::TranslationOptions,
        ) => ("Provider saved", false),
        SettingsFeedback::SettingsSaved(SettingsField::LaunchAtLogin) => {
            ("Startup preference saved", false)
        }
        SettingsFeedback::SettingsSaved(
            SettingsField::ClipboardHistoryEnabled | SettingsField::ClipboardHistoryLimit,
        ) => ("Clipboard settings saved", false),
        SettingsFeedback::SettingsSaved(SettingsField::SelectionToolbar) => {
            ("Selection toolbar preference saved", false)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::TargetLanguage) => {
            ("Target language wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::OcrHotkey | SettingsField::Hotkey) => {
            ("Shortcut wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(SettingsField::AnnotationHotkey) => {
            ("Shortcut wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(
            SettingsField::Provider
            | SettingsField::Ollama
            | SettingsField::Llm
            | SettingsField::TranslationSelection
            | SettingsField::ProviderEnabled(_)
            | SettingsField::TranslationOptions,
        ) => ("Provider wasn't saved", true),
        SettingsFeedback::SettingsSaveFailed(SettingsField::LaunchAtLogin) => {
            ("Startup preference wasn't saved", true)
        }
        SettingsFeedback::SettingsSaveFailed(
            SettingsField::ClipboardHistoryEnabled | SettingsField::ClipboardHistoryLimit,
        ) => ("Could not save clipboard settings", true),
        SettingsFeedback::SettingsSaveFailed(SettingsField::SelectionToolbar) => {
            ("Selection toolbar preference wasn't saved", true)
        }
        SettingsFeedback::ProviderEnabledCompleted { saved: true, .. }
        | SettingsFeedback::TranslationSettingsCompleted { saved: true, .. } => {
            ("Provider saved", false)
        }
        SettingsFeedback::ProviderEnabledCompleted { saved: false, .. }
        | SettingsFeedback::TranslationSettingsCompleted { saved: false, .. } => {
            ("Provider wasn't saved", true)
        }
        SettingsFeedback::CredentialSaved => ("API key saved", false),
        SettingsFeedback::CredentialRemoved => ("API key removed", false),
        SettingsFeedback::CredentialCopied { .. } => ("Copied", false),
        SettingsFeedback::CredentialOperationFailed => ("Credential operation failed", true),
    }
}

impl UiHandle {
    pub fn set_annotation_hotkey_status(&self, status: String) {
        let _ = slint::invoke_from_event_loop(move || {
            crate::annotation::set_status(status.clone());
            APP_WINDOW_REGISTRY.with(|s| {
                if let Some(settings) = s.borrow().as_ref().and_then(|r| r.settings.as_ref()) {
                    settings.set_annotation_hotkey_status(crate::annotation::status().into());
                }
            });
        });
    }
    pub fn show_settings_window(&self, settings: Settings) {
        let activate_user_requested_window = self.activate_user_requested_window;
        let credential_generation = Arc::clone(&self.credential_generation);
        let toast_records = Arc::clone(&self.toast_records);
        let _ = slint::invoke_from_event_loop(move || {
            crate::annotation::pin_editor::finish_now();
            cancel_idle_memory_trim();
            APP_WINDOW_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                let Some(registry) = registry.as_mut() else {
                    return;
                };
                registry.settings_close_generation =
                    registry.settings_close_generation.wrapping_add(1);
                let fresh = registry.settings.is_none();
                let window = match ensure_settings_window(registry) {
                    Ok(window) => window,
                    Err(error) => {
                        tracing::error!(%error, "settings window could not be created");
                        return;
                    }
                };
                // Reopening a live window must not discard its drafts or active modal.
                if fresh {
                    window.invoke_reset_settings_view();
                    window.set_draft_target_index(language_index(&settings.target_language));
                    window.set_draft_hotkey_label(settings.hotkey.to_string().into());
                    window.set_draft_annotation_hotkey_label(
                        settings.annotation_hotkey.to_string().into(),
                    );
                    window.set_draft_provider_id(settings.provider.id().into());
                    crate::llm_settings::reset(&window, &settings.llm, settings.provider);
                    window.set_launch_at_login(settings.launch_at_login);
                    window.set_selection_toolbar(settings.selection_toolbar);
                    window.set_hotkey_capturing(false);
                    window.set_annotation_hotkey_capturing(false);
                    clear_credential_transient(&window, &credential_generation);
                    clear_settings_toasts(&window, &toast_records);
                }
                app_window::schedule(
                    registry,
                    WindowRole::Settings,
                    true,
                    activate_user_requested_window,
                    0,
                );
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
                    registry.settings_show_timer.stop();
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
    /// Reject secret access for closed windows and retired Provider views.
    pub fn credential_access_is_current(&self, generation: u64) -> bool {
        credential_session_is_current(&self.credential_generation, generation)
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
    /// Show dismissible feedback owned by the current Settings window.
    pub fn show_settings_feedback(&self, feedback: SettingsFeedback) {
        if let SettingsFeedback::CredentialCopied { generation } = feedback {
            let current = self.credential_generation.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if !credential_session_is_current(&current, generation) {
                    return;
                }
                APP_WINDOW_REGISTRY.with(|r| {
                    if let Some(w) = r.borrow().as_ref().and_then(|r| r.settings.as_ref()) {
                        complete_credential_copy(w, current, generation);
                    }
                });
            });
            return;
        }
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
            if let SettingsFeedback::ProviderEnabledCompleted {
                request_id,
                provider,
                saved,
            } = feedback
                && !crate::llm_settings::enable_completed(&settings, request_id, provider, saved)
            {
                return;
            }
            if let SettingsFeedback::TranslationSettingsCompleted {
                request_id,
                field,
                saved,
            } = feedback
                && !crate::llm_settings::translation_completed(&settings, request_id, field, saved)
            {
                return;
            }
            insert_settings_toast_record(
                &toast_records,
                SettingsToastRecord {
                    id,
                    message: message.to_owned(),
                    error,
                },
            );
            render_settings_toasts(&settings, &toast_records);
            let settings_for_timeout = settings.as_weak();
            let records_for_timeout = Arc::clone(&toast_records);
            slint::Timer::single_shot(duration, move || {
                remove_settings_toast(&settings_for_timeout, &records_for_timeout, id);
            });
        });
    }
}
