//! Asynchronous video and optional audio capability. Native objects and paths stay
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
    pub video: crate::domain::screen_recording::Video,
    pub identity: Identity,
    pub bounds: Rect,
    pub monitor_bounds: Rect,
    pub excluded_windows: Vec<WindowToken>,
}
impl PrepareRequest {
    pub fn validate(&self) -> Result<()> {
        capture_bytes(self.bounds)?;
        self.video.bitrate(
            (self.bounds.right - self.bounds.left) as u32,
            (self.bounds.bottom - self.bounds.top) as u32,
        )?;
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
    pub video: crate::domain::screen_recording::Video,
    pub artifact: Artifact,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub duration: Duration,
    pub interrupted: Option<Interruption>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExportStage {
    AnalyzingColors,
    Transcoding,
    EncodingGif,
    Finalizing,
    Publishing,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    ExportProgress {
        identity: Identity,
        range_version: u64,
        stage: ExportStage,
        completed: u64,
        total: u64,
    },
    StartRejected {
        identity: Identity,
        error: crate::Error,
    },
    VideoConfigured {
        identity: Identity,
        result: crate::Result<crate::domain::screen_recording::Video>,
    },
    AudioConfigured {
        identity: Identity,
        result: crate::Result<crate::domain::recording_audio::Mode>,
        devices: crate::domain::recording_audio::Devices,
    },
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
    ConfigureVideo(crate::domain::screen_recording::Video),
    ConfigureAudio(crate::domain::recording_audio::Mode),
    Start,
    Pause,
    Resume,
    Finish,
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub formats: Vec<crate::domain::recording_gif::Format>,
    pub default_format: crate::domain::recording_gif::Format,
    pub gif_plan: std::result::Result<
        crate::domain::recording_gif::Plan,
        crate::domain::recording_gif::Rejection,
    >,
    pub gif_unavailable_message: String,
    pub range: Option<crate::domain::recording_trim::Range>,
    pub range_version: u64,
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
    /// Playback commands have a separate generation from capture/save commands.
    fn preview(
        &self,
        command: crate::domain::recording_preview::Command,
        listener: PreviewListener,
    ) -> Result<()>;
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
pub type PreviewListener = Arc<
    dyn Fn(crate::domain::recording_preview::Identity, crate::domain::recording_preview::Notice)
        + Send
        + Sync,
>;
