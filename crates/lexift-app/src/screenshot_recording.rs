//! Composition and native scheduling. Acceptance and restoration stay in Core.
use super::*;
use lexift_core::domain::{
    screen_recording::{Action, Event as Input, Identity},
    screenshot::recording::{Command, Event},
};
use lexift_core::ports::screen_recording::{Control, Notice, ScreenRecordingPort};

pub(super) struct Runtime {
    port: Option<Arc<dyn ScreenRecordingPort>>,
    active: Option<Identity>,
    save: Option<Arc<AtomicBool>>,
    shutdown_complete: Arc<AtomicBool>,
    shutdown_notify: Arc<tokio::sync::Notify>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
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
    let identity = match &command {
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
    match command {
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
            if matches!(
                command.action,
                Action::PresentFinalContent | Action::RestoreScreenshot | Action::EndScreenshot
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
                failed(c, identity, error);
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
