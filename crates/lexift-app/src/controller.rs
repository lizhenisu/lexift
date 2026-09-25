use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};

use lexift_core::{
    AppCommand, AppEvent, AppState, TranslationTaskId,
    domain::{
        geometry::{Point, Rect},
        runtime_config::RuntimeConfig,
        settings::{Settings, SettingsChange, SettingsFeedback},
        translation::PopupSessionId,
    },
    ports::{
        clipboard::ClipboardPort,
        credential::{
            CredentialAccessPurpose, CredentialError, CredentialErrorKind, CredentialSecret,
            CredentialStore,
        },
        hotkey::HotkeyHandler,
        screen::ScreenPort,
        selection::SelectionPort,
        settings::SettingsStore,
        speech::{SpeechPort, SpeechRequest},
        translator::TranslatorPort,
        tray::{TrayAction, TrayHandler},
    },
};
use tokio::{runtime::Handle, sync::Notify};

use crate::runtime::RuntimeManager;

pub(crate) trait ViewPort: Send + Sync {
    fn update(&self, state: AppState);
    fn show_popup(
        &self,
        session_id: PopupSessionId,
        anchor: Option<Point>,
        work_area: Option<Rect>,
    );
    fn hide_popup(&self, session_id: PopupSessionId);
    fn show_main_window(&self);
    fn show_settings_window(&self, settings: Settings);
    fn hide_settings_window(&self);
    fn clear_credential_draft(&self);
    fn present_credential_secret(
        &self,
        purpose: CredentialAccessPurpose,
        generation: u64,
        secret: CredentialSecret,
    );
    fn show_settings_feedback(&self, feedback: SettingsFeedback);
    fn quit(&self);
}

impl ViewPort for lexift_ui::UiHandle {
    fn update(&self, state: AppState) {
        self.update(state);
    }

    fn show_popup(
        &self,
        session_id: PopupSessionId,
        anchor: Option<Point>,
        work_area: Option<Rect>,
    ) {
        self.show_popup(session_id, anchor, work_area);
    }

    fn hide_popup(&self, session_id: PopupSessionId) {
        self.hide_popup(session_id);
    }

    fn show_main_window(&self) {
        self.show_main_window();
    }

    fn show_settings_window(&self, settings: Settings) {
        self.show_settings_window(settings);
    }

    fn hide_settings_window(&self) {
        self.hide_settings_window();
    }

    fn clear_credential_draft(&self) {
        self.clear_credential_draft();
    }

    fn present_credential_secret(
        &self,
        purpose: CredentialAccessPurpose,
        generation: u64,
        secret: CredentialSecret,
    ) {
        self.present_credential_secret(purpose, generation, secret);
    }

    fn show_settings_feedback(&self, feedback: SettingsFeedback) {
        self.show_settings_feedback(feedback);
    }

    fn quit(&self) {
        self.quit();
    }
}

/// Executes Core commands on background workers and returns events to Core.
pub(crate) struct AppController {
    runtime: Handle,
    state: Arc<Mutex<AppState>>,
    selection: Option<Arc<dyn SelectionPort>>,
    screen: Option<Arc<dyn ScreenPort>>,
    translator: Arc<dyn TranslatorPort>,
    settings_store: Arc<dyn SettingsStore>,
    runtime_manager: Option<Arc<RuntimeManager>>,
    credential_store: Option<Arc<dyn CredentialStore>>,
    credential_reference: Option<Arc<RwLock<Option<String>>>>,
    clipboard: Option<Arc<dyn ClipboardPort>>,
    speech: Option<Arc<dyn SpeechPort>>,
    ui: Arc<dyn ViewPort>,
    selection_capture_in_flight: AtomicBool,
    selection_toolbar_enabled: AtomicBool,
    state_changed: Arc<Notify>,
}

impl AppController {
    pub(crate) fn new(
        runtime: Handle,
        state: Arc<Mutex<AppState>>,
        selection: Option<Arc<dyn SelectionPort>>,
        screen: Option<Arc<dyn ScreenPort>>,
        translator: Arc<dyn TranslatorPort>,
        settings_store: Arc<dyn SettingsStore>,
        ui: Arc<dyn ViewPort>,
    ) -> Self {
        let selection_toolbar_enabled = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .desired_settings
            .selection_toolbar;
        Self {
            runtime,
            state,
            selection,
            screen,
            translator,
            settings_store,
            runtime_manager: None,
            credential_store: None,
            credential_reference: None,
            clipboard: None,
            speech: None,
            ui,
            selection_capture_in_flight: AtomicBool::new(false),
            selection_toolbar_enabled: AtomicBool::new(selection_toolbar_enabled),
            state_changed: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn with_runtime_manager(mut self, manager: Arc<RuntimeManager>) -> Self {
        self.runtime_manager = Some(manager);
        self
    }

    pub(crate) fn with_credential_management(
        mut self,
        store: Arc<dyn CredentialStore>,
        credential_reference: Arc<RwLock<Option<String>>>,
        clipboard: Option<Arc<dyn ClipboardPort>>,
    ) -> Self {
        self.credential_store = Some(store);
        self.credential_reference = Some(credential_reference);
        self.clipboard = clipboard;
        self
    }

    pub(crate) fn with_speech(mut self, speech: Option<Arc<dyn SpeechPort>>) -> Self {
        self.speech = speech;
        self
    }

    pub(crate) fn dispatch(self: &Arc<Self>, event: AppEvent) {
        if matches!(event, AppEvent::SelectionTranslationRequested)
            && self
                .selection_capture_in_flight
                .swap(true, Ordering::AcqRel)
        {
            tracing::debug!("selection capture is already running; ignoring repeated request");
            return;
        }
        if matches!(
            event,
            AppEvent::SelectionCaptured { .. }
                | AppEvent::SelectionCaptureEmpty { .. }
                | AppEvent::SelectionCaptureFailed { .. }
        ) {
            self.selection_capture_in_flight
                .store(false, Ordering::Release);
        }
        tracing::debug!(event = event_name(&event), "dispatching application event");
        let (snapshot, commands) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let commands = state.reduce(event);
            tracing::debug!(
                task_id = ?state.current_translation_task,
                phase = ?state.phase,
                command_count = commands.len(),
                "application transition completed"
            );
            (state.clone(), commands)
        };
        self.selection_toolbar_enabled.store(
            snapshot.desired_settings.selection_toolbar,
            Ordering::Release,
        );
        self.state_changed.notify_waiters();
        self.ui.update(snapshot);

        for command in commands {
            self.execute(command);
        }
    }

    pub(crate) fn selection_toolbar_enabled(&self) -> bool {
        self.selection_toolbar_enabled.load(Ordering::Acquire)
    }

    pub(crate) fn translate_hotkey_handler(self: &Arc<Self>) -> HotkeyHandler {
        let controller = Arc::clone(self);
        Arc::new(move || controller.dispatch(AppEvent::SelectionTranslationRequested))
    }

    pub(crate) fn tray_handler(self: &Arc<Self>) -> TrayHandler {
        let controller = Arc::clone(self);
        Arc::new(move |action| controller.dispatch(tray_event(action)))
    }

    fn execute(self: &Arc<Self>, command: AppCommand) {
        match command {
            AppCommand::CaptureSelection { task_id } => self.capture_selection(task_id),
            AppCommand::CaptureToolbarSelection { generation, anchor } => {
                self.capture_toolbar_selection(generation, anchor)
            }
            AppCommand::CopyToolbarText { text } => {
                if let Some(clipboard) = self.clipboard.clone() {
                    self.runtime.spawn(async move {
                        let result =
                            tokio::task::spawn_blocking(move || clipboard.write_text(&text)).await;
                        if !matches!(result, Ok(Ok(()))) {
                            tracing::warn!("selection toolbar could not copy text");
                        }
                    });
                }
            }
            AppCommand::Translate { task_id, request } => self.translate(task_id, request),
            AppCommand::TranslatePopup {
                session_id,
                task_id,
                request,
            } => self.translate_popup(session_id, task_id, request),
            AppCommand::ShowPopup { session_id, anchor } => self.show_popup(session_id, anchor),
            AppCommand::HidePopup { session_id } => self.ui.hide_popup(session_id),
            AppCommand::CopyPopupText { session_id, text } => {
                self.copy_popup_text(session_id, text)
            }
            AppCommand::SpeakPopupText {
                session_id,
                source,
                text,
                language,
            } => {
                if let Some(speech) = &self.speech {
                    if let Err(error) = speech.speak(SpeechRequest {
                        session_id,
                        source,
                        text,
                        language,
                    }) {
                        self.dispatch(AppEvent::PopupSpeechStateChanged {
                            session_id,
                            source,
                            speaking: false,
                            error: Some(error.to_string()),
                        });
                    }
                } else {
                    self.dispatch(AppEvent::PopupSpeechStateChanged {
                        session_id,
                        source,
                        speaking: false,
                        error: Some("Speech is unavailable".into()),
                    });
                }
            }
            AppCommand::StopPopupSpeech { session_id } => {
                if let Some(speech) = &self.speech {
                    let _ = speech.stop(session_id);
                }
            }
            AppCommand::ShowMainWindow => self.ui.show_main_window(),
            AppCommand::ShowSettingsWindow => {
                let settings = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .settings
                    .clone();
                self.ui.show_settings_window(settings);
            }
            AppCommand::HideSettingsWindow => self.ui.hide_settings_window(),
            AppCommand::ShowSettingsFeedback { feedback } => {
                self.ui.show_settings_feedback(feedback)
            }
            AppCommand::PersistSettings { settings, change } => {
                self.persist_settings(settings, change)
            }
            AppCommand::ApplyRuntimeConfig {
                settings,
                previous_settings,
                config,
                change,
            } => self.apply_runtime_config(settings, previous_settings, config, change),
            AppCommand::PersistCredential {
                credential_id,
                secret,
            } => self.persist_credential(credential_id, secret),
            AppCommand::RemoveCredential { credential_id } => self.remove_credential(credential_id),
            AppCommand::AccessCredential {
                credential_id,
                purpose,
                generation,
            } => self.access_credential(credential_id, purpose, generation),
            AppCommand::ClearCredentialDraft => self.ui.clear_credential_draft(),
            AppCommand::Exit => self.ui.quit(),
        }
    }

    fn access_credential(
        self: &Arc<Self>,
        credential_id: String,
        purpose: CredentialAccessPurpose,
        generation: u64,
    ) {
        let Some(store) = self.credential_store.clone() else {
            self.dispatch(AppEvent::CredentialAccessFailed {
                error: "Secure credential storage is unavailable".into(),
            });
            return;
        };
        let clipboard = self.clipboard.clone();
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let secret = store
                    .get(&credential_id)
                    .map_err(credential_error_message)?
                    .ok_or_else(|| "Stored DeepL API key is missing".to_owned())?;
                let secret = CredentialSecret::new(secret);
                if purpose == CredentialAccessPurpose::Copy {
                    let clipboard =
                        clipboard.ok_or_else(|| "System clipboard is unavailable".to_owned())?;
                    clipboard
                        .write_text(secret.expose())
                        .map_err(|error| format!("Could not copy DeepL API key: {error}"))?;
                    Ok(None)
                } else {
                    Ok(Some(secret))
                }
            })
            .await;
            match result {
                Ok(Ok(Some(secret))) => {
                    controller
                        .ui
                        .present_credential_secret(purpose, generation, secret);
                    controller.dispatch(AppEvent::CredentialAccessSucceeded { purpose });
                }
                Ok(Ok(None)) => {
                    controller.dispatch(AppEvent::CredentialAccessSucceeded { purpose });
                }
                Ok(Err(error)) => controller.dispatch(AppEvent::CredentialAccessFailed { error }),
                Err(_) => controller.dispatch(AppEvent::CredentialAccessFailed {
                    error: "Credential worker failed".into(),
                }),
            }
        });
    }

    fn persist_credential(self: &Arc<Self>, credential_id: String, secret: CredentialSecret) {
        let Some(store) = self.credential_store.clone() else {
            self.dispatch(AppEvent::CredentialSaveFailed {
                error: "Secure credential storage is unavailable".into(),
            });
            return;
        };
        let Some(reference) = self.credential_reference.clone() else {
            self.dispatch(AppEvent::CredentialSaveFailed {
                error: "Secure credential storage is unavailable".into(),
            });
            return;
        };
        let settings_store = Arc::clone(&self.settings_store);
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            controller.wait_for_settings_idle().await;
            let mut settings = controller
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .settings
                .clone();
            settings.deepl_credential_id = Some(credential_id.clone());
            let id_for_worker = credential_id.clone();
            let result = tokio::task::spawn_blocking(move || {
                let previous = store
                    .get(&id_for_worker)
                    .map_err(credential_error_message)?;
                store
                    .set(&id_for_worker, secret.expose())
                    .map_err(credential_error_message)?;
                if let Err(error) = settings_store.save(&settings) {
                    restore_credential(store.as_ref(), &id_for_worker, previous.as_deref());
                    return Err(format!("Could not save credential reference: {error}"));
                }
                Ok(())
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    *reference
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(credential_id.clone());
                    controller.dispatch(AppEvent::CredentialSaved { credential_id });
                }
                Ok(Err(error)) => controller.dispatch(AppEvent::CredentialSaveFailed { error }),
                Err(_) => controller.dispatch(AppEvent::CredentialSaveFailed {
                    error: "Credential worker failed".into(),
                }),
            }
        });
    }

    fn remove_credential(self: &Arc<Self>, credential_id: String) {
        let Some(store) = self.credential_store.clone() else {
            self.dispatch(AppEvent::CredentialRemoveFailed {
                error: "Secure credential storage is unavailable".into(),
            });
            return;
        };
        let Some(reference) = self.credential_reference.clone() else {
            self.dispatch(AppEvent::CredentialRemoveFailed {
                error: "Secure credential storage is unavailable".into(),
            });
            return;
        };
        let settings_store = Arc::clone(&self.settings_store);
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            controller.wait_for_settings_idle().await;
            let mut settings = controller
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .settings
                .clone();
            settings.deepl_credential_id = None;
            let id_for_worker = credential_id;
            let result = tokio::task::spawn_blocking(move || {
                let previous = store
                    .get(&id_for_worker)
                    .map_err(credential_error_message)?;
                if previous.is_some() {
                    store
                        .delete(&id_for_worker)
                        .map_err(credential_error_message)?;
                }
                if let Err(error) = settings_store.save(&settings) {
                    restore_credential(store.as_ref(), &id_for_worker, previous.as_deref());
                    return Err(format!("Could not remove credential reference: {error}"));
                }
                Ok(())
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    *reference
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                    controller.dispatch(AppEvent::CredentialRemoved);
                }
                Ok(Err(error)) => controller.dispatch(AppEvent::CredentialRemoveFailed { error }),
                Err(_) => controller.dispatch(AppEvent::CredentialRemoveFailed {
                    error: "Credential worker failed".into(),
                }),
            }
        });
    }

    fn persist_settings(self: &Arc<Self>, settings: Settings, change: SettingsChange) {
        let store = Arc::clone(&self.settings_store);
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            let settings_to_save = settings.clone();
            let result = tokio::task::spawn_blocking(move || store.save(&settings_to_save)).await;
            match result {
                Ok(Ok(())) => controller.dispatch(AppEvent::RuntimeConfigChanged {
                    config: settings.runtime_config(),
                    settings,
                    change,
                }),
                Ok(Err(error)) => controller.dispatch(AppEvent::SettingsSaveFailed {
                    change,
                    error: error.to_string(),
                }),
                Err(error) => controller.dispatch(AppEvent::SettingsSaveFailed {
                    change,
                    error: format!("Settings worker failed: {error}"),
                }),
            }
        });
    }

    async fn wait_for_settings_idle(&self) {
        loop {
            let notified = self.state_changed.notified();
            if !self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .settings_saving
            {
                return;
            }
            notified.await;
        }
    }

    fn apply_runtime_config(
        self: &Arc<Self>,
        settings: Settings,
        previous_settings: Settings,
        config: RuntimeConfig,
        change: SettingsChange,
    ) {
        let manager = self.runtime_manager.clone();
        let store = Arc::clone(&self.settings_store);
        let controller = Arc::clone(self);
        let field = change.field();
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                if let Some(manager) = manager {
                    manager.apply(field, config.clone())?;
                }
                Ok::<_, lexift_core::Error>(config)
            })
            .await;
            match result {
                Ok(Ok(config)) => controller.dispatch(AppEvent::RuntimeConfigUpdated {
                    settings,
                    config,
                    change,
                }),
                Ok(Err(error)) => {
                    let message = error.to_string();
                    let rollback =
                        tokio::task::spawn_blocking(move || store.save(&previous_settings)).await;
                    if !matches!(rollback, Ok(Ok(()))) {
                        tracing::warn!("settings rollback failed after runtime apply error");
                    }
                    controller.dispatch(AppEvent::RuntimeConfigUpdateFailed {
                        change,
                        error: message,
                    });
                }
                Err(error) => {
                    let rollback =
                        tokio::task::spawn_blocking(move || store.save(&previous_settings)).await;
                    if !matches!(rollback, Ok(Ok(()))) {
                        tracing::warn!("settings rollback failed after runtime worker error");
                    }
                    controller.dispatch(AppEvent::RuntimeConfigUpdateFailed {
                        change,
                        error: format!("Runtime configuration worker failed: {error}"),
                    });
                }
            }
        });
    }

    fn show_popup(&self, session_id: PopupSessionId, selection_anchor: Option<Point>) {
        let Some(screen) = &self.screen else {
            self.ui.show_popup(session_id, None, None);
            return;
        };

        let anchor = selection_anchor.or_else(|| match screen.cursor_position() {
            Ok(point) => Some(point),
            Err(error) => {
                tracing::warn!(%error, "popup cursor fallback is unavailable");
                None
            }
        });
        let work_area = anchor.and_then(|point| match screen.work_area_for_point(point) {
            Ok(area) => Some(area),
            Err(error) => {
                tracing::warn!(%error, "popup monitor work area is unavailable");
                None
            }
        });

        self.ui.show_popup(session_id, anchor, work_area);
    }

    fn capture_selection(self: &Arc<Self>, task_id: TranslationTaskId) {
        tracing::debug!(?task_id, "selection capture started");
        let Some(selection) = self.selection.clone() else {
            self.dispatch(AppEvent::SelectionCaptureFailed {
                task_id,
                error: "Selection capability is not available".into(),
            });
            return;
        };
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || selection.selected_text()).await;
            match result {
                Ok(Ok(Some(selection))) => {
                    controller.dispatch(AppEvent::SelectionCaptured { task_id, selection });
                }
                Ok(Ok(None)) => controller.dispatch(AppEvent::SelectionCaptureEmpty { task_id }),
                Ok(Err(error)) => {
                    controller.dispatch(AppEvent::SelectionCaptureFailed {
                        task_id,
                        error: error.to_string(),
                    });
                }
                Err(error) => controller.dispatch(AppEvent::SelectionCaptureFailed {
                    task_id,
                    error: format!("Selection worker failed: {error}"),
                }),
            }
        });
    }

    fn capture_toolbar_selection(self: &Arc<Self>, generation: u64, anchor: Point) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            // The target control receives mouse-up after our low-level hook returns.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let result =
                tokio::task::spawn_blocking(move || selection.selected_text_passive()).await;
            match result {
                Ok(Ok(Some(mut selection))) => {
                    selection.anchor = Some(anchor);
                    controller.dispatch(AppEvent::SelectionToolbarCaptured {
                        generation,
                        selection,
                    });
                }
                _ => controller.dispatch(AppEvent::SelectionToolbarCaptureEmpty { generation }),
            }
        });
    }

    fn translate(
        self: &Arc<Self>,
        task_id: TranslationTaskId,
        request: lexift_core::domain::translation::TranslateRequest,
    ) {
        tracing::debug!(?task_id, "translation request started");
        self.dispatch(AppEvent::TranslationStarted { task_id });
        let controller = Arc::clone(self);
        let translator = Arc::clone(&self.translator);
        self.runtime.spawn(async move {
            match lexift_core::usecases::translate_input::execute(translator.as_ref(), request)
                .await
            {
                Ok(result) => {
                    tracing::debug!(?task_id, "translation request finished");
                    controller.dispatch(AppEvent::TranslationFinished { task_id, result });
                }
                Err(error) => {
                    controller.dispatch(AppEvent::TranslationFailed {
                        task_id,
                        error: error.to_string(),
                    });
                }
            }
        });
    }

    fn translate_popup(
        self: &Arc<Self>,
        session_id: PopupSessionId,
        task_id: TranslationTaskId,
        request: lexift_core::domain::translation::TranslateRequest,
    ) {
        self.dispatch(AppEvent::PopupTranslationStarted {
            session_id,
            task_id,
        });
        let controller = Arc::clone(self);
        let translator = Arc::clone(&self.translator);
        self.runtime.spawn(async move {
            match lexift_core::usecases::translate_input::execute(translator.as_ref(), request)
                .await
            {
                Ok(result) => controller.dispatch(AppEvent::PopupTranslationFinished {
                    session_id,
                    task_id,
                    result,
                }),
                Err(error) => controller.dispatch(AppEvent::PopupTranslationFailed {
                    session_id,
                    task_id,
                    error: error.to_string(),
                }),
            }
        });
    }

    fn copy_popup_text(self: &Arc<Self>, session_id: PopupSessionId, text: String) {
        let Some(clipboard) = self.clipboard.clone() else {
            self.dispatch(AppEvent::PopupCopyFinished {
                session_id,
                error: Some("System clipboard is unavailable".into()),
            });
            return;
        };
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || clipboard.write_text(&text)).await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("Could not copy text: {error}")),
                Err(error) => Some(format!("Clipboard worker failed: {error}")),
            };
            controller.dispatch(AppEvent::PopupCopyFinished { session_id, error });
        });
    }
}

fn event_name(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::Started => "started",
        AppEvent::SelectionTranslationRequested => "selection_translation_requested",
        AppEvent::SelectionInteractionStarted => "selection_interaction_started",
        AppEvent::SelectionGestureCompleted { .. } => "selection_gesture_completed",
        AppEvent::SelectionToolbarCaptured { .. } => "selection_toolbar_captured",
        AppEvent::SelectionToolbarCaptureEmpty { .. } => "selection_toolbar_capture_empty",
        AppEvent::SelectionToolbarTranslateRequested => "selection_toolbar_translate_requested",
        AppEvent::SelectionToolbarCopyRequested => "selection_toolbar_copy_requested",
        AppEvent::SelectionToolbarDismissRequested => "selection_toolbar_dismiss_requested",
        AppEvent::InputTranslationRequested { .. } => "input_translation_requested",
        AppEvent::SelectionCaptured { .. } => "selection_captured",
        AppEvent::SelectionCaptureEmpty { .. } => "selection_capture_empty",
        AppEvent::SelectionCaptureFailed { .. } => "selection_capture_failed",
        AppEvent::TranslationStarted { .. } => "translation_started",
        AppEvent::TranslationFinished { .. } => "translation_finished",
        AppEvent::TranslationFailed { .. } => "translation_failed",
        AppEvent::PopupTranslationRequested { .. } => "popup_translation_requested",
        AppEvent::PopupTranslationStarted { .. } => "popup_translation_started",
        AppEvent::PopupTranslationFinished { .. } => "popup_translation_finished",
        AppEvent::PopupTranslationFailed { .. } => "popup_translation_failed",
        AppEvent::PopupPinChanged { .. } => "popup_pin_changed",
        AppEvent::PopupClosed { .. } => "popup_closed",
        AppEvent::PopupCopyRequested { .. } => "popup_copy_requested",
        AppEvent::PopupCopyFinished { .. } => "popup_copy_finished",
        AppEvent::PopupFeedbackCleared { .. } => "popup_feedback_cleared",
        AppEvent::PopupSpeechRequested { .. } => "popup_speech_requested",
        AppEvent::PopupSpeechStateChanged { .. } => "popup_speech_state_changed",
        AppEvent::MainWindowRequested => "main_window_requested",
        AppEvent::SettingsWindowRequested => "settings_window_requested",
        AppEvent::SettingsChangeRequested { .. } => "settings_change_requested",
        AppEvent::RuntimeConfigChanged { .. } => "runtime_config_changed",
        AppEvent::RuntimeConfigUpdated { .. } => "runtime_config_updated",
        AppEvent::RuntimeConfigUpdateFailed { .. } => "runtime_config_update_failed",
        AppEvent::SettingsSaveFailed { .. } => "settings_save_failed",
        AppEvent::CredentialSaveRequested { .. } => "credential_save_requested",
        AppEvent::CredentialSaved { .. } => "credential_saved",
        AppEvent::CredentialSaveFailed { .. } => "credential_save_failed",
        AppEvent::CredentialRemoveRequested => "credential_remove_requested",
        AppEvent::CredentialRemoved => "credential_removed",
        AppEvent::CredentialRemoveFailed { .. } => "credential_remove_failed",
        AppEvent::CredentialAccessRequested { purpose, .. } => match purpose {
            CredentialAccessPurpose::Reveal => "credential_reveal_requested",
            CredentialAccessPurpose::Edit => "credential_edit_requested",
            CredentialAccessPurpose::Copy => "credential_copy_requested",
        },
        AppEvent::CredentialAccessSucceeded { purpose } => match purpose {
            CredentialAccessPurpose::Reveal => "credential_reveal_succeeded",
            CredentialAccessPurpose::Edit => "credential_edit_succeeded",
            CredentialAccessPurpose::Copy => "credential_copy_succeeded",
        },
        AppEvent::CredentialAccessFailed { .. } => "credential_access_failed",
        AppEvent::ExitRequested => "exit_requested",
    }
}

fn credential_error_message(error: CredentialError) -> String {
    match error.kind() {
        CredentialErrorKind::Missing => "Credential does not exist",
        CredentialErrorKind::PermissionDenied => "Credential access was denied",
        CredentialErrorKind::PlatformFailure => "Secure credential storage is unavailable",
        CredentialErrorKind::InvalidFormat => "Credential value is invalid",
    }
    .into()
}

fn restore_credential(store: &dyn CredentialStore, id: &str, previous: Option<&str>) {
    let result = match previous {
        Some(secret) => store.set(id, secret),
        None => match store.delete(id) {
            Err(error) if error.kind() == CredentialErrorKind::Missing => Ok(()),
            result => result,
        },
    };
    if let Err(error) = result {
        tracing::warn!(kind = ?error.kind(), "credential rollback failed");
    }
}

fn tray_event(action: TrayAction) -> AppEvent {
    match action {
        TrayAction::OpenMainWindow => AppEvent::MainWindowRequested,
        TrayAction::OpenSettings => AppEvent::SettingsWindowRequested,
        TrayAction::Quit => AppEvent::ExitRequested,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
        time::{Duration, Instant},
    };

    use lexift_core::{
        TranslationPhase,
        domain::{
            runtime_config::{HotkeyConfig, RuntimeConfig},
            translation::{TranslateRequest, TranslateResult},
        },
        ports::{
            hotkey::{HotkeyHandler, HotkeyPort},
            translator::TranslationFuture,
        },
    };
    use tokio::runtime::Builder;

    use super::*;

    #[derive(Default)]
    struct RecordingView {
        states: Mutex<Vec<AppState>>,
        popup_shown: AtomicBool,
        main_window_shown: AtomicBool,
        popup_context: Mutex<Option<(Option<Point>, Option<Rect>)>>,
        settings_window_shown: AtomicBool,
        settings_window_hidden: AtomicBool,
        shown_settings: Mutex<Option<Settings>>,
        credential_draft_cleared: AtomicBool,
        presented_secret: Mutex<Option<(CredentialAccessPurpose, u64, String)>>,
        settings_feedback: Mutex<Vec<SettingsFeedback>>,
    }

    impl ViewPort for RecordingView {
        fn update(&self, state: AppState) {
            self.states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(state);
        }

        fn show_popup(
            &self,
            _session_id: PopupSessionId,
            anchor: Option<Point>,
            work_area: Option<Rect>,
        ) {
            *self
                .popup_context
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((anchor, work_area));
            self.popup_shown.store(true, Ordering::SeqCst);
        }

        fn hide_popup(&self, _session_id: PopupSessionId) {}

        fn show_main_window(&self) {
            self.main_window_shown.store(true, Ordering::SeqCst);
        }

        fn show_settings_window(&self, settings: Settings) {
            *self
                .shown_settings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(settings);
            self.settings_window_shown.store(true, Ordering::SeqCst);
        }

        fn hide_settings_window(&self) {
            self.settings_window_hidden.store(true, Ordering::SeqCst);
        }

        fn clear_credential_draft(&self) {
            self.credential_draft_cleared.store(true, Ordering::SeqCst);
        }

        fn present_credential_secret(
            &self,
            purpose: CredentialAccessPurpose,
            generation: u64,
            secret: CredentialSecret,
        ) {
            *self.presented_secret.lock().unwrap() =
                Some((purpose, generation, secret.into_inner()));
        }

        fn show_settings_feedback(&self, feedback: SettingsFeedback) {
            self.settings_feedback.lock().unwrap().push(feedback);
        }

        fn quit(&self) {}
    }

    struct ReorderingTranslator;

    #[derive(Default)]
    struct RecordingSettingsStore {
        saved: Mutex<Vec<Settings>>,
        failure: Option<String>,
        save_thread: Mutex<Option<thread::ThreadId>>,
    }

    impl SettingsStore for RecordingSettingsStore {
        fn load(&self) -> lexift_core::Result<Settings> {
            Ok(Settings::default())
        }

        fn save(&self, settings: &Settings) -> lexift_core::Result<()> {
            *self
                .save_thread
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(thread::current().id());
            if let Some(error) = &self.failure {
                return Err(lexift_core::Error::new(error.clone()));
            }
            self.saved
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(settings.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RejectingHotkeyPort {
        active: Mutex<Option<HotkeyConfig>>,
        rejected: Mutex<Option<HotkeyConfig>>,
    }

    impl HotkeyPort for RejectingHotkeyPort {
        fn register_translate_hotkey(
            &self,
            config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> lexift_core::Result<()> {
            *self.active.lock().unwrap() = Some(config);
            Ok(())
        }

        fn replace_translate_hotkey(
            &self,
            config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> lexift_core::Result<()> {
            if *self.rejected.lock().unwrap() == Some(config) {
                return Err(lexift_core::Error::new("hotkey conflict"));
            }
            *self.active.lock().unwrap() = Some(config);
            Ok(())
        }

        fn unregister_translate_hotkey(&self) -> lexift_core::Result<()> {
            *self.active.lock().unwrap() = None;
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingCredentialStore {
        values: Mutex<HashMap<String, String>>,
        fail_get: AtomicBool,
        fail_set: AtomicBool,
        fail_delete: AtomicBool,
    }

    impl CredentialStore for RecordingCredentialStore {
        fn get(
            &self,
            id: &str,
        ) -> lexift_core::ports::credential::CredentialResult<Option<String>> {
            if self.fail_get.load(Ordering::SeqCst) {
                return Err(CredentialError::new(
                    CredentialErrorKind::PermissionDenied,
                    "fixture get failure",
                ));
            }
            Ok(self
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(id)
                .cloned())
        }

        fn set(
            &self,
            id: &str,
            secret: &str,
        ) -> lexift_core::ports::credential::CredentialResult<()> {
            if self.fail_set.load(Ordering::SeqCst) {
                return Err(CredentialError::new(
                    CredentialErrorKind::PlatformFailure,
                    "fixture set failure",
                ));
            }
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id.into(), secret.into());
            Ok(())
        }

        fn delete(&self, id: &str) -> lexift_core::ports::credential::CredentialResult<()> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err(CredentialError::new(
                    CredentialErrorKind::PermissionDenied,
                    "fixture delete failure",
                ));
            }
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(id);
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingClipboard {
        text: Mutex<Option<String>>,
        fail_write: AtomicBool,
    }

    impl ClipboardPort for RecordingClipboard {
        fn read_text(&self) -> lexift_core::Result<Option<String>> {
            Ok(self.text.lock().unwrap().clone())
        }

        fn write_text(&self, text: &str) -> lexift_core::Result<()> {
            if self.fail_write.load(Ordering::SeqCst) {
                return Err(lexift_core::Error::new("fixture clipboard failure"));
            }
            *self.text.lock().unwrap() = Some(text.into());
            Ok(())
        }
    }

    struct CountingSelection {
        calls: AtomicUsize,
    }

    struct EmptySelection;

    struct MockScreen {
        cursor: Point,
        work_area: Rect,
    }

    impl ScreenPort for MockScreen {
        fn cursor_position(&self) -> lexift_core::Result<Point> {
            Ok(self.cursor)
        }

        fn work_area_for_point(&self, _point: Point) -> lexift_core::Result<Rect> {
            Ok(self.work_area)
        }
    }

    struct SequentialSelection(AtomicUsize);

    impl SelectionPort for SequentialSelection {
        fn selected_text(
            &self,
        ) -> lexift_core::Result<Option<lexift_core::domain::selection::Selection>> {
            let text = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                "old request"
            } else {
                "new request"
            };
            Ok(Some(lexift_core::domain::selection::Selection {
                text: text.into(),
                anchor: None,
            }))
        }
    }

    #[test]
    fn second_hotkey_during_translation_captures_and_displays_new_selection() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        let selection = Arc::new(SequentialSelection(AtomicUsize::new(0)));
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            state.clone(),
            Some(selection.clone()),
            None,
            Arc::new(ReorderingTranslator),
            Arc::new(RecordingSettingsStore::default()),
            Arc::new(RecordingView::default()),
        ));
        let hotkey = controller.translate_hotkey_handler();
        hotkey();
        let deadline = Instant::now() + Duration::from_secs(2);
        while state.lock().unwrap().phase != TranslationPhase::Translating {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        hotkey();
        while state.lock().unwrap().translated_text != "new request" {
            assert!(Instant::now() < deadline, "second selection did not finish");
            thread::sleep(Duration::from_millis(1));
        }
        // Allow the intentionally slower first request to finish too.
        thread::sleep(Duration::from_millis(150));
        let final_state = state.lock().unwrap();
        assert_eq!(selection.0.load(Ordering::SeqCst), 2);
        assert_eq!(final_state.source_text, "new request");
        assert_eq!(final_state.translated_text, "new request");
    }

    impl SelectionPort for CountingSelection {
        fn selected_text(
            &self,
        ) -> lexift_core::Result<Option<lexift_core::domain::selection::Selection>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(80));
            Ok(Some(lexift_core::domain::selection::Selection {
                text: "Hello world".into(),
                anchor: None,
            }))
        }
    }

    impl SelectionPort for EmptySelection {
        fn selected_text(
            &self,
        ) -> lexift_core::Result<Option<lexift_core::domain::selection::Selection>> {
            Ok(None)
        }
    }

    impl TranslatorPort for ReorderingTranslator {
        fn translate(&self, request: TranslateRequest) -> TranslationFuture<'_> {
            Box::pin(async move {
                let delay = if request.text == "old request" {
                    100
                } else {
                    10
                };
                tokio::time::sleep(Duration::from_millis(delay)).await;
                Ok(TranslateResult {
                    text: request.text,
                    detected_source_language: None,
                })
            })
        }
    }

    #[test]
    fn popup_receives_selection_anchor_and_its_monitor_work_area() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let anchor = Point { x: -640, y: 320 };
        let work_area = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let view = Arc::new(RecordingView::default());
        let controller = AppController::new(
            runtime.handle().clone(),
            Arc::new(Mutex::new(AppState::default())),
            None,
            Some(Arc::new(MockScreen {
                cursor: Point { x: 10, y: 20 },
                work_area,
            })),
            Arc::new(ReorderingTranslator),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        );

        controller.show_popup(PopupSessionId::new(1), Some(anchor));

        assert_eq!(
            *view
                .popup_context
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some((Some(anchor), Some(work_area)))
        );
    }

    #[test]
    fn popup_uses_current_cursor_when_selection_has_no_anchor() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let cursor = Point { x: 800, y: 450 };
        let work_area = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let view = Arc::new(RecordingView::default());
        let controller = AppController::new(
            runtime.handle().clone(),
            Arc::new(Mutex::new(AppState::default())),
            None,
            Some(Arc::new(MockScreen { cursor, work_area })),
            Arc::new(ReorderingTranslator),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        );

        controller.show_popup(PopupSessionId::new(1), None);

        assert_eq!(
            *view
                .popup_context
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some((Some(cursor), Some(work_area)))
        );
    }

    #[test]
    fn constructs_without_selection_capability() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let providers = lexift_translate::ProviderRegistry::with_mock();

        let _controller = AppController::new(
            runtime.handle().clone(),
            Arc::new(Mutex::new(AppState::default())),
            None,
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            Arc::new(RecordingView::default()),
        );
    }

    #[test]
    fn missing_selection_only_fails_the_requested_operation() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let providers = lexift_translate::ProviderRegistry::with_mock();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            None,
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        ));

        controller.dispatch(AppEvent::SelectionTranslationRequested);

        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.error_message, "Selection capability is not available");
        assert!(view.popup_shown.load(Ordering::SeqCst));
    }

    #[test]
    fn translate_hotkey_dispatches_the_selection_flow() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let providers = lexift_translate::ProviderRegistry::with_mock();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            None,
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        ));

        controller.translate_hotkey_handler()();

        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.phase, TranslationPhase::Error);
        assert_eq!(state.error_message, "Selection capability is not available");
        assert!(view.popup_shown.load(Ordering::SeqCst));
    }

    #[test]
    fn repeated_hotkey_while_capturing_does_not_start_another_capture() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let providers = lexift_translate::ProviderRegistry::with_mock();
        let state = Arc::new(Mutex::new(AppState::default()));
        let selection = Arc::new(CountingSelection {
            calls: AtomicUsize::new(0),
        });
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            Some(selection.clone()),
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            Arc::new(RecordingView::default()),
        ));
        let hotkey = controller.translate_hotkey_handler();

        hotkey();
        hotkey();
        thread::sleep(Duration::from_millis(200));

        assert_eq!(selection.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn empty_selection_opens_a_blank_popup() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let providers = lexift_translate::ProviderRegistry::with_mock();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            Some(Arc::new(EmptySelection)),
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        ));

        controller.translate_hotkey_handler()();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if view.popup_shown.load(Ordering::SeqCst)
                && state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .phase
                    == TranslationPhase::NoSelection
            {
                break;
            }
            assert!(Instant::now() < deadline, "selection capture timed out");
            thread::sleep(Duration::from_millis(5));
        }

        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.popup_sessions.len(), 1);
        assert!(state.popup_sessions[0].source_text.is_empty());
        assert!(state.popup_sessions[0].translated_text.is_empty());
        assert!(state.popup_sessions[0].error_message.is_empty());
        assert_eq!(state.current_translation_task, None);
    }

    #[test]
    fn runs_mock_translation_without_blocking_the_caller() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let platform = lexift_platform::PlatformCapabilities::mock();
        let providers = lexift_translate::ProviderRegistry::with_mock();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            platform.selection(),
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        ));

        controller.dispatch(AppEvent::SelectionTranslationRequested);
        assert_ne!(
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .phase,
            TranslationPhase::Success,
            "dispatch must return before the delayed translation finishes"
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let phase = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .phase;
            if phase == TranslationPhase::Success {
                break;
            }
            assert!(Instant::now() < deadline, "mock translation timed out");
            thread::sleep(Duration::from_millis(10));
        }

        let states = view
            .states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let phases: Vec<_> = states.iter().map(|state| state.phase).collect();
        assert!(phases.contains(&TranslationPhase::Capturing));
        assert!(phases.contains(&TranslationPhase::Translating));
        assert!(phases.contains(&TranslationPhase::Success));
        assert_eq!(
            states.last().expect("a final state").source_text,
            "Hello world"
        );
        assert_eq!(
            states.last().expect("a final state").translated_text,
            "你好，世界"
        );
        assert!(view.popup_shown.load(Ordering::SeqCst));
    }

    #[test]
    fn input_translation_succeeds_without_selection_capability() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let providers = lexift_translate::ProviderRegistry::with_mock();
        let state = Arc::new(Mutex::new(AppState::default()));
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            None,
            None,
            providers.default_translator(),
            Arc::new(RecordingSettingsStore::default()),
            Arc::new(RecordingView::default()),
        ));

        controller.dispatch(AppEvent::InputTranslationRequested {
            text: "Hello world".into(),
        });

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let phase = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .phase;
            if phase == TranslationPhase::Success {
                break;
            }
            assert!(Instant::now() < deadline, "input translation timed out");
            thread::sleep(Duration::from_millis(10));
        }

        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.source_text, "Hello world");
        assert_eq!(state.translated_text, "你好，世界");
    }

    #[test]
    fn late_result_does_not_overwrite_the_newest_translation() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("test runtime should start");
        let mut initial_state = AppState::default();
        initial_state.reduce(AppEvent::SelectionTranslationRequested);
        initial_state.reduce(AppEvent::SelectionTranslationRequested);
        let state = Arc::new(Mutex::new(initial_state));
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            None,
            None,
            Arc::new(ReorderingTranslator),
            Arc::new(RecordingSettingsStore::default()),
            Arc::new(RecordingView::default()),
        ));

        controller.translate(
            TranslationTaskId::new(1),
            TranslateRequest {
                text: "old request".into(),
                source_language: None,
                target_language: lexift_core::domain::language::Language("zh-CN".into()),
            },
        );
        controller.translate(
            TranslationTaskId::new(2),
            TranslateRequest {
                text: "new request".into(),
                source_language: None,
                target_language: lexift_core::domain::language::Language("zh-CN".into()),
            },
        );

        thread::sleep(Duration::from_millis(200));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(state.phase, TranslationPhase::Success);
        assert_eq!(state.translated_text, "new request");
    }

    #[test]
    fn tray_actions_map_to_core_events() {
        assert_eq!(
            tray_event(TrayAction::OpenMainWindow),
            AppEvent::MainWindowRequested
        );
        assert_eq!(
            tray_event(TrayAction::OpenSettings),
            AppEvent::SettingsWindowRequested
        );
        assert_eq!(tray_event(TrayAction::Quit), AppEvent::ExitRequested);
    }

    #[test]
    fn main_window_command_reaches_the_view() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let view = Arc::new(RecordingView::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::new(Mutex::new(AppState::default())),
            None,
            None,
            Arc::new(ReorderingTranslator),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        ));

        controller.dispatch(AppEvent::MainWindowRequested);

        assert!(view.main_window_shown.load(Ordering::SeqCst));
    }

    #[test]
    fn settings_window_receives_committed_settings() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let committed = Settings {
            target_language: lexift_core::domain::language::Language("fr".into()),
            ..Settings::default()
        };
        let view = Arc::new(RecordingView::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::new(Mutex::new(AppState::new(committed.clone()))),
            None,
            None,
            Arc::new(ReorderingTranslator),
            Arc::new(RecordingSettingsStore::default()),
            view.clone(),
        ));

        controller.dispatch(AppEvent::SettingsWindowRequested);

        assert_eq!(
            *view
                .shown_settings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            Some(committed)
        );
    }

    #[test]
    fn successful_settings_save_commits_from_a_blocking_worker() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let store = Arc::new(RecordingSettingsStore::default());
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            None,
            None,
            Arc::new(ReorderingTranslator),
            store.clone(),
            view.clone(),
        ));
        let requested = Settings {
            target_language: lexift_core::domain::language::Language("ja".into()),
            ..Settings::default()
        };
        let caller_thread = thread::current().id();

        controller.dispatch(AppEvent::SettingsChangeRequested {
            change: SettingsChange::TargetLanguage(requested.target_language.clone()),
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while state.lock().unwrap().settings != requested {
            assert!(Instant::now() < deadline, "settings save timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*store.saved.lock().unwrap(), vec![requested]);
        assert_ne!(*store.save_thread.lock().unwrap(), Some(caller_thread));
        assert!(!view.settings_window_hidden.load(Ordering::SeqCst));
    }

    #[test]
    fn failed_settings_save_keeps_committed_state_and_window_open() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let committed = Settings {
            target_language: lexift_core::domain::language::Language("de".into()),
            ..Settings::default()
        };
        let state = Arc::new(Mutex::new(AppState::new(committed.clone())));
        let view = Arc::new(RecordingView::default());
        let store = Arc::new(RecordingSettingsStore {
            failure: Some("Could not save settings".into()),
            ..Default::default()
        });
        let controller = Arc::new(AppController::new(
            runtime.handle().clone(),
            Arc::clone(&state),
            None,
            None,
            Arc::new(ReorderingTranslator),
            store,
            view.clone(),
        ));

        controller.dispatch(AppEvent::SettingsChangeRequested {
            change: SettingsChange::TargetLanguage(lexift_core::domain::language::Language(
                "fr".into(),
            )),
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let state = state.lock().unwrap();
            if !state.settings_saving {
                assert_eq!(state.settings, committed);
                assert_eq!(state.settings_error_message, "Could not save settings");
                break;
            }
            drop(state);
            assert!(Instant::now() < deadline, "settings failure timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!view.settings_window_hidden.load(Ordering::SeqCst));
    }

    #[test]
    fn runtime_apply_failure_rolls_back_persistence_and_keeps_the_old_hotkey() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let store = Arc::new(RecordingSettingsStore::default());
        let hotkey = Arc::new(RejectingHotkeyPort::default());
        let rejected: HotkeyConfig = "Ctrl + Shift + 7".parse().unwrap();
        *hotkey.rejected.lock().unwrap() = Some(rejected);
        let manager = Arc::new(crate::runtime::RuntimeManager::testing(
            RuntimeConfig::default(),
            Some(hotkey.clone()),
            Arc::new(ReorderingTranslator),
        ));
        manager.start_hotkey(Arc::new(|| {})).unwrap();
        let controller = Arc::new(
            AppController::new(
                runtime.handle().clone(),
                Arc::clone(&state),
                None,
                None,
                Arc::new(ReorderingTranslator),
                store.clone(),
                view.clone(),
            )
            .with_runtime_manager(manager),
        );
        let requested = Settings {
            hotkey: rejected,
            ..Settings::default()
        };

        controller.dispatch(AppEvent::SettingsChangeRequested {
            change: SettingsChange::Hotkey(rejected),
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = state.lock().unwrap().clone();
            if !snapshot.settings_saving {
                assert_eq!(snapshot.settings, Settings::default());
                assert_eq!(snapshot.runtime_config, RuntimeConfig::default());
                assert_eq!(snapshot.runtime_config_error_message, "hotkey conflict");
                break;
            }
            assert!(Instant::now() < deadline, "runtime rollback timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            *store.saved.lock().unwrap(),
            vec![requested, Settings::default()]
        );
        assert_eq!(
            *hotkey.active.lock().unwrap(),
            Some(HotkeyConfig::default())
        );
        assert!(!view.settings_window_hidden.load(Ordering::SeqCst));
    }

    #[test]
    fn credential_save_persists_secret_reference_and_updates_runtime() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        let view = Arc::new(RecordingView::default());
        let settings_store = Arc::new(RecordingSettingsStore::default());
        let credential_store = Arc::new(RecordingCredentialStore::default());
        let reference = Arc::new(RwLock::new(None));
        let controller = Arc::new(
            AppController::new(
                runtime.handle().clone(),
                Arc::clone(&state),
                None,
                None,
                Arc::new(ReorderingTranslator),
                settings_store.clone(),
                view.clone(),
            )
            .with_credential_management(
                credential_store.clone(),
                Arc::clone(&reference),
                None,
            ),
        );

        controller.dispatch(AppEvent::CredentialSaveRequested {
            secret: CredentialSecret::new("private-key"),
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while !state.lock().unwrap().credential_configured {
            assert!(Instant::now() < deadline, "credential save timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            credential_store.get("deepl-primary").unwrap().as_deref(),
            Some("private-key")
        );
        assert_eq!(reference.read().unwrap().as_deref(), Some("deepl-primary"));
        let saved = settings_store.saved.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            saved[0].deepl_credential_id.as_deref(),
            Some("deepl-primary")
        );
        assert!(view.credential_draft_cleared.load(Ordering::SeqCst));
    }

    #[test]
    fn credential_delete_failure_preserves_state_and_reference() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let settings = Settings {
            deepl_credential_id: Some("deepl-primary".into()),
            ..Settings::default()
        };
        let state = Arc::new(Mutex::new(AppState::with_credential_status(settings, true)));
        let credential_store = Arc::new(RecordingCredentialStore::default());
        credential_store
            .set("deepl-primary", "private-key")
            .unwrap();
        credential_store.fail_delete.store(true, Ordering::SeqCst);
        let reference = Arc::new(RwLock::new(Some("deepl-primary".into())));
        let controller = Arc::new(
            AppController::new(
                runtime.handle().clone(),
                Arc::clone(&state),
                None,
                None,
                Arc::new(ReorderingTranslator),
                Arc::new(RecordingSettingsStore::default()),
                Arc::new(RecordingView::default()),
            )
            .with_credential_management(
                credential_store.clone(),
                Arc::clone(&reference),
                None,
            ),
        );

        controller.dispatch(AppEvent::CredentialRemoveRequested);

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = state.lock().unwrap();
            if !snapshot.credential_busy {
                assert!(snapshot.credential_configured);
                assert_eq!(
                    snapshot.settings.deepl_credential_id.as_deref(),
                    Some("deepl-primary")
                );
                assert_eq!(
                    snapshot.credential_error_message,
                    "Credential access was denied"
                );
                break;
            }
            drop(snapshot);
            assert!(
                Instant::now() < deadline,
                "credential delete failure timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(reference.read().unwrap().as_deref(), Some("deepl-primary"));
        assert_eq!(
            credential_store.get("deepl-primary").unwrap().as_deref(),
            Some("private-key")
        );
    }

    #[test]
    fn credential_save_failure_does_not_commit_reference_or_status() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        let credential_store = Arc::new(RecordingCredentialStore::default());
        credential_store.fail_set.store(true, Ordering::SeqCst);
        let reference = Arc::new(RwLock::new(None));
        let controller = Arc::new(
            AppController::new(
                runtime.handle().clone(),
                Arc::clone(&state),
                None,
                None,
                Arc::new(ReorderingTranslator),
                Arc::new(RecordingSettingsStore::default()),
                Arc::new(RecordingView::default()),
            )
            .with_credential_management(
                credential_store.clone(),
                Arc::clone(&reference),
                None,
            ),
        );

        controller.dispatch(AppEvent::CredentialSaveRequested {
            secret: CredentialSecret::new("private-key"),
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = state.lock().unwrap();
            if !snapshot.credential_busy {
                assert!(!snapshot.credential_configured);
                assert_eq!(snapshot.settings.deepl_credential_id, None);
                assert_eq!(
                    snapshot.credential_error_message,
                    "Secure credential storage is unavailable"
                );
                break;
            }
            drop(snapshot);
            assert!(
                Instant::now() < deadline,
                "credential save failure timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*reference.read().unwrap(), None);
        assert_eq!(credential_store.get("deepl-primary").unwrap(), None);
    }

    #[test]
    fn credential_reveal_and_edit_deliver_one_time_secret_to_the_view() {
        for purpose in [
            CredentialAccessPurpose::Reveal,
            CredentialAccessPurpose::Edit,
        ] {
            let runtime = Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let settings = Settings {
                deepl_credential_id: Some("deepl-primary".into()),
                ..Settings::default()
            };
            let state = Arc::new(Mutex::new(AppState::with_credential_status(settings, true)));
            let view = Arc::new(RecordingView::default());
            let store = Arc::new(RecordingCredentialStore::default());
            store.set("deepl-primary", "private-key").unwrap();
            let controller = Arc::new(
                AppController::new(
                    runtime.handle().clone(),
                    Arc::clone(&state),
                    None,
                    None,
                    Arc::new(ReorderingTranslator),
                    Arc::new(RecordingSettingsStore::default()),
                    view.clone(),
                )
                .with_credential_management(
                    store,
                    Arc::new(RwLock::new(Some("deepl-primary".into()))),
                    None,
                ),
            );

            controller.dispatch(AppEvent::CredentialAccessRequested {
                purpose,
                generation: 17,
            });

            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(presented) = view.presented_secret.lock().unwrap().clone() {
                    assert_eq!(presented, (purpose, 17, "private-key".into()));
                    break;
                }
                assert!(Instant::now() < deadline, "credential access timed out");
                thread::sleep(Duration::from_millis(5));
            }
            assert!(!state.lock().unwrap().credential_busy);
            assert!(!format!("{:?}", state.lock().unwrap().clone()).contains("private-key"));
        }
    }

    #[test]
    fn credential_copy_bypasses_the_view_and_writes_the_clipboard() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let settings = Settings {
            deepl_credential_id: Some("deepl-primary".into()),
            ..Settings::default()
        };
        let state = Arc::new(Mutex::new(AppState::with_credential_status(settings, true)));
        let view = Arc::new(RecordingView::default());
        let store = Arc::new(RecordingCredentialStore::default());
        store.set("deepl-primary", "private-key").unwrap();
        let clipboard = Arc::new(RecordingClipboard::default());
        let controller = Arc::new(
            AppController::new(
                runtime.handle().clone(),
                Arc::clone(&state),
                None,
                None,
                Arc::new(ReorderingTranslator),
                Arc::new(RecordingSettingsStore::default()),
                view.clone(),
            )
            .with_credential_management(
                store,
                Arc::new(RwLock::new(Some("deepl-primary".into()))),
                Some(clipboard.clone()),
            ),
        );

        controller.dispatch(AppEvent::CredentialAccessRequested {
            purpose: CredentialAccessPurpose::Copy,
            generation: 23,
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if view
                .settings_feedback
                .lock()
                .unwrap()
                .contains(&SettingsFeedback::CredentialCopied)
            {
                break;
            }
            assert!(Instant::now() < deadline, "credential copy timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            clipboard.text.lock().unwrap().as_deref(),
            Some("private-key")
        );
        assert!(view.presented_secret.lock().unwrap().is_none());
        assert!(!state.lock().unwrap().credential_busy);
    }

    #[test]
    fn credential_copy_failure_is_recoverable_and_does_not_report_success() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let settings = Settings {
            deepl_credential_id: Some("deepl-primary".into()),
            ..Settings::default()
        };
        let state = Arc::new(Mutex::new(AppState::with_credential_status(settings, true)));
        let view = Arc::new(RecordingView::default());
        let store = Arc::new(RecordingCredentialStore::default());
        store.set("deepl-primary", "private-key").unwrap();
        let clipboard = Arc::new(RecordingClipboard::default());
        clipboard.fail_write.store(true, Ordering::SeqCst);
        let controller = Arc::new(
            AppController::new(
                runtime.handle().clone(),
                Arc::clone(&state),
                None,
                None,
                Arc::new(ReorderingTranslator),
                Arc::new(RecordingSettingsStore::default()),
                view.clone(),
            )
            .with_credential_management(
                store,
                Arc::new(RwLock::new(Some("deepl-primary".into()))),
                Some(clipboard),
            ),
        );

        controller.dispatch(AppEvent::CredentialAccessRequested {
            purpose: CredentialAccessPurpose::Copy,
            generation: 24,
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let snapshot = state.lock().unwrap();
            if !snapshot.credential_busy {
                assert_eq!(
                    snapshot.credential_error_message,
                    "Could not copy DeepL API key: fixture clipboard failure"
                );
                break;
            }
            drop(snapshot);
            assert!(
                Instant::now() < deadline,
                "credential copy failure timed out"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            *view.settings_feedback.lock().unwrap(),
            vec![SettingsFeedback::CredentialOperationFailed]
        );
        assert!(view.presented_secret.lock().unwrap().is_none());
    }
}
