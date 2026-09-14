mod migration;
mod paths;
mod schema;
mod store;

pub use schema::AppConfig;

pub fn load() -> lexift_core::Result<AppConfig> {
    store::load(paths::default_config_path())
}
