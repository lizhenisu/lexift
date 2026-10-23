//! A job-owned STA dialog and WIC encoder. No COM interface leaves its apartment.
mod dialog;
pub(super) mod import;

use lexift_core::{
    Error, Result,
    ports::annotation_save::{ImageFilePort, ImageFormat, ImageSaveRequest, SaveOutcome},
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{ERROR_CANCELLED, HGLOBAL, HWND, LPARAM, WPARAM},
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_ContainerFormatPng,
            GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
            WICBitmapEncoderNoCache,
        },
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
                StructuredStorage::{CreateStreamOnHGlobal, PROPBAG2},
            },
            Threading::GetCurrentThreadId,
            Variant::VARIANT,
        },
        UI::{
            Shell::{
                FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FOS_STRICTFILETYPES,
                FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible, PostMessageW, WM_CLOSE},
        },
    },
    core::{BOOL, PCWSTR, w},
};

pub(crate) struct WindowsImageFilePort;

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map_err(native_error)?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
fn native_error(e: windows::core::Error) -> Error {
    Error::new(e.to_string())
}
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(Some(0)).collect()
}

impl ImageFilePort for WindowsImageFilePort {
    fn open_image(
        &self,
        request: lexift_core::ports::annotation_save::ImageOpenRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Option<lexift_core::ports::screen_capture::CapturedRegion>> {
        import::open(request, cancelled)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn save_image(
        &self,
        request: ImageSaveRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<SaveOutcome> {
        request.validate()?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        let _shell_policy = super::window_shell_policy::ThreadGuard::new()?;
        let _apartment = Apartment::new()?;
        let dialog: IFileSaveDialog =
            unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(native_error)?;
        let title = wide(request.title.as_ref());
        // Let the shell supply the selected extension. Passing an explicit .png
        // would pin the initial suffix even after the user selects JPEG.
        let default_name = Path::new(&request.default_name);
        let default_name = if default_name
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| {
                request
                    .options
                    .formats
                    .iter()
                    .any(|f| f.accepts_extension(e))
            }) {
            default_name.with_extension("")
        } else {
            default_name.to_path_buf()
        };
        let name = wide(default_name.as_os_str());
        unsafe {
            dialog
                .SetOptions(
                    dialog.GetOptions().map_err(native_error)?
                        | FOS_FORCEFILESYSTEM
                        | FOS_OVERWRITEPROMPT
                        | FOS_PATHMUSTEXIST
                        | FOS_STRICTFILETYPES,
                )
                .map_err(native_error)?;
            dialog
                .SetFileName(PCWSTR(name.as_ptr()))
                .map_err(native_error)?;
            dialog
                .SetTitle(PCWSTR(title.as_ptr()))
                .map_err(native_error)?;
        }
        let events = dialog::configure(&dialog, &request.options)?;
        // Show pumps this STA. A cancellation observer posts only to windows of
        // this job's thread, including a confirmation dialog nested inside Show.
        // It starts before Show, covering cancellation during dialog creation.
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
        drop(events);
        if cancelled.load(Ordering::Acquire)
            || shown
                .as_ref()
                .is_err_and(|e| e.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0))
        {
            return Ok(SaveOutcome::Cancelled);
        }
        shown.map_err(native_error)?;
        let item = unsafe { dialog.GetResult() }.map_err(native_error)?;
        let raw = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(native_error)?;
        let path = unsafe { raw.to_string() };
        unsafe { CoTaskMemFree(Some(raw.0.cast())) };
        let path = PathBuf::from(path.map_err(|e| Error::new(e.to_string()))?);
        let format = dialog::selected_format(&dialog, &request.options).map_err(native_error)?;
        // Do not mutate the path after the native overwrite confirmation.
        if !dialog::extension_matches(&path, format) {
            return Err(Error::new(
                "Image file extension does not match its encoding",
            ));
        }
        let bytes = encode_image(&request, format)?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        Ok(if write_image(&path, &bytes, &cancelled)? {
            SaveOutcome::Saved
        } else {
            SaveOutcome::Cancelled
        })
    }
}

unsafe extern "system" fn close_dialog(window: HWND, _: LPARAM) -> BOOL {
    if unsafe { IsWindowVisible(window) }.as_bool() {
        unsafe {
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    BOOL(1)
}

fn encode_image(request: &ImageSaveRequest, format: ImageFormat) -> Result<Vec<u8>> {
    request.validate()?;
    let width =
        (i64::from(request.image.bounds.right) - i64::from(request.image.bounds.left)) as u32;
    let height =
        (i64::from(request.image.bounds.bottom) - i64::from(request.image.bounds.top)) as u32;
    let pixels = encoding_pixels(&request.image.pixels, format);
    let stride = width
        * match format {
            ImageFormat::Png => 4,
            ImageFormat::Jpeg => 3,
        };
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(native_error)?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true).map_err(native_error)?;
        let encoder = factory
            .CreateEncoder(
                &match format {
                    ImageFormat::Png => GUID_ContainerFormatPng,
                    ImageFormat::Jpeg => GUID_ContainerFormatJpeg,
                },
                std::ptr::null(),
            )
            .map_err(native_error)?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(native_error)?;
        let mut frame = None;
        let mut options = None;
        encoder
            .CreateNewFrame(&mut frame, &mut options)
            .map_err(native_error)?;
        let frame = frame.ok_or_else(|| Error::new("Image encoder did not create a frame"))?;
        if format == ImageFormat::Jpeg {
            let options = options
                .as_ref()
                .ok_or_else(|| Error::new("JPEG encoder has no quality options"))?;
            let property = PROPBAG2 {
                pstrName: windows::core::PWSTR(w!("ImageQuality").0.cast_mut()),
                ..Default::default()
            };
            let quality = VARIANT::from(ImageFormat::JPEG_QUALITY);
            options
                .Write(1, &property, &quality)
                .map_err(native_error)?;
        }
        frame.Initialize(options.as_ref()).map_err(native_error)?;
        frame.SetSize(width, height).map_err(native_error)?;
        let expected = match format {
            ImageFormat::Png => GUID_WICPixelFormat32bppBGRA,
            ImageFormat::Jpeg => GUID_WICPixelFormat24bppBGR,
        };
        let mut pixel_format = expected;
        frame
            .SetPixelFormat(&mut pixel_format)
            .map_err(native_error)?;
        if pixel_format != expected {
            return Err(Error::new(
                "Image encoder rejected the requested pixel format",
            ));
        }
        frame
            .WritePixels(height, stride, &pixels)
            .map_err(native_error)?;
        frame.Commit().map_err(native_error)?;
        encoder.Commit().map_err(native_error)?;
        let mut stat = STATSTG::default();
        stream
            .Stat(&mut stat, STATFLAG_NONAME)
            .map_err(native_error)?;
        let length =
            u32::try_from(stat.cbSize).map_err(|_| Error::new("Encoded image is too large"))?;
        stream
            .Seek(0, STREAM_SEEK_SET, None)
            .map_err(native_error)?;
        let mut bytes = vec![0; length as usize];
        let mut read = 0;
        stream
            .Read(bytes.as_mut_ptr().cast(), length, Some(&mut read))
            .ok()
            .map_err(native_error)?;
        if read != length {
            return Err(Error::new("Incomplete image encoding"));
        }
        Ok(bytes)
    }
}

/// Source channels are premultiplied. JPEG composites directly onto white;
/// unpremultiplying first would introduce unnecessary rounding at translucent edges.
fn encoding_pixels(source: &[u8], format: ImageFormat) -> Vec<u8> {
    let mut pixels =
        Vec::with_capacity(source.len() / 4 * if format == ImageFormat::Png { 4 } else { 3 });
    for &[r, g, b, a] in source.as_chunks::<4>().0 {
        match format {
            ImageFormat::Png => {
                let straight = |c: u8| {
                    if a == 0 {
                        0
                    } else {
                        ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                    }
                };
                pixels.extend_from_slice(&[straight(b), straight(g), straight(r), a]);
            }
            ImageFormat::Jpeg => {
                let white = |c: u8| c.saturating_add(255 - a);
                pixels.extend_from_slice(&[white(b), white(g), white(r)]);
            }
        }
    }
    pixels
}

/// Replace only after the complete file has been written beside the destination.
fn write_image(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<bool> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_file_name(format!(
        ".lexift-{}-{}.image.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|e| Error::new(e.to_string()))?;
    let _temporary = Temporary(temp.clone());
    {
        // Drop the handle before the temporary-path guard on every error path.
        let mut file = file;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| Error::new(e.to_string()))?;
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(false);
    }
    let source = wide(temp.as_os_str());
    let destination = wide(path.as_os_str());
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(native_error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{domain::geometry::Rect, ports::screen_capture::CapturedRegion};
    fn request() -> ImageSaveRequest {
        ImageSaveRequest {
            image: CapturedRegion {
                bounds: Rect {
                    left: -2,
                    top: -1,
                    right: 0,
                    bottom: 0,
                },
                pixels: vec![200, 10, 20, 255, 10, 200, 20, 255],
            },
            owner: None,
            default_name: "test.png".into(),
            title: String::new(),
            options: lexift_core::ports::annotation_save::ImageSaveOptions::png_and_jpeg(
                "Extension mismatch".into(),
            ),
        }
    }
    #[test]
    fn native_png_round_trip_preserves_dimensions_and_channels() {
        use windows::Win32::Graphics::Imaging::{
            GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
            WICDecodeMetadataCacheOnLoad,
        };
        std::thread::spawn(|| {
            let _apartment = Apartment::new().unwrap();
            let request = request();
            let encoded = encode_image(&request, ImageFormat::Png).unwrap();
            assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
            unsafe {
                let factory: IWICImagingFactory =
                    CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
                let stream = factory.CreateStream().unwrap();
                stream.InitializeFromMemory(&encoded).unwrap();
                let decoder = factory
                    .CreateDecoderFromStream(
                        &stream,
                        std::ptr::null(),
                        WICDecodeMetadataCacheOnLoad,
                    )
                    .unwrap();
                let frame = decoder.GetFrame(0).unwrap();
                let (mut width, mut height) = (0, 0);
                frame.GetSize(&mut width, &mut height).unwrap();
                assert_eq!((width, height), (2, 1));
                let converter = factory.CreateFormatConverter().unwrap();
                converter
                    .Initialize(
                        &frame,
                        &GUID_WICPixelFormat32bppRGBA,
                        WICBitmapDitherTypeNone,
                        None,
                        0.,
                        WICBitmapPaletteTypeCustom,
                    )
                    .unwrap();
                let mut pixels = vec![0; 8];
                converter
                    .CopyPixels(std::ptr::null(), 8, &mut pixels)
                    .unwrap();
                assert_eq!(pixels, request.image.pixels);
            }
        })
        .join()
        .unwrap();
    }
    #[test]
    fn transparent_pixels_use_white_for_jpeg_and_straight_alpha_for_png() {
        let rgba = [0, 0, 0, 0, 64, 32, 16, 128, 200, 100, 20, 255];
        assert_eq!(
            encoding_pixels(&rgba, ImageFormat::Jpeg),
            [255, 255, 255, 143, 159, 191, 20, 100, 200]
        );
        assert_eq!(
            encoding_pixels(&rgba, ImageFormat::Png),
            [0, 0, 0, 0, 32, 64, 128, 128, 20, 100, 200, 255]
        );
    }

    #[test]
    fn native_jpeg_round_trip_preserves_size_white_alpha_and_colour() {
        use windows::Win32::Graphics::Imaging::{
            GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
            WICDecodeMetadataCacheOnLoad,
        };
        std::thread::spawn(|| {
            let _apartment = Apartment::new().unwrap();
            let mut request = request();
            request.image.bounds = lexift_core::domain::geometry::Rect {
                left: -20,
                top: -10,
                right: 76,
                bottom: 54,
            };
            let colours = [[0, 0, 0, 0], [64, 32, 16, 128], [200, 100, 20, 255]];
            request.image.pixels = (0..64)
                .flat_map(|_| (0..96).flat_map(|x| colours[x / 32]))
                .collect();
            let bytes = encode_image(&request, ImageFormat::Jpeg).unwrap();
            assert_eq!(&bytes[..2], &[255, 216]);
            assert_eq!(&bytes[bytes.len() - 2..], &[255, 217]);
            unsafe {
                let factory: IWICImagingFactory =
                    CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
                let stream = factory.CreateStream().unwrap();
                stream.InitializeFromMemory(&bytes).unwrap();
                let decoder = factory
                    .CreateDecoderFromStream(
                        &stream,
                        std::ptr::null(),
                        WICDecodeMetadataCacheOnLoad,
                    )
                    .unwrap();
                let frame = decoder.GetFrame(0).unwrap();
                let (mut w, mut h) = (0, 0);
                frame.GetSize(&mut w, &mut h).unwrap();
                assert_eq!((w, h), (96, 64));
                let converter = factory.CreateFormatConverter().unwrap();
                converter
                    .Initialize(
                        &frame,
                        &GUID_WICPixelFormat24bppRGB,
                        WICBitmapDitherTypeNone,
                        None,
                        0.,
                        WICBitmapPaletteTypeCustom,
                    )
                    .unwrap();
                let mut pixels = vec![0u8; 96 * 64 * 3];
                converter
                    .CopyPixels(std::ptr::null(), 96 * 3, &mut pixels)
                    .unwrap();
                for (x, expected) in [
                    (16, [255u8, 255, 255]),
                    (48, [191, 159, 143]),
                    (80, [200, 100, 20]),
                ] {
                    let offset = (32 * 96 + x) * 3;
                    for (actual, expected) in pixels[offset..offset + 3].iter().zip(expected) {
                        assert!(
                            actual.abs_diff(expected) <= 3,
                            "JPEG colour mismatch: {actual} vs {expected}"
                        );
                    }
                }
                if let Ok(folder) = std::env::var("LEXIFT_IMAGE_SAVE_FIXTURES") {
                    std::fs::create_dir_all(&folder).unwrap();
                    std::fs::write(Path::new(&folder).join("alpha-colour.jpg"), bytes).unwrap();
                    std::fs::write(
                        Path::new(&folder).join("alpha-colour.png"),
                        encode_image(&request, ImageFormat::Png).unwrap(),
                    )
                    .unwrap();
                }
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn cancelled_save_does_not_create_dialog_or_file() {
        assert_eq!(
            WindowsImageFilePort
                .save_image(request(), Arc::new(AtomicBool::new(true)))
                .unwrap(),
            SaveOutcome::Cancelled
        );
    }

    #[test]
    #[ignore = "interactive save/extension/overwrite fixture; requires LEXIFT_IMAGE_SAVE_NATIVE_DIR"]
    fn native_format_dialog_round_trip() {
        let directory = std::path::PathBuf::from(
            std::env::var("LEXIFT_IMAGE_SAVE_NATIVE_DIR")
                .expect("set an ignored target/ fixture directory"),
        );
        std::fs::create_dir_all(&directory).unwrap();
        for (name, signature) in [
            ("port.jpg", &b"\xff\xd8"[..]),
            ("port.png", &b"\x89PNG"[..]),
        ] {
            let mut request = request();
            request.title = "Lexift image save format regression".into();
            request.default_name = directory.join("fixture.png").to_string_lossy().into_owned();
            assert_eq!(
                WindowsImageFilePort
                    .save_image(request, Arc::new(AtomicBool::new(false)))
                    .unwrap(),
                SaveOutcome::Saved
            );
            assert!(
                std::fs::read(directory.join(name))
                    .unwrap()
                    .starts_with(signature)
            );
        }
    }

    #[test]
    #[ignore = "opens and cancels a job-owned native dialog; requires interactive Windows"]
    fn cancellation_closes_visible_sta_dialog_and_joins_its_observer() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut request = request();
            request.title = "Lexift PNG cancellation regression".into();
            let _ = tx.send(WindowsImageFilePort.save_image(request, cancel));
        });
        // The cancellation observer also covers dialog construction, so this
        // verifies closure whether Show has already created its native window.
        std::thread::sleep(Duration::from_millis(500));
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(),
            SaveOutcome::Cancelled
        );
        worker.join().unwrap();
    }
    #[test]
    fn png_write_preserves_existing_file_on_cancellation_and_handles_unicode() {
        let directory =
            std::env::temp_dir().join(format!("lexift-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("标注截图.png");
        std::fs::write(&path, b"previous").unwrap();
        write_image(&path, b"next", &AtomicBool::new(true)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"previous");
        write_image(&path, b"next", &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"next");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory).unwrap();
        assert!(
            write_image(
                &directory.join("missing.png"),
                b"next",
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}
