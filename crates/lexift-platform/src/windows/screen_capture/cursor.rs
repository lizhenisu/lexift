//! Snapshot the native cursor without putting an HWND or GDI handle into the session.
use lexift_core::domain::{
    geometry::Rect,
    screenshot_cursor::{Cursor, Pixels},
};
use windows::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};

struct Bitmaps(ICONINFO);
impl Drop for Bitmaps {
    fn drop(&mut self) {
        for bitmap in [self.0.hbmColor, self.0.hbmMask] {
            if !bitmap.is_invalid() {
                let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
            }
        }
    }
}

fn dimensions(bitmap: HBITMAP) -> Option<(i32, i32)> {
    let mut info = BITMAP::default();
    let size = std::mem::size_of::<BITMAP>() as i32;
    (unsafe {
        GetObjectW(
            HGDIOBJ(bitmap.0),
            size,
            Some((&mut info as *mut BITMAP).cast()),
        )
    } == size)
        .then_some((info.bmWidth, info.bmHeight))
}

fn bitmap_rgba(bitmap: HBITMAP, dc: HDC, width: i32, height: i32) -> Option<Vec<u8>> {
    if !(1..=512).contains(&width) || !(1..=1024).contains(&height) {
        return None;
    }
    let mut pixels = vec![0; width as usize * height as usize * 4];
    let mut format = BITMAPINFO {
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
    let rows = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut format,
            DIB_RGB_COLORS,
        )
    };
    if rows != height {
        return None;
    }
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(pixels)
}

pub(super) fn read() -> Option<Cursor> {
    let mut native = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut native) }.ok()?;
    if native.flags != CURSOR_SHOWING || native.hCursor.is_invalid() {
        return None;
    }
    let mut info = Bitmaps(ICONINFO::default());
    unsafe { GetIconInfo(HICON(native.hCursor.0), &mut info.0) }.ok()?;
    let color = !info.0.hbmColor.is_invalid();
    let (width, height) = if color {
        dimensions(info.0.hbmColor)?
    } else {
        let (width, height) = dimensions(info.0.hbmMask)?;
        if height % 2 != 0 {
            return None;
        }
        (width, height / 2)
    };
    if !(1..=512).contains(&width)
        || !(1..=512).contains(&height)
        || info.0.xHotspot >= width as u32
        || info.0.yHotspot >= height as u32
    {
        return None;
    }
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return None;
    }
    let pixels = read_pixels(&info.0, dc, width, height, color, HICON(native.hCursor.0));
    let _ = unsafe { DeleteDC(dc) };
    let left = native.ptScreenPos.x.checked_sub(info.0.xHotspot as i32)?;
    let top = native.ptScreenPos.y.checked_sub(info.0.yHotspot as i32)?;
    Some(Cursor {
        bounds: Rect {
            left,
            top,
            right: left.checked_add(width)?,
            bottom: top.checked_add(height)?,
        },
        hotspot: lexift_core::domain::geometry::Point {
            x: native.ptScreenPos.x,
            y: native.ptScreenPos.y,
        },
        pixels: pixels?,
    })
}

fn read_pixels(
    info: &ICONINFO,
    dc: HDC,
    width: i32,
    height: i32,
    color: bool,
    icon: HICON,
) -> Option<Pixels> {
    let rgb = if color {
        Some(bitmap_rgba(info.hbmColor, dc, width, height)?)
    } else {
        None
    };
    if rgb
        .as_ref()
        .is_some_and(|pixels| pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0))
    {
        return render_alpha(dc, icon, width, height).map(Pixels::Alpha);
    }
    let mask_height = if color { height } else { height * 2 };
    if dimensions(info.hbmMask)? != (width, mask_height) {
        return None;
    }
    let mask = bitmap_rgba(info.hbmMask, dc, width, mask_height)?;
    let count = width as usize * height as usize;
    let and = mask[..count * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0])
        .collect();
    let xor = rgb
        .as_deref()
        .unwrap_or(&mask[count * 4..])
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Some(Pixels::Raster { and, xor })
}

/// GDI can interpret the icon's color bitmap differently from raw GetDIBits
/// bytes. Rendering over black and white recovers the effective blend.
fn render_alpha(dc: HDC, icon: HICON, width: i32, height: i32) -> Option<Vec<u8>> {
    let format = BITMAPINFO {
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
    let mut bits = std::ptr::null_mut();
    let bitmap =
        unsafe { CreateDIBSection(Some(dc), &format, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    let object = HGDIOBJ(bitmap.0);
    let old = unsafe { SelectObject(dc, object) };
    let result = (|| {
        if old.is_invalid() || bits.is_null() {
            return None;
        }
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(bits.cast::<u8>(), width as usize * height as usize * 4)
        };
        let draw = |bytes: &mut [u8], value| {
            bytes.fill(value);
            for pixel in bytes.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            unsafe {
                DrawIconEx(dc, 0, 0, icon, width, height, 0, None, DI_NORMAL).ok()?;
                let _ = GdiFlush();
            }
            Some(())
        };
        draw(bytes, 0)?;
        let mut rgba = bytes.to_vec();
        draw(bytes, 255)?;
        for (black, white) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(bytes.as_chunks::<4>().0)
        {
            let transmission = white[1].saturating_sub(black[1]);
            black[3] = 255 - transmission;
            black.swap(0, 2);
        }
        Some(rgba)
    })();
    if !old.is_invalid() {
        let _ = unsafe { SelectObject(dc, old) };
    }
    let _ = unsafe { DeleteObject(object) };
    result
}
