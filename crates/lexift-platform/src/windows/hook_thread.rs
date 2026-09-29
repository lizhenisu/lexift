//! Runs low-level input hooks on a message thread that never performs UI work.

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
};

use lexift_core::{Error, Result};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_QUIT,
    },
};

pub(super) struct HookMessageThread {
    id: u32,
    thread: Option<JoinHandle<()>>,
}

impl HookMessageThread {
    pub(super) fn start(
        name: &'static str,
        install: impl FnOnce() -> Result<Box<dyn FnOnce()>> + Send + 'static,
    ) -> Result<Self> {
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut message = MSG::default();
                unsafe {
                    let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                }
                let cleanup = match install() {
                    Ok(cleanup) => cleanup,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                if ready_sender
                    .send(Ok(unsafe { GetCurrentThreadId() }))
                    .is_ok()
                {
                    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
                }
                cleanup();
            })
            .map_err(|error| Error::new(format!("Could not start {name}: {error}")))?;
        match ready_receiver.recv() {
            Ok(Ok(id)) => {
                tracing::debug!(thread_id = id, name, "mouse hook message thread started");
                Ok(Self {
                    id,
                    thread: Some(thread),
                })
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new(format!("{name} stopped during startup")))
            }
        }
    }

    pub(super) fn stop(mut self) {
        if let Err(error) = unsafe { PostThreadMessageW(self.id, WM_QUIT, WPARAM(0), LPARAM(0)) } {
            tracing::warn!(%error, "hook thread could not be stopped");
            return;
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HookMessageThread;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn message_thread_installs_and_cleans_up_on_every_open() {
        let cleaned = Arc::new(AtomicUsize::new(0));
        for expected in 1..=2 {
            let count = Arc::clone(&cleaned);
            let thread = HookMessageThread::start("lexift-hook-test", move || {
                Ok(Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                }))
            })
            .unwrap();
            thread.stop();
            assert_eq!(cleaned.load(Ordering::SeqCst), expected);
        }
    }
}
