//! Revalidates the original focused editable element immediately before input injection.
use lexift_core::{
    Error, Result,
    domain::{clipboard_history::PasteTarget, selection::SelectionTarget},
    ports::{
        clipboard::ClipboardPort, clipboard_history::ClipboardPastePort, selection::SelectionPort,
    },
};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Ole::{SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound},
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern,
                UIA_IsReadOnlyAttributeId, UIA_TextEditPatternId, UIA_ValuePatternId,
            },
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
                VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
            },
        },
    },
    core::Interface,
};
pub(crate) struct Paste;
fn error() -> Error {
    Error::new("The original selection is not a supported editable input.")
}
struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|_| error())?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
fn focused(target: SelectionTarget) -> Result<Vec<i32>> {
    if super::super::selection::WindowsSelectionPort::new().passive_target() != Some(target) {
        return Err(Error::new(
            "The original input has changed. Select text again.",
        ));
    }
    let _apartment = Apartment::new()?;
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| error())?;
    let element = unsafe { automation.GetFocusedElement() }.map_err(|_| error())?;
    unsafe {
        if !element.CurrentIsEnabled().map_err(|_| error())?.as_bool()
            || element.CurrentIsPassword().map_err(|_| error())?.as_bool()
        {
            return Err(error());
        }
        // A TextEdit pattern alone does not prove writability. Value read-only
        // wins when available; rich editors must expose an explicit false text attribute.
        let editable = if let Ok(value) = element
            .GetCurrentPattern(UIA_ValuePatternId)
            .and_then(|p| p.cast::<IUIAutomationValuePattern>())
        {
            value.CurrentIsReadOnly().is_ok_and(|r| !r.as_bool())
        } else if let Ok(text) = element
            .GetCurrentPattern(UIA_TextEditPatternId)
            .and_then(|p| p.cast::<IUIAutomationTextPattern>())
        {
            text.GetSelection()
                .and_then(|ranges| ranges.GetElement(0))
                .and_then(|range| range.GetAttributeValue(UIA_IsReadOnlyAttributeId))
                .is_ok_and(|mut value| {
                    let writable = value.Anonymous.Anonymous.vt
                        == windows::Win32::System::Variant::VT_BOOL
                        && !value.Anonymous.Anonymous.Anonymous.boolVal.as_bool();
                    let _ = windows::Win32::System::Variant::VariantClear(&mut value);
                    writable
                })
        } else {
            false
        };
        if !editable {
            return Err(error());
        }
        let array = element.GetRuntimeId().map_err(|_| error())?;
        struct Array(*mut windows::Win32::System::Com::SAFEARRAY);
        impl Drop for Array {
            fn drop(&mut self) {
                unsafe {
                    let _ = SafeArrayDestroy(self.0);
                }
            }
        }
        let array = Array(array);
        let lower = SafeArrayGetLBound(array.0, 1).map_err(|_| error())?;
        let upper = SafeArrayGetUBound(array.0, 1).map_err(|_| error())?;
        if upper < lower || i64::from(upper) - i64::from(lower) > 128 {
            return Err(error());
        }
        let mut id = Vec::new();
        for index in lower..=upper {
            let mut value = 0i32;
            SafeArrayGetElement(array.0, &index, (&mut value as *mut i32).cast())
                .map_err(|_| error())?;
            id.push(value);
        }
        Ok(id)
    }
}
impl ClipboardPastePort for Paste {
    fn capture_current_target(&self) -> Result<PasteTarget> {
        let target = super::super::selection::WindowsSelectionPort::new()
            .passive_target()
            .ok_or_else(error)?;
        if target.process == std::process::id() {
            return Err(error());
        }
        self.capture_target(target).map_err(|_| error())
    }
    fn capture_target(&self, target: SelectionTarget) -> Result<PasteTarget> {
        Ok(PasteTarget {
            selection: target,
            runtime_id: focused(target)?,
        })
    }
    fn paste(&self, target: &PasteTarget, text: &str, cancelled: &AtomicBool) -> Result<()> {
        let check = || {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::new("Clipboard paste was cancelled."));
            }
            if focused(target.selection)? != target.runtime_id {
                return Err(Error::new(
                    "The original input has changed. Select text again.",
                ));
            }
            Ok(())
        };
        check()?;
        if [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
            .iter()
            .any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } < 0)
        {
            return Err(Error::new("Release modifier keys and try again."));
        }
        super::super::clipboard::WindowsClipboardPort
            .write_text(text)
            .map_err(|_| Error::new("Clipboard paste could not be sent to this input."))?;
        check()?;
        let key = |vk, up| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                },
            },
        };
        if [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
            .iter()
            .any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } < 0)
        {
            return Err(Error::new("Release modifier keys and try again."));
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::new("Clipboard paste was cancelled."));
        }
        let inputs = [
            key(VK_CONTROL, false),
            key(VK_V, false),
            key(VK_V, true),
            key(VK_CONTROL, true),
        ];
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent != 4 {
            if sent > 0 {
                let cleanup = [key(VK_V, true), key(VK_CONTROL, true)];
                unsafe {
                    SendInput(&cleanup, std::mem::size_of::<INPUT>() as i32);
                }
            }
            return Err(Error::new(
                "Clipboard paste could not be sent to this input.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_request_never_opens_clipboard_or_injects_input() {
        let sequence =
            unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let target = PasteTarget {
            selection: SelectionTarget {
                window: lexift_core::ports::magnifier::WindowToken(0),
                focus: None,
                process: 0,
                thread: 0,
            },
            runtime_id: vec![],
        };
        let error = Paste
            .paste(&target, "must not be written", &AtomicBool::new(true))
            .unwrap_err();
        assert_eq!(error.to_string(), "Clipboard paste was cancelled.");
        assert_eq!(sequence, unsafe {
            windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
        });
    }
}
