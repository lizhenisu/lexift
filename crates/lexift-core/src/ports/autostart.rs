use crate::Result;

/// Controls whether Lexift launches for the current user at sign-in.
pub trait AutostartPort: Send + Sync {
    fn set_enabled(&self, enabled: bool) -> Result<()>;
    fn is_enabled(&self) -> Result<bool>;
}
