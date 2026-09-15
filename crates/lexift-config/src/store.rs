use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use lexift_core::{Error, Result, domain::settings::Settings, ports::settings::SettingsStore};

use crate::{migration, paths, schema::ConfigFile};

#[derive(Debug, Clone)]
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn for_current_user() -> Result<Self> {
        Ok(Self::new(paths::default_config_path()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SettingsStore for FileSettingsStore {
    fn load(&self) -> Result<Settings> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                tracing::debug!(path = %self.path.display(), "settings file does not exist; using defaults");
                return Ok(Settings::default());
            }
            Err(error) => {
                return Err(Error::new(format!("Could not read settings: {error}")));
            }
        };
        let config: ConfigFile = toml::from_str(&contents)
            .map_err(|error| Error::new(format!("Could not parse settings: {error}")))?;
        let config = migration::migrate(config)?;
        tracing::debug!(
            path = %self.path.display(),
            schema_version = config.schema_version,
            "settings loaded"
        );
        Ok(config.into_settings())
    }

    fn save(&self, settings: &Settings) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::new("Settings path has no parent directory"))?;
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("Could not create settings directory: {error}")))?;
        let config = ConfigFile::from_settings(settings);
        let contents = toml::to_string_pretty(&config)
            .map_err(|error| Error::new(format!("Could not serialize settings: {error}")))?;
        let mut file = AtomicWriteFile::open(&self.path)
            .map_err(|error| Error::new(format!("Could not open settings for writing: {error}")))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| Error::new(format!("Could not write settings: {error}")))?;
        file.commit()
            .map_err(|error| Error::new(format!("Could not commit settings: {error}")))?;
        tracing::debug!(
            path = %self.path.display(),
            schema_version = config.schema_version,
            "settings saved"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use lexift_core::domain::language::Language;
    use tempfile::TempDir;

    use super::*;

    fn fixture() -> (TempDir, FileSettingsStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = FileSettingsStore::new(directory.path().join("nested").join("config.toml"));
        (directory, store)
    }

    fn settings(language: &str) -> Settings {
        Settings {
            target_language: Language(language.into()),
        }
    }

    #[test]
    fn missing_file_uses_defaults_without_creating_it() {
        let (_directory, store) = fixture();
        assert_eq!(store.load().unwrap(), Settings::default());
        assert!(!store.path().exists());
    }

    #[test]
    fn save_creates_parent_and_v1_file() {
        let (_directory, store) = fixture();
        store.save(&settings("ja")).unwrap();
        let contents = fs::read_to_string(store.path()).unwrap();
        assert!(contents.contains("schema_version = 1"));
        assert!(contents.contains("target_language = \"ja\""));
    }

    #[test]
    fn saved_settings_round_trip() {
        let (_directory, store) = fixture();
        store.save(&settings("fr")).unwrap();
        assert_eq!(store.load().unwrap(), settings("fr"));
    }

    #[test]
    fn current_schema_is_accepted() {
        let (_directory, store) = fixture();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            "schema_version = 1\n\n[settings]\ntarget_language = \"de\"\n",
        )
        .unwrap();
        assert_eq!(store.load().unwrap(), settings("de"));
    }

    #[test]
    fn future_schema_is_rejected() {
        let (_directory, store) = fixture();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            "schema_version = 99\n\n[settings]\ntarget_language = \"de\"\n",
        )
        .unwrap();
        assert!(store.load().unwrap_err().to_string().contains("future"));
    }

    #[test]
    fn malformed_toml_is_rejected() {
        let (_directory, store) = fixture();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(store.path(), "not = [valid").unwrap();
        assert!(store.load().unwrap_err().to_string().contains("parse"));
    }

    #[test]
    fn second_save_atomically_replaces_the_first() {
        let (_directory, store) = fixture();
        store.save(&settings("en-US")).unwrap();
        store.save(&settings("pt-BR")).unwrap();
        assert_eq!(store.load().unwrap(), settings("pt-BR"));
    }
}
