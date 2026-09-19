mod http;
#[cfg(feature = "mock")]
mod mock;
pub mod providers;
mod registry;

pub use registry::{DeepLTranslatorFactory, ProviderRegistry};
