//! Composition and native scheduling. Acceptance and restoration stay in Core.
use super::*;
use lexift_core::domain::{
    screen_recording::{Action, Event as Input, Identity},
    screenshot::recording::{Command, Event},
};
use lexift_core::ports::screen_recording::{Control, Notice, ScreenRecordingPort};

pub(super) struct Runtime {
    countdown: Option<tokio::task::JoinHandle<()>>,
    port: Option<Arc<dyn ScreenRecordingPort>>,
    active: Option<Identity>,
    save: Option<Arc<AtomicBool>>,
    shutdown_complete: Arc<AtomicBool>,
    shutdown_notify: Arc<tokio::sync::Notify>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            countdown: None,
            port: None,
            active: None,
            save: None,
            shutdown_complete: Arc::new(AtomicBool::new(true)),
            shutdown_notify: Default::default(),
        }
    }
}
pub(super) fn sync(r: &ScreenshotRuntime, quitting: bool) {
    if !quitting {
        return;
    }
    let mut slot = r.recording.lock().unwrap();
    if let Some(cancel) = slot.save.take() {
        cancel.store(true, Ordering::Release);
    }
    if let Some(task) = slot.countdown.take() {
        task.abort();
    }
    slot.active.take();
    let port = slot.port.take();
    let complete = slot.shutdown_complete.clone();
    let notify = slot.shutdown_notify.clone();
    drop(slot);
    if let Some(port) = port {
        let _ = port.shutdown(Box::new(move || {
            complete.store(true, Ordering::Release);
            notify.notify_waiters();
        }));
    }
}
pub(super) async fn wait_for_shutdown(r: &ScreenshotRuntime) {
    let (complete, notify) = {
        let slot = r.recording.lock().unwrap();
        (slot.shutdown_complete.clone(), slot.shutdown_notify.clone())
    };
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if complete.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}
fn emit(c: &Arc<AppController>, event: Event) {
    c.dispatch(AppEvent::Screenshot(ScreenshotEvent::Recording(event)));
}
fn failed(c: &Arc<AppController>, identity: Identity, error: Error) {
    tracing::warn!(session=identity.session,operation=identity.operation,request=identity.request,%error,"Recording scheduling failed");
    emit(c, Event::Failed { identity, error });
}
pub(super) fn execute(r: &Arc<ScreenshotRuntime>, c: &Arc<AppController>, command: Command) {
    if let Command::CancelExport(identity) = &command {
        let valid = c
            .state
            .lock()
            .unwrap()
            .screenshot
            .recording
            .as_ref()
            .is_some_and(|s| s.lifecycle.identity == *identity);
        if valid {
            let cancel = r.recording.lock().unwrap().save.clone();
            if let Some(cancel) = cancel {
                cancel.store(true, Ordering::Release);
            }
        }
        return;
    }
    if let Command::Preview(command) = command {
        use lexift_core::domain::recording_preview::Action as A;
        let current = c
            .state
            .lock()
            .unwrap()
            .screenshot
            .recording
            .as_ref()
            .and_then(|s| s.preview.as_ref())
            .is_some_and(|p| p.identity == command.identity);
        if !current {
            return;
        }
        if matches!(
            command.action,
            A::PrepareView | A::Handoff | A::Install | A::Save | A::ConfirmDiscard | A::DestroyView
        ) {
            c.ui.screen_recording(Command::Preview(command));
            return;
        }
        let port = r.recording.lock().unwrap().port.clone();
        let controller = c.clone();
        let listener: lexift_core::ports::screen_recording::PreviewListener =
            Arc::new(move |identity, notice| {
                emit(&controller, Event::PreviewNotice { identity, notice })
            });
        if let Err(error) = port
            .ok_or_else(|| Error::new("Recording preview unavailable"))
            .and_then(|p| p.preview(command, listener))
        {
            emit(
                c,
                Event::PreviewNotice {
                    identity: command.identity,
                    notice: lexift_core::domain::recording_preview::Notice::Failed(
                        error.to_string(),
                    ),
                },
            );
        }
        return;
    }
    let identity = match &command {
        Command::Preview(_) | Command::CancelExport(_) => unreachable!(),
        Command::PrepareView { identity, .. } => *identity,
        Command::PrepareBackend(request) => request.identity,
        Command::Execute(command) => command.identity,
        Command::Save(request) => request.identity,
    };
    let current = {
        let state = c.state.lock().unwrap();
        state.screenshot.session == identity.session
            && match &command {
                Command::Execute(command)
                    if matches!(
                        command.action,
                        Action::RestoreScreenshot | Action::EndScreenshot
                    ) =>
                {
                    state.screenshot.recording.is_none()
                }
                _ => state
                    .screenshot
                    .recording
                    .as_ref()
                    .is_some_and(|s| s.lifecycle.identity == identity),
            }
    };
    if !current {
        return;
    }
    if !matches!(&command,Command::Execute(c) if matches!(c.action,Action::WaitCountdown(_)))
        && let Some(task) = r.recording.lock().unwrap().countdown.take()
    {
        task.abort();
    }
    match command {
        Command::Preview(_) | Command::CancelExport(_) => unreachable!(),
        Command::PrepareView { .. } => c.ui.screen_recording(command),
        Command::PrepareBackend(request) => {
            let port = {
                let mut slot = r.recording.lock().unwrap();
                if slot.port.is_none() {
                    match lexift_platform::screen_recording_port() {
                        Ok(port) => {
                            slot.shutdown_complete.store(false, Ordering::Release);
                            slot.port = Some(port);
                        }
                        Err(error) => {
                            drop(slot);
                            failed(c, identity, error);
                            return;
                        }
                    }
                }
                slot.active = Some(identity);
                slot.port.as_ref().unwrap().clone()
            };
            let (runtime, controller) = (r.clone(), c.clone());
            let listener = Arc::new(move |notice| {
                if matches!(notice, Notice::Retired(_)) {
                    let mut slot = runtime.recording.lock().unwrap();
                    if slot.active.is_some_and(|i| {
                        i.session == identity.session && i.operation == identity.operation
                    }) {
                        slot.active = None;
                        slot.save = None;
                    }
                }
                emit(&controller, Event::Native(notice));
            });
            if let Err(error) = port.prepare(request, listener) {
                // No ownership was transferred, so retirement has no native work to await.
                r.recording.lock().unwrap().active = None;
                failed(c, identity, error);
            }
        }
        Command::Execute(command) => {
            if let Action::WaitCountdown(deadline) = command.action {
                // Store the task before cancellation can advance domain identity.
                let state = c.state.lock().unwrap();
                if state
                    .screenshot
                    .recording
                    .as_ref()
                    .is_none_or(|s| s.lifecycle.identity != identity)
                {
                    return;
                }
                let mut slot = r.recording.lock().unwrap();
                let controller = c.clone();
                let task = c.runtime.spawn(async move {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                    tracing::debug!(
                        session = identity.session,
                        request = identity.request,
                        lateness_ms = deadline.elapsed().as_secs_f64() * 1000.,
                        "Recording countdown deadline reached"
                    );
                    emit(
                        &controller,
                        Event::Input {
                            identity,
                            event: Input::CountdownElapsed(std::time::Instant::now()),
                        },
                    );
                });
                if let Some(old) = slot.countdown.replace(task) {
                    old.abort();
                }
                return;
            }
            if matches!(
                command.action,
                Action::PrepareCountdown(_)
                    | Action::RetireCountdown
                    | Action::RestoreReady
                    | Action::PresentFinalContent
                    | Action::RestoreScreenshot
                    | Action::EndScreenshot
            ) {
                c.ui.screen_recording(Command::Execute(command));
                return;
            }
            let (port, active) = {
                let mut slot = r.recording.lock().unwrap();
                if let Some(cancel) = &slot.save {
                    cancel.store(true, Ordering::Release);
                }
                let active = slot.active.is_some();
                if active {
                    slot.active = Some(identity);
                }
                (slot.port.clone(), active)
            };
            if command.action == Action::Release && !active {
                emit(c, Event::Native(Notice::Retired(identity)));
                return;
            }
            let result = match port {
                Some(port) => match command.action {
                    Action::ConfigureVideo(video) => {
                        port.control(identity, Control::ConfigureVideo(video))
                    }
                    Action::ConfigureAudio(mode) => {
                        port.control(identity, Control::ConfigureAudio(mode))
                    }
                    Action::Start => port.control(identity, Control::Start),
                    Action::Pause => port.control(identity, Control::Pause),
                    Action::Resume => port.control(identity, Control::Resume),
                    Action::Finish(_) => port.control(identity, Control::Finish),
                    Action::Release => port.release(identity),
                    _ => Err(Error::new("Unsupported recording command")),
                },
                None => Err(Error::new("Recording service unavailable")),
            };
            if let Err(error) = result {
                match command.action {
                    Action::ConfigureVideo(_) => emit(
                        c,
                        Event::Native(Notice::VideoConfigured {
                            identity,
                            result: Err(error),
                        }),
                    ),
                    Action::ConfigureAudio(_) => emit(
                        c,
                        Event::Native(Notice::AudioConfigured {
                            identity,
                            result: Err(error),
                            devices: Default::default(),
                        }),
                    ),
                    Action::Start => {
                        emit(c, Event::Native(Notice::StartRejected { identity, error }))
                    }
                    _ => failed(c, identity, error),
                }
            }
        }
        Command::Save(request) => {
            let cancel = Arc::new(AtomicBool::new(false));
            let port = {
                let mut slot = r.recording.lock().unwrap();
                slot.save = Some(cancel.clone());
                slot.active = Some(identity);
                slot.port.clone()
            };
            let Some(port) = port else {
                emit(
                    c,
                    Event::Input {
                        identity,
                        event: Input::SaveFailed,
                    },
                );
                return;
            };
            let controller = c.clone();
            let result = port.save(
                request,
                cancel,
                Box::new(move |result| {
                    emit(&controller, Event::Saved { identity, result });
                }),
            );
            if let Err(error) = result {
                emit(
                    c,
                    Event::Saved {
                        identity,
                        result: Err(error),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod trim_tests {
    use lexift_core::{
        AppCommand, AppEvent, AppState,
        domain::{
            annotation::Session as Document,
            annotation_export::{ExportRequest, ExportSnapshot},
            geometry::Rect,
            recording_preview as p,
            recording_trim::Range,
            screen_recording as life,
            screenshot::{
                ScreenshotCommand, ScreenshotEvent, ScreenshotPhase,
                recording::{Command, Event, Session},
            },
        },
        ports::screen_recording::{ArtifactInfo, Notice, SaveRequest},
    };
    use std::{sync::Arc, time::Duration};
    #[test]
    fn export_freezes_range_rejects_old_progress_and_closes_only_after_cancel_completion() {
        let mut app = AppState::default();
        let bounds = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let mut lifecycle = life::State::new(7, 1, bounds, bounds).unwrap();
        lifecycle.phase = life::Phase::PendingSave;
        lifecycle.artifact = Some(life::Artifact(1));
        let id = lifecycle.identity;
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
        let mut session = Session::new(lifecycle, request, bounds);
        let mut preview = p::State::new(id, life::Artifact(1));
        preview.configure(300, 60);
        preview.phase = p::Phase::Paused;
        preview.presented = true;
        preview.visible = true;
        preview.range = Range {
            start: 17,
            end: 103,
        };
        preview.range_version = 1;
        let view_id = preview.identity;
        session.preview = Some(preview);
        session.artifact = Some(ArtifactInfo {
            artifact: life::Artifact(1),
            video: Default::default(),
            width: 1920,
            height: 1080,
            frames: 300,
            duration: Duration::from_secs(5),
            interrupted: None,
        });
        app.screenshot.session = 7;
        app.screenshot.operation = 1;
        app.screenshot.phase = ScreenshotPhase::Recording;
        app.screenshot.recording = Some(session);
        let send =
            |app: &mut AppState, e| app.reduce(AppEvent::Screenshot(ScreenshotEvent::Recording(e)));
        let save = SaveRequest {
            identity: id,
            artifact: life::Artifact(1),
            range: None,
            range_version: 0,
            owner: None,
            default_name: "owned".into(),
            title: "Save".into(),
            extension_mismatch_message: "MP4".into(),
        };
        let commands = send(&mut app, Event::Save(save));
        let request = match &commands[0] {
            AppCommand::Screenshot(ScreenshotCommand::Recording(Command::Save(r))) => r.clone(),
            _ => panic!("save not dispatched"),
        };
        assert_eq!(
            request.range,
            Some(Range {
                start: 17,
                end: 103
            })
        );
        assert_eq!(request.range_version, 1);
        send(
            &mut app,
            Event::Native(Notice::ExportProgress {
                identity: request.identity,
                range_version: 0,
                stage: lexift_core::ports::screen_recording::ExportStage::Transcoding,
                completed: 9,
                total: 86,
            }),
        );
        assert!(
            app.screenshot
                .recording
                .as_ref()
                .unwrap()
                .export_progress
                .is_none()
        );
        send(
            &mut app,
            Event::Native(Notice::ExportProgress {
                identity: request.identity,
                range_version: 1,
                stage: lexift_core::ports::screen_recording::ExportStage::Transcoding,
                completed: 9,
                total: 86,
            }),
        );
        assert_eq!(
            app.screenshot.recording.as_ref().unwrap().export_progress,
            Some((9, 86))
        );
        assert!(matches!(
            &send(
                &mut app,
                Event::CancelExport {
                    identity: view_id,
                    close: true
                }
            )[0],
            AppCommand::Screenshot(ScreenshotCommand::Recording(Command::CancelExport(_)))
        ));
        let commands = send(
            &mut app,
            Event::Saved {
                identity: request.identity,
                result: Ok(lexift_core::ports::annotation_save::SaveOutcome::Cancelled),
            },
        );
        assert!(
            matches!(&commands[0],AppCommand::Screenshot(ScreenshotCommand::Recording(Command::Preview(c))) if c.action==p::Action::Pause)
        );
        let session = app.screenshot.recording.as_ref().unwrap();
        assert_eq!(
            session.preview.as_ref().unwrap().range,
            Range {
                start: 17,
                end: 103
            }
        );
        assert_eq!(session.lifecycle.phase, life::Phase::PendingSave);
    }
}
