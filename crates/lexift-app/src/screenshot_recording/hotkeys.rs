//! Adapter scheduling only; Core accepts registrations and UI validates input.
use super::*;
use lexift_core::domain::recording_hotkeys::{Registration, Scope};
pub(super) struct Lease {
    pub scope: Scope,
    pub cancel: AtomicBool,
    registered: AtomicBool,
    pub retired: AtomicBool,
    port: Option<Arc<dyn lexift_core::ports::hotkey::HotkeyPort>>,
    pub notify: tokio::sync::Notify,
}
impl Lease {
    pub fn retire(&self) {
        self.cancel.store(true, Ordering::Release);
        // If registration is still running, its completion owns cleanup. This
        // handshake prevents an early unregister from racing a late registration.
        if !self.registered.load(Ordering::Acquire) {
            return;
        }
        if let Some(port) = &self.port
            && let Err(error) = port.unregister_recording_hotkeys(self.scope)
        {
            tracing::warn!(%error,"Recording shortcut retirement failed");
        }
        self.retired.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}
pub(super) fn execute(
    r: &Arc<ScreenshotRuntime>,
    c: &Arc<AppController>,
    command: &Command,
) -> bool {
    match command {
        Command::Shortcut { .. } => {
            c.ui.screen_recording(command.clone());
            true
        }
        Command::ReleaseHotkeys(scope) => {
            let lease = r
                .recording
                .lock()
                .unwrap()
                .hotkeys
                .as_ref()
                .filter(|l| l.scope == *scope)
                .cloned();
            if let Some(lease) = lease {
                lease.cancel.store(true, Ordering::Release);
                c.runtime.spawn_blocking(move || lease.retire());
            }
            true
        }
        Command::PrepareHotkeys(identity) => {
            let identity = *identity;
            let scope = Scope::new(identity);
            let keys = {
                let state = c.state.lock().unwrap();
                if state.screenshot.phase
                    != lexift_core::domain::screenshot::ScreenshotPhase::Recording
                    || state.screenshot.recording.as_ref().is_none_or(|s| {
                        s.lifecycle.identity != identity
                            || s.lifecycle.phase
                                != lexift_core::domain::screen_recording::Phase::Starting
                    })
                {
                    return true;
                }
                [
                    state.settings.recording_pause_hotkey,
                    state.settings.recording_finish_hotkey,
                ]
            };
            let mut slot = r.recording.lock().unwrap();
            if slot
                .hotkeys
                .as_ref()
                .is_some_and(|l| !l.retired.load(Ordering::Acquire))
            {
                drop(slot);
                emit(
                    c,
                    Event::HotkeysPrepared {
                        identity,
                        registration: Registration {
                            scope,
                            pause: None,
                            finish: None,
                        },
                    },
                );
                return true;
            }
            let lease = Arc::new(Lease {
                scope,
                cancel: AtomicBool::new(false),
                registered: AtomicBool::new(false),
                retired: AtomicBool::new(false),
                port: c.runtime_manager.as_ref().and_then(|m| m.hotkey_port()),
                notify: Default::default(),
            });
            slot.hotkeys = Some(lease.clone());
            drop(slot);
            let controller = c.clone();
            c.runtime.spawn_blocking(move || {
                let started = std::time::Instant::now();
                let target = controller.clone();
                let registration = lease
                    .port
                    .as_ref()
                    .ok_or_else(|| Error::new("Recording shortcuts unavailable"))
                    .and_then(|p| {
                        p.register_recording_hotkeys(
                            scope,
                            keys,
                            Arc::new(move |action| {
                                emit(&target, Event::Shortcut { scope, action })
                            }),
                        )
                    })
                    .unwrap_or(Registration {
                        scope,
                        pause: None,
                        finish: None,
                    });
                lease.registered.store(true, Ordering::Release);
                if lease.cancel.load(Ordering::Acquire) {
                    lease.retire();
                }
                tracing::debug!(
                    session = scope.session,
                    operation = scope.operation,
                    generation = scope.generation,
                    pause = registration.pause.is_some(),
                    finish = registration.finish.is_some(),
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
                    "Recording shortcuts prepared"
                );
                emit(
                    &controller,
                    Event::HotkeysPrepared {
                        identity,
                        registration,
                    },
                );
            });
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::runtime_config::HotkeyConfig,
        ports::hotkey::{HotkeyHandler, HotkeyPort},
    };
    struct Port(std::sync::atomic::AtomicUsize);
    impl HotkeyPort for Port {
        fn register_translate_hotkey(&self, _: HotkeyConfig, _: HotkeyHandler) -> Result<()> {
            Ok(())
        }
        fn replace_translate_hotkey(&self, _: HotkeyConfig, _: HotkeyHandler) -> Result<()> {
            Ok(())
        }
        fn unregister_translate_hotkey(&self) -> Result<()> {
            Ok(())
        }
        fn unregister_recording_hotkeys(&self, _: Scope) -> Result<()> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }
    fn lease(port: Arc<Port>) -> Lease {
        Lease {
            scope: Scope {
                session: 1,
                operation: 2,
                generation: 3,
            },
            cancel: AtomicBool::new(false),
            registered: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            port: Some(port),
            notify: Default::default(),
        }
    }
    #[test]
    fn cancellation_before_registration_cannot_report_early_retirement() {
        let p = Arc::new(Port(Default::default()));
        let l = lease(p.clone());
        l.retire();
        assert!(l.cancel.load(Ordering::Acquire));
        assert!(!l.retired.load(Ordering::Acquire));
        assert_eq!(p.0.load(Ordering::Acquire), 0);
        l.registered.store(true, Ordering::Release);
        l.retire();
        assert!(l.retired.load(Ordering::Acquire));
        assert_eq!(p.0.load(Ordering::Acquire), 1);
    }
    #[test]
    fn completed_registration_retires_only_its_adapter_lease() {
        let p = Arc::new(Port(Default::default()));
        let l = lease(p.clone());
        l.registered.store(true, Ordering::Release);
        l.retire();
        assert!(l.cancel.load(Ordering::Acquire));
        assert!(l.retired.load(Ordering::Acquire));
        assert_eq!(p.0.load(Ordering::Acquire), 1);
    }
}
