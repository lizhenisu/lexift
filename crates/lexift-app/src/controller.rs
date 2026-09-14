use std::sync::{Arc, Mutex};

use lexift_core::{
    AppCommand, AppEvent, AppState,
    ports::{selection::SelectionPort, translator::TranslatorPort},
};
use tokio::runtime::Handle;

pub(crate) trait ViewPort: Send + Sync {
    fn update(&self, state: AppState);
    fn show_popup(&self);
    fn hide_popup(&self);
    fn quit(&self);
}

impl ViewPort for lexift_ui::UiHandle {
    fn update(&self, state: AppState) {
        self.update(state);
    }

    fn show_popup(&self) {
        self.show_popup();
    }

    fn hide_popup(&self) {
        self.hide_popup();
    }

    fn quit(&self) {
        self.quit();
    }
}

/// Executes Core commands on background workers and returns events to Core.
pub(crate) struct AppController {
    runtime: Handle,
    state: Arc<Mutex<AppState>>,
    selection: Arc<dyn SelectionPort>,
    translator: Arc<dyn TranslatorPort>,
    ui: Arc<dyn ViewPort>,
}

impl AppController {
    pub(crate) fn new(
        runtime: Handle,
        state: Arc<Mutex<AppState>>,
        selection: Arc<dyn SelectionPort>,
        translator: Arc<dyn TranslatorPort>,
        ui: Arc<dyn ViewPort>,
    ) -> Self {
        Self {
            runtime,
            state,
            selection,
            translator,
            ui,
        }
    }

    pub(crate) fn dispatch(self: &Arc<Self>, event: AppEvent) {
        tracing::debug!(?event, "dispatching application event");
        let (snapshot, commands) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let commands = state.reduce(event);
            (state.clone(), commands)
        };
        self.ui.update(snapshot);

        for command in commands {
            self.execute(command);
        }
    }

    fn execute(self: &Arc<Self>, command: AppCommand) {
        match command {
            AppCommand::CaptureSelection => self.capture_selection(),
            AppCommand::Translate(request) => self.translate(request),
            AppCommand::ShowPopup => self.ui.show_popup(),
            AppCommand::HidePopup => self.ui.hide_popup(),
            AppCommand::Exit => self.ui.quit(),
        }
    }

    fn capture_selection(self: &Arc<Self>) {
        let controller = Arc::clone(self);
        let selection = Arc::clone(&self.selection);
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || selection.selected_text()).await;
            match result {
                Ok(Ok(Some(selection))) => {
                    controller.dispatch(AppEvent::SelectionCaptured(selection));
                }
                Ok(Ok(None)) => controller.dispatch(AppEvent::TranslationFailed(
                    "No selected text was found".into(),
                )),
                Ok(Err(error)) => {
                    controller.dispatch(AppEvent::TranslationFailed(error.to_string()));
                }
                Err(error) => controller.dispatch(AppEvent::TranslationFailed(format!(
                    "Selection worker failed: {error}"
                ))),
            }
        });
    }

    fn translate(self: &Arc<Self>, request: lexift_core::domain::translation::TranslateRequest) {
        self.dispatch(AppEvent::TranslationStarted);
        let controller = Arc::clone(self);
        let translator = Arc::clone(&self.translator);
        self.runtime.spawn(async move {
            match lexift_core::usecases::translate_input::execute(translator.as_ref(), request)
                .await
            {
                Ok(result) => controller.dispatch(AppEvent::TranslationFinished(result)),
                Err(error) => {
                    controller.dispatch(AppEvent::TranslationFailed(error.to_string()));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread,
        time::{Duration, Instant},
    };

    use lexift_core::TranslationPhase;
    use tokio::runtime::Builder;

    use super::*;

    #[derive(Default)]
    struct RecordingView {
        states: Mutex<Vec<AppState>>,
        popup_shown: AtomicBool,
    }

    impl ViewPort for RecordingView {
        fn update(&self, state: AppState) {
            self.states
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(state);
        }

        fn show_popup(&self) {
            self.popup_shown.store(true, Ordering::SeqCst);
        }

        fn hide_popup(&self) {}

        fn quit(&self) {}
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
            platform
                .selection()
                .expect("mock platform should provide selection"),
            providers
                .default_translator()
                .expect("mock registry should provide a translator"),
            view.clone(),
        ));

        controller.dispatch(AppEvent::TranslateRequested);
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
}
