use std::sync::{Arc, Mutex};

use lexift_core::{
    Result,
    domain::runtime_config::HotkeyConfig,
    ports::hotkey::{HotkeyHandler, HotkeyPort},
};

pub(crate) struct HotkeyRuntimeManager {
    port: Option<Arc<dyn HotkeyPort>>,
    state: Mutex<HotkeyState>,
}

#[derive(Default)]
struct HotkeyState {
    current: Option<HotkeyConfig>,
    handler: Option<HotkeyHandler>,
    annotation_current: Option<HotkeyConfig>,
    annotation_handler: Option<HotkeyHandler>,
}

impl HotkeyRuntimeManager {
    pub(crate) fn start_annotation(
        &self,
        config: HotkeyConfig,
        handler: HotkeyHandler,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.annotation_handler = Some(Arc::clone(&handler));
        if state.current == Some(config) {
            return Err(lexift_core::Error::new(
                "Shortcut is already used by translation",
            ));
        }
        self.port
            .as_ref()
            .ok_or_else(|| lexift_core::Error::new("Global shortcuts are unavailable"))?
            .register_annotation_hotkey(config, handler)?;
        state.annotation_current = Some(config);
        Ok(())
    }
    pub(crate) fn new(port: Option<Arc<dyn HotkeyPort>>) -> Self {
        Self {
            port,
            state: Mutex::new(HotkeyState::default()),
        }
    }

    pub(crate) fn start(&self, config: HotkeyConfig, handler: HotkeyHandler) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.handler = Some(Arc::clone(&handler));
        if state.annotation_current == Some(config) {
            return Err(lexift_core::Error::new(
                "Shortcut is already used by annotation",
            ));
        }
        if let Some(port) = &self.port {
            port.register_translate_hotkey(config, Arc::clone(&handler))?;
        }
        state.current = Some(config);
        Ok(())
    }

    pub(crate) fn apply(&self, config: HotkeyConfig) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.current == Some(config) {
            return Ok(());
        }
        if state.annotation_current == Some(config) {
            return Err(lexift_core::Error::new(
                "Shortcut is already used by annotation",
            ));
        }
        let Some(handler) = state.handler.as_ref().map(Arc::clone) else {
            // A missing platform capability must not prevent settings on unsupported systems.
            state.current = Some(config);
            return Ok(());
        };
        if let Some(port) = &self.port {
            port.replace_translate_hotkey(config, handler)?;
        }
        state.current = Some(config);
        Ok(())
    }

    pub(crate) fn apply_annotation(&self, config: HotkeyConfig) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.annotation_current == Some(config) {
            return Ok(());
        }
        if state.current == Some(config) {
            return Err(lexift_core::Error::new(
                "Shortcut is already used by translation",
            ));
        }
        let Some(handler) = state.annotation_handler.as_ref().map(Arc::clone) else {
            state.annotation_current = Some(config);
            return Ok(());
        };
        let port = self
            .port
            .as_ref()
            .ok_or_else(|| lexift_core::Error::new("Global shortcuts are unavailable"))?;
        if state.annotation_current.is_some() {
            port.replace_annotation_hotkey(config, handler)?;
        } else {
            port.register_annotation_hotkey(config, handler)?;
        }
        state.annotation_current = Some(config);
        Ok(())
    }
}

impl Drop for HotkeyRuntimeManager {
    fn drop(&mut self) {
        if let Some(port) = &self.port {
            let _ = port.unregister_annotation_hotkey();
        }
        if let Some(port) = &self.port
            && let Err(error) = port.unregister_translate_hotkey()
        {
            tracing::warn!(%error, "global hotkey could not be unregistered");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use lexift_core::{Error, domain::runtime_config::HotkeyConfig};

    use super::*;

    #[derive(Default)]
    struct FakeHotkeyPort {
        active: Mutex<Option<HotkeyConfig>>,
        rejected: Mutex<Option<HotkeyConfig>>,
        annotation_active: Mutex<Option<HotkeyConfig>>,
        annotation_rejected: Mutex<Option<HotkeyConfig>>,
        annotation_available: Mutex<bool>,
    }

    impl HotkeyPort for FakeHotkeyPort {
        fn register_annotation_hotkey(
            &self,
            config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> Result<()> {
            if !*self.annotation_available.lock().unwrap()
                || *self.annotation_rejected.lock().unwrap() == Some(config)
            {
                return Err(Error::new("annotation shortcut unavailable"));
            }
            *self.annotation_active.lock().unwrap() = Some(config);
            Ok(())
        }

        fn replace_annotation_hotkey(
            &self,
            config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> Result<()> {
            if *self.annotation_rejected.lock().unwrap() == Some(config) {
                return Err(Error::new("hotkey conflict"));
            }
            *self.annotation_active.lock().unwrap() = Some(config);
            Ok(())
        }

        fn unregister_annotation_hotkey(&self) -> Result<()> {
            *self.annotation_active.lock().unwrap() = None;
            Ok(())
        }
        fn register_translate_hotkey(
            &self,
            config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> Result<()> {
            *self.active.lock().unwrap() = Some(config);
            Ok(())
        }

        fn replace_translate_hotkey(
            &self,
            config: HotkeyConfig,
            _handler: HotkeyHandler,
        ) -> Result<()> {
            if *self.rejected.lock().unwrap() == Some(config) {
                return Err(Error::new("hotkey conflict"));
            }
            *self.active.lock().unwrap() = Some(config);
            Ok(())
        }

        fn unregister_translate_hotkey(&self) -> Result<()> {
            *self.active.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn unavailable_annotation_shortcut_preserves_translation_registration() {
        let port = Arc::new(FakeHotkeyPort::default());
        let manager = HotkeyRuntimeManager::new(Some(port.clone()));
        let initial = HotkeyConfig::default();
        manager.start(initial, Arc::new(|| {})).unwrap();
        assert!(
            manager
                .start_annotation("Alt + A".parse().unwrap(), Arc::new(|| {}))
                .is_err()
        );
        assert_eq!(*port.active.lock().unwrap(), Some(initial));
        let replacement = "Ctrl + Shift + 7".parse().unwrap();
        manager.apply(replacement).unwrap();
        assert_eq!(*port.active.lock().unwrap(), Some(replacement));
    }

    #[test]
    fn registers_and_reloads_the_hotkey() {
        let port = Arc::new(FakeHotkeyPort::default());
        let manager = HotkeyRuntimeManager::new(Some(port.clone()));
        let initial = HotkeyConfig::default();
        let replacement: HotkeyConfig = "Ctrl + Shift + 7".parse().unwrap();
        manager.start(initial, Arc::new(|| {})).unwrap();
        assert_eq!(*port.active.lock().unwrap(), Some(initial));
        manager.apply(replacement).unwrap();
        assert_eq!(*port.active.lock().unwrap(), Some(replacement));
    }

    #[test]
    fn failed_reload_preserves_the_previous_hotkey() {
        let port = Arc::new(FakeHotkeyPort::default());
        let manager = HotkeyRuntimeManager::new(Some(port.clone()));
        let initial = HotkeyConfig::default();
        let rejected: HotkeyConfig = "Ctrl + Alt + F12".parse().unwrap();
        *port.rejected.lock().unwrap() = Some(rejected);
        manager.start(initial, Arc::new(|| {})).unwrap();
        assert!(manager.apply(rejected).is_err());
        assert_eq!(*port.active.lock().unwrap(), Some(initial));
        assert_eq!(manager.state.lock().unwrap().current, Some(initial));
    }

    #[test]
    fn annotation_replacement_preserves_old_registration_on_conflict_and_can_retry() {
        let port = Arc::new(FakeHotkeyPort::default());
        *port.annotation_available.lock().unwrap() = true;
        let manager = HotkeyRuntimeManager::new(Some(port.clone()));
        let original: HotkeyConfig = "Alt + A".parse().unwrap();
        let blocked: HotkeyConfig = "Ctrl + Shift + 8".parse().unwrap();
        let replacement: HotkeyConfig = "Ctrl + Shift + 9".parse().unwrap();
        manager
            .start(HotkeyConfig::default(), Arc::new(|| {}))
            .unwrap();
        manager.start_annotation(original, Arc::new(|| {})).unwrap();
        *port.annotation_rejected.lock().unwrap() = Some(blocked);
        assert!(manager.apply_annotation(blocked).is_err());
        assert_eq!(*port.annotation_active.lock().unwrap(), Some(original));
        manager.apply_annotation(replacement).unwrap();
        assert_eq!(*port.annotation_active.lock().unwrap(), Some(replacement));
        assert!(manager.apply(HotkeyConfig::default()).is_ok());
        assert!(manager.apply(replacement).is_err());
        assert!(manager.apply_annotation(HotkeyConfig::default()).is_err());
    }

    #[test]
    fn failed_startup_registration_can_be_retried_from_settings() {
        let port = Arc::new(FakeHotkeyPort::default());
        let manager = HotkeyRuntimeManager::new(Some(port.clone()));
        manager
            .start(HotkeyConfig::default(), Arc::new(|| {}))
            .unwrap();
        let initial: HotkeyConfig = "Alt + A".parse().unwrap();
        assert!(manager.start_annotation(initial, Arc::new(|| {})).is_err());
        *port.annotation_available.lock().unwrap() = true;
        manager.apply_annotation(initial).unwrap();
        assert_eq!(*port.annotation_active.lock().unwrap(), Some(initial));
    }

    #[test]
    fn startup_collision_keeps_translation_and_allows_annotation_recovery() {
        let port = Arc::new(FakeHotkeyPort::default());
        *port.annotation_available.lock().unwrap() = true;
        let manager = HotkeyRuntimeManager::new(Some(port.clone()));
        let translation = HotkeyConfig::default();
        manager.start(translation, Arc::new(|| {})).unwrap();
        assert!(
            manager
                .start_annotation(translation, Arc::new(|| {}))
                .is_err()
        );
        assert_eq!(*port.active.lock().unwrap(), Some(translation));
        let annotation = HotkeyConfig::annotation_default();
        manager.apply_annotation(annotation).unwrap();
        assert_eq!(*port.annotation_active.lock().unwrap(), Some(annotation));
    }
}
