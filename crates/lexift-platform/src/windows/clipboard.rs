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
    fn write_image(
        &self,
        image: &lexift_core::ports::screen_capture::CapturedRegion,
    ) -> Result<()> {
        let data = encode_image(image)?;
        let memory = OwnedGlobal::allocate(data.len())?;
        let destination = unsafe { GlobalLock(memory.handle()) } as *mut u8;
        if destination.is_null() {
            return Err(Error::new("Could not allocate clipboard image"));
        }
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), destination, data.len());
            let _ = GlobalUnlock(memory.handle());
        }
        let _clipboard = open_clipboard_with_retry()?;
        unsafe { EmptyClipboard() }
            .map_err(|_| Error::new("Could not clear the Windows clipboard"))?;
        unsafe {
            SetClipboardData(
                u32::from(windows::Win32::System::Ole::CF_DIBV5.0),
                Some(HANDLE(memory.handle().0)),
            )
        }
        .map_err(|_| Error::new("Could not write image to the Windows clipboard"))?;
        memory.transfer_to_clipboard();
        Ok(())
    }

    fn read_text(&self) -> Result<Option<String>> {
        read_unicode_text()
    }

    fn write_text(&self, text: &str) -> Result<()> {
        write_text(text, false)
    }
    fn write_sensitive_text(&self, text: &str) -> Result<()> {
        write_text(text, true)
    }
}
fn write_text(text: &str, sensitive: bool) -> Result<()> {
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
    unsafe { EmptyClipboard() }.map_err(|_| Error::new("Could not clear the Windows clipboard"))?;
    if sensitive {
        let format = unsafe {
            windows::Win32::System::DataExchange::RegisterClipboardFormatW(windows::core::w!(
                "ExcludeClipboardContentFromMonitorProcessing"
            ))
        };
        if format == 0 {
            return Err(Error::new("Could not exclude sensitive clipboard text"));
        }
        let marker = OwnedGlobal::allocate(4)?;
        let p = unsafe { GlobalLock(marker.handle()) } as *mut u32;
        if p.is_null() {
            return Err(Error::new("Could not exclude sensitive clipboard text"));
        }
        unsafe {
            p.write(1);
            let _ = GlobalUnlock(marker.handle());
        }
        unsafe { SetClipboardData(format, Some(HANDLE(marker.handle().0))) }
            .map_err(|_| Error::new("Could not exclude sensitive clipboard text"))?;
        marker.transfer_to_clipboard();
    }
    unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.handle().0))) }
        .map_err(|_| Error::new("Could not write text to the Windows clipboard"))?;
    memory.transfer_to_clipboard();
    Ok(())
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

/// Packed top-down DIBV5: explicit BGRA masks and straight-alpha pixels.
fn encode_image(image: &lexift_core::ports::screen_capture::CapturedRegion) -> Result<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
    let w = i64::from(image.bounds.right) - i64::from(image.bounds.left);
    let h = i64::from(image.bounds.bottom) - i64::from(image.bounds.top);
    let bytes = w
        .checked_mul(h)
        .filter(|n| *n > 0 && *n <= 40_000_000)
        .and_then(|n| usize::try_from(n * 4).ok());
    if w <= 0
        || h <= 0
        || w > i64::from(i32::MAX)
        || h > i64::from(i32::MAX)
        || bytes != Some(image.pixels.len())
    {
        return Err(Error::new("Invalid clipboard image"));
    }
    let header = BITMAPV5HEADER {
        bV5Size: mem::size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: w as i32,
        bV5Height: -(h as i32),
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: image.pixels.len() as u32,
        bV5RedMask: 0x00ff0000,
        bV5GreenMask: 0x0000ff00,
        bV5BlueMask: 0x000000ff,
        bV5AlphaMask: 0xff000000,
        bV5CSType: 0x73524742,
        ..Default::default()
    };
    let mut data = Vec::with_capacity(mem::size_of_val(&header) + image.pixels.len());
    data.extend_from_slice(unsafe {
        std::slice::from_raw_parts(
            (&header as *const BITMAPV5HEADER).cast::<u8>(),
            mem::size_of_val(&header),
        )
    });
    for rgba in image.pixels.as_chunks::<4>().0 {
        let straight = |value: u8| {
            if rgba[3] == 0 {
                0
            } else {
                ((u32::from(value) * 255 + u32::from(rgba[3]) / 2) / u32::from(rgba[3])).min(255)
                    as u8
            }
        };
        data.extend_from_slice(&[
            straight(rgba[2]),
            straight(rgba[1]),
            straight(rgba[0]),
            rgba[3],
        ]);
    }
    Ok(data)
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use lexift_core::{domain::geometry::Rect, ports::screen_capture::CapturedRegion};
    #[test]
    fn dib_v5_preserves_orientation_masks_and_unpremultiplies() {
        let image = CapturedRegion {
            bounds: Rect {
                left: -10,
                top: 20,
                right: -8,
                bottom: 21,
            },
            pixels: vec![64, 32, 16, 128, 20, 40, 60, 255],
        };
        let bytes = encode_image(&image).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 124);
        assert_eq!(i32::from_le_bytes(bytes[8..12].try_into().unwrap()), -1);
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            0x00ff0000
        );
        assert_eq!(&bytes[124..], &[32, 64, 128, 128, 60, 40, 20, 255]);
    }
    #[test]
    fn malformed_image_is_rejected_before_clipboard_changes() {
        assert!(
            encode_image(&CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10
                },
                pixels: vec![0; 4]
            })
            .is_err()
        );
    }
}
