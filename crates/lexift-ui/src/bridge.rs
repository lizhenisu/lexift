use std::{cell::Cell, rc::Rc};

use lexift_core::{
    AppEvent, AppState,
    domain::{
        geometry::{Point, Rect},
        language::Language,
        settings::Settings,
    },
};
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize};

use crate::{
    AppWindow, LanguageMenuWindow, SettingsWindow, TranslationPopup, binding, mapper, placement,
};

const POPUP_GAP_PX: i32 = 12;
const WORK_AREA_MARGIN_PX: i32 = 8;

pub struct Ui {
    main: AppWindow,
    popup: TranslationPopup,
    settings: SettingsWindow,
    language_menu: LanguageMenuWindow,
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
        let settings = SettingsWindow::new()?;
        let language_menu = LanguageMenuWindow::new()?;
        main.set_show_selection_demo(show_selection_demo);
        binding::apply(&main, &popup, &settings, mapper::view_state(initial_state));
        Ok(Self {
            main,
            popup,
            settings,
            language_menu,
            prepare_popup,
            background_mode: Rc::new(Cell::new(false)),
        })
    }

    pub fn handle(&self) -> UiHandle {
        UiHandle {
            main: self.main.as_weak(),
            popup: self.popup.as_weak(),
            settings: self.settings.as_weak(),
            language_menu: self.language_menu.as_weak(),
            prepare_popup: self.prepare_popup,
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
        self.settings.window().on_close_requested(move || {
            if let Some(settings) = settings.upgrade() {
                settings.set_language_menu_open(false);
                let _ = settings.hide();
            }
            if let Some(language_menu) = language_menu.upgrade() {
                let _ = language_menu.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        self.settings.on_cancel_requested(move || {
            if let Some(settings) = settings.upgrade() {
                settings.set_language_menu_open(false);
                let _ = settings.hide();
            }
            if let Some(language_menu) = language_menu.upgrade() {
                let _ = language_menu.hide();
            }
        });
        let settings_handler = Rc::clone(&handler);
        self.settings.on_save_requested(move |target_language| {
            settings_handler(AppEvent::SettingsSaveRequested {
                settings: Settings {
                    target_language: Language(target_language.to_string()),
                },
            });
        });
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        self.settings.on_language_menu_requested(
            move |selector_x, selector_y, selector_width, selector_height, pointer_x, pointer_y| {
                let (Some(settings), Some(language_menu)) =
                    (settings.upgrade(), language_menu.upgrade())
                else {
                    return;
                };
                if settings.get_language_menu_open() {
                    settings.set_language_menu_open(false);
                    let _ = language_menu.hide();
                    return;
                }
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
                    selector_width,
                    selector_height,
                    pointer_x,
                    pointer_y,
                    scale,
                );
                language_menu.set_selected_index(settings.get_draft_target_index());
                language_menu
                    .set_scroll_y(-40.0 * settings.get_draft_target_index().clamp(0, 6) as f32);
                language_menu
                    .window()
                    .set_position(PhysicalPosition::new(geometry.left, geometry.top));
                language_menu.window().set_size(PhysicalSize::new(
                    geometry.width as u32,
                    geometry.height as u32,
                ));
                if language_menu.show().is_ok() {
                    settings.set_language_menu_open(true);
                    language_menu.invoke_request_focus();
                }
            },
        );
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        self.language_menu.on_selected(move |index| {
            if let Some(settings) = settings.upgrade() {
                settings.set_draft_target_index(index);
                settings.set_language_menu_open(false);
            }
            if let Some(language_menu) = language_menu.upgrade() {
                let _ = language_menu.hide();
            }
        });
        let settings = self.settings.as_weak();
        let language_menu = self.language_menu.as_weak();
        self.language_menu.on_dismissed(move || {
            if let Some(settings) = settings.upgrade() {
                settings.set_language_menu_open(false);
            }
            if let Some(language_menu) = language_menu.upgrade() {
                let _ = language_menu.hide();
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

    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.main.show()?;
        slint::run_event_loop_until_quit()?;
        let _ = self.popup.hide();
        let _ = self.settings.hide();
        let _ = self.language_menu.hide();
        self.main.hide()
    }
}

#[derive(Clone)]
pub struct UiHandle {
    main: slint::Weak<AppWindow>,
    popup: slint::Weak<TranslationPopup>,
    settings: slint::Weak<SettingsWindow>,
    language_menu: slint::Weak<LanguageMenuWindow>,
    prepare_popup: fn(&slint::Window),
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

    pub fn show_settings_window(&self, settings: Settings) {
        let window = self.settings.clone();
        let language_menu = self.language_menu.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window.upgrade() {
                window.set_language_menu_open(false);
                window.set_draft_target_index(language_index(&settings.target_language));
                if window.window().is_minimized() {
                    window.window().set_minimized(false);
                }
                let _ = window.show();
            }
            if let Some(language_menu) = language_menu.upgrade() {
                let _ = language_menu.hide();
            }
        });
    }

    pub fn hide_settings_window(&self) {
        let settings = self.settings.clone();
        let language_menu = self.language_menu.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(settings) = settings.upgrade() {
                settings.set_language_menu_open(false);
                let _ = settings.hide();
            }
            if let Some(language_menu) = language_menu.upgrade() {
                let _ = language_menu.hide();
            }
        });
    }

    pub fn quit(&self) {
        let _ = slint::invoke_from_event_loop(|| {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LanguageMenuGeometry {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

fn language_menu_geometry(
    cursor: Point,
    work_area: Rect,
    selector_width: f32,
    selector_height: f32,
    pointer_x: f32,
    pointer_y: f32,
    scale: f32,
) -> LanguageMenuGeometry {
    let gap = (2.0 * scale).round() as i32;
    let desired_height = (242.0 * scale).round() as i32;
    let width = ((selector_width * scale).round() as i32)
        .max(1)
        .min((work_area.right - work_area.left).max(1));
    let selector_left = cursor.x - (pointer_x * scale).round() as i32;
    let selector_top = cursor.y - (pointer_y * scale).round() as i32;
    let selector_bottom = selector_top + (selector_height * scale).round() as i32;
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
        LanguageMenuGeometry, MainWindowClosePolicy, close_policy, language_index,
        language_menu_geometry,
    };

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
                400.0,
                40.0,
                50.0,
                20.0,
                1.0,
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
                400.0,
                40.0,
                50.0,
                20.0,
                1.0,
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
            400.0,
            40.0,
            50.0,
            20.0,
            1.0,
        );
        assert_eq!(constrained.top, 412);
        assert_eq!(constrained.height, 88);
    }
}
