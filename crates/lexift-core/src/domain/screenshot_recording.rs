//! Screenshot ownership around the recording lifecycle. Pixels and native handles
//! remain in adapters; cancellation waits for actual retirement before restoration.
use super::{ScreenshotCommand, ScreenshotPhase, ScreenshotState};
use crate::domain::recording_preview as preview;
use crate::domain::{
    annotation_export::ExportRequest, geometry::Rect, screen_recording as lifecycle,
};
use crate::ports::{
    magnifier::WindowToken,
    screen_recording::{ArtifactInfo, PrepareRequest, Progress},
};
use crate::{AppCommand, Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub preview: Option<preview::State>,
    pub export_progress: Option<(u64, u64)>,
    pub export_stage: Option<crate::ports::screen_recording::ExportStage>,
    close_after_export: bool,
    pending_release: Option<lifecycle::Command>,
    pub lifecycle: lifecycle::State,
    pub request: ExportRequest,
    pub monitor: Rect,
    pub excluded: Vec<WindowToken>,
    pub progress: Option<Progress>,
    pub artifact: Option<ArtifactInfo>,
    pub backend_request: Option<lifecycle::Identity>,
}
impl Session {
    /// Start ownership without leaking playback retirement internals to adapters.
    pub fn new(lifecycle: lifecycle::State, request: ExportRequest, monitor: Rect) -> Self {
        Self {
            preview: None,
            export_progress: None,
            export_stage: None,
            close_after_export: false,
            pending_release: None,
            lifecycle,
            request,
            monitor,
            excluded: vec![],
            progress: None,
            artifact: None,
            backend_request: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    CancelExport {
        identity: preview::Identity,
        close: bool,
    },
    PreviewRequested,
    PreviewInput {
        identity: preview::Identity,
        input: preview::Input,
    },
    PreviewNotice {
        identity: preview::Identity,
        notice: preview::Notice,
    },
    CountdownFailed {
        identity: lifecycle::Identity,
        error: Error,
    },
    Requested(ExportRequest),
    Native(crate::ports::screen_recording::Notice),
    Save(crate::ports::screen_recording::SaveRequest),
    Saved {
        identity: lifecycle::Identity,
        result: crate::Result<crate::ports::annotation_save::SaveOutcome>,
    },
    ViewReady {
        identity: lifecycle::Identity,
        result: Result<Vec<WindowToken>>,
    },
    Input {
        identity: lifecycle::Identity,
        event: lifecycle::Event,
    },
    Progress(Progress),
    Finalized {
        identity: lifecycle::Identity,
        artifact: ArtifactInfo,
    },
    Failed {
        identity: lifecycle::Identity,
        error: Error,
    },
    LayoutChanged {
        session: u64,
        operation: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    CancelExport(lifecycle::Identity),
    Preview(preview::Command),
    PrepareView {
        identity: lifecycle::Identity,
        bounds: Rect,
        request: ExportRequest,
    },
    PrepareBackend(PrepareRequest),
    Execute(lifecycle::Command),
    Save(crate::ports::screen_recording::SaveRequest),
}
fn wrap(command: Command) -> Vec<AppCommand> {
    vec![AppCommand::Screenshot(ScreenshotCommand::Recording(
        command,
    ))]
}
pub(super) fn reduce(s: &mut ScreenshotState, event: Event) -> Vec<AppCommand> {
    if let Event::Requested(request) = event {
        if !s.editable()
            || s.long.is_some()
            || s.recording.is_some()
            || request.session != s.session
            || request.request <= s.operation
            || s.bounds != Some(request.snapshot.monitor)
        {
            return vec![];
        }
        let bounds = request.snapshot.monitor;
        let Some(monitor) = s.layout.iter().copied().find(|m| {
            bounds.left >= m.left
                && bounds.top >= m.top
                && bounds.right <= m.right
                && bounds.bottom <= m.bottom
        }) else {
            return vec![];
        };
        let lifecycle = match lifecycle::State::new(s.session, request.request, bounds, monitor) {
            Ok(lifecycle) => lifecycle,
            Err(error) => {
                s.error = Some(error.to_string());
                return vec![];
            }
        };
        let command = Command::PrepareView {
            identity: lifecycle.identity,
            bounds: lifecycle.source,
            request: request.clone(),
        };
        s.operation = request.request;
        s.phase = ScreenshotPhase::Recording;
        s.color_pending = None;
        s.color_result = None;
        s.error = None;
        s.recording = Some(Session::new(lifecycle, request, monitor));
        return wrap(command);
    }
    let Some(recording) = s
        .recording
        .as_mut()
        .filter(|_| s.phase == ScreenshotPhase::Recording)
    else {
        return vec![];
    };
    match &event {
        Event::CancelExport { identity, close }
            if recording.lifecycle.phase == lifecycle::Phase::Saving
                && recording
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.identity == *identity) =>
        {
            recording.close_after_export |= *close;
            return wrap(Command::CancelExport(recording.lifecycle.identity));
        }

        Event::PreviewRequested if recording.lifecycle.phase == lifecycle::Phase::PendingSave => {
            if let Some(p) = recording.preview.as_mut() {
                return p
                    .input(p.identity, preview::Input::Retry)
                    .map(|c| wrap(Command::Preview(c)))
                    .unwrap_or_default();
            }
            if let Some(artifact) = recording.lifecycle.artifact {
                let mut p = preview::State::new(recording.lifecycle.identity, artifact);
                if let Some(info) = &recording.artifact {
                    p.configure(info.frames, info.video.frame_rate.get());
                }
                let command = p.command(preview::Action::PrepareView);
                recording.preview = Some(p);
                return wrap(Command::Preview(command));
            }
            return vec![];
        }
        Event::PreviewInput { identity, input } => {
            if !matches!(recording.lifecycle.phase, lifecycle::Phase::PendingSave) {
                return vec![];
            }
            let Some(p) = recording.preview.as_mut() else {
                return vec![];
            };
            let Some(command) = p.input(*identity, input.clone()) else {
                return vec![];
            };
            if command.action == preview::Action::Discard {
                let identity = recording.lifecycle.identity;
                return reduce(
                    s,
                    Event::Input {
                        identity,
                        event: lifecycle::Event::DiscardCompleted,
                    },
                );
            }
            return wrap(Command::Preview(command));
        }
        Event::PreviewNotice { identity, notice } => {
            let Some(p) = recording.preview.as_mut() else {
                return vec![];
            };
            if p.identity == *identity
                && let preview::Notice::Failed(error) = notice
            {
                s.error = Some(error.clone());
            }
            if p.identity == *identity && *notice == preview::Notice::Visible {
                // Once playback takes over, the old selection hosts can retire.
                // Subsequent cancellation restores the pre-screenshot desktop.
                recording.lifecycle.restore_editor = false;
            }
            let Some(command) = p.notice(*identity, notice.clone()) else {
                return vec![];
            };
            let mut commands = wrap(Command::Preview(command));
            if command.action == preview::Action::DestroyView
                && let Some(release) = recording.pending_release.take()
            {
                recording.backend_request = Some(release.identity);
                commands.extend(wrap(Command::Execute(release)));
            }
            return commands;
        }
        _ => {}
    }
    let event = match event {
        Event::Native(notice) => {
            use crate::ports::screen_recording::Notice;
            if let Notice::ExportProgress {
                identity,
                range_version,
                stage,
                completed,
                total,
            } = &notice
            {
                if recording.lifecycle.phase == lifecycle::Phase::Saving
                    && recording.lifecycle.identity == *identity
                    && recording
                        .preview
                        .as_ref()
                        .map_or(*range_version == 0, |p| p.range_version == *range_version)
                    && recording
                        .export_stage
                        .is_none_or(|current| current <= *stage)
                    && *total > 0
                    && *completed <= *total
                    && recording.export_progress.is_none_or(|(done, _)| {
                        recording.export_stage != Some(*stage) || *completed >= done
                    })
                {
                    recording.export_progress = Some((*completed, *total));
                    recording.export_stage = Some(*stage);
                }
                return vec![];
            }

            let identity = match &notice {
                Notice::Progress(progress) => return reduce(s, Event::Progress(*progress)),
                Notice::ExportProgress { .. } => unreachable!(),
                Notice::Prepared(id)
                | Notice::Started(id)
                | Notice::Paused(id)
                | Notice::Resumed(id)
                | Notice::Retired(id)
                | Notice::Interrupted { identity: id, .. }
                | Notice::Finished { identity: id, .. }
                | Notice::Failed { identity: id, .. } => *id,
                Notice::VideoConfigured { identity, .. }
                | Notice::AudioConfigured { identity, .. }
                | Notice::StartRejected { identity, .. } => *identity,
            };
            if recording.backend_request != Some(identity) {
                return vec![];
            }
            let current = recording.lifecycle.identity;
            match notice {
                Notice::StartRejected { error, .. } => {
                    s.error = Some(error.to_string());
                    Event::Input {
                        identity: current,
                        event: lifecycle::Event::StartRejected,
                    }
                }
                Notice::VideoConfigured { result, .. } => {
                    if recording.lifecycle.phase != lifecycle::Phase::PreparingVideo {
                        return vec![];
                    }
                    let event = match result {
                        Ok(video) if Some(video) == recording.lifecycle.pending_video => {
                            lifecycle::Event::VideoConfigured
                        }
                        Ok(_) => return vec![],
                        Err(error) => {
                            s.error = Some(error.to_string());
                            lifecycle::Event::VideoConfigurationFailed
                        }
                    };
                    Event::Input {
                        identity: current,
                        event,
                    }
                }
                Notice::AudioConfigured {
                    result, devices, ..
                } => {
                    if recording.lifecycle.phase != lifecycle::Phase::PreparingAudio {
                        return vec![];
                    }
                    let event = match result {
                        Ok(mode) if Some(mode) == recording.lifecycle.pending_audio => {
                            recording.lifecycle.audio_devices = devices;
                            lifecycle::Event::AudioConfigured
                        }
                        Ok(_) => return vec![],
                        Err(error) => {
                            s.error = Some(error.to_string());
                            lifecycle::Event::AudioConfigurationFailed
                        }
                    };
                    Event::Input {
                        identity: current,
                        event,
                    }
                }
                Notice::Prepared(_) => Event::Input {
                    identity: current,
                    event: lifecycle::Event::BackendReady,
                },
                Notice::Started(_) => Event::Input {
                    identity: current,
                    event: lifecycle::Event::Started,
                },
                Notice::Paused(_) => Event::Input {
                    identity: current,
                    event: lifecycle::Event::Paused,
                },
                Notice::Resumed(_) => Event::Input {
                    identity: current,
                    event: lifecycle::Event::Resumed,
                },
                Notice::Retired(_) => Event::Input {
                    identity: current,
                    event: lifecycle::Event::Retired,
                },
                Notice::Interrupted { reason, .. } => Event::Input {
                    identity: current,
                    event: lifecycle::Event::Interrupted(reason),
                },
                Notice::Finished { artifact, .. } => Event::Finalized {
                    identity: current,
                    artifact,
                },
                Notice::Failed { error, .. } => Event::Failed {
                    identity: current,
                    error,
                },
                Notice::Progress(_) | Notice::ExportProgress { .. } => unreachable!(),
            }
        }
        Event::Save(mut request)
            if request.identity == recording.lifecycle.identity
                && recording.lifecycle.phase == lifecycle::Phase::PendingSave
                && recording.preview.as_ref().is_none_or(|p| {
                    matches!(
                        p.phase,
                        preview::Phase::Paused
                            | preview::Phase::Ended
                            | preview::Phase::Failed
                            | preview::Phase::Preparing
                    )
                })
                && recording.lifecycle.artifact == Some(request.artifact) =>
        {
            let Some(command) = recording
                .lifecycle
                .apply(request.identity, lifecycle::Event::Save)
            else {
                return vec![];
            };
            request.identity = command.identity;
            request.range = recording
                .preview
                .as_ref()
                .filter(|p| p.frames > 0)
                .map(|p| p.range);
            request.range_version = recording.preview.as_ref().map_or(0, |p| p.range_version);
            use crate::domain::recording_gif::{Format, Plan, Rejection};
            request.formats = vec![Format::Mp4, Format::Gif];
            request.default_format = Format::Mp4;
            request.gif_plan =
                recording
                    .artifact
                    .as_ref()
                    .map_or(Err(Rejection::InvalidSource), |info| {
                        Plan::new(
                            request
                                .range
                                .unwrap_or(crate::domain::recording_trim::Range::full(info.frames)),
                            info.frames,
                            info.video.frame_rate.get(),
                            info.width,
                            info.height,
                        )
                    });
            recording.export_progress = None;
            recording.export_stage = None;

            s.error = None;
            return wrap(Command::Save(request));
        }
        Event::Saved { identity, result }
            if identity == recording.lifecycle.identity
                && recording.lifecycle.phase == lifecycle::Phase::Saving =>
        {
            if let Some(p) = recording.preview.as_mut() {
                p.save_finished();
            }
            let event = match result {
                Ok(crate::ports::annotation_save::SaveOutcome::Saved) => lifecycle::Event::Saved,
                Ok(crate::ports::annotation_save::SaveOutcome::Cancelled) => {
                    lifecycle::Event::SaveCancelled
                }
                Err(error) => {
                    s.error = Some(error.to_string());
                    lifecycle::Event::SaveFailed
                }
            };
            recording.export_progress = None;
            recording.export_stage = None;
            let close = std::mem::take(&mut recording.close_after_export);
            let mut commands = reduce(s, Event::Input { identity, event });
            if close && let Some(p) = s.recording.as_ref().and_then(|r| r.preview.as_ref()) {
                let id = p.identity;
                commands.extend(reduce(
                    s,
                    Event::PreviewInput {
                        identity: id,
                        input: preview::Input::Close,
                    },
                ));
            }
            return commands;
        }
        event => event,
    };
    let command = match event {
        Event::ViewReady { identity, result }
            if identity == recording.lifecycle.identity
                && recording.lifecycle.phase == lifecycle::Phase::PreparingView =>
        {
            match result {
                Ok(excluded) => {
                    recording.excluded = excluded;
                    recording
                        .lifecycle
                        .apply(identity, lifecycle::Event::ViewReady)
                }
                Err(error) => {
                    s.error = Some(error.to_string());
                    recording.lifecycle.apply(identity, lifecycle::Event::Fatal)
                }
            }
        }
        Event::Input { identity, event } if event != lifecycle::Event::Save => {
            let command = recording.lifecycle.apply(identity, event);
            if command.as_ref().is_some_and(|c| {
                matches!(
                    c.action,
                    lifecycle::Action::ConfigureVideo(_)
                        | lifecycle::Action::ConfigureAudio(_)
                        | lifecycle::Action::Start
                )
            }) {
                s.error = None;
            }
            command
        }
        Event::Finalized { identity, artifact }
            if identity == recording.lifecycle.identity
                && recording.lifecycle.phase == lifecycle::Phase::Finishing =>
        {
            if let Some(reason) = artifact.interrupted {
                recording.lifecycle.finish_reason = lifecycle::FinishReason::Interrupted(reason);
                if reason == lifecycle::Interruption::LayoutChanged {
                    recording.lifecycle.restore_editor = false;
                }
            }
            let command = recording
                .lifecycle
                .apply(identity, lifecycle::Event::Finalized(artifact.artifact));
            recording.artifact = Some(artifact);
            command
        }
        Event::CountdownFailed { identity, error } if identity == recording.lifecycle.identity => {
            s.error = Some(error.to_string());
            recording
                .lifecycle
                .apply(identity, lifecycle::Event::CountdownFailed)
        }
        Event::Failed { identity, error } if identity == recording.lifecycle.identity => {
            s.error = Some(error.to_string());
            recording.lifecycle.apply(identity, lifecycle::Event::Fatal)
        }
        Event::Progress(progress)
            if progress.session == s.session
                && progress.operation == recording.lifecycle.identity.operation
                && matches!(
                    recording.lifecycle.phase,
                    lifecycle::Phase::Recording
                        | lifecycle::Phase::Pausing
                        | lifecycle::Phase::Paused
                        | lifecycle::Phase::Resuming
                )
                && recording
                    .progress
                    .is_none_or(|p| progress.sequence > p.sequence) =>
        {
            recording.progress = Some(progress);
            None
        }
        Event::LayoutChanged { session, operation } => recording
            .lifecycle
            .source_layout_changed(session, operation),
        _ => None,
    };
    let Some(command) = command else {
        if recording.lifecycle.phase == lifecycle::Phase::PendingSave && recording.preview.is_none()
        {
            return reduce(s, Event::PreviewRequested);
        }
        return vec![];
    };
    if command.action == lifecycle::Action::Release
        && let Some(p) = recording.preview.as_mut()
        && p.phase != preview::Phase::Closed
    {
        recording.pending_release = Some(command);
        return wrap(Command::Preview(p.close()));
    }
    if matches!(
        command.action,
        lifecycle::Action::PrepareBackend
            | lifecycle::Action::ConfigureVideo(_)
            | lifecycle::Action::ConfigureAudio(_)
            | lifecycle::Action::Start
            | lifecycle::Action::Pause
            | lifecycle::Action::Resume
            | lifecycle::Action::Finish(_)
            | lifecycle::Action::Release
    ) {
        recording.backend_request = Some(command.identity);
    }
    match command.action {
        lifecycle::Action::PrepareBackend => wrap(Command::PrepareBackend(PrepareRequest {
            video: recording.lifecycle.video,
            identity: command.identity,
            bounds: recording.lifecycle.source,
            monitor_bounds: recording.monitor,
            excluded_windows: recording.excluded.clone(),
        })),
        lifecycle::Action::RestoreScreenshot => {
            s.phase = ScreenshotPhase::Editing;
            s.recording = None;
            wrap(Command::Execute(command))
        }
        lifecycle::Action::EndScreenshot => {
            s.phase = ScreenshotPhase::Idle;
            s.recording = None;
            wrap(Command::Execute(command))
        }
        _ => wrap(Command::Execute(command)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        annotation::Session as Document,
        annotation_export::ExportSnapshot,
        screenshot::{LastRegion, ScreenshotEvent},
    };
    use crate::ports::screen_recording::{Notice, SaveRequest};
    use crate::{AppEvent, AppState};
    use std::{sync::Arc, time::Duration};
    fn begin() -> AppState {
        let mut state = AppState::default();
        let bounds = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        state.screenshot.session = 7;
        state.screenshot.phase = ScreenshotPhase::Editing;
        state.screenshot.bounds = Some(bounds);
        state.screenshot.layout = vec![bounds];
        state.screenshot.last_region = Some(LastRegion {
            bounds,
            layout: vec![bounds],
        });
        let request = ExportRequest {
            session: 7,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Document::new()),
                monitor: bounds,
                monitors: vec![(bounds, 1.)],
                scale: 1.,
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        assert_eq!(emit(&mut state, Event::Requested(request)).len(), 1);
        state
    }
    fn id(s: &AppState) -> lifecycle::Identity {
        s.screenshot.recording.as_ref().unwrap().lifecycle.identity
    }
    fn emit(s: &mut AppState, event: Event) -> Vec<AppCommand> {
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recording(event)))
    }
    fn input(s: &mut AppState, event: lifecycle::Event) -> Vec<AppCommand> {
        emit(
            s,
            Event::Input {
                identity: id(s),
                event,
            },
        )
    }
    fn retire(s: &mut AppState) -> Vec<AppCommand> {
        if let Some(p) = s
            .screenshot
            .recording
            .as_ref()
            .and_then(|r| r.preview.as_ref())
            .filter(|p| p.phase == preview::Phase::Retiring)
        {
            emit(
                s,
                Event::PreviewNotice {
                    identity: p.identity,
                    notice: preview::Notice::Retired,
                },
            );
        }
        let identity = id(s);
        emit(s, Event::Native(Notice::Retired(identity)))
    }
    fn ready(s: &mut AppState) {
        emit(
            s,
            Event::ViewReady {
                identity: id(s),
                result: Ok(vec![]),
            },
        );
        emit(s, Event::Native(Notice::Prepared(id(s))));
    }
    #[test]
    fn video_confirmation_checks_backend_identity_value_and_retirement() {
        use lifecycle::{FrameRate, Quality, Video};
        let mut s = begin();
        ready(&mut s);
        let before = id(&s);
        let video = Video {
            frame_rate: FrameRate::Fps60,
            quality: Quality::Standard,
        };
        input(&mut s, lifecycle::Event::ConfigureVideo(video));
        let pending = id(&s);
        assert!(input(&mut s, lifecycle::Event::Start).is_empty());
        assert!(
            input(
                &mut s,
                lifecycle::Event::ConfigureAudio(crate::domain::recording_audio::Mode::System)
            )
            .is_empty()
        );
        emit(
            &mut s,
            Event::Native(Notice::VideoConfigured {
                identity: before,
                result: Ok(video),
            }),
        );
        emit(
            &mut s,
            Event::Native(Notice::VideoConfigured {
                identity: pending,
                result: Ok(Video::default()),
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::PreparingVideo
        );
        emit(
            &mut s,
            Event::Native(Notice::VideoConfigured {
                identity: pending,
                result: Ok(video),
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.video,
            video
        );
        input(&mut s, lifecycle::Event::ConfigureVideo(Video::default()));
        let pending = id(&s);
        emit(
            &mut s,
            Event::Native(Notice::VideoConfigured {
                identity: pending,
                result: Err(Error::new("unsupported")),
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.video,
            video
        );
        input(&mut s, lifecycle::Event::ConfigureVideo(Video::default()));
        let pending = id(&s);
        input(&mut s, lifecycle::Event::Cancel);
        emit(
            &mut s,
            Event::Native(Notice::VideoConfigured {
                identity: pending,
                result: Ok(Video::default()),
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::Retiring
        );
    }
    #[test]
    fn audio_preparation_is_versioned_and_cancellation_rejects_late_confirmation() {
        use crate::domain::recording_audio::{Devices, Mode};
        let mut s = begin();
        ready(&mut s);
        let old = id(&s);
        let commands = input(&mut s, lifecycle::Event::ConfigureAudio(Mode::Mixed));
        assert!(
            matches!(&commands[0], AppCommand::Screenshot(ScreenshotCommand::Recording(Command::Execute(c))) if c.action == lifecycle::Action::ConfigureAudio(Mode::Mixed))
        );
        let pending = id(&s);
        assert!(input(&mut s, lifecycle::Event::Start).is_empty());
        let devices = Devices {
            system: Some(11),
            microphone: Some(12),
        };
        emit(
            &mut s,
            Event::Native(Notice::AudioConfigured {
                identity: old,
                result: Ok(Mode::Mixed),
                devices,
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::PreparingAudio
        );
        emit(
            &mut s,
            Event::Native(Notice::AudioConfigured {
                identity: pending,
                result: Ok(Mode::Mixed),
                devices,
            }),
        );
        let state = &s.screenshot.recording.as_ref().unwrap().lifecycle;
        assert_eq!(state.audio, Mode::Mixed);
        assert_eq!(state.audio_devices, devices);
        input(&mut s, lifecycle::Event::ConfigureAudio(Mode::System));
        let pending = id(&s);
        input(&mut s, lifecycle::Event::Cancel);
        emit(
            &mut s,
            Event::Native(Notice::AudioConfigured {
                identity: pending,
                result: Ok(Mode::System),
                devices: Devices {
                    system: Some(11),
                    microphone: None,
                },
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::Retiring
        );
        retire(&mut s);
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Editing);
    }
    fn running(s: &mut AppState) {
        ready(s);
        input(s, lifecycle::Event::Start);
        emit(s, Event::Native(Notice::Started(id(s))));
    }
    fn frozen(s: &mut AppState) {
        input(s, lifecycle::Event::Finish);
        input(s, lifecycle::Event::FinalContentPresented);
        let artifact = ArtifactInfo {
            video: Default::default(),
            artifact: lifecycle::Artifact(9),
            width: 1920,
            height: 1080,
            frames: 30,
            duration: Duration::from_secs(1),
            interrupted: None,
        };
        emit(
            s,
            Event::Native(Notice::Finished {
                identity: id(s),
                artifact,
            }),
        );
    }
    #[test]
    fn full_chain_keeps_image_history_and_last_region_unchanged() {
        let mut s = begin();
        let region = s.screenshot.last_region.clone();
        let history = s.image_history.clone();
        running(&mut s);
        assert!(!s.screenshot.editable());
        frozen(&mut s);
        for outcome in [
            lifecycle::Event::SaveCancelled,
            lifecycle::Event::SaveFailed,
            lifecycle::Event::Saved,
        ] {
            let request = SaveRequest {
                formats: vec![],
                default_format: crate::domain::recording_gif::Format::Mp4,
                gif_plan: Err(crate::domain::recording_gif::Rejection::InvalidSource),
                gif_unavailable_message: String::new(),
                range: None,
                range_version: 0,
                identity: id(&s),
                artifact: lifecycle::Artifact(9),
                owner: None,
                default_name: "test".into(),
                title: "Save".into(),
                extension_mismatch_message: "MP4".into(),
            };
            assert!(matches!(
                &emit(&mut s, Event::Save(request))[0],
                AppCommand::Screenshot(ScreenshotCommand::Recording(Command::Save(_)))
            ));
            input(&mut s, outcome);
        }
        let commands = retire(&mut s);
        assert!(
            matches!(&commands[0], AppCommand::Screenshot(ScreenshotCommand::Recording(Command::Execute(c))) if c.action == lifecycle::Action::EndScreenshot)
        );
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Idle);
        assert_eq!(s.screenshot.last_region, region);
        assert_eq!(s.image_history, history);
    }
    #[test]
    fn native_events_match_last_dispatched_native_request_not_view_ack() {
        let mut s = begin();
        running(&mut s);
        let backend = s
            .screenshot
            .recording
            .as_ref()
            .unwrap()
            .backend_request
            .unwrap();
        assert_ne!(id(&s), backend);
        let stale = lifecycle::Identity {
            request: backend.request - 1,
            ..backend
        };
        emit(
            &mut s,
            Event::Native(Notice::Interrupted {
                identity: stale,
                reason: lifecycle::Interruption::CaptureClosed,
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::Recording
        );
        emit(
            &mut s,
            Event::Native(Notice::Interrupted {
                identity: backend,
                reason: lifecycle::Interruption::CaptureClosed,
            }),
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::Finishing
        );
    }
    #[test]
    fn cancel_waits_for_retirement_and_rejects_late_preparation() {
        let mut s = begin();
        let old = id(&s);
        let session = s.screenshot.session;
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled { session }));
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Recording);
        emit(
            &mut s,
            Event::ViewReady {
                identity: old,
                result: Ok(vec![]),
            },
        );
        assert_eq!(
            s.screenshot.recording.as_ref().unwrap().lifecycle.phase,
            lifecycle::Phase::Retiring
        );
        retire(&mut s);
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Editing);
    }
    #[test]
    fn progress_and_layout_changes_cannot_replace_a_pending_save() {
        let mut s = begin();
        running(&mut s);
        frozen(&mut s);
        let identity = id(&s);
        emit(
            &mut s,
            Event::LayoutChanged {
                session: identity.session,
                operation: identity.operation,
            },
        );
        assert_eq!(id(&s), identity);
        emit(
            &mut s,
            Event::Progress(Progress {
                session: identity.session,
                operation: identity.operation,
                sequence: 100,
                active_time: Duration::from_secs(5),
                submitted_frames: 150,
            }),
        );
        assert!(s.screenshot.recording.as_ref().unwrap().progress.is_none());
        input(&mut s, lifecycle::Event::Cancel);
        let commands = retire(&mut s);
        assert!(
            matches!(&commands[0], AppCommand::Screenshot(ScreenshotCommand::Recording(Command::Execute(c))) if c.action == lifecycle::Action::EndScreenshot)
        );
    }
}
