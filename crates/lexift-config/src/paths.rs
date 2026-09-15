use std::path::PathBuf;

pub(crate) fn default_config_path() -> lexift_core::Result<PathBuf> {
    dirs::config_dir()
        .map(|directory| directory.join("Lexift").join("config.toml"))
        .ok_or_else(|| lexift_core::Error::new("Could not determine the user config directory"))
}
