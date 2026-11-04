//! A bounded, software-only native recording service. UI callers only enqueue work.
mod audio;
mod audio_capture;
mod audio_convert;
#[cfg(test)]
mod audio_fixture;
mod audio_source;
#[cfg(test)]
mod audio_visual_fixture;
mod diagnostics;
mod encoder;
pub(crate) mod preview;
pub(crate) mod save;
#[cfg(test)]
mod tests;
mod trim;
use super::{
    recording_capture::{Capture, qpc_100ns},
    recording_pixels::nv12,
};
use lexift_core::{
    Error, Result,
    domain::{
        recording_clock::Clock,
        screen_recording::{Artifact, Identity, Interruption},
    },
    ports::{annotation_save::SaveOutcome, screen_recording::*},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn err(e: impl std::fmt::Display) -> Error {
    Error::new(e.to_string())
}
type Completion = Box<dyn FnOnce(Result<SaveOutcome>) + Send>;
enum Message {
    Preview(
        lexift_core::domain::recording_preview::Command,
        PreviewListener,
    ),
    Prepare(PrepareRequest, Listener),
    Control(Identity, Control),
    Wake,
    Save(SaveRequest, Arc<AtomicBool>, Completion),
    SaveDone(Result<SaveOutcome>, Completion),
    Shutdown,
    #[cfg(test)]
    ExportForTest(PathBuf, std::sync::mpsc::Sender<Result<PathBuf>>),
}
struct Shared {
    shutdown_callback: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    saving: AtomicBool,
    active: AtomicBool,
    cancel: AtomicBool,
    shutdown: AtomicBool,
    latest: Mutex<Option<Identity>>,
}
pub(crate) struct WindowsScreenRecordingPort {
    sender: SyncSender<Message>,
    shared: Arc<Shared>,
}
impl WindowsScreenRecordingPort {
    pub fn new() -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(16);
        let shared = Arc::new(Shared {
            shutdown_callback: Mutex::new(None),
            saving: AtomicBool::new(false),
            active: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            latest: Mutex::new(None),
        });
        let state = shared.clone();
        let worker_sender = sender.clone();
        thread::Builder::new()
            .name("lexift-video-encoder".into())
            .spawn(move || service(receiver, worker_sender, state))
            .map_err(err)?;
        Ok(Self { sender, shared })
    }
    fn send(&self, message: Message) -> Result<()> {
        self.sender
            .try_send(message)
            .map_err(|_| Error::new("Recording service is busy or unavailable"))
    }
}
impl Drop for WindowsScreenRecordingPort {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.cancel.store(true, Ordering::Release);
        let _ = self.sender.try_send(Message::Shutdown);
    }
}
impl ScreenRecordingPort for WindowsScreenRecordingPort {
    fn preview(
        &self,
        command: lexift_core::domain::recording_preview::Command,
        listener: PreviewListener,
    ) -> Result<()> {
        self.send(Message::Preview(command, listener))
    }
    fn shutdown(&self, completed: Box<dyn FnOnce() + Send>) -> Result<()> {
        let mut callback = self.shared.shutdown_callback.lock().unwrap();
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(Error::new("Recording service is already shutting down"));
        }
        *callback = Some(completed);
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.cancel.store(true, Ordering::Release);
        let _ = self.sender.try_send(Message::Shutdown);
        Ok(())
    }
    fn is_available(&self) -> bool {
        !self.shared.shutdown.load(Ordering::Acquire)
    }
    fn prepare(&self, request: PrepareRequest, listener: Listener) -> Result<()> {
        request.validate()?;
        if self.shared.active.swap(true, Ordering::AcqRel) {
            return Err(Error::new("Previous recording resources have not retired"));
        }
        self.shared.cancel.store(false, Ordering::Release);
        *self.shared.latest.lock().unwrap() = Some(request.identity);
        if let Err(e) = self.send(Message::Prepare(request, listener)) {
            self.shared.active.store(false, Ordering::Release);
            *self.shared.latest.lock().unwrap() = None;
            return Err(e);
        }
        Ok(())
    }
    fn control(&self, id: Identity, control: Control) -> Result<()> {
        if control == Control::Cancel {
            return self.release(id);
        }
        let mut latest = self.shared.latest.lock().unwrap();
        if self.shared.cancel.load(Ordering::Acquire)
            || !latest.is_some_and(|old| {
                old.session == id.session
                    && old.operation == id.operation
                    && id.request > old.request
            })
        {
            return Err(Error::new("Recording request is stale"));
        }
        self.send(Message::Control(id, control))?;
        *latest = Some(id);
        Ok(())
    }
    fn release(&self, id: Identity) -> Result<()> {
        let mut latest = self.shared.latest.lock().unwrap();
        if !latest.is_some_and(|old| {
            old.session == id.session && old.operation == id.operation && id.request >= old.request
        }) {
            return Err(Error::new("Recording release is stale"));
        }
        *latest = Some(id);
        self.shared.cancel.store(true, Ordering::Release);
        // Cancellation is out-of-band. A full queue cannot lose its identity,
        // and no queued release can accidentally retire a replacement task.
        let _ = self.sender.try_send(Message::Wake);
        Ok(())
    }
    fn save(
        &self,
        request: SaveRequest,
        cancel: Arc<AtomicBool>,
        completed: Completion,
    ) -> Result<()> {
        let mut latest = self.shared.latest.lock().unwrap();
        let id = request.identity;
        if self.shared.cancel.load(Ordering::Acquire)
            || !latest.is_some_and(|old| {
                old.session == id.session
                    && old.operation == id.operation
                    && id.request > old.request
            })
        {
            return Err(Error::new("Recording save is stale"));
        }
        if self.shared.saving.swap(true, Ordering::AcqRel) {
            return Err(Error::new("Video save is already pending"));
        }
        if let Err(error) = self.send(Message::Save(request, cancel, completed)) {
            self.shared.saving.store(false, Ordering::Release);
            return Err(error);
        }
        *latest = Some(id);
        Ok(())
    }
}
struct FileArtifact {
    path: PathBuf,
    folder: PathBuf,
}
impl FileArtifact {
    fn new() -> Result<Arc<Self>> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join("Lexift").join("recordings");
        std::fs::create_dir_all(&root).map_err(err)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(err)?
            .as_nanos();
        let folder = root.join(format!(
            "{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&folder).map_err(err)?;
        Ok(Arc::new(Self {
            path: folder.join("capture.partial.mp4"),
            folder,
        }))
    }
}
impl Drop for FileArtifact {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.folder);
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Ready,
    Starting,
    Recording,
    Pausing,
    Paused,
    Resuming,
    Finishing,
    Sealed,
    Failed,
}
struct Job {
    preview: Option<preview::Worker>,
    wake: Option<SyncSender<Message>>,
    audio: audio::Sources,
    audio_configuration: Option<(
        lexift_core::domain::recording_audio::Mode,
        Identity,
        Instant,
    )>,
    diagnostics: diagnostics::Diagnostics,
    encoder: Option<encoder::Encoder>,
    capture: Option<Capture>,
    request: PrepareRequest,
    listener: Listener,
    phase: Phase,
    clock: Option<Clock>,
    origin: Instant,
    fresh: i64,
    frames: u64,
    audio_target: u64,
    audio_wait_failed: bool,
    sequence: u64,
    current: Vec<u8>,
    next: Vec<u8>,
    pixels: Vec<u8>,
    pending: Vec<u8>,
    changed: bool,
    have_frame: bool,
    source_time: i64,
    progress_at: Instant,
    start_deadline: Instant,
    info: Option<ArtifactInfo>,
    interrupted: Option<Interruption>,
    file: Option<Arc<FileArtifact>>,
}
impl Job {
    fn prepare(request: PrepareRequest, listener: Listener) -> Self {
        Self {
            preview: None,
            wake: None,
            audio: audio::Sources::default(),
            audio_configuration: None,
            diagnostics: diagnostics::Diagnostics::default(),
            encoder: None,
            capture: None,
            request,
            listener,
            phase: Phase::Preparing,
            clock: None,
            origin: Instant::now(),
            fresh: 0,
            frames: 0,
            audio_target: 0,
            audio_wait_failed: false,
            sequence: 0,
            current: Vec::new(),
            next: Vec::new(),
            pixels: Vec::new(),
            pending: Vec::new(),
            changed: true,
            have_frame: false,
            source_time: 0,
            progress_at: Instant::now(),
            start_deadline: Instant::now(),
            info: None,
            interrupted: None,
            file: None,
        }
    }
    fn arrival_notice(&self) -> Arc<super::screen_capture::arrival::Notice> {
        let sender = self.wake.clone();
        super::screen_capture::arrival::Notice::new(move || {
            if let Some(sender) = &sender {
                let _ = sender.try_send(Message::Wake);
            }
        })
    }
    fn initialize(&mut self) -> Result<()> {
        self.request.validate()?;
        unsafe {
            for token in &self.request.excluded_windows {
                let mut affinity = 0;
                windows::Win32::UI::WindowsAndMessaging::GetWindowDisplayAffinity(
                    windows::Win32::Foundation::HWND(token.0 as *mut _),
                    &mut affinity,
                )
                .map_err(err)?;
                if affinity != windows::Win32::UI::WindowsAndMessaging::WDA_EXCLUDEFROMCAPTURE.0 {
                    return Err(Error::new("Recording decoration is not capture excluded"));
                }
            }
        }
        let b = self.request.bounds;
        let (w, h) = ((b.right - b.left) as u32, (b.bottom - b.top) as u32);
        encoder::validate(w, h, self.request.video).map_err(err)?;
        self.current = vec![0; w as usize * h as usize * 4];
        self.next = vec![0; self.current.len()];
        self.pixels = vec![0; w as usize * h as usize * 3 / 2];
        self.pending = vec![0; self.pixels.len()];
        self.phase = Phase::Ready;
        (self.listener)(Notice::Prepared(self.request.identity));
        Ok(())
    }
    fn fail(&mut self, error: Error) {
        tracing::warn!(session=self.request.identity.session,operation=self.request.identity.operation,%error,"Recording native operation failed");
        self.phase = Phase::Failed;
        self.capture.take();
        self.audio = audio::Sources::default();
        self.encoder.take();
        self.current = Vec::new();
        self.next = Vec::new();
        self.pixels = Vec::new();
        self.pending = Vec::new();
        self.file.take();
        (self.listener)(Notice::Failed {
            identity: self.request.identity,
            error,
        });
    }
    fn progress(&mut self) {
        if let Some(clock) = &self.clock {
            self.sequence += 1;
            (self.listener)(Notice::Progress(Progress {
                session: self.request.identity.session,
                operation: self.request.identity.operation,
                sequence: self.sequence,
                active_time: clock.active(self.origin.elapsed()),
                submitted_frames: self.frames.saturating_sub(1),
            }));
            self.progress_at = Instant::now();
        }
    }
    fn control(&mut self, id: Identity, action: Control) -> Result<()> {
        if id.session != self.request.identity.session
            || id.operation != self.request.identity.operation
            || id.request < self.request.identity.request
        {
            return Ok(());
        }
        self.request.identity = id;
        if let Some(reason) = self.interrupted {
            if action == Control::Finish {
                self.audio.stop();
                self.capture.take();
                self.phase = Phase::Finishing;
                return Ok(());
            }
            (self.listener)(Notice::Interrupted {
                identity: id,
                reason,
            });
            return Ok(());
        }
        match (self.phase, action) {
            (Phase::Ready, Control::ConfigureVideo(video)) => {
                if self.audio_configuration.is_some() {
                    return Err(Error::new("Recording audio preparation is pending"));
                }
                let b = self.request.bounds;
                let result =
                    encoder::validate((b.right - b.left) as u32, (b.bottom - b.top) as u32, video)
                        .map_err(err)
                        .map(|()| video);
                if result.is_ok() {
                    self.request.video = video;
                }
                (self.listener)(Notice::VideoConfigured {
                    identity: id,
                    result,
                });
            }
            (Phase::Ready, Control::ConfigureAudio(mode)) => {
                if self.audio_configuration.is_some() {
                    return Err(Error::new("Recording audio preparation is pending"));
                }
                match self.audio.prepare(mode) {
                    Ok(()) => self.audio_configuration = Some((mode, id, Instant::now())),
                    Err(error) => {
                        self.audio.retain(self.audio.mode);
                        (self.listener)(Notice::AudioConfigured {
                            devices: self.audio.devices(),
                            identity: id,
                            result: Err(error),
                        });
                    }
                }
            }
            (Phase::Ready, Control::Start) => {
                self.diagnostics.started = Some(Instant::now());
                if self.audio_configuration.is_some() {
                    return Err(Error::new("Recording audio preparation is pending"));
                }
                let b = self.request.bounds;
                let started = (|| -> Result<()> {
                    self.audio.prepared(self.audio.mode)?;
                    self.file = Some(FileArtifact::new()?);
                    self.encoder = Some(
                        encoder::Encoder::new_av(
                            &self.file.as_ref().unwrap().path,
                            (b.right - b.left) as u32,
                            (b.bottom - b.top) as u32,
                            self.audio.mode != lexift_core::domain::recording_audio::Mode::Silent,
                            self.request.video,
                        )
                        .map_err(err)?,
                    );
                    Ok(())
                })();
                if let Err(error) = started {
                    self.encoder.take();
                    self.file.take();
                    (self.listener)(Notice::StartRejected {
                        identity: id,
                        error,
                    });
                    return Ok(());
                }
                self.fresh = qpc_100ns().map_err(err)?;
                let notice = self.arrival_notice();
                self.capture = Some(Capture::new(self.request.bounds, notice).map_err(err)?);
                self.audio.start();
                tracing::debug!(
                    session = id.session,
                    operation = id.operation,
                    gpu_bytes = self.capture.as_ref().unwrap().texture_bytes(),
                    "Recording native resources prepared"
                );
                self.phase = Phase::Starting;
                self.start_deadline = Instant::now() + Duration::from_secs(5);
            }
            (Phase::Recording, Control::Pause) => {
                self.audio.stop();
                self.clock.as_mut().unwrap().pause(self.origin.elapsed());
                self.capture.take();
                self.phase = Phase::Pausing;
            }
            (Phase::Paused, Control::Resume) => {
                self.audio.clear_segment();
                self.fresh = qpc_100ns().map_err(err)?;
                self.capture =
                    Some(Capture::new(self.request.bounds, self.arrival_notice()).map_err(err)?);
                self.audio.start();
                self.phase = Phase::Resuming;
                self.start_deadline = Instant::now() + Duration::from_secs(5);
            }
            (
                Phase::Recording | Phase::Paused | Phase::Resuming | Phase::Starting,
                Control::Finish,
            ) => {
                self.diagnostics.stopping = Some(Instant::now());
                if self.frames == 0 {
                    return Err(Error::new("Recording has no accepted frames"));
                }
                // The final presentation fence precedes this command. Waiting
                // for a fresh WGC tail must not extend the recorded duration.
                self.clock.as_mut().unwrap().stop(self.origin.elapsed());
                self.audio.stop();
                self.capture.take();
                self.fresh = qpc_100ns().map_err(err)?;
                match Capture::new(self.request.bounds, self.arrival_notice()) {
                    Ok(capture) => self.capture = Some(capture),
                    Err(_) => {
                        self.interrupted = Some(Interruption::CaptureUnavailable);
                        self.phase = Phase::Finishing;
                        return Ok(());
                    }
                }
                self.phase = Phase::Finishing;
                self.start_deadline = Instant::now() + Duration::from_secs(5);
            }
            (_, Control::Cancel) => {}
            _ => return Err(Error::new("Recording control is unavailable in this phase")),
        }
        Ok(())
    }
    fn seal(&mut self, interrupted: Option<Interruption>) -> Result<()> {
        self.capture.take();
        self.audio.stop();
        if self.frames == 0 {
            return Err(Error::new("Recording has no accepted frames"));
        }
        let (pts, duration) = self
            .clock
            .as_ref()
            .unwrap()
            .sample(self.frames - 1)
            .unwrap();
        self.submit_pending(pts, duration, true)?;
        let finalize_started = Instant::now();
        self.encoder
            .as_ref()
            .ok_or_else(|| Error::new("Recording encoder is unavailable"))?
            .finish()
            .map_err(err)?;
        self.observe_encoded();
        self.encoder.take();
        let audio_mode = self.audio.mode;
        let audio_devices = self.audio.devices();
        let audio_samples = self.audio.written;
        self.audio = audio::Sources::default();
        let file = Arc::get_mut(self.file.as_mut().unwrap())
            .ok_or_else(|| Error::new("Recording artifact is already borrowed"))?;
        let sealed = file.folder.join("capture.mp4");
        std::fs::rename(&file.path, &sealed).map_err(err)?;
        file.path = sealed;
        self.phase = Phase::Sealed;
        self.current = Vec::new();
        self.next = Vec::new();
        self.pixels = Vec::new();
        self.pending = Vec::new();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let b = self.request.bounds;
        let clock = self.clock.as_ref().unwrap();
        let (pts, duration) = clock.sample(self.frames - 1).unwrap();
        let info = ArtifactInfo {
            video: self.request.video,
            artifact: Artifact(NEXT.fetch_add(1, Ordering::Relaxed)),
            width: (b.right - b.left) as u32,
            height: (b.bottom - b.top) as u32,
            frames: self.frames,
            duration: Duration::from_nanos((pts + duration) as u64 * 100),
            interrupted,
        };
        tracing::info!(
            session = self.request.identity.session,
            operation = self.request.identity.operation,
            video_fps = self.request.video.frame_rate.get(),
            video_quality = ?self.request.video.quality,
            target_bitrate = self.request.video.bitrate(info.width, info.height).ok(),
            ?audio_mode,
            ?audio_devices,
            audio_samples,
            frames = info.frames,
            active_ms = info.duration.as_secs_f64() * 1000.,
            source_frames = self.diagnostics.source_frames,
            content_updates = self.diagnostics.content_updates,
            first_submitted_ms = self
                .diagnostics
                .first_submitted
                .map(|d| d.as_secs_f64() * 1000.),
            stop_to_sealed_ms = self
                .diagnostics
                .stopping
                .map(|s| s.elapsed().as_secs_f64() * 1000.),
            first_encoded_upper_bound_ms = self
                .diagnostics
                .first_encoded
                .map(|d| d.as_secs_f64() * 1000.),
            encoded_statistics_unavailable = self.diagnostics.encoded_statistics_unavailable,
            finalize_ms = finalize_started.elapsed().as_secs_f64() * 1000.,
            read_p95_ms = self.diagnostics.read.p95_ms(),
            delivery_p95_ms = self.diagnostics.delivery.p95_ms(),
            conversion_p95_ms = self.diagnostics.conversion.p95_ms(),
            submit_p95_ms = self.diagnostics.submit.p95_ms(),
            "Recording sealed with bounded native timing aggregates"
        );
        #[cfg(test)]
        eprintln!(
            "native aggregates fps={} read={} convert={} submit={} updates={} sources={}",
            self.request.video.frame_rate.get(),
            self.diagnostics.read.p95_ms(),
            self.diagnostics.conversion.p95_ms(),
            self.diagnostics.submit.p95_ms(),
            self.diagnostics.content_updates,
            self.diagnostics.source_frames
        );
        self.info = Some(info.clone());
        (self.listener)(Notice::Finished {
            identity: self.request.identity,
            artifact: info,
        });
        Ok(())
    }
    fn tick(&mut self) -> Result<()> {
        if let Some((mode, identity, since)) = self.audio_configuration {
            let result = self.audio.prepared(mode).and_then(|ready| {
                if ready {
                    Ok(Some(mode))
                } else if since.elapsed() > Duration::from_secs(5) {
                    tracing::warn!(
                        preparation_ms = since.elapsed().as_millis(),
                        "Recording audio preparation timed out"
                    );
                    Err(Error::new("Recording audio unavailable"))
                } else {
                    Ok(None)
                }
            });
            match result {
                Ok(None) => return Ok(()),
                Ok(Some(mode)) => {
                    self.audio.mode = mode;
                    self.audio.retain(mode);
                    self.audio_configuration = None;
                    (self.listener)(Notice::AudioConfigured {
                        devices: self.audio.devices(),
                        identity,
                        result: Ok(mode),
                    });
                }
                Err(error) => {
                    self.audio.retain(self.audio.mode);
                    self.audio_configuration = None;
                    (self.listener)(Notice::AudioConfigured {
                        devices: self.audio.devices(),
                        identity,
                        result: Err(error),
                    });
                }
            }
        }
        if matches!(
            self.phase,
            Phase::Starting
                | Phase::Recording
                | Phase::Pausing
                | Phase::Paused
                | Phase::Resuming
                | Phase::Finishing
        ) && let Err(error) = self.audio.collect()
        {
            return self.audio_interrupted(error);
        }
        if self.phase == Phase::Pausing {
            let stopped = match self.audio.stopped() {
                Ok(stopped) => stopped,
                Err(error) => return self.audio_interrupted(error),
            };
            if stopped {
                self.audio.collect()?;
                self.write_audio_to(
                    self.frames * 48_000 / u64::from(self.request.video.frame_rate.get()),
                )?;
                self.phase = Phase::Paused;
                self.progress();
                (self.listener)(Notice::Paused(self.request.identity));
            }
            return Ok(());
        }
        if self.phase == Phase::Finishing {
            match self.audio.stopped() {
                Ok(false) => return Ok(()),
                Err(error) => return self.audio_interrupted(error),
                Ok(true) => {
                    // Stop publishes tail PCM before its stopped flag. Drain
                    // again after observing that flag, closing the queue race.
                    if let Err(error) = self.audio.collect() {
                        return self.audio_interrupted(error);
                    }
                }
            }
        }
        if self.phase == Phase::Finishing && self.capture.is_none() {
            return self.seal(self.interrupted);
        }
        if self.phase == Phase::Recording {
            self.write_ready_audio()?;
        }
        if matches!(
            self.phase,
            Phase::Starting | Phase::Resuming | Phase::Recording
        ) {
            match self.audio.running() {
                Ok(false) if self.phase == Phase::Starting || self.phase == Phase::Resuming => {
                    return Ok(());
                }
                Err(error) if self.phase == Phase::Starting => {
                    return self.audio_interrupted(error);
                }
                Err(_) => {
                    self.audio.stop();
                    self.audio.retire_sources();
                    self.capture.take();
                    self.interrupted = Some(Interruption::AudioUnavailable);
                    if let Some(clock) = &mut self.clock {
                        clock.stop(self.origin.elapsed());
                    }
                    self.phase = Phase::Paused;
                    (self.listener)(Notice::Interrupted {
                        identity: self.request.identity,
                        reason: Interruption::AudioUnavailable,
                    });
                    return Ok(());
                }
                _ => {}
            }
        }
        if !matches!(
            self.phase,
            Phase::Starting | Phase::Recording | Phase::Resuming | Phase::Finishing
        ) {
            return Ok(());
        }
        if matches!(self.phase, Phase::Starting | Phase::Resuming) && !self.audio.media_ready() {
            if Instant::now() >= self.start_deadline {
                return self.audio_interrupted(Error::new(
                    "Recording microphone has no valid startup samples",
                ));
            }
            return Ok(());
        }
        let capture = self
            .capture
            .as_ref()
            .ok_or_else(|| Error::new("Recording capture is unavailable"))?;
        match capture.read(&mut self.next, self.fresh) {
            Ok(Some(timing)) => {
                if timing.source_100ns < self.source_time {
                    return Err(Error::new("Recording source clock moved backwards"));
                }
                self.source_time = timing.source_100ns;
                self.diagnostics.source_frames += 1;
                self.diagnostics.delivery.record(timing.received.elapsed());
                self.diagnostics
                    .read
                    .record(Duration::from_secs_f64(timing.read_ms / 1000.));
                self.have_frame = true;
                if self.current != self.next {
                    std::mem::swap(&mut self.current, &mut self.next);
                    self.changed = true;
                }
                if self.phase == Phase::Finishing {
                    let now = self.origin.elapsed();
                    self.clock.as_mut().unwrap().stop(now);
                    let target = self.clock.as_ref().unwrap().frames(now).max(self.frames);
                    // Repeat the previously accepted pixels for late slots, then
                    // encode the freshly presented content exactly at the tail.
                    while self.frames < target {
                        self.write_pixels()?;
                    }
                    self.convert_current();
                    self.pending.copy_from_slice(&self.pixels);
                    return self.seal(None);
                }
                if self.phase == Phase::Starting {
                    // Media time zero is the WGC sample's QPC, not the later
                    // moment GPU readback/initialization happened to finish.
                    let offset = lexift_core::domain::recording_clock::capture_offset(
                        qpc_100ns().map_err(err)?,
                        timing.source_100ns,
                        self.request.video.frame_rate.get(),
                    )
                    .ok_or_else(|| Error::new("Invalid recording capture timestamp"))?;
                    let observed = Instant::now();
                    self.origin = if offset >= 0 {
                        observed.checked_sub(Duration::from_nanos(offset as u64))
                    } else {
                        observed.checked_add(Duration::from_nanos(offset.unsigned_abs()))
                    }
                    .ok_or_else(|| Error::new("Recording capture timestamp overflow"))?;
                    self.clock = Clock::new(self.request.video.frame_rate.get(), Duration::ZERO);
                    self.audio.segment(timing.source_100ns, 0);
                    self.phase = Phase::Recording;
                    (self.listener)(Notice::Started(self.request.identity));
                } else if self.phase == Phase::Resuming {
                    let offset = lexift_core::domain::recording_clock::capture_offset(
                        qpc_100ns().map_err(err)?,
                        timing.source_100ns,
                        self.request.video.frame_rate.get(),
                    )
                    .ok_or_else(|| Error::new("Invalid recording capture timestamp"))?;
                    let now = self.origin.elapsed();
                    let at = if offset >= 0 {
                        now.checked_sub(Duration::from_nanos(offset as u64))
                    } else {
                        now.checked_add(Duration::from_nanos(offset.unsigned_abs()))
                    }
                    .ok_or_else(|| Error::new("Recording capture predates media origin"))?;
                    self.clock.as_mut().unwrap().resume(at);
                    let sample = self.clock.as_ref().unwrap().audio_sample(at)?;
                    self.audio.segment(timing.source_100ns, sample);
                    self.phase = Phase::Recording;
                    (self.listener)(Notice::Resumed(self.request.identity));
                }
            }
            Ok(None) => {}
            Err(e) => {
                let reason = if e.message().contains("layout") {
                    Interruption::LayoutChanged
                } else {
                    Interruption::CaptureUnavailable
                };
                let finishing = self.phase == Phase::Finishing;
                self.capture.take();
                self.interrupted = Some(reason);
                if let Some(clock) = self.clock.as_mut() {
                    clock.stop(self.origin.elapsed());
                }
                if finishing {
                    return self.seal(Some(reason));
                }
                (self.listener)(Notice::Interrupted {
                    identity: self.request.identity,
                    reason,
                });
                // Core issues Finish with its new request identity; no more source frames are accepted.
                self.phase = Phase::Paused;
                return Ok(());
            }
        }
        if self.phase == Phase::Finishing && Instant::now() >= self.start_deadline {
            return self.seal(Some(Interruption::CaptureUnavailable));
        }
        if matches!(self.phase, Phase::Starting | Phase::Resuming) {
            if Instant::now() >= self.start_deadline {
                return Err(Error::new("Fresh recording frame timed out"));
            }
            return Ok(());
        }
        if !self.have_frame {
            return Ok(());
        }
        let clock = self.clock.as_ref().unwrap();
        let now = self.origin.elapsed();
        if clock.backlog_exceeded(now, self.frames) {
            tracing::warn!(
                session = self.request.identity.session,
                operation = self.request.identity.operation,
                "Recording encoding backlog exceeded one second"
            );
            self.capture.take();
            self.audio.stop();
            self.clock.as_mut().unwrap().stop(now);
            self.interrupted = Some(Interruption::CaptureUnavailable);
            self.phase = Phase::Paused;
            (self.listener)(Notice::Interrupted {
                identity: self.request.identity,
                reason: Interruption::CaptureUnavailable,
            });
            return Ok(());
        }
        // Keep the existing single tail slot available for a fresh arrival.
        // Sampling an unchanged source immediately at every CFR boundary can
        // alias two independent 60Hz clocks into duplicate/dropped pairs. A
        // static/late source still advances at the next absolute frame deadline.
        if clock.frame_ready(now, self.frames, self.changed) {
            if self.phase != Phase::Finishing {
                self.convert_current();
            }
            self.write_pixels()?;
        }
        if self.progress_at.elapsed() >= Duration::from_millis(250) {
            self.progress();
        }
        Ok(())
    }
    fn convert_current(&mut self) {
        if self.changed {
            let started = Instant::now();
            let b = self.request.bounds;
            nv12(
                &self.current,
                (b.right - b.left) as usize,
                (b.bottom - b.top) as usize,
                &mut self.pixels,
            );
            self.diagnostics.conversion.record(started.elapsed());
            self.diagnostics.content_updates += 1;
            self.changed = false;
        }
    }
    fn write_pixels(&mut self) -> Result<()> {
        // Keep one unsubmitted slot so Finish can install the final presented
        // annotation frame without adding pause time or rewriting encoded samples.
        if self.frames > 0 {
            let (pts, duration) = self
                .clock
                .as_ref()
                .unwrap()
                .sample(self.frames - 1)
                .ok_or_else(|| Error::new("Recording timeline overflow"))?;
            self.submit_pending(pts, duration, false)?;
        }
        self.pending.copy_from_slice(&self.pixels);
        self.frames += 1;
        Ok(())
    }
    fn submit_pending(&mut self, pts: i64, duration: i64, final_audio: bool) -> Result<()> {
        let end = ((pts + duration) as u128 * 48_000).div_ceil(10_000_000) as u64;
        self.audio_target = end;
        if final_audio {
            // The stop/drain fence has completed. Apply the explicit tail
            // padding rule without re-entering the running backlog guard.
            self.write_audio_to(end)?;
        } else {
            self.write_ready_audio()?;
        }
        let started = Instant::now();
        self.encoder
            .as_ref()
            .unwrap()
            .write(&self.pending, pts, duration)
            .map_err(err)?;
        self.diagnostics.submit.record(started.elapsed());
        if self.diagnostics.first_submitted.is_none() {
            self.diagnostics.first_submitted = self.diagnostics.started.map(|s| s.elapsed());
        }
        self.observe_encoded();
        Ok(())
    }
    fn write_audio_to(&mut self, target: u64) -> Result<()> {
        if self.audio.mode == lexift_core::domain::recording_audio::Mode::Silent {
            return Ok(());
        }
        // The tick/stop boundary collects once. Rendering never calls back into
        // a source that could fail midway through submitting an accepted frame.
        while self.audio.written < target {
            let count = (target - self.audio.written).min(480) as usize;
            let samples = self.audio.render(count)?;
            self.encoder
                .as_ref()
                .unwrap()
                .write_audio(&samples, self.audio.written)
                .map_err(err)?;
            self.audio.written += count as u64;
        }
        Ok(())
    }
    fn write_ready_audio(&mut self) -> Result<()> {
        if self.audio.mode == lexift_core::domain::recording_audio::Mode::Silent {
            return Ok(());
        }
        if self.audio_target.saturating_sub(self.audio.written) > 24_000 {
            self.audio_wait_failed = true;
            return Err(Error::new("Recording audio backlog exceeded 500ms"));
        }
        let now = qpc_100ns().map_err(err)?;
        while self.audio.written < self.audio_target {
            let requested = (self.audio_target - self.audio.written).min(480) as usize;
            let count = self.audio.ready_count(requested, now);
            if count == 0 {
                break;
            }
            self.write_audio_to(self.audio.written + count as u64)?;
        }
        Ok(())
    }
    fn audio_interrupted(&mut self, error: Error) -> Result<()> {
        tracing::warn!(session=self.request.identity.session, operation=self.request.identity.operation, %error, "Recording audio interrupted");
        self.audio.stop();
        self.audio.retire_sources();
        self.capture.take();
        if self.phase == Phase::Starting {
            // A rejected start must not let a later retry reuse validated PCM
            // from the retired endpoint as its new readiness acknowledgement.
            self.audio.clear_segment();
            self.encoder.take();
            self.file.take();
            self.phase = Phase::Ready;
            let message = error.to_string();
            let error = if [
                "Recording audio unavailable",
                "Recording audio permission denied",
                "Recording audio device changed",
                "Recording audio service unavailable",
                "Recording audio format unsupported",
                "Recording audio queue is full",
                "Recording audio timing failed",
            ]
            .contains(&message.as_str())
            {
                error
            } else if message.contains("500ms") {
                Error::new("Recording audio queue is full")
            } else {
                Error::new("Recording audio timing failed")
            };
            (self.listener)(Notice::StartRejected {
                identity: self.request.identity,
                error,
            });
            return Ok(());
        }
        if let Some(clock) = &mut self.clock {
            clock.stop(self.origin.elapsed());
        }
        if self.phase == Phase::Finishing {
            return self.seal(Some(Interruption::AudioUnavailable));
        }
        self.interrupted = Some(Interruption::AudioUnavailable);
        self.phase = Phase::Paused;
        (self.listener)(Notice::Interrupted {
            identity: self.request.identity,
            reason: Interruption::AudioUnavailable,
        });
        Ok(())
    }
    fn observe_encoded(&mut self) {
        if self.diagnostics.first_encoded.is_some()
            || self.diagnostics.encoded_statistics_unavailable
        {
            return;
        }
        // Observe at existing sample/finalization boundaries, without another
        // polling loop. Optional statistics cannot fail a working recording.
        match self.encoder.as_ref().unwrap().encoded_samples() {
            Ok(0) => {}
            Ok(_) => self.diagnostics.first_encoded = self.diagnostics.started.map(|s| s.elapsed()),
            Err(_) => self.diagnostics.encoded_statistics_unavailable = true,
        }
    }
}
fn service(receiver: Receiver<Message>, sender: SyncSender<Message>, shared: Arc<Shared>) {
    let apartment = super::screen_capture::capture::Apartment::new();
    let media = encoder::Media::new();
    let startup = apartment
        .as_ref()
        .err()
        .map(ToString::to_string)
        .or_else(|| media.as_ref().err().map(ToString::to_string));
    let mut job: Option<Job> = None;
    let mut saving: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)> = None;
    let mut release: Option<Identity> = None;
    let mut quitting = false;
    loop {
        let ticking = job.as_ref().is_some_and(|j| {
            j.audio_configuration.is_some()
                || matches!(
                    j.phase,
                    Phase::Starting
                        | Phase::Recording
                        | Phase::Pausing
                        | Phase::Resuming
                        | Phase::Finishing
                )
        });
        match receiver.recv_timeout(Duration::from_millis(if ticking { 5 } else { 250 })) {
            Ok(Message::Prepare(request, listener)) => {
                let mut j = Job::prepare(request, listener);
                j.wake = Some(sender.clone());
                if let Some(e) = &startup {
                    j.fail(err(e));
                } else if !shared.cancel.load(Ordering::Acquire)
                    && let Err(e) = j.initialize()
                {
                    j.fail(e);
                }
                job = Some(j);
            }
            Ok(Message::Control(id, action)) => {
                if action == Control::Cancel {
                    release = Some(id);
                } else if !shared.cancel.load(Ordering::Acquire)
                    && let Some(j) = job.as_mut()
                    && let Err(e) = j.control(id, action)
                {
                    j.fail(e);
                }
            }
            Ok(Message::Preview(command, listener)) => {
                use lexift_core::domain::recording_preview::{Action, Notice};
                if let Some(j) = job.as_mut()
                    && j.phase == Phase::Sealed
                    && j.info
                        .as_ref()
                        .is_some_and(|i| i.artifact == command.identity.artifact)
                    && j.request.identity.session == command.identity.session
                    && j.request.identity.operation == command.identity.operation
                {
                    let invalid_range = matches!(command.action,Action::Open{range,fps,..}|Action::SetRange{range,fps,..} if !range.valid(j.frames) || fps!=j.request.video.frame_rate.get());
                    let result = if invalid_range {
                        Err(Error::new("Invalid recording preview range"))
                    } else if let Action::Open {
                        target,
                        range,
                        fps,
                        version: _,
                    } = command.action
                    {
                        j.preview.take();
                        preview::Worker::open(
                            command.identity,
                            target,
                            j.file.as_ref().unwrap().clone(),
                            Duration::from_secs_f64(
                                j.frames.saturating_sub(1) as f64
                                    / f64::from(j.request.video.frame_rate.get()),
                            ),
                            Some((range, fps)),
                            listener.clone(),
                        )
                        .map(|w| j.preview = Some(w))
                    } else if let Some(w) = j.preview.as_ref() {
                        w.control(command)
                    } else if command.action == Action::Close {
                        listener(command.identity, Notice::Retired);
                        Ok(())
                    } else {
                        Err(Error::new("Recording preview unavailable"))
                    };
                    if let Err(error) = result {
                        listener(command.identity, Notice::Failed(error.to_string()));
                    }
                } else {
                    listener(
                        command.identity,
                        Notice::Failed("Recording preview unavailable".into()),
                    );
                }
            }
            Ok(Message::Wake) => {}
            Ok(Message::Save(request, cancel, callback)) => {
                if saving.is_some() {
                    callback(Err(Error::new("Video save is already pending")));
                    continue;
                }
                if let Some(j) = job.as_mut()
                    && j.phase == Phase::Sealed
                    && j.info
                        .as_ref()
                        .is_some_and(|a| a.artifact == request.artifact)
                    && request.identity.session == j.request.identity.session
                    && request.identity.operation == j.request.identity.operation
                {
                    j.request.identity = request.identity;
                    let file = j.file.as_ref().unwrap().clone();
                    let tx = sender.clone();
                    let token = cancel.clone();
                    let info = j.info.as_ref().unwrap().clone();
                    let listener = j.listener.clone();
                    let completion = Arc::new(Mutex::new(Some(callback)));
                    let owned = completion.clone();
                    match thread::Builder::new()
                        .name("lexift-recording-export".into())
                        .spawn(move || {
                            let callback = owned.lock().unwrap().take().unwrap();
                            let result = save::save(&file.path, request, token, &info, listener);
                            drop(file);
                            let _ = tx.send(Message::SaveDone(result, callback));
                        }) {
                        Ok(handle) => saving = Some((cancel, handle)),
                        Err(error) => {
                            shared.saving.store(false, Ordering::Release);
                            let callback = completion.lock().unwrap().take();
                            if let Some(callback) = callback {
                                callback(Err(err(error)));
                            }
                        }
                    }
                } else {
                    shared.saving.store(false, Ordering::Release);
                    callback(Err(Error::new("Video artifact is unavailable")));
                }
            }
            Ok(Message::SaveDone(result, callback)) => {
                if let Some((_, handle)) = saving.take() {
                    let _ = handle.join();
                }
                shared.saving.store(false, Ordering::Release);
                callback(result);
            }
            #[cfg(test)]
            Ok(Message::ExportForTest(destination, reply)) => {
                let result = if let Some(j) = &job
                    && j.phase == Phase::Sealed
                {
                    let file = j.file.as_ref().unwrap();
                    save::publish(&file.path, &destination, &AtomicBool::new(false))
                        .map(|_| file.path.clone())
                } else {
                    Err(Error::new("No sealed test artifact"))
                };
                let _ = reply.send(result);
            }
            Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                quitting = true;
                if let Some(j) = &job {
                    release = Some(
                        *shared
                            .latest
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap_or(&j.request.identity),
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if shared.shutdown.load(Ordering::Acquire) {
            quitting = true;
        }
        if shared.cancel.load(Ordering::Acquire)
            && job.is_some()
            && release.is_none()
            && let Some(id) = *shared.latest.lock().unwrap()
        {
            release = Some(id);
        }
        if let Some(id) = release {
            if let Some((cancel, _)) = &saving {
                cancel.store(true, Ordering::Release);
            } else {
                let listener = job.as_ref().map(|j| j.listener.clone());
                drop(job.take());
                release = None;
                *shared.latest.lock().unwrap() = None;
                shared.active.store(false, Ordering::Release);
                if let Some(listener) = listener {
                    listener(Notice::Retired(id));
                }
            }
        } else if let Some(j) = job.as_mut()
            && let Err(e) = j.tick()
        {
            if std::mem::take(&mut j.audio_wait_failed) {
                if let Err(error) = j.audio_interrupted(e) {
                    j.fail(error);
                }
            } else {
                j.fail(e);
            }
        }
        if quitting && job.is_none() && saving.is_none() && !shared.active.load(Ordering::Acquire) {
            break;
        }
    }
    drop(media);
    drop(apartment);
    let callback = shared.shutdown_callback.lock().unwrap().take();
    if let Some(callback) = callback {
        callback();
    }
}
