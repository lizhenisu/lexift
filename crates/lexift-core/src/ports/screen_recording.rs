//! Asynchronous silent-video capability. Native objects and artifact paths stay
//! in the adapter; view preparation and annotation rendering are separate bridges.
use crate::{
    Result,
    domain::{
        geometry::Rect,
        screen_recording::{Artifact, Identity, Interruption, capture_bytes},
    },
    ports::{annotation_save::SaveOutcome, magnifier::WindowToken},
};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareRequest {
    pub identity: Identity,
    pub bounds: Rect,
    pub monitor_bounds: Rect,
    pub excluded_windows: Vec<WindowToken>,
}
impl PrepareRequest {
    pub fn validate(&self) -> Result<()> {
        capture_bytes(self.bounds)?;
        let m = self.monitor_bounds;
        let b = self.bounds;
        if b.left < m.left || b.top < m.top || b.right > m.right || b.bottom > m.bottom {
            return Err(crate::Error::new(
                "Recording region left its owning monitor",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub session: u64,
    pub operation: u64,
    pub sequence: u64,
    pub active_time: Duration,
    pub submitted_frames: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactInfo {
    pub artifact: Artifact,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub duration: Duration,
    pub interrupted: Option<Interruption>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Prepared(Identity),
    Started(Identity),
    Paused(Identity),
    Resumed(Identity),
    Progress(Progress),
    Interrupted {
        identity: Identity,
        reason: Interruption,
    },
    Finished {
        identity: Identity,
        artifact: ArtifactInfo,
    },
    Failed {
        identity: Identity,
        error: crate::Error,
    },
    Retired(Identity),
}
pub type Listener = Arc<dyn Fn(Notice) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Start,
    Pause,
    Resume,
    Finish,
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub identity: Identity,
    pub artifact: Artifact,
    pub owner: Option<WindowToken>,
    pub default_name: String,
    pub title: String,
    pub extension_mismatch_message: String,
}

/// Implementations own a bounded native service. Scheduling never waits for
/// capture, GPU readback, encoding, dialogs or finalization on the caller thread.
/// Notices for one task are ordered, with monotonically increasing progress
/// sequence numbers. Pause/resume notices follow the corresponding clock change;
/// completion means the container is finalized and all capture resources retired.
pub trait ScreenRecordingPort: Send + Sync {
    /// Retire the native service and invoke completion after apartment-owned
    /// resources are released. This does not block the calling/UI thread.
    fn shutdown(&self, completed: Box<dyn FnOnce() + Send>) -> Result<()>;
    fn is_available(&self) -> bool;
    /// Validate media types and reserve resources without starting desktop capture.
    fn prepare(&self, request: PrepareRequest, listener: Listener) -> Result<()>;
    fn control(&self, identity: Identity, control: Control) -> Result<()>;
    /// Releasing a task invalidates its artifact and retires native resources.
    /// The adapter emits Retired only after ownership is actually released.
    fn release(&self, identity: Identity) -> Result<()>;
    /// The sealed artifact survives cancellation/failure of a save attempt.
    fn save(
        &self,
        request: SaveRequest,
        cancelled: Arc<AtomicBool>,
        completed: Box<dyn FnOnce(Result<SaveOutcome>) + Send>,
    ) -> Result<()>;
}
