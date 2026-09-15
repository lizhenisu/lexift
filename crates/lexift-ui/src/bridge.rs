use std::{cell::Cell, rc::Rc};

use lexift_core::{
    AppEvent, AppState,
    domain::geometry::{Point, Rect},
};
use slint::ComponentHandle;

use crate::{AppWindow, TranslationPopup, binding, mapper, placement};

const POPUP_GAP_PX: i32 = 12;
const WORK_AREA_MARGIN_PX: i32 = 8;

pub struct Ui {
    main: AppWindow,
    popup: TranslationPopup,
    prepare_popup: fn(&slint::Window),
    background_mode: Rc<Cell<bool>>,
}

impl Ui {
    pub fn new(
        initial_state: &AppState,
        show_selection_demo: bool,
        prepare_popup: fn(&slint::Window),
    ) -> Result<Self, slint::PlatformError> {
        let main = AppWindow::new()?;
        let popup = TranslationPopup::new()?;
        main.set_show_selection_demo(show_selection_demo);
        binding::apply(&main, &popup, mapper::view_state(initial_state));
        Ok(Self {
            main,
            popup,
            prepare_popup,
            background_mode: Rc::new(Cell::new(false)),
        })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            main: self.main.as_weak(),
            popup: self.popup.as_weak(),
            prepare_popup: self.prepare_popup,
        }
    }

    pub fn on_event(&self, handler: impl Fn(AppEvent) + 'static) {
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
        let selection_handler = Rc::clone(&handler);
        self.main.on_selection_translation_requested(move || {
            selection_handler(AppEvent::SelectionTranslationRequested);
        });
        self.main.on_input_translation_requested(move |text| {
            handler(AppEvent::InputTranslationRequested {
                text: text.to_string(),
            });
        });
    }

    pub fn set_background_mode(&self, enabled: bool) {
        self.background_mode.set(enabled);
    }

    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.main.show()?;
        slint::run_event_loop_until_quit()?;
        let _ = self.popup.hide();
        self.main.hide()
    }
}

#[derive(Clone)]
pub struct UiHandle {
    main: slint::Weak<AppWindow>,
    popup: slint::Weak<TranslationPopup>,
    prepare_popup: fn(&slint::Window),
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

    pub fn show_popup(&self, anchor: Option<Point>, work_area: Option<Rect>) {
        let popup = self.popup.clone();
        let prepare_popup = self.prepare_popup;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(popup) = popup.upgrade() {
                prepare_popup(popup.window());
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
                if !popup.window().is_visible() {
                    let _ = popup.show();
                    // Some backends only create the native handle when first shown.
                    prepare_popup(popup.window());
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
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(main) = main.upgrade() {
                if main.window().is_minimized() {
                    main.window().set_minimized(false);
                }
                let _ = main.show();
            }
        });
    }

    pub fn quit(&self) {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
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
    use super::{MainWindowClosePolicy, close_policy};

    #[test]
    fn main_window_only_hides_when_tray_registration_succeeded() {
        assert_eq!(close_policy(true), MainWindowClosePolicy::HideToTray);
        assert_eq!(close_policy(false), MainWindowClosePolicy::Exit);
    }
}
