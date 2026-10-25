//! On-demand notification-area menu. Deferred callbacks are scoped to one opening.
use std::{cell::RefCell, rc::Rc, time::Duration};

use lexift_core::{AppEvent, ports::tray::TrayMenuRequest};
use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::{
    TrayMenuWindow,
    bridge::{PassiveWindowPreparation, PopupPointerInput, WindowLifecycleCallbacks},
};

struct Registry {
    window: Option<TrayMenuWindow>,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    generation: u64,
}

thread_local! {
    static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    REGISTRY.with(|slot| {
        *slot.borrow_mut() = Some(Registry {
            window: None,
            handler: None,
            prepare,
            lifecycle,
            generation: 0,
        })
    });
}

pub(crate) fn set_handler(handler: Rc<dyn Fn(AppEvent)>) {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut() {
            r.handler = Some(handler);
        }
    });
}

pub(crate) fn is_open() -> bool {
    REGISTRY.with(|slot| slot.borrow().as_ref().is_some_and(|r| r.window.is_some()))
}

pub(crate) fn refresh_pin_recovery() {
    REGISTRY.with(|s| {
        if let Some(w) = s.borrow().as_ref().and_then(|r| r.window.as_ref()) {
            w.set_pin_recovery_available(crate::pinned_screenshot::policy::recoverable());
            w.set_pin_hide_available(crate::pinned_screenshot::visibility::available(true));
            w.set_pin_restore_available(crate::pinned_screenshot::visibility::available(false));
        }
    });
}
pub(crate) fn apply_theme() {
    REGISTRY.with(|slot| {
        if let Some(w) = slot.borrow().as_ref().and_then(|r| r.window.as_ref()) {
            crate::theme::apply(w);
        }
    });
}

pub(crate) fn close(return_focus: bool) {
    let restore = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        r.generation = r.generation.wrapping_add(1);
        let window = r.window.take()?;
        // Never return focus if cancellation raced with activation of another app.
        let focused = window
            .window()
            .with_winit_window(|w| w.has_focus())
            .unwrap_or(false);
        (r.lifecycle.set_popup_dismissal)(window.window(), false);
        let _ = window.hide();
        drop(window);
        (return_focus && focused).then(|| Rc::clone(&r.lifecycle.tray_menu_cancelled))
    });
    if let Some(restore) = restore {
        restore();
    }
    crate::bridge::schedule_idle_memory_trim();
}

fn finish_later(generation: u64, action: Option<AppEvent>, restore: bool) {
    let _ = slint::invoke_from_event_loop(move || {
        let handler = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot
                .as_ref()
                .filter(|r| r.generation == generation && r.window.is_some())?;
            Some(r.handler.clone())
        });
        let Some(handler) = handler else { return };
        close(restore);
        if let Some((handler, action)) = handler.zip(action) {
            handler(action);
        }
    });
}

pub(crate) fn show(request: TrayMenuRequest) {
    close(false);
    let result = REGISTRY.with(|slot| -> Result<(), slint::PlatformError> {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else {
            return Ok(());
        };
        let window = TrayMenuWindow::new()?;
        crate::theme::apply(&window);
        let labels = crate::i18n::tray_labels();
        window.set_open_label(labels.open.clone().into());
        window.set_settings_label(labels.settings.clone().into());
        window.set_quit_label(labels.quit.clone().into());
        window.set_pin_recovery_available(crate::pinned_screenshot::policy::recoverable());
        window.set_pin_hide_available(crate::pinned_screenshot::visibility::available(true));
        window.set_pin_restore_available(crate::pinned_screenshot::visibility::available(false));
        window.set_import_available(crate::annotation::image_import::available());
        window.set_selected(if request.keyboard { 0 } else { -1 });
        let generation = r.generation;
        window.on_action(move |index| {
            if index == 5 && !crate::pinned_screenshot::policy::recoverable() {
                return;
            }
            if matches!(index, 6 | 7)
                && !crate::pinned_screenshot::visibility::available(index == 6)
            {
                return;
            }
            if matches!(index, 3 | 4) && !crate::annotation::image_import::available() {
                return;
            }
            if let Some(event) = action_for_index(index, request.anchor) {
                finish_later(generation, Some(event), false);
            }
        });
        window.on_cancel(move || finish_later(generation, None, true));
        let weak = window.as_weak();
        window.on_character(move |text| {
            let Some(window) = weak.upgrade() else { return };
            let matches = initial_matches(
                &[
                    &labels.open,
                    &labels.settings,
                    &crate::i18n::tr("Image history"),
                    &crate::i18n::tr("Open image"),
                    &crate::i18n::tr("Pin image from clipboard"),
                    &crate::i18n::tr("Turn off mouse passthrough for all pins"),
                    &crate::i18n::tr("Hide all pins"),
                    &crate::i18n::tr("Restore hidden pins"),
                    &labels.quit,
                ],
                &text,
            );
            if matches == [5] && !crate::pinned_screenshot::policy::recoverable() {
                return;
            }
            if matches.len() == 1
                && matches!(matches[0], 6 | 7)
                && !crate::pinned_screenshot::visibility::available(matches[0] == 6)
            {
                return;
            }
            if matches.len() == 1 {
                finish_later(
                    generation,
                    action_for_index(matches[0], request.anchor),
                    false,
                );
            } else if let Some(next) = matches
                .iter()
                .copied()
                .find(|i| *i > window.get_selected())
                .or_else(|| matches.first().copied())
            {
                window.set_selected(next);
            }
        });
        window.window().on_close_requested(move || {
            finish_later(generation, None, true);
            slint::CloseRequestResponse::KeepWindowShown
        });
        let mut was_focused = false;
        window.window().on_winit_window_event(move |_, event| {
            if let winit::event::WindowEvent::Focused(focused) = event {
                if was_focused && !focused {
                    finish_later(generation, None, false);
                }
                was_focused |= focused;
            }
            EventResult::Propagate
        });
        window.window().set_position(slint::PhysicalPosition::new(
            request.anchor.x,
            request.anchor.y,
        ));
        r.window = Some(window);
        crate::bridge::cancel_idle_memory_trim();
        schedule_show(generation, request, 0);
        Ok(())
    });
    if let Err(error) = result {
        tracing::warn!(%error, "could not create tray menu");
    }
}

fn schedule_show(generation: u64, request: TrayMenuRequest, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let Some(r) = slot.as_ref().filter(|r| r.generation == generation) else {
                    return;
                };
                let Some(window) = r.window.as_ref() else {
                    return;
                };
                match (r.prepare)(window.window()) {
                    PassiveWindowPreparation::Ready => {
                        place(window, request, &r.lifecycle);
                        if !(r.lifecycle.configure_menu_window)(window.window())
                            || window.show().is_err()
                        {
                            finish_later(generation, None, false);
                            return;
                        }
                        place(window, request, &r.lifecycle);
                        let weak = window.as_weak();
                        let sink = Rc::new(move |input| {
                            if matches!(input, PopupPointerInput::DismissRequested { .. }) {
                                finish_later(generation, None, false);
                            } else if let Some(window) = weak.upgrade() {
                                crate::bridge::dispatch_pointer_input(window.window(), input);
                            }
                        });
                        // Showing may refresh native styles from Winit's cached flags.
                        if !(r.lifecycle.configure_menu_window)(window.window())
                            || !(r.lifecycle.complete_passive_window_show)(window.window(), sink)
                            || !(r.lifecycle.activate_user_requested_window)(window.window())
                            || !(r.lifecycle.set_popup_dismissal)(window.window(), true)
                        {
                            tracing::warn!("could not initialize tray menu interaction");
                            finish_later(generation, None, false);
                        }
                        // Moving to another monitor can change the scale factor after showing.
                        let weak = window.as_weak();
                        let lifecycle = r.lifecycle.clone();
                        slint::Timer::single_shot(Duration::ZERO, move || {
                            if let Some(window) = weak.upgrade() {
                                place(&window, request, &lifecycle);
                            }
                        });
                    }
                    PassiveWindowPreparation::Pending if attempt < 8 => {
                        schedule_show(generation, request, attempt + 1)
                    }
                    _ => finish_later(generation, None, false),
                }
            });
        },
    );
}

fn place(window: &TrayMenuWindow, request: TrayMenuRequest, lifecycle: &WindowLifecycleCallbacks) {
    window.window().set_size(slint::LogicalSize::new(
        window.get_menu_width(),
        window.get_menu_height(),
    ));
    if let Some(area) = (lifecycle.popup_work_area)(request.anchor) {
        let size = slint::LogicalSize::new(window.get_menu_width(), window.get_menu_height())
            .to_physical(window.window().scale_factor());
        let p = crate::placement::place_popup(request.anchor, size.width, size.height, area, 0, 0)
            .position;
        window
            .window()
            .set_position(slint::PhysicalPosition::new(p.x, p.y));
    }
}

fn action_for_index(index: i32, anchor: lexift_core::domain::geometry::Point) -> Option<AppEvent> {
    Some(match index {
        0 => AppEvent::MainWindowRequested,
        1 => AppEvent::SettingsWindowRequested,
        2 => AppEvent::ImageHistory(lexift_core::domain::image_history::workflow::Event::Open {
            anchor,
        }),
        3 | 4 => AppEvent::ImageImport(lexift_core::domain::image_import::Event::Requested {
            target: if index == 3 {
                lexift_core::domain::image_import::Target::FileEditor
            } else {
                lexift_core::domain::image_import::Target::ClipboardPin
            },
            anchor,
            owner: None,
            title: crate::i18n::tr("Open image"),
        }),
        5 => AppEvent::PinnedImage(lexift_core::domain::pinned_image::Event::RestoreMouseInput),
        6 | 7 => AppEvent::PinnedImage(lexift_core::domain::pinned_image::Event::WindowVisibility(
            lexift_core::domain::pinned_image::visibility::Event::Requested {
                hidden: index == 6,
                anchor,
            },
        )),
        8 => AppEvent::ExitRequested,
        _ => return None,
    })
}

fn initial_matches(labels: &[&str], text: &str) -> Vec<i32> {
    let mut chars = text.chars();
    let Some(ch) = chars.next().filter(|ch| ch.is_alphanumeric()) else {
        return vec![];
    };
    if chars.next().is_some() {
        return vec![];
    }
    let initial = ch.to_lowercase().to_string();
    labels
        .iter()
        .enumerate()
        .filter_map(|(i, label)| {
            label
                .trim_start()
                .to_lowercase()
                .starts_with(&initial)
                .then_some(i as i32)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_tray_renders_all_locales_themes_dpis_and_recovery_states() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Backend(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for Backend {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
                self.0.borrow_mut().push(w.clone());
                Ok(w)
            }
        }
        let adapters = Rc::new(RefCell::new(vec![]));
        slint::platform::set_platform(Box::new(Backend(adapters.clone()))).unwrap();
        let w = TrayMenuWindow::new().unwrap();
        let adapter = adapters.borrow().last().unwrap().clone();
        for language in lexift_core::domain::ui_language::UiLanguage::ALL {
            crate::i18n::select(language);
            let labels = crate::i18n::tray_labels();
            w.set_open_label(labels.open.into());
            w.set_settings_label(labels.settings.into());
            w.set_quit_label(labels.quit.into());
            for dark in [false, true] {
                w.global::<crate::Colors>().set_preference(i32::from(dark));
                for dpi in [1., 1.25] {
                    w.window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    for flags in 0..4 {
                        let enabled = flags & 1 != 0;
                        w.set_pin_recovery_available(enabled);
                        w.set_pin_hide_available(enabled);
                        w.set_pin_restore_available(flags & 2 != 0);
                        w.set_selected(if flags == 0 {
                            -1
                        } else if enabled {
                            6
                        } else {
                            7
                        });
                        let width = (w.get_menu_width() * dpi).ceil() as u32;
                        let height = (w.get_menu_height() * dpi).ceil() as u32;
                        adapter.set_size(slint::PhysicalSize::new(width, height));
                        adapter.request_redraw();
                        let mut pixels =
                            vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                        adapter.draw_if_needed(|r| {
                            r.render(&mut pixels, width as usize);
                        });
                        assert!(pixels.windows(2).any(|p| p[0] != p[1]));
                        assert!(width < 1500);
                        if let Ok(dir) = std::env::var("LEXIFT_PIN_POLICY_RENDER_DIR") {
                            crate::i18n::write_test_bitmap(
                                &dir,
                                &format!("tray-{}-{dark}-{dpi}-{flags}", language.code()),
                                width,
                                height,
                                pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn reopening_drops_previous_window_and_invalidates_queued_work() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| true,
                |_, _, _| true,
                |_, _| true,
                |_, _| true,
                || true,
            ),
        );
        let request = TrayMenuRequest {
            anchor: Default::default(),
            keyboard: true,
        };
        show(request);
        let (weak, generation) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let window = r.window.as_ref().unwrap();
            assert_eq!(window.get_selected(), 0);
            (window.as_weak(), r.generation)
        });
        show(TrayMenuRequest {
            keyboard: false,
            ..request
        });
        assert!(weak.upgrade().is_none());
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_ne!(r.generation, generation);
            assert_eq!(r.window.as_ref().unwrap().get_selected(), -1);
        });
        close(false);
        assert!(!is_open());
        slint::platform::update_timers_and_animations();
        assert!(!is_open());
    }

    #[test]
    fn initial_navigation_handles_duplicates_unicode_and_invalid_keys() {
        assert_eq!(
            initial_matches(&["Open", "Options", "Quit"], "o"),
            vec![0, 1]
        );
        assert_eq!(
            initial_matches(&["Ouvrir", "Paramètres", "Quitter"], "P"),
            vec![1]
        );
        assert_eq!(
            initial_matches(&["打开 Lexift", "设置", "退出"], "设"),
            vec![1]
        );
        assert!(initial_matches(&["Open"], "").is_empty());
        assert!(initial_matches(&["Open"], "op").is_empty());
        assert!(initial_matches(&["Open"], " ").is_empty());
    }
    #[test]
    fn menu_actions_preserve_existing_application_intents() {
        assert_eq!(
            action_for_index(0, Default::default()),
            Some(AppEvent::MainWindowRequested)
        );
        assert_eq!(
            action_for_index(1, Default::default()),
            Some(AppEvent::SettingsWindowRequested)
        );
        assert_eq!(
            action_for_index(8, Default::default()),
            Some(AppEvent::ExitRequested)
        );
        assert_eq!(action_for_index(-1, Default::default()), None);
        assert!(matches!(
            action_for_index(3, Default::default()),
            Some(AppEvent::ImageImport(
                lexift_core::domain::image_import::Event::Requested {
                    target: lexift_core::domain::image_import::Target::FileEditor,
                    ..
                }
            ))
        ));
        assert!(matches!(
            action_for_index(4, Default::default()),
            Some(AppEvent::ImageImport(
                lexift_core::domain::image_import::Event::Requested {
                    target: lexift_core::domain::image_import::Target::ClipboardPin,
                    ..
                }
            ))
        ));
        assert_eq!(
            action_for_index(5, Default::default()),
            Some(AppEvent::PinnedImage(
                lexift_core::domain::pinned_image::Event::RestoreMouseInput
            ))
        );
        for (index, hidden) in [(6, true), (7, false)] {
            assert_eq!(
                action_for_index(index, Default::default()),
                Some(AppEvent::PinnedImage(
                    lexift_core::domain::pinned_image::Event::WindowVisibility(
                        lexift_core::domain::pinned_image::visibility::Event::Requested {
                            hidden,
                            anchor: Default::default()
                        }
                    )
                ))
            );
        }
        assert_eq!(action_for_index(9, Default::default()), None);
    }
}
