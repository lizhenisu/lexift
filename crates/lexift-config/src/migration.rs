use lexift_core::{Error, Result};

use crate::schema::ConfigFile;

pub(crate) const CURRENT_SCHEMA_VERSION: u32 = 2;

pub(crate) fn migrate(mut config: ConfigFile) -> Result<ConfigFile> {
    match config.schema_version {
        CURRENT_SCHEMA_VERSION => Ok(config),
        1 => {
            config.schema_version = CURRENT_SCHEMA_VERSION;
            Ok(config)
        }
        version if version > CURRENT_SCHEMA_VERSION => Err(Error::new(format!(
            "Unsupported future settings schema version {version}"
        ))),
        version => Err(Error::new(format!(
            "Unsupported older settings schema version {version}"
        ))),
    }
}
