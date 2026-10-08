//! Main and Settings share bounded hidden preparation; Shell role is not activation.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRole {
    Auxiliary,
    Settings,
}

pub(super) fn schedule(
    registry: &mut AppWindowRegistry,
    role: WindowRole,
    activate: bool,
    activation: fn(&slint::Window) -> bool,
    attempt: u8,
) {
    let generation = match role {
        WindowRole::Auxiliary => registry.main_close_generation,
        WindowRole::Settings => registry.settings_close_generation,
    };
    let timer = match role {
        WindowRole::Auxiliary => &registry.main_show_timer,
        WindowRole::Settings => &registry.settings_show_timer,
    };
    timer.start(
        slint::TimerMode::SingleShot,
        if attempt == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(16)
        },
        move || {
            APP_WINDOW_REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(registry) = slot.as_mut() else {
                    return;
                };
                let current = match role {
                    WindowRole::Auxiliary => registry.main_close_generation,
                    WindowRole::Settings => registry.settings_close_generation,
                };
                if generation != current {
                    return;
                }
                let preparation = match role {
                    WindowRole::Auxiliary => registry
                        .main
                        .as_ref()
                        .map(|w| (registry.prepare_app_window)(w.window(), role)),
                    WindowRole::Settings => registry
                        .settings
                        .as_ref()
                        .map(|w| (registry.prepare_app_window)(w.window(), role)),
                };
                match preparation {
                    Some(PassiveWindowPreparation::Ready) => {
                        finish(registry, role, activate, activation)
                    }
                    Some(PassiveWindowPreparation::Pending) if attempt < 19 => {
                        schedule(registry, role, activate, activation, attempt + 1)
                    }
                    Some(_) => {
                        tracing::warn!(?role, attempt, "App window hidden preparation exhausted");
                        match role {
                            WindowRole::Auxiliary => {
                                registry.main.take();
                            }
                            WindowRole::Settings => {
                                registry.settings.take();
                            }
                        }
                        schedule_idle_memory_trim();
                    }
                    None => {}
                }
            });
        },
    );
}

fn finish(
    registry: &mut AppWindowRegistry,
    role: WindowRole,
    activate: bool,
    activation: fn(&slint::Window) -> bool,
) {
    match role {
        WindowRole::Auxiliary => {
            let Some(main) = registry.main.as_ref() else {
                return;
            };
            if main.window().is_minimized() {
                main.window().set_minimized(false);
            }
            if let Err(error) = main.show() {
                tracing::error!(%error, "Main could not be shown");
                return;
            }
            install_app_window_icon(main, registry.configure_app_window_icon);
            if activate {
                activation(main.window());
            }
        }
        WindowRole::Settings => {
            let Some(window) = registry.settings.as_ref() else {
                return;
            };
            if window.window().is_minimized() {
                window.window().set_minimized(false);
            }
            if let Err(error) = window.show() {
                tracing::error!(%error, "Settings could not be shown");
                return;
            }
            if activate {
                activation(window.window());
            }
            install_app_window_icon(window, registry.configure_app_window_icon);
            let color = window.get_resize_fallback_color();
            if !install_settings_platform_hooks(
                &registry.configure_resize_background,
                &registry.configure_window_paint_repair,
                window,
                color,
            ) {
                retry_settings_resize_background(
                    Rc::clone(&registry.configure_resize_background),
                    Rc::clone(&registry.configure_window_paint_repair),
                    window.as_weak(),
                    RESIZE_BACKGROUND_RETRY_ATTEMPTS,
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform(Rc<Cell<Duration>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
        fn duration_since_start(&self) -> Duration {
            self.0.get()
        }
    }
    #[test]
    fn hidden_roles_pending_failure_and_stale_show_requests() {
        let time = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestPlatform(time.clone()))).unwrap();
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let response = Rc::new(Cell::new(PassiveWindowPreparation::Ready));
        let events = recorded.clone();
        let reply = response.clone();
        let lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        )
        .with_app_window_preparation(move |window, role| {
            assert!(!window.is_visible());
            events.borrow_mut().push(role);
            reply.get()
        });
        let ui = Ui::new(
            &AppState::default(),
            false,
            |_| PassiveWindowPreparation::Ready,
            lifecycle,
        )
        .unwrap();
        APP_WINDOW_REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            ensure_main_window(r).unwrap();
            ensure_settings_window(r).unwrap();
            schedule(r, WindowRole::Auxiliary, false, |_| true, 0);
            schedule(r, WindowRole::Settings, false, |_| true, 0);
        });
        slint::platform::update_timers_and_animations();
        assert_eq!(recorded.borrow().len(), 2);
        assert!(recorded.borrow().contains(&WindowRole::Auxiliary));
        assert!(recorded.borrow().contains(&WindowRole::Settings));
        APP_WINDOW_REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.main.as_ref().unwrap().window().is_visible());
            assert!(r.settings.as_ref().unwrap().window().is_visible());
            r.main.take();
            ensure_main_window(r).unwrap();
            schedule(
                r,
                WindowRole::Auxiliary,
                true,
                |_| panic!("stale request activated"),
                0,
            );
            r.main_close_generation += 1;
        });
        recorded.borrow_mut().clear();
        slint::platform::update_timers_and_animations();
        assert!(recorded.borrow().is_empty());
        response.set(PassiveWindowPreparation::Pending);
        APP_WINDOW_REGISTRY.with(|slot| {
            schedule(
                slot.borrow_mut().as_mut().unwrap(),
                WindowRole::Auxiliary,
                true,
                |_| panic!("pending activated"),
                0,
            )
        });
        for i in 0..=20 {
            time.set(Duration::from_millis(i * 20));
            slint::platform::update_timers_and_animations();
        }
        assert_eq!(recorded.borrow().len(), 20);
        APP_WINDOW_REGISTRY.with(|slot| {
            assert!(slot.borrow().as_ref().unwrap().main.is_none());
            slot.borrow_mut().take();
        });
        drop(ui);
    }
}
