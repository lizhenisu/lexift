use std::path::PathBuf;

use crate::AppConfig;

pub(crate) fn load(_path: PathBuf) -> lexift_core::Result<AppConfig> {
    Ok(AppConfig::default())
}
