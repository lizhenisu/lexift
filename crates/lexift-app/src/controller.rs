use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use lexift_core::{
    AppCommand, AppEvent, AppState, TranslationTaskId,
    domain::{
        geometry::{Point, Rect},
        settings::Settings,
    },
    ports::{
        hotkey::HotkeyHandler,
        screen::ScreenPort,
        selection::SelectionPort,
        settings::SettingsStore,
        translator::TranslatorPort,
        tray::{TrayAction, TrayHandler},
    },
};
use tokio::runtime::Handle;

pub(crate) trait ViewPort: Send + Sync {
    fn update(&self, state: AppState);
    fn show_popup(&self, anchor: Option<Point>, work_area: Option<Rect>);
    fn hide_popup(&self);
    fn show_main_window(&self);
    fn show_settings_window(&self, settings: Settings);
    fn hide_settings_window(&self);
    fn quit(&self);
}

impl ViewPort for lexift_ui::UiHandle {
    fn update(&self, state: AppState) {
        self.update(state);
    }

    fn show_popup(&self, anchor: Option<Point>, work_area: Option<Rect>) {
        self.show_popup(anchor, work_area);
    }

    fn hide_popup(&self) {
        self.hide_popup();
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
    ui: Arc<dyn ViewPort>,
    selection_capture_in_flight: AtomicBool,
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
        Self {
            runtime,
            state,
            selection,
            screen,
            translator,
            settings_store,
            ui,
            selection_capture_in_flight: AtomicBool::new(false),
        }
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
        self.ui.update(snapshot);

        for command in commands {
            self.execute(command);
        }
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
            AppCommand::Translate { task_id, request } => self.translate(task_id, request),
            AppCommand::ShowPopup { anchor } => self.show_popup(anchor),
            AppCommand::HidePopup => self.ui.hide_popup(),
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
            AppCommand::PersistSettings { settings } => self.persist_settings(settings),
            AppCommand::Exit => self.ui.quit(),
        }
    }

    fn persist_settings(self: &Arc<Self>, settings: Settings) {
        let store = Arc::clone(&self.settings_store);
        let controller = Arc::clone(self);
        self.runtime.spawn(async move {
            let settings_to_save = settings.clone();
            let result = tokio::task::spawn_blocking(move || store.save(&settings_to_save)).await;
            match result {
                Ok(Ok(())) => controller.dispatch(AppEvent::SettingsSaved { settings }),
                Ok(Err(error)) => controller.dispatch(AppEvent::SettingsSaveFailed {
                    error: error.to_string(),
                }),
                Err(error) => controller.dispatch(AppEvent::SettingsSaveFailed {
                    error: format!("Settings worker failed: {error}"),
                }),
            }
        });
    }

    fn show_popup(&self, selection_anchor: Option<Point>) {
        let Some(screen) = &self.screen else {
            self.ui.show_popup(None, None);
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

        self.ui.show_popup(anchor, work_area);
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
}

fn event_name(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::Started => "started",
        AppEvent::SelectionTranslationRequested => "selection_translation_requested",
        AppEvent::InputTranslationRequested { .. } => "input_translation_requested",
        AppEvent::SelectionCaptured { .. } => "selection_captured",
        AppEvent::SelectionCaptureEmpty { .. } => "selection_capture_empty",
        AppEvent::SelectionCaptureFailed { .. } => "selection_capture_failed",
        AppEvent::TranslationStarted { .. } => "translation_started",
        AppEvent::TranslationFinished { .. } => "translation_finished",
        AppEvent::TranslationFailed { .. } => "translation_failed",
        AppEvent::PopupHidden => "popup_hidden",
        AppEvent::MainWindowRequested => "main_window_requested",
        AppEvent::SettingsWindowRequested => "settings_window_requested",
        AppEvent::SettingsSaveRequested { .. } => "settings_save_requested",
        AppEvent::SettingsSaved { .. } => "settings_saved",
        AppEvent::SettingsSaveFailed { .. } => "settings_save_failed",
        AppEvent::ExitRequested => "exit_requested",
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
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
        time::{Duration, Instant},
    };

    use lexift_core::{
        TranslationPhase,
        domain::translation::{TranslateRequest, TranslateResult},
        ports::translator::TranslationFuture,
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
    }

    impl ViewPort for RecordingView {
        fn update(&self, state: AppState) {
            self.states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(state);
        }

        fn show_popup(&self, anchor: Option<Point>, work_area: Option<Rect>) {
            *self
                .popup_context
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((anchor, work_area));
            self.popup_shown.store(true, Ordering::SeqCst);
        }

        fn hide_popup(&self) {}

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
                Ok(TranslateResult { text: request.text })
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

        controller.show_popup(Some(anchor));

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

        controller.show_popup(None);

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
    fn empty_selection_does_not_open_an_error_popup() {
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
            if state
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

        assert!(!view.popup_shown.load(Ordering::SeqCst));
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
                target_language: lexift_core::domain::language::Language("zh-CN".into()),
            },
        );
        controller.translate(
            TranslationTaskId::new(2),
            TranslateRequest {
                text: "new request".into(),
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
        };
        let caller_thread = thread::current().id();

        controller.dispatch(AppEvent::SettingsSaveRequested {
            settings: requested.clone(),
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while state.lock().unwrap().settings != requested {
            assert!(Instant::now() < deadline, "settings save timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*store.saved.lock().unwrap(), vec![requested]);
        assert_ne!(*store.save_thread.lock().unwrap(), Some(caller_thread));
        assert!(view.settings_window_hidden.load(Ordering::SeqCst));
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

        controller.dispatch(AppEvent::SettingsSaveRequested {
            settings: Settings {
                target_language: lexift_core::domain::language::Language("fr".into()),
            },
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
}
