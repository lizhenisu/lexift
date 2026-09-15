use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, mpsc},
    thread::{self, JoinHandle},
};

use lexift_core::{
    Error, Result,
    ports::hotkey::{HotkeyHandler, HotkeyPort},
};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{MOD_ALT, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey},
        WindowsAndMessaging::{
            GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
        },
    },
};

const TRANSLATE_HOTKEY_ID: i32 = 1;
const TRANSLATE_VIRTUAL_KEY: u32 = b'X' as u32;

pub(crate) struct WindowsHotkeyPort {
    listener: Mutex<Option<HotkeyListener>>,
}

impl WindowsHotkeyPort {
    pub(crate) fn new() -> Self {
        Self {
            listener: Mutex::new(None),
        }
    }
}

impl HotkeyPort for WindowsHotkeyPort {
    fn register_translate_hotkey(&self, handler: HotkeyHandler) -> Result<()> {
        let mut listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if listener.is_some() {
            return Err(Error::new("Alt+X global hotkey is already registered"));
        }

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("lexift-hotkey".into())
            .spawn(move || run_listener(handler, ready_sender))
            .map_err(|_| Error::new("Could not start the global hotkey listener"))?;

        match ready_receiver.recv() {
            Ok(Ok(thread_id)) => {
                *listener = Some(HotkeyListener {
                    thread_id,
                    thread: Some(thread),
                });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new(
                    "Global hotkey listener stopped during registration",
                ))
            }
        }
    }
}

impl Drop for WindowsHotkeyPort {
    fn drop(&mut self) {
        let listener = self
            .listener
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(listener) = listener.take() {
            listener.shutdown();
        }
    }
}

struct HotkeyListener {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl HotkeyListener {
    fn shutdown(mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_listener(handler: HotkeyHandler, ready_sender: mpsc::SyncSender<Result<u32>>) {
    let mut message = MSG::default();
    unsafe {
        // Creating the queue before the handshake makes shutdown messages reliable.
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    let thread_id = unsafe { GetCurrentThreadId() };
    let registration = unsafe {
        RegisterHotKey(
            None,
            TRANSLATE_HOTKEY_ID,
            MOD_ALT | MOD_NOREPEAT,
            TRANSLATE_VIRTUAL_KEY,
        )
    };
    if let Err(error) = registration {
        let _ = ready_sender.send(Err(Error::new(format!(
            "Could not register Alt+X global hotkey: {error}"
        ))));
        return;
    }
    let _registration = HotkeyRegistration;

    if ready_sender.send(Ok(thread_id)).is_err() {
        return;
    }

    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result <= 0 {
            break;
        }
        forward_translate_message(message.message, message.wParam.0, &handler);
    }
}

struct HotkeyRegistration;

impl Drop for HotkeyRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(None, TRANSLATE_HOTKEY_ID);
        }
    }
}

fn forward_translate_message(message: u32, hotkey_id: usize, handler: &HotkeyHandler) -> bool {
    if message != WM_HOTKEY || hotkey_id != TRANSLATE_HOTKEY_ID as usize {
        return false;
    }
    tracing::debug!("translate hotkey received");
    let _ = catch_unwind(AssertUnwindSafe(|| handler()));
    true
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;

    #[test]
    fn forwards_only_the_translate_hotkey_message() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let handler: HotkeyHandler = Arc::new(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        assert!(!forward_translate_message(WM_HOTKEY, 999, &handler));
        assert!(!forward_translate_message(WM_QUIT, 1, &handler));
        assert!(forward_translate_message(
            WM_HOTKEY,
            TRANSLATE_HOTKEY_ID as usize,
            &handler
        ));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn registers_forwards_conflicts_and_shuts_down() {
        let (triggered_sender, triggered_receiver) = mpsc::sync_channel(1);
        let hotkey = WindowsHotkeyPort::new();
        hotkey
            .register_translate_hotkey(Arc::new(move || {
                let _ = triggered_sender.send(());
            }))
            .expect("Alt+X should register for the manual platform test");
        assert_eq!(
            hotkey
                .register_translate_hotkey(Arc::new(|| {}))
                .expect_err("one adapter must not register twice")
                .to_string(),
            "Alt+X global hotkey is already registered"
        );

        let thread_id = hotkey
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .expect("listener should be registered")
            .thread_id;
        unsafe {
            PostThreadMessageW(
                thread_id,
                WM_HOTKEY,
                WPARAM(TRANSLATE_HOTKEY_ID as usize),
                LPARAM(0),
            )
            .expect("test hotkey message should post");
        }

        triggered_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("listener should forward the test hotkey message");

        let conflicting = WindowsHotkeyPort::new();
        assert!(
            conflicting
                .register_translate_hotkey(Arc::new(|| {}))
                .is_err()
        );
        drop(hotkey);
        conflicting
            .register_translate_hotkey(Arc::new(|| {}))
            .expect("dropping the owner should release Alt+X");
        drop(conflicting);
    }
}
