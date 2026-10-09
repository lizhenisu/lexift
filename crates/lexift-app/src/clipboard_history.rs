//! Composition and cancellation for history ports. Domain decisions remain in Core.
use crate::controller::AppController;
use lexift_core::{
    AppEvent, AppState,
    domain::clipboard_history::{HistoryCommand, HistoryEntry, HistoryEvent},
    ports::clipboard_history::{ClipboardHistoryPort, ClipboardPastePort, HistoryCapabilities},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};
type Completion = Arc<dyn Fn(HistoryEvent) + Send + Sync>;
enum Io {
    Recording(bool, Completion),
    Load(Completion),
    Save(u64, Vec<HistoryEntry>, Completion),
    Stop,
}
pub(crate) struct HistoryRuntime {
    monitor: Arc<dyn ClipboardHistoryPort>,
    paste: Arc<dyn ClipboardPastePort>,
    io: mpsc::Sender<Io>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    recording: AtomicBool,
    shutting_down: AtomicBool,
    session: Mutex<Option<(u64, Arc<AtomicBool>)>>,
}
fn completion(controller: &Arc<AppController>) -> Completion {
    let weak = Arc::downgrade(controller);
    Arc::new(move |event| {
        if let Some(c) = weak.upgrade() {
            c.dispatch(AppEvent::ClipboardHistory(event));
        }
    })
}
impl HistoryRuntime {
    pub(crate) fn new((monitor, store, paste): HistoryCapabilities) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let observer = monitor.clone();
        let worker =
            thread::Builder::new()
                .name("lexift-history-storage".into())
                .spawn(move || {
                    let mut pending = None;
                    let mut saved_revision = 0;
                    loop {
                        let job = match pending.take() {
                            Some(j) => j,
                            None => match rx.recv() {
                                Ok(j) => j,
                                Err(_) => break,
                            },
                        };
                        match job {
                            Io::Stop => {
                                observer.stop();
                                break;
                            }
                            Io::Recording(enabled, done) => {
                                let callback = done.clone();
                                if observer
                                .set_recording(
                                    enabled,
                                    Arc::new(move |sample| match sample {
                                        Ok(Some(entry)) => callback(HistoryEvent::Captured(entry)),
                                        Err(e)
                                            if e.to_string()
                                                == "This clipboard text is too large to record." =>
                                        {
                                            callback(HistoryEvent::CaptureSkipped)
                                        }
                                        _ => {}
                                    }),
                                )
                                .is_err()
                            {
                                done(HistoryEvent::MonitoringFailed);
                            } else {done(HistoryEvent::MonitoringReady(enabled));}
                            }
                            Io::Load(done) => done(HistoryEvent::Loaded(store.load())),
                            Io::Save(mut revision, mut entries, mut done) => {
                                while let Ok(next) = rx.try_recv() {
                                    match next {
                                        Io::Save(r, e, d) => {
                                            if r > revision {
                                                revision = r;
                                                entries = e;
                                                done = d;
                                            }
                                        }
                                        other => {
                                            pending = Some(other);
                                            break;
                                        }
                                    }
                                }
                                if revision <= saved_revision {
                                    continue;
                                }
                                saved_revision = revision;
                                let success = store.save(&entries).is_ok();
                                done(HistoryEvent::Stored { revision, success });
                            }
                        }
                    }
                })?;
        Ok(Self {
            monitor,
            paste,
            io: tx,
            worker: Mutex::new(Some(worker)),
            recording: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            session: Mutex::new(None),
        })
    }
    pub(crate) fn start(&self, controller: &Arc<AppController>) {
        let _ = self.io.send(Io::Load(completion(controller)));
    }
    pub(crate) fn sync(&self, controller: &Arc<AppController>, state: &AppState) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let id = state.clipboard_history.session.as_ref().map(|s| s.id);
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        if session.as_ref().map(|s| s.0) != id {
            if let Some((_, cancel)) = session.take() {
                cancel.store(true, Ordering::Release);
            }
            if let Some(id) = id {
                *session = Some((id, Arc::new(AtomicBool::new(false))));
            }
        }
        drop(session);
        let enabled = state.clipboard_history.loaded && state.settings.clipboard_history_enabled;
        if self.recording.swap(enabled, Ordering::AcqRel) != enabled {
            let _ = self.io.send(Io::Recording(enabled, completion(controller)));
        }
    }
    pub(crate) fn execute(&self, controller: &Arc<AppController>, command: HistoryCommand) {
        match command {
            HistoryCommand::Copy {
                session,
                request,
                entry,
            } => controller.copy_history_entry(session, request, entry),
            HistoryCommand::Save { revision, entries } => {
                let _ = self
                    .io
                    .send(Io::Save(revision, entries, completion(controller)));
            }
            HistoryCommand::CaptureTarget { session, target } => {
                let port = self.paste.clone();
                let done = completion(controller);
                controller.spawn_history(move || {
                    done(HistoryEvent::TargetReady {
                        session,
                        target: port.capture_target(target),
                    })
                });
            }
            HistoryCommand::Paste {
                session,
                request,
                entry,
            } => {
                let cancel = self
                    .session
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .filter(|s| s.0 == session)
                    .map(|s| s.1.clone());
                if let Some(cancel) = cancel {
                    let port = self.paste.clone();
                    let done = completion(controller);
                    controller.spawn_history(move || {
                        let result = paste_current(&*port, &entry.text, &cancel);
                        done(HistoryEvent::PasteFinished {
                            session,
                            request,
                            result,
                        });
                    });
                }
            }
        }
    }
    pub(crate) fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.monitor.stop();
        if let Some((_, cancel)) = self
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            cancel.store(true, Ordering::Release);
        }
        let _ = self.io.send(Io::Stop);
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }
}
impl Drop for HistoryRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Capture and inject within one cancellable operation; never reuse a prior click's target.
fn paste_current(
    port: &dyn ClipboardPastePort,
    text: &str,
    cancel: &AtomicBool,
) -> lexift_core::Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(lexift_core::Error::new("Clipboard paste was cancelled."));
    }
    let target = port.capture_current_target()?;
    if cancel.load(Ordering::Acquire) {
        return Err(lexift_core::Error::new("Clipboard paste was cancelled."));
    }
    port.paste(&target, text, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        Result,
        domain::{clipboard_history::PasteTarget, selection::SelectionTarget},
        ports::clipboard_history::{ClipboardHistoryStore, HistoryHandler},
    };
    #[derive(Default)]
    struct Monitor;
    impl ClipboardHistoryPort for Monitor {
        fn set_recording(&self, _: bool, _: HistoryHandler) -> Result<()> {
            Ok(())
        }
        fn stop(&self) {}
    }
    struct Paste;
    impl ClipboardPastePort for Paste {
        fn capture_target(&self, _: SelectionTarget) -> Result<PasteTarget> {
            unreachable!()
        }
        fn paste(&self, _: &PasteTarget, _: &str, _: &AtomicBool) -> Result<()> {
            unreachable!()
        }
    }
    #[test]
    fn current_target_failure_and_cancel_never_reach_clipboard_or_injection() {
        struct Current {
            captures: std::sync::atomic::AtomicUsize,
            writes: std::sync::atomic::AtomicUsize,
            valid: bool,
        }
        impl ClipboardPastePort for Current {
            fn capture_target(&self, _: SelectionTarget) -> Result<PasteTarget> {
                unreachable!()
            }
            fn capture_current_target(&self) -> Result<PasteTarget> {
                let index = self.captures.fetch_add(1, Ordering::SeqCst);
                if !self.valid {
                    return Err(lexift_core::Error::new(
                        lexift_core::domain::clipboard_history::UNSUPPORTED_TARGET_MESSAGE,
                    ));
                }
                Ok(PasteTarget {
                    selection: SelectionTarget {
                        window: lexift_core::ports::magnifier::WindowToken(1),
                        focus: None,
                        process: 2,
                        thread: 3,
                    },
                    runtime_id: vec![index as i32],
                })
            }
            fn paste(&self, target: &PasteTarget, text: &str, _: &AtomicBool) -> Result<()> {
                let index = self.writes.fetch_add(1, Ordering::SeqCst);
                assert_eq!(target.runtime_id, vec![index as i32]);
                assert_eq!(text, "  full\ntext  ");
                Ok(())
            }
        }
        let mut current = Current {
            captures: Default::default(),
            writes: Default::default(),
            valid: false,
        };
        let cancel = AtomicBool::new(false);
        assert!(paste_current(&current, "  full\ntext  ", &cancel).is_err());
        assert_eq!(current.writes.load(Ordering::SeqCst), 0);
        current.valid = true;
        current.captures.store(0, Ordering::SeqCst);
        paste_current(&current, "  full\ntext  ", &cancel).unwrap();
        paste_current(&current, "  full\ntext  ", &cancel).unwrap();
        cancel.store(true, Ordering::SeqCst);
        assert!(paste_current(&current, "  full\ntext  ", &cancel).is_err());
        assert_eq!(current.captures.load(Ordering::SeqCst), 2);
        assert_eq!(current.writes.load(Ordering::SeqCst), 2);
    }
    #[derive(Default)]
    struct Store(Mutex<Vec<Vec<HistoryEntry>>>);
    impl ClipboardHistoryStore for Store {
        fn load(&self) -> Result<Vec<HistoryEntry>> {
            Ok(vec![])
        }
        fn save(&self, e: &[HistoryEntry]) -> Result<()> {
            self.0.lock().unwrap().push(e.to_vec());
            Ok(())
        }
    }
    #[test]
    fn serial_storage_rejects_out_of_order_old_snapshots_after_clear() {
        let store = Arc::new(Store::default());
        let runtime =
            HistoryRuntime::new((Arc::new(Monitor), store.clone(), Arc::new(Paste))).unwrap();
        let (tx, rx) = mpsc::channel();
        let done: Completion = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        runtime
            .io
            .send(Io::Save(
                2,
                vec![HistoryEntry {
                    id: 2,
                    text: Arc::from("latest"),
                }],
                done.clone(),
            ))
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        runtime
            .io
            .send(Io::Save(
                1,
                vec![HistoryEntry {
                    id: 1,
                    text: Arc::from("old"),
                }],
                done.clone(),
            ))
            .unwrap();
        runtime.io.send(Io::Save(3, vec![], done.clone())).unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        runtime
            .io
            .send(Io::Save(
                2,
                vec![HistoryEntry {
                    id: 2,
                    text: Arc::from("late"),
                }],
                done,
            ))
            .unwrap();
        runtime.shutdown();
        let writes = store.0.lock().unwrap();
        assert_eq!(writes.len(), 2);
        assert!(writes.last().unwrap().is_empty());
        assert_eq!(&*writes[0][0].text, "latest");
    }
}
