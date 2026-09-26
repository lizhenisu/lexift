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
        config.into_settings()
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
            ..Settings::default()
        }
    }

    #[test]
    fn themes_round_trip_and_legacy_config_defaults_to_light() {
        use lexift_core::domain::settings::ThemePreference;
        let (_directory, store) = fixture();
        for theme in [
            ThemePreference::Light,
            ThemePreference::Dark,
            ThemePreference::System,
        ] {
            let value = Settings {
                theme,
                ..Settings::default()
            };
            store.save(&value).unwrap();
            assert_eq!(store.load().unwrap(), value);
        }
        let saved = fs::read_to_string(store.path()).unwrap();
        let legacy = saved
            .lines()
            .filter(|line| !line.starts_with("theme ="))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(store.path(), legacy).unwrap();
        assert_eq!(store.load().unwrap().theme, ThemePreference::Light);
        fs::write(
            store.path(),
            saved.replace("theme = \"system\"", "theme = \"invalid\""),
        )
        .unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn missing_file_uses_defaults_without_creating_it() {
        let (_directory, store) = fixture();
        assert_eq!(store.load().unwrap(), Settings::default());
        assert!(!store.path().exists());
    }

    #[test]
    fn save_creates_parent_and_v4_file() {
        let (_directory, store) = fixture();
        store.save(&settings("ja")).unwrap();
        let contents = fs::read_to_string(store.path()).unwrap();
        assert!(contents.contains("schema_version = 4"));
        assert!(contents.contains("target_language = \"ja\""));
        assert!(contents.contains("provider = \"deepl\""));
        assert!(contents.contains("key = \"X\""));
        assert!(contents.contains("launch_at_login = false"));
        assert!(!contents.contains("deepl-primary"));
    }

    #[test]
    fn saved_settings_round_trip() {
        let (_directory, store) = fixture();
        let mut configured = settings("fr");
        configured.selection_toolbar = false;
        store.save(&configured).unwrap();
        assert_eq!(store.load().unwrap(), configured);
    }

    #[test]
    fn runtime_configuration_round_trips() {
        let (_directory, store) = fixture();
        let settings = Settings {
            hotkey: "Ctrl + Shift + F12".parse().unwrap(),
            ..settings("fr")
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load().unwrap(), settings);
    }

    #[test]
    fn v1_schema_is_migrated_without_a_credential_reference() {
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
    fn v2_schema_adds_runtime_configuration_defaults() {
        let (_directory, store) = fixture();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            "schema_version = 2\n\n[settings]\ntarget_language = \"de\"\n",
        )
        .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.target_language.0, "de");
        assert_eq!(loaded.hotkey.to_string(), "Alt + X");
        assert_eq!(loaded.provider.to_string(), "DeepL");
        assert!(!loaded.launch_at_login);
        assert!(loaded.selection_toolbar);
    }

    #[test]
    fn v3_schema_adds_launch_at_login_default() {
        let (_directory, store) = fixture();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            "schema_version = 3\n\n[settings]\ntarget_language = \"de\"\nprovider = \"deepl\"\n",
        )
        .unwrap();
        let loaded = store.load().unwrap();
        assert!(!loaded.launch_at_login);
    }

    #[test]
    fn credential_reference_round_trips_without_a_secret() {
        let (_directory, store) = fixture();
        let settings = Settings {
            target_language: Language("ja".into()),
            deepl_credential_id: Some("deepl-primary".into()),
            ..Settings::default()
        };
        store.save(&settings).unwrap();
        let contents = fs::read_to_string(store.path()).unwrap();
        assert!(contents.contains("schema_version = 4"));
        assert!(contents.contains("[credentials]"));
        assert!(contents.contains("deepl = \"deepl-primary\""));
        assert!(!contents.contains("api_key"));
        assert!(!contents.contains("auth_key"));
        assert_eq!(store.load().unwrap(), settings);
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
