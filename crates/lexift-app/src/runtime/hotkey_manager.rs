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
}

impl HotkeyRuntimeManager {
    pub(crate) fn start_annotation(&self, handler: HotkeyHandler) -> Result<()> {
        self.port
            .as_ref()
            .ok_or_else(|| lexift_core::Error::new("Global shortcuts are unavailable"))?
            .register_annotation_hotkey(handler)
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
    }

    impl HotkeyPort for FakeHotkeyPort {
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
        assert!(manager.start_annotation(Arc::new(|| {})).is_err());
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
}
