//! Native image selection and bounded WIC decoding. COM references stay on this STA.
use super::*;
use lexift_core::{
    domain::geometry::Rect,
    ports::{annotation_save::ImageOpenRequest, screen_capture::CapturedRegion},
};
use std::io::Read;
use windows::Win32::{
    Graphics::Imaging::*,
    System::Com::StructuredStorage::{PROPVARIANT, PropVariantToUInt32},
    UI::Shell::{Common::COMDLG_FILTERSPEC, FOS_FILEMUSTEXIST, FileOpenDialog, IFileOpenDialog},
};

pub(crate) fn dimensions(width: u32, height: u32) -> Result<usize> {
    let count = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || count > 40_000_000 {
        return Err(Error::new("Invalid or oversized image"));
    }
    usize::try_from(count * 4).map_err(|_| Error::new("Invalid image size"))
}

pub(super) fn open(
    request: ImageOpenRequest,
    cancelled: Arc<AtomicBool>,
) -> Result<Option<CapturedRegion>> {
    let _apartment = Apartment::new()?;
    let _shell_policy = super::super::window_shell_policy::ThreadGuard::new()?;
    if cancelled.load(Ordering::Acquire) {
        return Ok(None);
    }
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(native_error)?;
    let title = wide(request.title.as_ref());
    unsafe {
        dialog
            .SetOptions(
                dialog.GetOptions().map_err(native_error)?
                    | FOS_FORCEFILESYSTEM
                    | FOS_FILEMUSTEXIST
                    | FOS_PATHMUSTEXIST,
            )
            .map_err(native_error)?;
        dialog
            .SetTitle(PCWSTR(title.as_ptr()))
            .map_err(native_error)?;
        dialog
            .SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("PNG / JPEG"),
                pszSpec: w!("*.png;*.jpg;*.jpeg"),
            }])
            .map_err(native_error)?;
    }
    let thread_id = unsafe { GetCurrentThreadId() };
    let done = AtomicBool::new(false);
    let shown = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if cancelled.load(Ordering::Acquire) {
                    unsafe {
                        let _ = EnumThreadWindows(thread_id, Some(close_dialog), LPARAM(0));
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let result = if cancelled.load(Ordering::Acquire) {
            Err(windows::core::Error::from_hresult(
                windows::core::HRESULT::from_win32(ERROR_CANCELLED.0),
            ))
        } else {
            unsafe { dialog.Show(request.owner.map(|t| HWND(t.0 as *mut _))) }
        };
        done.store(true, Ordering::Release);
        result
    });
    if cancelled.load(Ordering::Acquire)
        || shown
            .as_ref()
            .is_err_and(|e| e.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0))
    {
        return Ok(None);
    }
    shown.map_err(native_error)?;
    let item = unsafe { dialog.GetResult() }.map_err(native_error)?;
    let raw = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(native_error)?;
    let path = unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    let path = PathBuf::from(path.map_err(|_| Error::new("Invalid image path"))?);
    // Bound encoded input as well as decoded pixels, before reading an untrusted file.
    let file = std::fs::File::open(path).map_err(|_| Error::new("Could not read image"))?;
    if file
        .metadata()
        .map_err(|_| Error::new("Could not read image"))?
        .len()
        > 160_000_000
    {
        return Err(Error::new("Image file is too large"));
    }
    let mut bytes = Vec::new();
    file.take(160_000_001)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::new("Could not read image"))?;
    if bytes.len() > 160_000_000 {
        return Err(Error::new("Image file is too large"));
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(None);
    }
    let image = decode(&bytes)?;
    Ok((!cancelled.load(Ordering::Acquire)).then_some(image))
}

pub(crate) fn decode(bytes: &[u8]) -> Result<CapturedRegion> {
    let _apartment = Apartment::new()?;
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(native_error)?;
        let stream = factory.CreateStream().map_err(native_error)?;
        stream.InitializeFromMemory(bytes).map_err(native_error)?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(native_error)?;
        let format = decoder.GetContainerFormat().map_err(native_error)?;
        if format != GUID_ContainerFormatPng && format != GUID_ContainerFormatJpeg {
            return Err(Error::new("Unsupported image format"));
        }
        let frame = decoder.GetFrame(0).map_err(native_error)?;
        let (mut width, mut height) = (0, 0);
        frame
            .GetSize(&mut width, &mut height)
            .map_err(native_error)?;
        let length = dimensions(width, height)?;
        let orientation = if format == GUID_ContainerFormatJpeg {
            frame
                .GetMetadataQueryReader()
                .ok()
                .and_then(|r| {
                    let mut p = PROPVARIANT::default();
                    r.GetMetadataByName(w!("/app1/ifd/{ushort=274}"), &mut p)
                        .ok()
                        .map(|_| p)
                })
                .and_then(|p| PropVariantToUInt32(&p).ok())
                .filter(|v| (1..=8).contains(v))
                .unwrap_or(1)
        } else {
            1
        };
        let converter = factory.CreateFormatConverter().map_err(native_error)?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(native_error)?;
        let mut pixels = vec![0; length];
        converter
            .CopyPixels(std::ptr::null(), width * 4, &mut pixels)
            .map_err(native_error)?;
        orient(width, height, pixels, orientation)
    }
}

/// EXIF transforms operate on integer source pixels, preserving premultiplied channels.
fn orient(width: u32, height: u32, pixels: Vec<u8>, orientation: u32) -> Result<CapturedRegion> {
    let (ow, oh) = if orientation >= 5 {
        (height, width)
    } else {
        (width, height)
    };
    let output = if orientation == 1 {
        pixels
    } else {
        let mut out = vec![0; dimensions(ow, oh)?];
        for y in 0..height {
            for x in 0..width {
                let (dx, dy) = match orientation {
                    2 => (width - 1 - x, y),
                    3 => (width - 1 - x, height - 1 - y),
                    4 => (x, height - 1 - y),
                    5 => (y, x),
                    6 => (height - 1 - y, x),
                    7 => (height - 1 - y, width - 1 - x),
                    8 => (y, width - 1 - x),
                    _ => (x, y),
                };
                let src = ((y * width + x) * 4) as usize;
                let dst = ((dy * ow + dx) * 4) as usize;
                out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
            }
        }
        out
    };
    Ok(CapturedRegion {
        bounds: Rect {
            left: 0,
            top: 0,
            right: ow as i32,
            bottom: oh as i32,
        },
        pixels: output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wic_checks_container_alpha_and_exif_before_installation() {
        std::thread::spawn(|| {
            let _a = Apartment::new().unwrap();
            let request = ImageSaveRequest {
                image: CapturedRegion {
                    bounds: Rect {
                        left: 0,
                        top: 0,
                        right: 2,
                        bottom: 1,
                    },
                    pixels: vec![0, 0, 0, 0, 64, 32, 16, 128],
                },
                owner: None,
                default_name: String::new(),
                title: String::new(),
                options: lexift_core::ports::annotation_save::ImageSaveOptions::png_and_jpeg(
                    String::new(),
                ),
            };
            let png = super::super::encode_image(&request, ImageFormat::Png).unwrap();
            assert_eq!(decode(&png).unwrap().pixels, request.image.pixels);
            assert!(decode(b"not an image").is_err());
            assert!(decode(&png[..20]).is_err());
            let jpeg = super::super::encode_image(&request, ImageFormat::Jpeg).unwrap();
            // One EXIF SHORT orientation entry in a little-endian TIFF IFD.
            let exif = [
                b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0,
                1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
            ];
            let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
            oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
            oriented.extend_from_slice(&exif);
            oriented.extend_from_slice(&jpeg[2..]);
            let image = decode(&oriented).unwrap();
            assert_eq!((image.bounds.right, image.bounds.bottom), (1, 2));
        })
        .join()
        .unwrap();
    }
    #[test]
    fn limits_and_exif_transforms() {
        assert!(dimensions(u32::MAX, u32::MAX).is_err());
        assert!(dimensions(0, 1).is_err());
        let pixels = [1u8, 2, 3, 4, 5, 6]
            .into_iter()
            .flat_map(|v| [v, 0, 0, 255])
            .collect::<Vec<_>>();
        for orientation in 1..=8 {
            let image = orient(2, 3, pixels.clone(), orientation).unwrap();
            assert_eq!(image.pixels.len(), 24);
        }
        assert_eq!(
            orient(2, 3, pixels, 6)
                .unwrap()
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            [5, 3, 1, 6, 4, 2]
        );
    }
}
