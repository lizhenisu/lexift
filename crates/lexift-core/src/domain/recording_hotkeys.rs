//! Temporary recording controls, isolated from changing transport request IDs.
use super::{runtime_config::HotkeyConfig, screen_recording::Identity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TogglePause,
    Finish,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub session: u64,
    pub operation: u64,
    pub generation: u64,
}
impl Scope {
    pub fn new(id: Identity) -> Self {
        Self {
            session: id.session,
            operation: id.operation,
            generation: id.request,
        }
    }
    pub fn owns(self, id: Identity) -> bool {
        self.session == id.session && self.operation == id.operation
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub scope: Scope,
    pub pause: Option<HotkeyConfig>,
    pub finish: Option<HotkeyConfig>,
}
impl Registration {
    pub fn accepts(&self, action: Action) -> bool {
        match action {
            Action::TogglePause => self.pause.is_some(),
            Action::Finish => self.finish.is_some(),
        }
    }
}
pub fn default_pause() -> HotkeyConfig {
    "Ctrl+Alt+F9".parse().expect("fixed shortcut")
}
pub fn default_finish() -> HotkeyConfig {
    "Ctrl+Alt+F10".parse().expect("fixed shortcut")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_survives_transport_requests_without_accepting_other_sessions() {
        let scope = Scope::new(Identity {
            session: 1,
            operation: 2,
            request: 3,
        });
        assert!(scope.owns(Identity {
            session: 1,
            operation: 2,
            request: 7
        }));
        assert!(!scope.owns(Identity {
            session: 2,
            operation: 2,
            request: 3
        }));
        assert_eq!(default_pause().to_string(), "Ctrl + Alt + F9");
        assert_ne!(default_pause(), default_finish());
    }
}

#[cfg(test)]
mod settings_tests {
    use crate::domain::settings::{Settings, SettingsChange};
    #[test]
    fn six_way_conflicts_and_recording_lock_preserve_other_keys() {
        let settings = Settings::default();
        for key in [
            settings.hotkey,
            settings.annotation_hotkey,
            settings.ocr_hotkey,
            settings.screenshot_hotkey,
            settings.recording_finish_hotkey,
        ] {
            assert!(
                SettingsChange::RecordingPauseHotkey(key)
                    .validate_shortcuts(&settings, false)
                    .is_err()
            );
        }
        assert!(
            SettingsChange::RecordingPauseHotkey("Ctrl+Shift+F9".parse().unwrap())
                .validate_shortcuts(&settings, true)
                .is_err()
        );
        assert!(
            SettingsChange::RecordingPauseHotkey("Ctrl+Shift+F9".parse().unwrap())
                .validate_shortcuts(&settings, false)
                .is_ok()
        );
        assert!(
            SettingsChange::Hotkey(settings.recording_pause_hotkey)
                .validate_shortcuts(&settings, false)
                .is_err()
        );
    }
}
