//! One scoped message loop owns both temporary recording registrations.
use super::*;
use lexift_core::domain::recording_hotkeys::{Action, Registration, Scope};
type Handler = std::sync::Arc<dyn Fn(Action) + Send + Sync>;
pub(super) struct Listener {
    scope: Scope,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}
impl Listener {
    fn stop(mut self) -> Result<()> {
        let started = std::time::Instant::now();
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            .map_err(|e| Error::new(e.to_string()))?;
        let result = self
            .thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| Error::new("Recording shortcuts unavailable"));
        tracing::debug!(
            session = self.scope.session,
            operation = self.scope.operation,
            generation = self.scope.generation,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
            "Recording shortcuts retired"
        );
        result
    }
}
impl WindowsHotkeyPort {
    pub(super) fn start_recording(
        &self,
        scope: Scope,
        keys: [HotkeyConfig; 2],
        handler: Handler,
    ) -> Result<Registration> {
        let mut slot = self.recording.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_some() {
            return Err(Error::new("Recording shortcut resources are retiring"));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("lexift-recording-hotkeys".into())
            .spawn(move || {
                let mut msg = MSG::default();
                unsafe {
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                }
                let mut accepted = [false; 2];
                for i in 0..2 {
                    if i == 1 && keys[0] == keys[1] {
                        continue;
                    }
                    accepted[i] = unsafe {
                        RegisterHotKey(
                            None,
                            100 + i as i32,
                            native_modifiers(keys[i]),
                            native_key(keys[i].key),
                        )
                    }
                    .is_ok();
                }
                let ready = tx.send((unsafe { GetCurrentThreadId() }, accepted)).is_ok();
                if ready {
                    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                        if msg.message == WM_HOTKEY {
                            if native_modal_input() {
                                continue;
                            }
                            let action = match msg.wParam.0 {
                                100 if accepted[0] => Some(Action::TogglePause),
                                101 if accepted[1] => Some(Action::Finish),
                                _ => None,
                            };
                            if let Some(action) = action {
                                let _ = catch_unwind(AssertUnwindSafe(|| handler(action)));
                            }
                        }
                    }
                }
                for (i, ok) in accepted.iter().enumerate() {
                    if *ok {
                        unsafe {
                            let _ = UnregisterHotKey(None, 100 + i as i32);
                        }
                    }
                }
            })
            .map_err(|e| Error::new(e.to_string()))?;
        match rx.recv() {
            Ok((thread_id, accepted)) => {
                *slot = Some(Listener {
                    scope,
                    thread_id,
                    thread: Some(thread),
                });
                Ok(Registration {
                    scope,
                    pause: accepted[0].then_some(keys[0]),
                    finish: accepted[1].then_some(keys[1]),
                })
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new("Recording shortcuts unavailable"))
            }
        }
    }
    pub(super) fn stop_recording(&self, scope: Scope) -> Result<()> {
        // Serialize retirement with registration; never create a replacement while joining.
        let mut slot = self.recording.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|l| l.scope == scope) {
            slot.take().unwrap().stop()?;
        }
        Ok(())
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = thread.join();
        }
    }
}

/// Native modal menus/dialogs and move loops own the application's input.
/// External application input is deliberately unaffected by this guard.
fn native_modal_input() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GUI_INMENUMODE, GUI_INMOVESIZE, GUITHREADINFO, GetClassNameW, GetForegroundWindow,
        GetGUIThreadInfo, GetWindowThreadProcessId,
    };
    unsafe {
        let foreground = GetForegroundWindow();
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(foreground, Some(&mut pid));
        if pid != windows::Win32::System::Threading::GetCurrentProcessId() {
            return false;
        }
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread, &mut gui).is_ok()
            && gui.flags.0 & (GUI_INMENUMODE.0 | GUI_INMOVESIZE.0) != 0
        {
            return true;
        }
        let mut name = [0u16; 64];
        let len = GetClassNameW(foreground, &mut name);
        len > 0 && String::from_utf16_lossy(&name[..len as usize]) == "#32770"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit temporary native hotkey ownership, collision and retirement gate"]
    fn native_recording_hotkeys_gate() {
        use lexift_core::domain::recording_hotkeys::{default_finish, default_pause};
        let p = WindowsHotkeyPort::new();
        let keys = [default_pause(), default_finish()];
        for round in 0..20 {
            let scope = Scope {
                session: 1,
                operation: round,
                generation: 1,
            };
            let blocked = round % 2 == 0;
            if blocked {
                unsafe {
                    RegisterHotKey(
                        None,
                        900,
                        native_modifiers(keys[0]),
                        native_key(keys[0].key),
                    )
                    .unwrap();
                }
            }
            let registration = p.start_recording(scope, keys, Arc::new(|_| {})).unwrap();
            assert_eq!(registration.pause.is_some(), !blocked);
            assert!(registration.finish.is_some());
            assert!(p.start_recording(scope, keys, Arc::new(|_| {})).is_err());
            p.stop_recording(Scope {
                session: 2,
                ..scope
            })
            .unwrap();
            assert!(p.probe_recording_hotkey(keys[1]).is_err());
            p.stop_recording(scope).unwrap();
            assert!(p.recording.lock().unwrap().is_none());
            p.probe_recording_hotkey(keys[1]).unwrap();
            if blocked {
                unsafe {
                    UnregisterHotKey(None, 900).unwrap();
                }
            }
            p.probe_recording_hotkey(keys[0]).unwrap();
            eprintln!("RECORDING_HOTKEY_GATE round={round} partial={blocked} retired=true");
        }
    }
}
