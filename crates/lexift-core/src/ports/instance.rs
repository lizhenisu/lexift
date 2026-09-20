use std::sync::Arc;

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Primary,
    AlreadyRunning,
}

pub type InstanceActivationHandler = Arc<dyn Fn() + Send + Sync + 'static>;

/// Owns the process-wide instance gate and forwards second-launch activation.
pub trait InstancePort: Send + Sync {
    fn acquire(&self) -> Result<InstanceStatus>;
    fn set_activation_handler(&self, handler: InstanceActivationHandler) -> Result<()>;
}
