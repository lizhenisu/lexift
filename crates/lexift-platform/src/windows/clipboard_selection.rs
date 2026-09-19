use std::{
    mem::size_of,
    ptr::NonNull,
    thread,
    time::{Duration, Instant},
};

use lexift_core::{Error, Result};
use windows::Win32::{
    Foundation::HGLOBAL,
    System::{
        Com::{CoTaskMemFree, DATADIR_GET, FORMATETC, IDataObject},
        DataExchange::{
            CloseClipboard, CountClipboardFormats, GetClipboardData, GetClipboardSequenceNumber,
            IsClipboardFormatAvailable, OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{
            CF_UNICODETEXT, OleFlushClipboard, OleGetClipboard, OleInitialize, OleSetClipboard,
            OleUninitialize, ReleaseStgMedium,
        },
    },
    UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY, VK_C, VK_CONTROL, VK_MENU, VK_X,
    },
    UI::Shell::SHCreateDataObject,
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
    capture_on_sta_thread_with_after_read(|| Ok(()))
}

fn capture_on_sta_thread_with_after_read<F>(after_read: F) -> Result<Option<String>>
where
    F: FnOnce() -> Result<()>,
{
    let _apartment = OleApartment::initialize()?;
    if !wait_for_trigger_keys_release(any_trigger_key_is_down, thread::sleep) {
        // Holding the shortcut is normal user input. Do not inject Ctrl+C
        // while modifiers are held, or touch the clipboard on this path.
        tracing::debug!(
            strategy = "clipboard",
            "Selection copy skipped: trigger keys remain held"
        );
        return Ok(None);
    }

    let snapshot = ClipboardSnapshot::capture()?;
    let before_copy = snapshot.sequence;
    let mut transaction = ClipboardTransaction::new(snapshot);

    if let Err(error) = send_copy_shortcut() {
        // A partial SendInput followed by the cleanup key-up events can still
        // complete the copy. Observe that change so Drop can restore it.
        if let Some(copied_sequence) = wait_for_sequence_change(before_copy) {
            transaction.mark_copy(copied_sequence);
        }
        return Err(error);
    }
    let Some(copied_sequence) = wait_for_sequence_change(before_copy) else {
        #[cfg(test)]
        eprintln!(
            "Clipboard capture: no sequence change within {} ms after SendInput",
            COPY_TIMEOUT.as_millis()
        );
        return Ok(None);
    };
    transaction.mark_copy(copied_sequence);

    let copied_text = read_unicode_text()?;
    #[cfg(test)]
    eprintln!(
        "Clipboard capture: sequence changed; Unicode format present={}, nonblank text={}",
        copied_text.is_some(),
        copied_text
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
    );
    let selected_text = copied_text.and_then(normalize_clipboard_text);
    after_read()?;
    transaction.restore()?;
    Ok(selected_text)
}

pub(super) struct OleApartment;

impl OleApartment {
    pub(super) fn initialize() -> Result<Self> {
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

pub(super) struct ClipboardSnapshot {
    sequence: u32,
    data: Option<IDataObject>,
}

impl ClipboardSnapshot {
    pub(super) fn capture() -> Result<Self> {
        for _ in 0..3 {
            let before = unsafe { GetClipboardSequenceNumber() };
            let data = if unsafe { CountClipboardFormats() } == 0 {
                None
            } else {
                let source = unsafe { OleGetClipboard() }
                    .map_err(|_| Error::new("Could not preserve the clipboard"))?;
                Some(materialize_data_object(&source).map_err(|error| {
                    trace_restore_error("materialize clipboard snapshot", &error);
                    Error::new(
                        "Could not preserve every clipboard format; selection copy cancelled",
                    )
                })?)
            };
            let after = unsafe { GetClipboardSequenceNumber() };

            if before == after {
                return Ok(Self {
                    sequence: after,
                    data,
                });
            }
        }

        Err(Error::new("Clipboard changed while it was being preserved"))
    }

    pub(super) fn set_as_clipboard_contents(&self) -> windows::core::Result<()> {
        // Restoration intentionally uses the OLE data object. An ownerless
        // OpenClipboard/EmptyClipboard sequence cannot legally restore data
        // with SetClipboardData and also mishandles private/custom formats.
        match self.data.as_ref() {
            Some(data) => unsafe { OleSetClipboard(data) },
            None => unsafe { OleSetClipboard(None::<&IDataObject>) },
        }
    }

    pub(super) fn flush(&self) -> Result<()> {
        unsafe { OleFlushClipboard() }.map_err(|error| {
            trace_restore_error("OleFlushClipboard", &error);
            Error::new("Could not finalize clipboard restoration")
        })
    }
}

/// Detaches clipboard data before Ctrl+C invalidates the original clipboard
/// proxy. OLE owns storage media; no format-specific handle freeing is needed.
fn materialize_data_object(source: &IDataObject) -> windows::core::Result<IDataObject> {
    let snapshot: IDataObject = unsafe { SHCreateDataObject(None, None, None::<&IDataObject>) }?;
    let formats = unsafe { source.EnumFormatEtc(DATADIR_GET.0 as u32) }?;
    loop {
        let mut format = [FORMATETC::default()];
        let mut fetched = 0;
        unsafe { formats.Next(&mut format, Some(&mut fetched)) }.ok()?;
        if fetched == 0 {
            break;
        }
        let result = (|| {
            let mut medium = unsafe { source.GetData(&format[0]) }?;
            // Advertise the actual medium, not the source's union of media.
            format[0].tymed = medium.tymed;
            if let Err(error) = unsafe { snapshot.SetData(&format[0], &medium, true) } {
                unsafe { ReleaseStgMedium(&mut medium) };
                return Err(error);
            }
            Ok(())
        })();
        unsafe { CoTaskMemFree(Some(format[0].ptd.cast())) };
        result?;
    }
    Ok(snapshot)
}

fn trace_restore_error(operation: &'static str, error: &windows::core::Error) {
    // The manual test does not install a tracing subscriber. Keep its native
    // diagnostic visible without exposing any clipboard contents.
    #[cfg(test)]
    eprintln!(
        "Clipboard restore: {operation} HRESULT={:#010X}",
        error.code().0 as u32
    );
    tracing::warn!(
        strategy = "clipboard",
        operation,
        hresult = format_args!("{:#010X}", error.code().0 as u32),
        "Windows clipboard restoration failed"
    );
}

struct ClipboardTransaction {
    snapshot: Option<ClipboardSnapshot>,
    copied_sequence: Option<u32>,
    restored: bool,
    preserve_newer: bool,
}

impl ClipboardTransaction {
    fn new(snapshot: ClipboardSnapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            copied_sequence: None,
            restored: false,
            preserve_newer: false,
        }
    }

    fn mark_copy(&mut self, sequence: u32) {
        self.copied_sequence = Some(sequence);
    }

    fn restore(&mut self) -> Result<()> {
        let deadline = Instant::now() + OPEN_CLIPBOARD_TIMEOUT;
        loop {
            match self.restore_once() {
                Err(error) if error.code().0 as u32 == 0x800401D0 && Instant::now() < deadline => {
                    // CLIPBRD_E_CANT_OPEN: nothing was replaced. Recheck the
                    // sequence guard on every retry in case the user copies.
                    thread::sleep(POLL_INTERVAL);
                }
                Err(error) => {
                    trace_restore_error("OleSetClipboard", &error);
                    return Err(Error::new("Could not restore the clipboard"));
                }
                Ok(result) => return result,
            }
        }
    }

    fn restore_once(&mut self) -> windows::core::Result<Result<()>> {
        let current_sequence = unsafe { GetClipboardSequenceNumber() };
        match restore_action(
            self.copied_sequence,
            current_sequence,
            self.restored,
            self.preserve_newer,
        ) {
            RestoreAction::Skip => return Ok(Ok(())),
            RestoreAction::PreserveNewer => {
                self.preserve_newer = true;
                self.snapshot.take();
                tracing::debug!(
                    strategy = "clipboard",
                    "Clipboard changed after selection copy; preserving newer user content"
                );
                return Ok(Ok(()));
            }
            RestoreAction::Restore => {}
        }

        let Some(snapshot) = self.snapshot.as_ref() else {
            return Ok(Ok(()));
        };
        snapshot.set_as_clipboard_contents()?;

        // OleSetClipboard changes the sequence number. Retarget the guard so
        // a failed flush can be retried by Drop without overwriting a newer
        // clipboard update that happens in between.
        self.copied_sequence = Some(unsafe { GetClipboardSequenceNumber() });
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Ok(Ok(()));
        };
        if let Err(error) = snapshot.flush() {
            return Ok(Err(error));
        }
        self.restored = true;
        self.snapshot.take();
        Ok(Ok(()))
    }
}

impl Drop for ClipboardTransaction {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::warn!(error = %error, "Could not restore clipboard after selection capture");
        }
    }
}

fn wait_for_trigger_keys_release(
    mut keys_down: impl FnMut() -> bool,
    mut sleep: impl FnMut(Duration),
) -> bool {
    let mut remaining = KEY_RELEASE_TIMEOUT;
    while keys_down() {
        if remaining.is_zero() {
            return false;
        }
        let interval = remaining.min(POLL_INTERVAL);
        sleep(interval);
        remaining -= interval;
    }
    true
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
        let cleanup = [keyboard_input(VK_C, true), keyboard_input(VK_CONTROL, true)];
        let cleaned = unsafe { SendInput(&cleanup, size_of::<INPUT>() as i32) };
        if cleaned != cleanup.len() as u32 {
            tracing::warn!(
                strategy = "clipboard",
                operation = "SendInput cleanup",
                inserted = cleaned,
                expected = cleanup.len(),
                "Could not release every injected clipboard shortcut key"
            );
        }
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

pub(super) fn read_unicode_text() -> Result<Option<String>> {
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

pub(super) struct OpenClipboardGuard;

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

pub(super) fn open_clipboard_with_retry() -> Result<OpenClipboardGuard> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreAction {
    Skip,
    Restore,
    PreserveNewer,
}

fn restore_action(
    copied: Option<u32>,
    current: u32,
    restored: bool,
    preserve_newer: bool,
) -> RestoreAction {
    if restored || preserve_newer || copied.is_none() {
        RestoreAction::Skip
    } else if copied == Some(current) {
        RestoreAction::Restore
    } else {
        RestoreAction::PreserveNewer
    }
}

fn normalize_clipboard_text(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn held_shortcut_skips_copy_after_bounded_wait() {
        let mut waited = std::time::Duration::ZERO;
        assert!(!super::wait_for_trigger_keys_release(
            || true,
            |delay| waited += delay
        ));
        assert_eq!(waited, super::KEY_RELEASE_TIMEOUT);
    }

    #[test]
    fn released_shortcut_can_proceed_without_waiting_again() {
        let mut states = [true, true, false].into_iter();
        let mut waits = 0;
        assert!(super::wait_for_trigger_keys_release(
            || states.next().unwrap(),
            |_| waits += 1,
        ));
        assert_eq!(waits, 2);
        assert!(super::wait_for_trigger_keys_release(
            || false,
            |_| panic!("unexpected wait")
        ));
    }

    use super::{
        RestoreAction, clipboard_sequence_changed, normalize_clipboard_text, restore_action,
    };
    use windows::Win32::System::{
        Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::{CF_UNICODETEXT, OleFlushClipboard, OleSetClipboard, ReleaseStgMedium},
    };

    fn clipboard_text_data_object(text: &str) -> IDataObject {
        let data: IDataObject =
            unsafe { super::SHCreateDataObject(None, None, None::<&IDataObject>) }.unwrap();
        let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) }.unwrap();
        let pointer = unsafe { GlobalLock(memory) } as *mut u16;
        assert!(!pointer.is_null());
        unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len()) };
        let _ = unsafe { GlobalUnlock(memory) };
        let format = unicode_text_format();
        let mut medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: memory },
            ..Default::default()
        };
        if let Err(error) = unsafe { data.SetData(&format, &medium, true) } {
            unsafe { ReleaseStgMedium(&mut medium) };
            panic!("fixture SetData failed: {error}");
        }
        data
    }

    fn unicode_text_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_UNICODETEXT.0,
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
            ..Default::default()
        }
    }

    fn set_clipboard_text(text: &str) {
        let data = clipboard_text_data_object(text);
        unsafe { OleSetClipboard(&data) }.unwrap();
        unsafe { OleFlushClipboard() }.unwrap();
    }

    fn clipboard_formats() -> Vec<u32> {
        let _clipboard = super::open_clipboard_with_retry()
            .expect("could not open clipboard to inspect formats");
        let mut formats = Vec::new();
        let mut current = 0;
        loop {
            let next =
                unsafe { windows::Win32::System::DataExchange::EnumClipboardFormats(current) };
            if next == 0 {
                break;
            }
            formats.push(next);
            current = next;
        }
        formats
    }

    fn run_non_text_restore_test(required_formats: &[u32], fixture_name: &str) {
        let before = clipboard_formats();
        assert!(
            required_formats
                .iter()
                .any(|format| before.contains(format)),
            "copy a {fixture_name} to the clipboard before running this test"
        );
        eprintln!(
            "{fixture_name} clipboard detected. Switch to the target application and select text. Do not copy or press Alt+X."
        );
        for remaining in (1..=10).rev() {
            eprintln!("Capturing in {remaining} seconds...");
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        let selected = super::capture_selected_text().expect("selection capture should succeed");
        let after = clipboard_formats();
        let missing: Vec<_> = before
            .iter()
            .copied()
            .filter(|format| !after.contains(format))
            .collect();
        eprintln!(
            "Clipboard after capture: original formats preserved={}",
            missing.is_empty()
        );
        assert!(selected.is_some(), "selection was not captured");
        assert!(
            missing.is_empty(),
            "restored clipboard is missing {} original format(s)",
            missing.len()
        );
    }

    #[test]
    fn materialized_object_survives_source_release() {
        let _apartment = super::OleApartment::initialize().unwrap();
        let units: Vec<u16> = "snapshot sentinel".encode_utf16().chain(Some(0)).collect();
        let source = clipboard_text_data_object("snapshot sentinel");
        let snapshot = super::materialize_data_object(&source).unwrap();
        drop(source);
        let format = unicode_text_format();
        let mut captured = unsafe { snapshot.GetData(&format) }.unwrap();
        let memory = unsafe { captured.u.hGlobal };
        let pointer = unsafe { GlobalLock(memory) } as *const u16;
        assert!(!pointer.is_null());
        let matches = unsafe { std::slice::from_raw_parts(pointer, units.len()) } == units;
        let _ = unsafe { GlobalUnlock(memory) };
        unsafe { ReleaseStgMedium(&mut captured) };
        assert!(matches);
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn interactive_newer_clipboard_content_wins() {
        const OLD: &str = "lexift-old-clipboard";
        const NEW: &str = "lexift-newer-clipboard";

        {
            let _apartment = super::OleApartment::initialize().unwrap();
            set_clipboard_text(OLD);
        }
        eprintln!("Switch to the target application and select text. Do not copy or press Alt+X.");
        for remaining in (1..=10).rev() {
            eprintln!("Capturing in {remaining} seconds...");
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let selected = super::capture_on_sta_thread_with_after_read(|| {
            set_clipboard_text(NEW);
            Ok(())
        })
        .expect("selection capture should succeed");
        let after = super::read_unicode_text().expect("clipboard should be readable");

        eprintln!(
            "Clipboard after competing copy: newer content preserved={}",
            after.as_deref() == Some(NEW)
        );
        assert!(selected.is_some(), "selection was not captured");
        assert_eq!(after.as_deref(), Some(NEW));
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn interactive_selection_capture_restores_image_clipboard() {
        use windows::Win32::System::Ole::{CF_BITMAP, CF_DIB, CF_DIBV5, CF_ENHMETAFILE};
        run_non_text_restore_test(
            &[
                u32::from(CF_BITMAP.0),
                u32::from(CF_DIB.0),
                u32::from(CF_DIBV5.0),
                u32::from(CF_ENHMETAFILE.0),
            ],
            "bitmap/image",
        );
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn interactive_selection_capture_restores_file_clipboard() {
        use windows::Win32::System::Ole::CF_HDROP;
        run_non_text_restore_test(&[u32::from(CF_HDROP.0)], "file (CF_HDROP)");
    }

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
        assert_eq!(
            restore_action(Some(43), 44, false, false),
            RestoreAction::PreserveNewer
        );
    }

    #[test]
    fn transaction_restore_state_is_explicit() {
        assert_eq!(restore_action(None, 42, false, false), RestoreAction::Skip);
        assert_eq!(
            restore_action(Some(43), 43, false, false),
            RestoreAction::Restore
        );
        assert_eq!(
            restore_action(Some(43), 43, true, false),
            RestoreAction::Skip
        );
        assert_eq!(
            restore_action(Some(43), 43, false, true),
            RestoreAction::Skip
        );
    }

    #[test]
    fn whitespace_only_clipboard_text_is_not_a_selection() {
        assert_eq!(normalize_clipboard_text(" \r\n\t ".into()), None);
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop session"]
    fn interactive_selection_capture_restores_empty_clipboard() {
        // This opt-in manual test deliberately clears the clipboard fixture.
        // Normal tests never touch the system clipboard.
        eprintln!("Empty-clipboard test: clearing current clipboard contents.");
        {
            let _apartment = super::OleApartment::initialize().unwrap();
            unsafe { super::OleSetClipboard(None::<&super::IDataObject>) }
                .expect("could not clear the clipboard fixture");
            // Destroy the fixture's OLE owner window before sleeping/joining.
            // Otherwise another application's copy can synchronously message
            // this STA while it is blocked instead of pumping messages.
        }
        let format_count = || {
            let _clipboard = super::open_clipboard_with_retry()
                .expect("could not open clipboard to verify empty state");
            unsafe { super::CountClipboardFormats() }
        };
        assert_eq!(format_count(), 0, "clipboard fixture must be empty");
        eprintln!("Switch to the target application and select text. Do not copy or press Alt+X.");
        for remaining in (1..=10).rev() {
            eprintln!("Capturing in {remaining} seconds...");
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let selected = super::capture_selected_text();
        let after = format_count();
        eprintln!("Clipboard after capture: format count={after}");
        assert_eq!(after, 0, "clipboard did not return to the empty state");
        assert!(
            selected
                .expect("selection capture should succeed")
                .is_some(),
            "No selection captured; an empty clipboard alone does not verify restoration"
        );
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
        eprintln!("Switch to the target application and select text. Do not copy or press Alt+X.");
        for remaining in (1..=10).rev() {
            eprintln!("Capturing in {remaining} seconds...");
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        let selected = super::capture_selected_text().expect("selection capture should succeed");
        let after = super::read_unicode_text().expect("restored clipboard should be readable");

        let sentinel_preserved = after.as_deref() == Some(SENTINEL);
        eprintln!("Clipboard after capture: sentinel preserved={sentinel_preserved}");
        assert!(
            sentinel_preserved,
            "original sentinel was not preserved/restored"
        );
        assert!(
            selected.is_some(),
            "No nonblank selection captured; see clipboard diagnostics above. An unchanged sentinel alone does not verify restoration."
        );
    }
}
