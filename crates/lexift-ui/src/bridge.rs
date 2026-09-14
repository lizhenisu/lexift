use lexift_core::{AppEvent, AppState};
use slint::ComponentHandle;

use crate::{AppWindow, TranslationPopup, binding, mapper};

pub struct Ui {
    main: AppWindow,
    popup: TranslationPopup,
}

impl Ui {
    pub fn new(initial_state: &AppState) -> Result<Self, slint::PlatformError> {
        let main = AppWindow::new()?;
        let popup = TranslationPopup::new()?;
        binding::apply(&main, &popup, mapper::view_state(initial_state));
        Ok(Self { main, popup })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            main: self.main.as_weak(),
            popup: self.popup.as_weak(),
        }
    }

    pub fn on_event(&self, handler: impl Fn(AppEvent) + 'static) {
        self.main
            .on_translation_requested(move || handler(AppEvent::SelectionTranslationRequested));
    }

    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.main.run()
    }
}

#[derive(Clone)]
pub struct UiHandle {
    main: slint::Weak<AppWindow>,
    popup: slint::Weak<TranslationPopup>,
}

impl UiHandle {
    /// Queues state rendering on the Slint event-loop thread.
    pub fn update(&self, state: AppState) {
        let main = self.main.clone();
        let popup = self.popup.clone();
        let view_state = mapper::view_state(&state);
        let _ = slint::invoke_from_event_loop(move || {
            if let (Some(main), Some(popup)) = (main.upgrade(), popup.upgrade()) {
                binding::apply(&main, &popup, view_state);
            }
        });
    }

    pub fn show_popup(&self) {
        let popup = self.popup.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(popup) = popup.upgrade() {
                let _ = popup.show();
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

    pub fn quit(&self) {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    }
}
