//! A job-owned STA dialog and WIC encoder. No COM interface leaves its apartment.
use lexift_core::{
    Error, Result,
    ports::annotation_save::{AnnotationSavePort, PngSaveRequest, SaveOutcome},
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
            CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
            IWICImagingFactory, WICBitmapEncoderNoCache,
        },
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
                StructuredStorage::CreateStreamOnHGlobal,
            },
            Threading::GetCurrentThreadId,
        },
        UI::{
            Shell::{
                Common::COMDLG_FILTERSPEC, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT,
                FOS_PATHMUSTEXIST, FOS_STRICTFILETYPES, FileSaveDialog, IFileSaveDialog,
                SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible, PostMessageW, WM_CLOSE},
        },
    },
    core::{BOOL, PCWSTR, w},
};

pub(crate) struct WindowsAnnotationSavePort;

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

impl AnnotationSavePort for WindowsAnnotationSavePort {
    fn is_available(&self) -> bool {
        true
    }
    fn save_png(&self, request: PngSaveRequest, cancelled: Arc<AtomicBool>) -> Result<SaveOutcome> {
        request.validate()?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        let _apartment = Apartment::new()?;
        let dialog: IFileSaveDialog =
            unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) }
                .map_err(native_error)?;
        let title = wide(request.title.as_ref());
        let name = wide(request.default_name.as_ref());
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
                .SetFileTypes(&[COMDLG_FILTERSPEC {
                    pszName: w!("PNG (*.png)"),
                    pszSpec: w!("*.png"),
                }])
                .map_err(native_error)?;
            dialog
                .SetDefaultExtension(w!("png"))
                .map_err(native_error)?;
            dialog
                .SetFileName(PCWSTR(name.as_ptr()))
                .map_err(native_error)?;
            dialog
                .SetTitle(PCWSTR(title.as_ptr()))
                .map_err(native_error)?;
        }
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
        let bytes = encode_png(&request)?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(SaveOutcome::Cancelled);
        }
        Ok(if write_png(&path, &bytes, &cancelled)? {
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

fn encode_png(request: &PngSaveRequest) -> Result<Vec<u8>> {
    request.validate()?;
    let width =
        (i64::from(request.image.bounds.right) - i64::from(request.image.bounds.left)) as u32;
    let height =
        (i64::from(request.image.bounds.bottom) - i64::from(request.image.bounds.top)) as u32;
    let mut rgba = request.image.pixels.clone();
    for p in rgba.as_chunks_mut::<4>().0 {
        let alpha = u32::from(p[3]);
        if alpha != 0 && alpha != 255 {
            for color in &mut p[..3] {
                *color = ((u32::from(*color) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
        p.swap(0, 2);
    }
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(native_error)?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true).map_err(native_error)?;
        let encoder = factory
            .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
            .map_err(native_error)?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(native_error)?;
        let mut frame = None;
        let mut options = None;
        encoder
            .CreateNewFrame(&mut frame, &mut options)
            .map_err(native_error)?;
        let frame = frame.ok_or_else(|| Error::new("PNG encoder did not create a frame"))?;
        frame.Initialize(options.as_ref()).map_err(native_error)?;
        frame.SetSize(width, height).map_err(native_error)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format).map_err(native_error)?;
        if format != GUID_WICPixelFormat32bppBGRA {
            return Err(Error::new("PNG encoder rejected BGRA"));
        }
        frame
            .WritePixels(height, width * 4, &rgba)
            .map_err(native_error)?;
        frame.Commit().map_err(native_error)?;
        encoder.Commit().map_err(native_error)?;
        let mut stat = STATSTG::default();
        stream
            .Stat(&mut stat, STATFLAG_NONAME)
            .map_err(native_error)?;
        let length = u32::try_from(stat.cbSize).map_err(|_| Error::new("PNG is too large"))?;
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
            return Err(Error::new("Incomplete PNG encoding"));
        }
        Ok(bytes)
    }
}

/// Replace only after the complete file has been written beside the destination.
fn write_png(path: &Path, bytes: &[u8], cancelled: &AtomicBool) -> Result<bool> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_file_name(format!(
        ".lexift-{}-{}.png.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|e| Error::new(e.to_string()))?;
    let _temporary = Temporary(temp.clone());
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| Error::new(e.to_string()))?;
    drop(file);
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
    fn request() -> PngSaveRequest {
        PngSaveRequest {
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
            let encoded = encode_png(&request).unwrap();
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
    fn cancelled_save_does_not_create_dialog_or_file() {
        assert_eq!(
            WindowsAnnotationSavePort
                .save_png(request(), Arc::new(AtomicBool::new(true)))
                .unwrap(),
            SaveOutcome::Cancelled
        );
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
            let _ = tx.send(WindowsAnnotationSavePort.save_png(request, cancel));
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
        write_png(&path, b"next", &AtomicBool::new(true)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"previous");
        write_png(&path, b"next", &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"next");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&directory).unwrap();
        assert!(
            write_png(
                &directory.join("missing.png"),
                b"next",
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}
