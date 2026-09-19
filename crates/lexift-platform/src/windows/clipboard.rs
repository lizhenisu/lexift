use std::{mem, ptr};

use lexift_core::{Error, Result, ports::clipboard::ClipboardPort};
use windows::Win32::{
    Foundation::{GlobalFree, HANDLE, HGLOBAL},
    System::{
        DataExchange::{EmptyClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
};

use super::clipboard_selection::{open_clipboard_with_retry, read_unicode_text};

pub(crate) struct WindowsClipboardPort;

impl ClipboardPort for WindowsClipboardPort {
    fn read_text(&self) -> Result<Option<String>> {
        read_unicode_text()
    }

    fn write_text(&self, text: &str) -> Result<()> {
        let encoded = encode_unicode_clipboard_text(text);
        let bytes = encoded.len() * mem::size_of::<u16>();
        let memory = OwnedGlobal::allocate(bytes)?;
        let destination = unsafe { GlobalLock(memory.handle()) } as *mut u16;
        if destination.is_null() {
            return Err(Error::new("Could not allocate clipboard text"));
        }
        unsafe { ptr::copy_nonoverlapping(encoded.as_ptr(), destination, encoded.len()) };
        let _ = unsafe { GlobalUnlock(memory.handle()) };

        let _clipboard = open_clipboard_with_retry()?;
        unsafe { EmptyClipboard() }
            .map_err(|_| Error::new("Could not clear the Windows clipboard"))?;
        unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.handle().0))) }
            .map_err(|_| Error::new("Could not write text to the Windows clipboard"))?;
        memory.transfer_to_clipboard();
        Ok(())
    }
}

fn encode_unicode_clipboard_text(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

struct OwnedGlobal(HGLOBAL);

impl OwnedGlobal {
    fn allocate(bytes: usize) -> Result<Self> {
        unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }
            .map(Self)
            .map_err(|_| Error::new("Could not allocate clipboard text"))
    }

    fn handle(&self) -> HGLOBAL {
        self.0
    }

    fn transfer_to_clipboard(self) {
        mem::forget(self);
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        let _ = unsafe { GlobalFree(Some(self.0)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_is_utf16_and_null_terminated() {
        assert_eq!(
            encode_unicode_clipboard_text("Lexift 密钥"),
            "Lexift 密钥"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "mutates the interactive Windows clipboard while running"]
    fn writes_unicode_text_and_restores_the_complete_clipboard_snapshot() {
        use crate::windows::clipboard_selection::{ClipboardSnapshot, OleApartment};

        let _apartment = OleApartment::initialize().expect("OLE should initialize");
        let snapshot = ClipboardSnapshot::capture().expect("clipboard should be preserved");
        let port = WindowsClipboardPort;
        let result = (|| {
            port.write_text("Lexift clipboard 密钥")?;
            let actual = port.read_text()?;
            if actual.as_deref() != Some("Lexift clipboard 密钥") {
                return Err(Error::new("clipboard Unicode round trip did not match"));
            }
            Ok(())
        })();
        snapshot
            .set_as_clipboard_contents()
            .expect("clipboard snapshot should restore");
        snapshot.flush().expect("clipboard restore should finalize");
        result.expect("clipboard adapter should round trip Unicode text");
    }
}
