use crate::{Result, domain::settings::Settings};

pub trait SettingsStore {
    fn load(&self) -> Result<Settings>;
    fn save(&self, settings: &Settings) -> Result<()>;
}
