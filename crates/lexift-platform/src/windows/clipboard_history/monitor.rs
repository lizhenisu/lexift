//! Event-driven Unicode clipboard observer; clipboard reads never run on the UI thread.
use lexift_core::{
    Error, Result,
    domain::clipboard_history::{HistoryEntry, MAX_ENTRY_BYTES},
    ports::clipboard_history::{ClipboardHistoryPort, HistoryHandler},
};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{
                AddClipboardFormatListener, GetClipboardData, GetClipboardSequenceNumber,
                IsClipboardFormatAvailable, RegisterClipboardFormatW,
                RemoveClipboardFormatListener,
            },
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
        UI::WindowsAndMessaging::*,
    },
    core::w,
};
#[derive(Default)]
pub(crate) struct Monitor {
    thread: Mutex<Option<Worker>>,
}
struct Worker {
    hwnd: isize,
    join: thread::JoinHandle<()>,
}
struct Context {
    handler: HistoryHandler,
}
fn error() -> Error {
    Error::new("Clipboard history monitoring is unavailable")
}
fn sample() -> (u32, Result<Option<HistoryEntry>>) {
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let read = (|| {
        let _clipboard = super::super::clipboard_selection::open_clipboard_with_retry()?;
        if unsafe { GetClipboardSequenceNumber() } != sequence {
            return Ok(None);
        }
        for (name, always) in [
            (w!("ExcludeClipboardContentFromMonitorProcessing"), true),
            (w!("CanIncludeInClipboardHistory"), false),
        ] {
            let format = unsafe { RegisterClipboardFormatW(name) };
            if format == 0 {
                return Ok(None);
            }
            if unsafe { IsClipboardFormatAvailable(format) }.is_ok() {
                if always {
                    return Ok(None);
                }
                let data = unsafe { GetClipboardData(format) }.map_err(|_| error())?;
                let global = windows::Win32::Foundation::HGLOBAL(data.0);
                if unsafe { GlobalSize(global) } < 4 {
                    return Ok(None);
                }
                let ptr = unsafe { GlobalLock(global) };
                if ptr.is_null() {
                    return Ok(None);
                }
                let excluded = unsafe { ptr.cast::<u32>().read_unaligned() } == 0;
                unsafe {
                    let _ = GlobalUnlock(global);
                }
                if excluded {
                    return Ok(None);
                }
            }
        }
        if unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)) }.is_err() {
            return Ok(None);
        }
        let data = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)) }.map_err(|_| error())?;
        let global = windows::Win32::Foundation::HGLOBAL(data.0);
        let size = unsafe { GlobalSize(global) };
        if size < 2 {
            return Ok(None);
        }
        let ptr = unsafe { GlobalLock(global) };
        if ptr.is_null() {
            return Ok(None);
        }
        struct Lock(windows::Win32::Foundation::HGLOBAL);
        impl Drop for Lock {
            fn drop(&mut self) {
                unsafe {
                    let _ = GlobalUnlock(self.0);
                }
            }
        }
        let _lock = Lock(global);
        let words = unsafe {
            std::slice::from_raw_parts(ptr.cast::<u16>(), (size / 2).min(MAX_ENTRY_BYTES + 1))
        };
        // GlobalSize is allocation capacity, not text length. Scan only a bounded
        // prefix so padded allocations can still contain a valid small string.
        let end = words.iter().position(|&c| c == 0).ok_or_else(|| {
            if words.len() > MAX_ENTRY_BYTES {
                Error::new("This clipboard text is too large to record.")
            } else {
                error()
            }
        })?;
        let text = String::from_utf16(&words[..end]).map_err(|_| error())?;
        if text.len() > MAX_ENTRY_BYTES {
            return Err(Error::new("This clipboard text is too large to record."));
        }
        if text.is_empty() || unsafe { GetClipboardSequenceNumber() } != sequence {
            return Ok(None);
        }
        Ok(Some(HistoryEntry {
            id: 0,
            text: Arc::from(text),
        }))
    })();
    (sequence, read)
}
fn capture(context: &Context) {
    let (seq, entry) = sample();
    super::transient::deliver(seq, entry, context.handler.clone());
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE || msg == WM_APP + 1 {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Context;
        if !ptr.is_null() {
            capture(unsafe { &*ptr });
        }
        return LRESULT(0);
    }
    if msg == WM_CLOSE {
        unsafe {
            let _ = RemoveClipboardFormatListener(hwnd);
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}
impl ClipboardHistoryPort for Monitor {
    fn set_recording(&self, enabled: bool, handler: HistoryHandler) -> Result<()> {
        if !enabled {
            self.stop();
            return Ok(());
        }
        let mut slot = self.thread.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("lexift-clipboard-history".into())
            .spawn(move || {
                let context = Context { handler };
                unsafe {
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(proc),
                        lpszClassName: w!("Lexift.ClipboardHistory"),
                        ..Default::default()
                    };
                    RegisterClassW(&class);
                    let Ok(hwnd) = CreateWindowExW(
                        Default::default(),
                        class.lpszClassName,
                        w!(""),
                        Default::default(),
                        0,
                        0,
                        0,
                        0,
                        Some(HWND_MESSAGE),
                        None,
                        None,
                        None,
                    ) else {
                        let _ = tx.send(Err(error()));
                        return;
                    };
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&context as *const Context) as isize);
                    if AddClipboardFormatListener(hwnd).is_err() {
                        let _ = DestroyWindow(hwnd);
                        let _ = tx.send(Err(error()));
                        return;
                    }
                    let _ = tx.send(Ok(hwnd.0 as isize));
                    let _ = PostMessageW(Some(hwnd), WM_APP + 1, WPARAM(0), LPARAM(0));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .map_err(|_| error())?;
        match rx.recv() {
            Ok(Ok(hwnd)) => {
                *slot = Some(Worker { hwnd, join });
                Ok(())
            }
            _ => {
                let _ = join.join();
                Err(error())
            }
        }
    }
    fn stop(&self) {
        let worker = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(worker) = worker {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(worker.hwnd as *mut _)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            if worker.join.thread().id() != thread::current().id() {
                let _ = worker.join.join();
            }
        }
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::ports::clipboard::ClipboardPort;
    #[test]
    #[ignore = "uses the interactive Windows clipboard, preserving and restoring its complete snapshot"]
    fn native_notifications_capture_initial_text_and_exclude_credential_copies() {
        use crate::windows::{
            clipboard::WindowsClipboardPort,
            clipboard_selection::{ClipboardSnapshot, OleApartment},
        };
        let _apartment = OleApartment::initialize().unwrap();
        let snapshot = ClipboardSnapshot::capture().unwrap();
        let monitor = Monitor::default();
        let result = (|| -> Result<()> {
            WindowsClipboardPort.write_text("history native initial fixture")?;
            let (tx, rx) = mpsc::channel();
            monitor.set_recording(
                true,
                Arc::new(move |event| {
                    if let Ok(Some(e)) = event {
                        let _ = tx.send(e);
                    }
                }),
            )?;
            let initial = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| error())?;
            if &*initial.text != "history native initial fixture" {
                return Err(error());
            }
            WindowsClipboardPort.write_sensitive_text("history credential exclusion fixture")?;
            if rx
                .recv_timeout(std::time::Duration::from_millis(250))
                .is_ok()
            {
                return Err(Error::new("Sensitive clipboard content was recorded"));
            }
            WindowsClipboardPort.write_text("history native user copy fixture")?;
            let next = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| error())?;
            if &*next.text != "history native user copy fixture" {
                return Err(error());
            }
            monitor.stop();
            // A large HGLOBAL allocation may hold a small text. Its capacity must
            // not be confused with the UTF-8 body limit.
            {
                use windows::Win32::{
                    Foundation::{GlobalFree, HANDLE, HGLOBAL},
                    System::{
                        DataExchange::{EmptyClipboard, SetClipboardData},
                        Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc},
                    },
                };
                struct Allocation(HGLOBAL);
                impl Drop for Allocation {
                    fn drop(&mut self) {
                        unsafe {
                            let _ = GlobalFree(Some(self.0));
                        }
                    }
                }
                let data = Allocation(
                    unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 4 * MAX_ENTRY_BYTES) }
                        .map_err(|_| error())?,
                );
                let ptr = unsafe { GlobalLock(data.0) }.cast::<u16>();
                if ptr.is_null() {
                    return Err(error());
                }
                let words = "padded allocation fixture"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect::<Vec<_>>();
                unsafe {
                    std::ptr::copy_nonoverlapping(words.as_ptr(), ptr, words.len());
                    let _ = GlobalUnlock(data.0);
                }
                let _guard = super::super::super::clipboard_selection::open_clipboard_with_retry()?;
                unsafe { EmptyClipboard() }.map_err(|_| error())?;
                unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(data.0.0))) }
                    .map_err(|_| error())?;
                std::mem::forget(data);
            }
            if sample().1?.as_ref().map(|e| &*e.text) != Some("padded allocation fixture") {
                return Err(error());
            }
            WindowsClipboardPort.write_text("history stopped fixture")?;
            if rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_ok()
            {
                return Err(error());
            }
            Ok(())
        })();
        monitor.stop();
        snapshot.set_as_clipboard_contents().unwrap();
        snapshot.flush().unwrap();
        result.unwrap();
    }
}
