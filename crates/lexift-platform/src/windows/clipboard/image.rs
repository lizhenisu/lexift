//! Copy clipboard-owned handles while locked; decode only owned bytes after closing it.
use super::*;
use crate::windows::annotation_save::import::{decode, dimensions};
use lexift_core::{domain::geometry::Rect, ports::screen_capture::CapturedRegion};
use windows::Win32::{
    Foundation::HGLOBAL,
    Graphics::Gdi::*,
    System::{
        DataExchange::{GetClipboardData, IsClipboardFormatAvailable, RegisterClipboardFormatW},
        Memory::GlobalSize,
        Ole::{CF_BITMAP, CF_DIB, CF_DIBV5},
    },
};

pub(super) fn read() -> Result<Option<CapturedRegion>> {
    let (format, bytes) = {
        let _guard = open_clipboard_with_retry()?;
        let png = unsafe { RegisterClipboardFormatW(windows::core::w!("PNG")) };
        let format = [
            png,
            u32::from(CF_DIBV5.0),
            u32::from(CF_DIB.0),
            u32::from(CF_BITMAP.0),
        ]
        .into_iter()
        .find(|f| *f != 0 && unsafe { IsClipboardFormatAvailable(*f) }.is_ok());
        let Some(format) = format else {
            return Ok(None);
        };
        let handle = unsafe { GetClipboardData(format) }
            .map_err(|_| Error::new("Could not read clipboard image"))?;
        let bytes = if format == u32::from(CF_BITMAP.0) {
            bitmap(HBITMAP(handle.0))?
        } else {
            let global = HGLOBAL(handle.0);
            let size = unsafe { GlobalSize(global) };
            if size == 0 || size > 160_000_000 {
                return Err(Error::new("Invalid or oversized clipboard image"));
            }
            let data = unsafe { GlobalLock(global) };
            if data.is_null() {
                return Err(Error::new("Could not lock clipboard image"));
            }
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) }.to_vec();
            unsafe {
                let _ = GlobalUnlock(global);
            }
            bytes
        };
        (format == png, bytes)
    };
    if format {
        decode(&bytes).map(Some)
    } else {
        dib(&bytes).map(Some)
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| Error::new("Truncated bitmap"))?
            .try_into()
            .unwrap(),
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| Error::new("Truncated bitmap"))?
            .try_into()
            .unwrap(),
    ))
}
fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    (((u64::from((value & mask) >> shift) * 255) + u64::from(max) / 2) / u64::from(max)) as u8
}
/// Validate every layout boundary before decoding an untrusted packed DIB.
fn dib(bytes: &[u8]) -> Result<CapturedRegion> {
    let header = u32_at(bytes, 0)? as usize;
    if ![40, 52, 56, 108, 124].contains(&header) || bytes.len() < header {
        return Err(Error::new("Unsupported bitmap header"));
    }
    let width = u32_at(bytes, 4)? as i32;
    let signed_height = u32_at(bytes, 8)? as i32;
    if width <= 0 || signed_height == i32::MIN {
        return Err(Error::new("Invalid bitmap dimensions"));
    }
    let height = signed_height.unsigned_abs();
    let width = width as u32;
    let length = dimensions(width, height)?;
    let bits = u16_at(bytes, 14)?;
    let compression = u32_at(bytes, 16)?;
    if u16_at(bytes, 12)? != 1
        || ![1, 4, 8, 16, 24, 32].contains(&bits)
        || ![0, 3, 6].contains(&compression)
        || (compression != 0 && bits != 16 && bits != 32)
    {
        return Err(Error::new("Unsupported bitmap encoding"));
    }
    let mut offset = header;
    let (r, g, b, a) = if compression != 0 {
        let masks = if header == 40 {
            offset += if compression == 6 { 16 } else { 12 };
            40
        } else {
            40
        };
        (
            u32_at(bytes, masks)?,
            u32_at(bytes, masks + 4)?,
            u32_at(bytes, masks + 8)?,
            if header >= 56 || compression == 6 {
                u32_at(bytes, masks + 12)?
            } else {
                0
            },
        )
    } else if bits == 16 {
        (0x7c00, 0x3e0, 0x1f, 0)
    } else {
        (
            0xff0000,
            0xff00,
            0xff,
            if header >= 108 { u32_at(bytes, 52)? } else { 0 },
        )
    };
    if bits >= 16 && bits != 24 {
        let masks = [r, g, b, a];
        for (i, mask) in masks.iter().copied().enumerate() {
            let shifted = mask.checked_shr(mask.trailing_zeros()).unwrap_or(0);
            if (i < 3 && mask == 0)
                || (bits < 32 && mask >> bits != 0)
                || shifted & shifted.wrapping_add(1) != 0
                || masks[..i].iter().any(|m| m & mask != 0)
            {
                return Err(Error::new("Invalid bitmap masks"));
            }
        }
    }
    let colors = u32_at(bytes, 32)? as usize;
    let palette = offset;
    if bits <= 8 {
        let count = if colors == 0 { 1usize << bits } else { colors };
        if count > 1usize << bits {
            return Err(Error::new("Invalid bitmap palette"));
        }
        offset += count * 4;
    } else {
        offset = offset
            .checked_add(
                colors
                    .checked_mul(4)
                    .ok_or_else(|| Error::new("Invalid bitmap palette"))?,
            )
            .ok_or_else(|| Error::new("Invalid bitmap palette"))?;
    }
    let stride = ((u64::from(width) * u64::from(bits)).div_ceil(32) * 4) as usize;
    let pixel_bytes = stride
        .checked_mul(height as usize)
        .ok_or_else(|| Error::new("Invalid bitmap stride"))?;
    // Windows can synthesize V5 from a legacy BI_BITFIELDS DIB while retaining
    // its external RGB masks. Accept that layout only when the declared size,
    // total allocation, absent profile and duplicate masks agree exactly.
    // Direct V5 data (including pixels that resemble masks) keeps its offset.
    if header == 124
        && colors == 0
        && compression == 3
        && a == 0
        && u32_at(bytes, 20)? as usize == pixel_bytes
        && u32_at(bytes, 112)? == 0
        && u32_at(bytes, 116)? == 0
        && offset
            .checked_add(pixel_bytes)
            .and_then(|n| n.checked_add(12))
            == Some(bytes.len())
        && u32_at(bytes, offset)? == r
        && u32_at(bytes, offset + 4)? == g
        && u32_at(bytes, offset + 8)? == b
    {
        offset += 12;
    }
    let end = offset
        .checked_add(
            stride
                .checked_mul(height as usize)
                .ok_or_else(|| Error::new("Invalid bitmap stride"))?,
        )
        .ok_or_else(|| Error::new("Invalid bitmap stride"))?;
    if end > bytes.len() {
        return Err(Error::new("Truncated bitmap pixels"));
    }
    let mut pixels = vec![0; length];
    for y in 0..height as usize {
        let row = offset
            + if signed_height > 0 {
                height as usize - 1 - y
            } else {
                y
            } * stride;
        for x in 0..width as usize {
            let (red, green, blue, alpha) = match bits {
                1 | 4 | 8 => {
                    let index = match bits {
                        1 => (bytes[row + x / 8] >> (7 - x % 8)) & 1,
                        4 => (bytes[row + x / 2] >> (if x % 2 == 0 { 4 } else { 0 })) & 15,
                        _ => bytes[row + x],
                    } as usize;
                    if palette + index * 4 + 4 > offset {
                        return Err(Error::new("Invalid bitmap palette index"));
                    }
                    let p = palette + index * 4;
                    (bytes[p + 2], bytes[p + 1], bytes[p], 255)
                }
                24 => {
                    let p = row + x * 3;
                    (bytes[p + 2], bytes[p + 1], bytes[p], 255)
                }
                _ => {
                    let p = row + x * (bits as usize / 8);
                    let v = if bits == 16 {
                        u32::from(u16_at(bytes, p)?)
                    } else {
                        u32_at(bytes, p)?
                    };
                    (
                        channel(v, r),
                        channel(v, g),
                        channel(v, b),
                        if a == 0 { 255 } else { channel(v, a) },
                    )
                }
            };
            let p = (y * width as usize + x) * 4;
            let premul = |c: u8| ((u16::from(c) * u16::from(alpha) + 127) / 255) as u8;
            pixels[p..p + 4].copy_from_slice(&[premul(red), premul(green), premul(blue), alpha]);
        }
    }
    Ok(CapturedRegion {
        bounds: Rect {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        },
        pixels,
    })
}
fn bitmap(handle: HBITMAP) -> Result<Vec<u8>> {
    let mut object = BITMAP::default();
    if unsafe {
        GetObjectW(
            handle.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut object as *mut BITMAP).cast()),
        )
    } == 0
        || object.bmWidth <= 0
        || object.bmHeight <= 0
    {
        return Err(Error::new("Invalid clipboard bitmap"));
    }
    let length = dimensions(object.bmWidth as u32, object.bmHeight as u32)?;
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = 40;
    info.bmiHeader.biWidth = object.bmWidth;
    info.bmiHeader.biHeight = -object.bmHeight;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    let mut bytes = vec![0; 40 + length];
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return Err(Error::new("Could not read clipboard bitmap"));
    }
    let rows = unsafe {
        GetDIBits(
            dc,
            handle,
            0,
            object.bmHeight as u32,
            Some(bytes[40..].as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        let _ = DeleteDC(dc);
    }
    if rows != object.bmHeight {
        return Err(Error::new("Could not read clipboard bitmap"));
    }
    let header = unsafe {
        std::slice::from_raw_parts(
            (&info.bmiHeader as *const BITMAPINFOHEADER).cast::<u8>(),
            40,
        )
    };
    bytes[..40].copy_from_slice(header);
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthesized_v5_masks_do_not_shift_pixels_or_change_direct_v5() {
        let mut bytes = vec![0; 124];
        bytes[..4].copy_from_slice(&124u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[16..20].copy_from_slice(&3u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&8u32.to_le_bytes());
        let masks = [0xff0000u32, 0xff00, 0xff];
        for (i, mask) in masks.iter().enumerate() {
            bytes[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
        }
        let pixels = [30, 20, 10, 0, 60, 50, 40, 0];
        let mut direct = bytes.clone();
        direct.extend_from_slice(&pixels);
        assert_eq!(
            dib(&direct).unwrap().pixels,
            [10, 20, 30, 255, 40, 50, 60, 255]
        );
        for mask in masks {
            bytes.extend_from_slice(&mask.to_le_bytes());
        }
        bytes.extend_from_slice(&pixels);
        assert_eq!(dib(&bytes).unwrap().pixels, dib(&direct).unwrap().pixels);
        // Extra bytes without the precise duplicate-mask signature are not skipped.
        bytes[124] = 1;
        assert_ne!(dib(&bytes).unwrap().pixels, dib(&direct).unwrap().pixels);
    }
    #[test]
    #[ignore = "writes a generated legacy DIB to the real Windows clipboard"]
    fn native_legacy_dib_synthesis_preserves_every_pixel() {
        let mut bytes = fixture();
        bytes.resize(68, 0);
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[16..20].copy_from_slice(&3u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&8u32.to_le_bytes());
        for (i, mask) in [0xff0000u32, 0xff00, 0xff].iter().enumerate() {
            bytes[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
        }
        bytes[52..60].copy_from_slice(&[30, 20, 10, 0, 60, 50, 40, 0]);
        bytes.truncate(60);
        let memory = OwnedGlobal::allocate(bytes.len()).unwrap();
        let destination = unsafe { GlobalLock(memory.handle()) };
        assert!(!destination.is_null());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast::<u8>(), bytes.len());
            let _ = GlobalUnlock(memory.handle());
        }
        {
            let _guard = open_clipboard_with_retry().unwrap();
            unsafe {
                EmptyClipboard().unwrap();
                SetClipboardData(u32::from(CF_DIB.0), Some(HANDLE(memory.handle().0))).unwrap();
            }
            memory.transfer_to_clipboard();
        }
        assert_eq!(
            read().unwrap().unwrap().pixels,
            [10, 20, 30, 255, 40, 50, 60, 255]
        );
    }
    #[test]
    fn explicit_alpha_and_palette_are_validated_and_premultiplied() {
        let mut bytes = vec![0; 128];
        bytes[..4].copy_from_slice(&124u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes());
        bytes[16..20].copy_from_slice(&3u32.to_le_bytes());
        for (offset, mask) in [
            (40, 0xff0000u32),
            (44, 0xff00),
            (48, 0xff),
            (52, 0xff000000),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&mask.to_le_bytes());
        }
        bytes[124..].copy_from_slice(&[32, 64, 128, 128]);
        assert_eq!(dib(&bytes).unwrap().pixels, [64, 32, 16, 128]);
        bytes[44..48].copy_from_slice(&0xff0000u32.to_le_bytes());
        assert!(dib(&bytes).is_err());
        let mut palette = vec![0; 52];
        palette[..40].copy_from_slice(&fixture()[..40]);
        palette[14..16].copy_from_slice(&1u16.to_le_bytes());
        palette[40..48].copy_from_slice(&[0, 0, 0, 0, 30, 20, 10, 0]);
        palette[48] = 0x80;
        assert_eq!(
            dib(&palette).unwrap().pixels,
            [10, 20, 30, 255, 0, 0, 0, 255]
        );
    }
    #[test]
    #[ignore = "writes a generated fixture to the real Windows clipboard"]
    fn native_clipboard_roundtrip_preserves_owned_pixels() {
        let image = CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 2,
                bottom: 1,
            },
            pixels: vec![255, 10, 20, 255, 64, 32, 16, 128],
        };
        let port = super::super::WindowsClipboardPort;
        port.write_image(&image).unwrap();
        assert_eq!(port.read_image().unwrap().unwrap().pixels, image.pixels);
    }
    fn fixture() -> Vec<u8> {
        let mut b = vec![0; 48];
        b[..4].copy_from_slice(&40u32.to_le_bytes());
        b[4..8].copy_from_slice(&2i32.to_le_bytes());
        b[8..12].copy_from_slice(&1i32.to_le_bytes());
        b[12..14].copy_from_slice(&1u16.to_le_bytes());
        b[14..16].copy_from_slice(&24u16.to_le_bytes());
        b[40..46].copy_from_slice(&[3, 2, 1, 6, 5, 4]);
        b
    }
    #[test]
    fn dib_padding_direction_and_bounds() {
        let b = fixture();
        assert_eq!(dib(&b).unwrap().pixels, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert!(dib(&b[..45]).is_err());
        let mut b = b;
        b[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(dib(&b).unwrap().pixels[0], 1);
        b[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(dib(&b).is_err());
    }
}
