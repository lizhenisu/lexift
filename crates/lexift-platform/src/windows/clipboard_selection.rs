use std::{
    mem::size_of,
    ptr::NonNull,
    thread,
    time::{Duration, Instant},
};

use lexift_core::{Error, Result};
use windows::Win32::{
    Foundation::{ERROR_SUCCESS, GetLastError, GlobalFree, HANDLE, HGLOBAL, SetLastError},
    Graphics::Gdi::{DeleteEnhMetaFile, DeleteMetaFile, DeleteObject, HENHMETAFILE, HGDIOBJ},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
            GetClipboardSequenceNumber, IsClipboardFormatAvailable, METAFILEPICT, OpenClipboard,
            SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{
            CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_ENHMETAFILE,
            CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_UNICODETEXT, CLIPBOARD_FORMAT,
            OleDuplicateData, OleInitialize, OleUninitialize,
        },
    },
    UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY, VK_C, VK_CONTROL, VK_MENU, VK_X,
    },
};

const KEY_RELEASE_TIMEOUT: Duration = Duration::from_millis(160);
const COPY_TIMEOUT: Duration = Duration::from_millis(400);
const COPY_STABLE_PERIOD: Duration = Duration::from_millis(24);
const OPEN_CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(80);
const POLL_INTERVAL: Duration = Duration::from_millis(8);

pub(super) fn capture_selected_text() -> Result<Option<String>> {
    let started_at = Instant::now();
    let result = thread::Builder::new()
        .name("lexift-clipboard-selection".into())
        .spawn(capture_on_sta_thread)
        .map_err(|_| Error::new("Could not start clipboard selection capture"))?
        .join()
        .map_err(|_| Error::new("Clipboard selection capture stopped unexpectedly"))?;

    tracing::debug!(
        strategy = "clipboard",
        elapsed_ms = started_at.elapsed().as_millis(),
        success = result.is_ok(),
        "Clipboard selection fallback finished"
    );
    result
}

fn capture_on_sta_thread() -> Result<Option<String>> {
    let _apartment = OleApartment::initialize()?;
    wait_for_trigger_keys_release()?;

    let snapshot = ClipboardSnapshot::capture()?;
    let before_copy = snapshot.sequence;
    let mut transaction = ClipboardTransaction::new(snapshot);

    send_copy_shortcut()?;
    let Some(copied_sequence) = wait_for_sequence_change(before_copy) else {
        return Ok(None);
    };
    transaction.mark_copy(copied_sequence);

    let selected_text = read_unicode_text()?.and_then(normalize_clipboard_text);
    transaction.restore()?;
    Ok(selected_text)
}

struct OleApartment;

impl OleApartment {
    fn initialize() -> Result<Self> {
        unsafe { OleInitialize(None) }
            .map_err(|_| Error::new("Could not initialize Windows clipboard services"))?;
        Ok(Self)
    }
}

impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct ClipboardEntry {
    format: u32,
    handle: Option<HANDLE>,
}

impl Drop for ClipboardEntry {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            free_duplicated_clipboard_data(self.format, handle);
        }
    }
}

struct ClipboardSnapshot {
    sequence: u32,
    entries: Vec<ClipboardEntry>,
}

impl ClipboardSnapshot {
    fn capture() -> Result<Self> {
        for _ in 0..3 {
            let before = unsafe { GetClipboardSequenceNumber() };
            let entries = {
                let _clipboard = open_clipboard_with_retry()?;
                duplicate_open_clipboard()?
            };
            let after = unsafe { GetClipboardSequenceNumber() };

            if before == after {
                return Ok(Self {
                    sequence: after,
                    entries,
                });
            }
        }

        Err(Error::new("Clipboard changed while it was being preserved"))
    }

    fn restore(mut self) -> Result<()> {
        let _clipboard = open_clipboard_with_retry()?;
        unsafe { EmptyClipboard() }.map_err(|_| Error::new("Could not restore the clipboard"))?;

        let mut failed = false;
        for entry in &mut self.entries {
            let Some(handle) = entry.handle.take() else {
                continue;
            };
            if unsafe { SetClipboardData(entry.format, Some(handle)) }.is_err() {
                entry.handle = Some(handle);
                failed = true;
            }
        }

        if failed {
            Err(Error::new("Could not restore every clipboard format"))
        } else {
            Ok(())
        }
    }
}

fn duplicate_open_clipboard() -> Result<Vec<ClipboardEntry>> {
    let mut entries = Vec::new();
    let mut format = 0;

    loop {
        unsafe { SetLastError(ERROR_SUCCESS) };
        let next_format = unsafe { EnumClipboardFormats(format) };
        if next_format == 0 {
            if unsafe { GetLastError() } == ERROR_SUCCESS {
                return Ok(entries);
            }
            return Err(Error::new("Could not enumerate the clipboard formats"));
        }
        format = next_format;

        if format == u32::from(CF_OWNERDISPLAY.0) {
            return Err(Error::new("The clipboard contains owner-rendered data"));
        }
        let source = unsafe { GetClipboardData(format) }
            .map_err(|_| Error::new("Could not materialize a clipboard format"))?;
        let duplicate =
            unsafe { OleDuplicateData(source, CLIPBOARD_FORMAT(format as u16), GMEM_MOVEABLE) };
        if duplicate.is_invalid() {
            return Err(Error::new("Could not duplicate a clipboard format"));
        }
        entries.push(ClipboardEntry {
            format,
            handle: Some(duplicate),
        });
    }
}

fn free_duplicated_clipboard_data(format: u32, handle: HANDLE) {
    let format = format as u16;
    if [CF_BITMAP.0, CF_DSPBITMAP.0, CF_PALETTE.0].contains(&format) {
        let _ = unsafe { DeleteObject(HGDIOBJ(handle.0)) };
    } else if [CF_ENHMETAFILE.0, CF_DSPENHMETAFILE.0].contains(&format) {
        let _ = unsafe { DeleteEnhMetaFile(Some(HENHMETAFILE(handle.0))) };
    } else if [CF_METAFILEPICT.0, CF_DSPMETAFILEPICT.0].contains(&format) {
        let global = HGLOBAL(handle.0);
        let pointer = unsafe { GlobalLock(global) } as *const METAFILEPICT;
        if let Some(metafile) = NonNull::new(pointer.cast_mut()) {
            let _ = unsafe { DeleteMetaFile(metafile.as_ref().hMF) };
            let _ = unsafe { GlobalUnlock(global) };
        }
        let _ = unsafe { GlobalFree(Some(global)) };
    } else {
        let _ = unsafe { GlobalFree(Some(HGLOBAL(handle.0))) };
    }
}

struct ClipboardTransaction {
    snapshot: Option<ClipboardSnapshot>,
    copied_sequence: Option<u32>,
}

impl ClipboardTransaction {
    fn new(snapshot: ClipboardSnapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            copied_sequence: None,
        }
    }

    fn mark_copy(&mut self, sequence: u32) {
        self.copied_sequence = Some(sequence);
    }

    fn restore(&mut self) -> Result<()> {
        let Some(copied_sequence) = self.copied_sequence.take() else {
            return Ok(());
        };
        let current_sequence = unsafe { GetClipboardSequenceNumber() };
        if !should_restore_clipboard(copied_sequence, current_sequence) {
            self.snapshot.take();
            tracing::debug!(
                strategy = "clipboard",
                "Clipboard changed after selection copy; preserving newer user content"
            );
            return Ok(());
        }

        self.snapshot
            .take()
            .map(ClipboardSnapshot::restore)
            .unwrap_or(Ok(()))
    }
}

impl Drop for ClipboardTransaction {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::warn!(error = %error, "Could not restore clipboard after selection capture");
        }
    }
}

fn wait_for_trigger_keys_release() -> Result<()> {
    let deadline = Instant::now() + KEY_RELEASE_TIMEOUT;
    while any_trigger_key_is_down() {
        if Instant::now() >= deadline {
            return Err(Error::new("Selection shortcut keys are still pressed"));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

fn any_trigger_key_is_down() -> bool {
    [VK_MENU, VK_CONTROL, VK_X]
        .into_iter()
        .any(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0)
}

fn send_copy_shortcut() -> Result<()> {
    let inputs = [
        keyboard_input(VK_CONTROL, false),
        keyboard_input(VK_C, false),
        keyboard_input(VK_C, true),
        keyboard_input(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(Error::new("Could not send the selection copy shortcut"))
    }
}

fn keyboard_input(key: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn wait_for_sequence_change(before: u32) -> Option<u32> {
    let deadline = Instant::now() + COPY_TIMEOUT;
    let mut latest_sequence = None;
    let mut stable_since = None;

    loop {
        let now = Instant::now();
        let current = unsafe { GetClipboardSequenceNumber() };
        if clipboard_sequence_changed(before, current) && latest_sequence != Some(current) {
            latest_sequence = Some(current);
            stable_since = Some(now);
        } else if let (Some(sequence), Some(since)) = (latest_sequence, stable_since)
            && now.duration_since(since) >= COPY_STABLE_PERIOD
        {
            return Some(sequence);
        }
        if now >= deadline {
            return latest_sequence;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn read_unicode_text() -> Result<Option<String>> {
    let _clipboard = open_clipboard_with_retry()?;
    if unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)) }.is_err() {
        return Ok(None);
    }
    let handle = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)) }
        .map_err(|_| Error::new("Could not access copied selection text"))?;
    let global = HGLOBAL(handle.0);
    let byte_len = unsafe { GlobalSize(global) };
    if byte_len < size_of::<u16>() {
        return Ok(Some(String::new()));
    }

    let pointer = NonNull::new(unsafe { GlobalLock(global) } as *mut u16)
        .ok_or_else(|| Error::new("Could not read copied selection text"))?;
    let _lock = GlobalLockGuard(global);
    let units =
        unsafe { std::slice::from_raw_parts(pointer.as_ptr(), byte_len / size_of::<u16>()) };
    let text_len = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16(&units[..text_len])
        .map(Some)
        .map_err(|_| Error::new("Copied selection was not valid Unicode text"))
}

struct GlobalLockGuard(HGLOBAL);

impl Drop for GlobalLockGuard {
    fn drop(&mut self) {
        let _ = unsafe { GlobalUnlock(self.0) };
    }
}

struct OpenClipboardGuard;

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

fn open_clipboard_with_retry() -> Result<OpenClipboardGuard> {
    let deadline = Instant::now() + OPEN_CLIPBOARD_TIMEOUT;
    loop {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(OpenClipboardGuard);
        }
        if Instant::now() >= deadline {
            return Err(Error::new("Could not open the Windows clipboard"));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn clipboard_sequence_changed(before: u32, current: u32) -> bool {
    before != current
}

fn should_restore_clipboard(copied: u32, current: u32) -> bool {
    copied == current
}

fn normalize_clipboard_text(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::{clipboard_sequence_changed, normalize_clipboard_text, should_restore_clipboard};

    #[test]
    fn unchanged_sequence_is_not_a_copy() {
        assert!(!clipboard_sequence_changed(42, 42));
    }

    #[test]
    fn one_sequence_change_allows_clipboard_read() {
        assert!(clipboard_sequence_changed(42, 43));
    }

    #[test]
    fn newer_clipboard_content_is_not_overwritten() {
        assert!(should_restore_clipboard(43, 43));
        assert!(!should_restore_clipboard(43, 44));
    }

    #[test]
    fn whitespace_only_clipboard_text_is_not_a_selection() {
        assert_eq!(normalize_clipboard_text(" \r\n\t ".into()), None);
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn interactive_selection_capture_restores_text_clipboard() {
        const SENTINEL: &str = "lexift-clipboard-sentinel";

        let before = super::read_unicode_text().expect("clipboard should be readable");
        assert_eq!(
            before.as_deref(),
            Some(SENTINEL),
            "copy the sentinel text before running this ignored test"
        );
        eprintln!("Focus an application with selected text within three seconds");
        std::thread::sleep(std::time::Duration::from_secs(3));

        let selected = super::capture_selected_text().expect("selection capture should succeed");
        let after = super::read_unicode_text().expect("restored clipboard should be readable");

        assert!(selected.is_some());
        assert_eq!(after.as_deref(), Some(SENTINEL));
    }
}
