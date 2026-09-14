use lexift_core::domain::settings::Settings;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppConfig {
    pub schema_version: u32,
    pub settings: Settings,
}
