//! One-shot screen pixels. All GDI objects are retired before returning to callers.
use lexift_core::{
    Error, Result,
    domain::geometry::Rect,
    ports::{screen::ScreenPort, screen_capture::CapturedRegion},
};
use windows::Win32::Graphics::Gdi::*;

pub(super) fn freeze() -> Result<Vec<CapturedRegion>> {
    super::screen::WindowsScreenPort::new()
        .displays()?
        .into_iter()
        .map(capture)
        .collect()
}

fn capture(bounds: Rect) -> Result<CapturedRegion> {
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let length = i64::from(width)
        .checked_mul(i64::from(height))
        .filter(|n| *n > 0 && *n <= 40_000_000)
        .ok_or_else(|| Error::new("Screen capture is too large"))? as usize
        * 4;
    let mut pixels = vec![0u8; length];
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(Error::new("Could not capture desktop"));
        }
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        if memory.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() {
                let _ = DeleteObject(bitmap.into());
            }
            if !memory.is_invalid() {
                let _ = DeleteDC(memory);
            }
            ReleaseDC(None, screen);
            return Err(Error::new("Could not allocate desktop capture"));
        }
        let previous = SelectObject(memory, bitmap.into());
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            Some(screen),
            bounds.left,
            bounds.top,
            SRCCOPY | CAPTUREBLT,
        );
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        if copied.is_err() || rows != height {
            return Err(Error::new("Could not capture desktop"));
        }
    }
    for p in pixels.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        p[3] = 255;
    }
    Ok(CapturedRegion { bounds, pixels })
}
