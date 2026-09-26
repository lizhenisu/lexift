use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, mpsc},
    thread::{self, JoinHandle},
};

use lexift_core::{
    Error, Result,
    domain::runtime_config::{HotkeyConfig, HotkeyKey},
    ports::hotkey::{HotkeyHandler, HotkeyPort},
};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
            UnregisterHotKey,
        },
        WindowsAndMessaging::{
            GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
        },
    },
};

const TRANSLATE_HOTKEY_ID: i32 = 1;
pub(crate) struct WindowsHotkeyPort {
    listener: Mutex<Option<HotkeyListener>>,
    annotation_listener: Mutex<Option<HotkeyListener>>,
}

impl WindowsHotkeyPort {
    pub(crate) fn new() -> Self {
        Self {
            listener: Mutex::new(None),
            annotation_listener: Mutex::new(None),
        }
    }
}

impl HotkeyPort for WindowsHotkeyPort {
    fn register_annotation_hotkey(&self, handler: HotkeyHandler) -> Result<()> {
        let mut listener = self
            .annotation_listener
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if listener.is_some() {
            return Err(Error::new("Annotation shortcut is already registered"));
        }
        // Each listener owns its registration on a separate message-loop thread.
        *listener = Some(start_listener("Alt + A".parse()?, handler)?);
        Ok(())
    }

    fn unregister_annotation_hotkey(&self) -> Result<()> {
        if let Some(listener) = self
            .annotation_listener
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            listener.shutdown();
        }
        Ok(())
    }

    fn register_translate_hotkey(
        &self,
        config: HotkeyConfig,
        handler: HotkeyHandler,
    ) -> Result<()> {
        let mut listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if listener.is_some() {
            return Err(Error::new(
                "A global translate hotkey is already registered",
            ));
        }
        *listener = Some(start_listener(config, handler)?);
        Ok(())
    }

    fn replace_translate_hotkey(&self, config: HotkeyConfig, handler: HotkeyHandler) -> Result<()> {
        let mut listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if listener
            .as_ref()
            .is_some_and(|current| current.config == config)
        {
            return Ok(());
        }
        // Register the replacement first. If Windows rejects it, the current listener remains live.
        let replacement = start_listener(config, handler)?;
        if let Some(previous) = listener.replace(replacement) {
            previous.shutdown();
        }
        Ok(())
    }

    fn unregister_translate_hotkey(&self) -> Result<()> {
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(listener) = listener {
            listener.shutdown();
        }
        Ok(())
    }
}

fn start_listener(config: HotkeyConfig, handler: HotkeyHandler) -> Result<HotkeyListener> {
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("lexift-hotkey".into())
        .spawn(move || run_listener(config, handler, ready_sender))
        .map_err(|_| Error::new("Could not start the global hotkey listener"))?;

    match ready_receiver.recv() {
        Ok(Ok(thread_id)) => Ok(HotkeyListener {
            thread_id,
            thread: Some(thread),
            config,
        }),
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

impl Drop for WindowsHotkeyPort {
    fn drop(&mut self) {
        if let Some(listener) = self
            .annotation_listener
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            listener.shutdown();
        }
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
    config: HotkeyConfig,
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

fn run_listener(
    config: HotkeyConfig,
    handler: HotkeyHandler,
    ready_sender: mpsc::SyncSender<Result<u32>>,
) {
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
            native_modifiers(config),
            native_key(config.key),
        )
    };
    if let Err(error) = registration {
        let _ = ready_sender.send(Err(Error::new(format!(
            "Could not register {config} global hotkey: {error}"
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

fn native_modifiers(
    config: HotkeyConfig,
) -> windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS {
    let mut modifiers = MOD_NOREPEAT;
    if config.modifiers.control {
        modifiers |= MOD_CONTROL;
    }
    if config.modifiers.alt {
        modifiers |= MOD_ALT;
    }
    if config.modifiers.shift {
        modifiers |= MOD_SHIFT;
    }
    if config.modifiers.meta {
        modifiers |= MOD_WIN;
    }
    modifiers
}

fn native_key(key: HotkeyKey) -> u32 {
    match key {
        HotkeyKey::Letter(value) => value as u32,
        HotkeyKey::Digit(value) => b'0' as u32 + u32::from(value),
        HotkeyKey::Function(value) => 0x70 + u32::from(value - 1),
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
    fn annotation_and_translation_are_independent() {
        let hotkey = WindowsHotkeyPort::new();
        let (translate_tx, translate_rx) = mpsc::channel();
        let (annotation_tx, annotation_rx) = mpsc::channel();
        hotkey
            .register_translate_hotkey(
                HotkeyConfig::default(),
                Arc::new(move || {
                    let _ = translate_tx.send(());
                }),
            )
            .unwrap();
        hotkey
            .register_annotation_hotkey(Arc::new(move || {
                let _ = annotation_tx.send(());
            }))
            .unwrap();
        let conflicting = WindowsHotkeyPort::new();
        assert!(
            conflicting
                .register_annotation_hotkey(Arc::new(|| {}))
                .is_err()
        );
        let translate_thread = hotkey.listener.lock().unwrap().as_ref().unwrap().thread_id;
        let annotation_thread = hotkey
            .annotation_listener
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .thread_id;
        assert_ne!(translate_thread, annotation_thread);
        unsafe {
            PostThreadMessageW(
                annotation_thread,
                WM_HOTKEY,
                WPARAM(TRANSLATE_HOTKEY_ID as usize),
                LPARAM(0),
            )
            .unwrap();
        }
        annotation_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(translate_rx.try_recv().is_err());
        hotkey.unregister_annotation_hotkey().unwrap();
        conflicting
            .register_annotation_hotkey(Arc::new(|| {}))
            .unwrap();
        unsafe {
            PostThreadMessageW(
                translate_thread,
                WM_HOTKEY,
                WPARAM(TRANSLATE_HOTKEY_ID as usize),
                LPARAM(0),
            )
            .unwrap();
        }
        translate_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn registers_forwards_conflicts_and_shuts_down() {
        let (triggered_sender, triggered_receiver) = mpsc::sync_channel(1);
        let hotkey = WindowsHotkeyPort::new();
        hotkey
            .register_translate_hotkey(
                HotkeyConfig::default(),
                Arc::new(move || {
                    let _ = triggered_sender.send(());
                }),
            )
            .expect("Alt+X should register for the manual platform test");
        assert_eq!(
            hotkey
                .register_translate_hotkey(HotkeyConfig::default(), Arc::new(|| {}))
                .expect_err("one adapter must not register twice")
                .to_string(),
            "A global translate hotkey is already registered"
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

        let replacement: HotkeyConfig = "Ctrl + Shift + F12".parse().unwrap();
        hotkey
            .replace_translate_hotkey(replacement, Arc::new(|| {}))
            .expect("an available replacement hotkey should register atomically");
        assert_eq!(
            hotkey
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .map(|listener| listener.config),
            Some(replacement)
        );

        let released_old = WindowsHotkeyPort::new();
        released_old
            .register_translate_hotkey(HotkeyConfig::default(), Arc::new(|| {}))
            .expect("replacement should release the old hotkey");
        drop(released_old);

        let conflicting = WindowsHotkeyPort::new();
        assert!(
            conflicting
                .register_translate_hotkey(replacement, Arc::new(|| {}))
                .is_err()
        );
        drop(hotkey);
        conflicting
            .register_translate_hotkey(replacement, Arc::new(|| {}))
            .expect("dropping the owner should release the replacement hotkey");
        drop(conflicting);
    }

    #[test]
    fn maps_supported_keys_and_modifiers() {
        let config: HotkeyConfig = "Ctrl + Shift + F12".parse().unwrap();
        let modifiers = native_modifiers(config);
        assert!(modifiers.contains(MOD_CONTROL));
        assert!(modifiers.contains(MOD_SHIFT));
        assert!(modifiers.contains(MOD_NOREPEAT));
        assert_eq!(native_key(config.key), 0x7b);
    }
}
