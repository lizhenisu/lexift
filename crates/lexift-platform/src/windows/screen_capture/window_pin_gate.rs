use super::*;
use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    Foundation::TypedEventHandler,
    Win32::{
        Foundation::COLORREF,
        Graphics::{Dwm::DwmFlush, Gdi::*},
        System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        UI::HiDpi::GetDpiForWindow,
    },
    core::w,
};

const ROI: RECT = RECT {
    left: 40,
    top: 48,
    right: 1320,
    bottom: 688,
};

fn clock() -> i64 {
    let (mut now, mut frequency) = (0, 0);
    unsafe {
        QueryPerformanceCounter(&mut now).unwrap();
        QueryPerformanceFrequency(&mut frequency).unwrap();
    }
    (i128::from(now) * 10_000_000 / i128::from(frequency)) as i64
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

/// Convert stable client-relative logical edges to the current WGC surface.
fn crop(hwnd: HWND, size: windows::Graphics::SizeInt32) -> Option<Rect> {
    use windows::Win32::Foundation::POINT;
    let mut client = RECT::default();
    let mut frame = RECT::default();
    let mut origin = POINT::default();
    unsafe {
        GetClientRect(hwnd, &mut client).ok()?;
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut frame as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .ok()?;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 || frame.right - frame.left != size.Width || frame.bottom - frame.top != size.Height
    {
        return None;
    }
    let left = scale(ROI.left, dpi);
    let top = scale(ROI.top, dpi);
    let right = scale(ROI.right, dpi);
    let bottom = scale(ROI.bottom, dpi);
    if right > client.right || bottom > client.bottom {
        return None;
    }
    let (dx, dy) = (origin.x - frame.left, origin.y - frame.top);
    let region = Rect {
        left: dx + left,
        top: dy + top,
        right: dx + right,
        bottom: dy + bottom,
    };
    (region.left >= 0
        && region.top >= 0
        && region.right <= size.Width
        && region.bottom <= size.Height)
        .then_some(region)
}

struct Sample {
    pixels: CapturedRegion,
    time: i64,
    acquired: Instant,
    readback: Duration,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<Sample>,
    stopped: bool,
    source_closed: bool,
    published: bool,
    overwritten: u64,
    gpu_peak_estimate: usize,
    crop_peak: usize,
    error: Option<String>,
}

struct Worker {
    mailbox: Arc<Mutex<Mailbox>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn start(source: HWND) -> Self {
        let token = source.0 as isize;
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let shared = mailbox.clone();
        let thread = thread::spawn(move || {
            let result = capture(HWND(token as _), &shared);
            let mut state = shared.lock().unwrap();
            if let Err(error) = result {
                state.error = Some(error.to_string());
            } else if !state.stopped && !state.published {
                state.error = Some("Source closed before a usable frame".into());
            }
            state.stopped = true;
            state.pending = None;
        });
        Self {
            mailbox,
            thread: Some(thread),
        }
    }

    fn take(&self) -> Option<Sample> {
        let mut state = self.mailbox.lock().unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        state.pending.take()
    }

    fn stop(&mut self) {
        {
            let mut state = self.mailbox.lock().unwrap();
            state.stopped = true;
            state.pending = None;
        }
        if let Some(worker) = self.thread.take() {
            worker.thread().unpark();
            worker.join().unwrap();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture(hwnd: HWND, shared: &Arc<Mutex<Mailbox>>) -> windows::core::Result<()> {
    if shared.lock().unwrap().stopped {
        return Ok(());
    }
    let backend = Backend::new()?;
    let owner = thread::current();
    let arrival = crate::windows::screen_capture::arrival::Notice::new({
        let owner = owner.clone();
        move || owner.unpark()
    });
    let mut capture = WindowCapture::new(
        hwnd.0 as isize,
        &backend.interop,
        &backend.winrt,
        Some(&arrival),
    )?;
    let ended = shared.clone();
    let subscription = capture.item.Closed(&TypedEventHandler::new(move |_, _| {
        let mut state = ended.lock().unwrap();
        state.source_closed = true;
        state.pending = None;
        owner.unpark();
        Ok(())
    }))?;
    let mut suspended = false;
    let mut geometry = None;
    let mut fresh_after = clock();
    let result = (|| {
        loop {
            let ended = {
                let state = shared.lock().unwrap();
                state.source_closed || state.stopped
            };
            if ended {
                break;
            }
            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                break;
            }
            let iconic = unsafe { IsIconic(hwnd) }.as_bool();
            let next_geometry = if iconic {
                None
            } else {
                crop(hwnd, capture.size)
            };
            if iconic != suspended || next_geometry != geometry {
                let mut state = shared.lock().unwrap();
                state.pending = None;
                suspended = iconic;
                geometry = next_geometry;
                fresh_after = clock();
                drop(state);
                drop(latest_pending(CAPTURE_BUFFERS as usize, || {
                    capture.pool.TryGetNextFrame().ok().map(FrameLease)
                }));
            }
            arrival.take();
            if let Some(frame) = latest_pending(CAPTURE_BUFFERS as usize, || {
                capture.pool.TryGetNextFrame().ok().map(FrameLease)
            }) {
                let size = frame.0.ContentSize()?;
                let time = frame.0.SystemRelativeTime()?.Duration;
                if !suspended && time >= fresh_after && size.Width > 0 && size.Height > 0 {
                    if size != capture.size {
                        drop(frame);
                        capture.staging = None;
                        capture.pool.Recreate(
                            &backend.winrt,
                            DirectXPixelFormat::B8G8R8A8UIntNormalized,
                            CAPTURE_BUFFERS,
                            size,
                        )?;
                        capture.size = size;
                    } else if let Some(region) = geometry {
                        let started = Instant::now();
                        let access: IDirect3DDxgiInterfaceAccess = frame.0.Surface()?.cast()?;
                        capture.texture = Some(unsafe { access.GetInterface()? });
                        let mut pixels = CapturedRegion {
                            bounds: region,
                            pixels: vec![],
                        };
                        let bounds = Rect {
                            left: 0,
                            top: 0,
                            right: size.Width,
                            bottom: size.Height,
                        };
                        let read = capture.read_crop(
                            bounds,
                            region,
                            &backend.device,
                            &backend.context,
                            &mut pixels,
                        );
                        capture.texture = None;
                        if read?
                            && pixels.bounds == region
                            && crop(hwnd, size) == Some(region)
                            && !unsafe { IsIconic(hwnd) }.as_bool()
                        {
                            let mut state = shared.lock().unwrap();
                            if !state.stopped && !state.source_closed {
                                state.published = true;
                                let bytes = pixels.pixels.len();
                                state.crop_peak = state.crop_peak.max(bytes);
                                state.gpu_peak_estimate = state.gpu_peak_estimate.max(
                                    size.Width as usize
                                        * size.Height as usize
                                        * 4
                                        * CAPTURE_BUFFERS as usize
                                        + bytes,
                                );
                                state.overwritten += u64::from(
                                    state
                                        .pending
                                        .replace(Sample {
                                            pixels,
                                            time,
                                            acquired: started,
                                            readback: started.elapsed(),
                                        })
                                        .is_some(),
                                );
                            }
                        }
                    }
                }
            }
            thread::park_timeout(Duration::from_millis(16));
        }
        Ok(())
    })();
    capture.item.RemoveClosed(subscription)?;
    result
}

struct SourceBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    size: (i32, i32, u32),
}
impl Drop for SourceBuffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.original);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Window(HWND, std::cell::RefCell<Option<SourceBuffer>>);
impl Window {
    fn new(x: i32, y: i32, width: i32, height: i32, preview: bool) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | if preview {
                        WS_EX_TOPMOST
                    } else {
                        WINDOW_EX_STYLE(0)
                    },
                w!("STATIC"),
                w!("Lexift synthetic window pin fixture"),
                WS_POPUP,
                x,
                y,
                width,
                height,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Self(hwnd, Default::default())
    }

    fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        unsafe {
            SetWindowPos(
                self.0,
                None,
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
        }
    }

    fn dpi(&self) -> u32 {
        unsafe { GetDpiForWindow(self.0) }
    }

    fn paint(&self, paint: impl FnOnce(HDC)) {
        unsafe {
            let dc = GetDC(Some(self.0));
            assert!(!dc.is_invalid());
            paint(dc);
            assert_ne!(ReleaseDC(Some(self.0), dc), 0);
            GdiFlush().unwrap();
        }
    }

    fn source(&self, id: u32) {
        let dpi = self.dpi();
        self.paint(|window_dc| {
            let mut client = RECT::default();
            unsafe {
                GetClientRect(self.0, &mut client).unwrap();
            }
            let (width, height) = (client.right, client.bottom);
            let size = (width, height, dpi);
            let mut retained = self.1.borrow_mut();
            if retained.as_ref().is_none_or(|buffer| buffer.size != size) {
                let dc = unsafe { CreateCompatibleDC(Some(window_dc)) };
                let bitmap = unsafe { CreateCompatibleBitmap(window_dc, width, height) };
                assert!(!dc.is_invalid() && !bitmap.is_invalid());
                let original = unsafe { SelectObject(dc, bitmap.into()) };
                *retained = Some(SourceBuffer {
                    dc,
                    bitmap,
                    original,
                    size,
                });
                fill(dc, client, 0x573321);
                for (rect, color) in [
                    (ROI, 0x2090d0),
                    (
                        RECT {
                            left: 240,
                            top: 128,
                            right: 620,
                            bottom: 412,
                        },
                        0x8d3fe3,
                    ),
                    (
                        RECT {
                            left: 720,
                            top: 360,
                            right: 1230,
                            bottom: 600,
                        },
                        0x3fc45b,
                    ),
                ] {
                    fill(dc, scale_rect(rect, dpi), color);
                }
            }
            let dc = retained.as_ref().unwrap().dc;
            fill(
                dc,
                scale_rect(
                    RECT {
                        left: 80,
                        top: 88,
                        right: 160,
                        bottom: 160,
                    },
                    dpi,
                ),
                id | 0x400000,
            );
            // A single blit prevents WGC from sampling halfway through painting the grid.
            unsafe {
                BitBlt(window_dc, 0, 0, width, height, Some(dc), 0, 0, SRCCOPY).unwrap();
            }
        });
    }

    fn present(&self, sample: &Sample, bgra: &mut Vec<u8>) {
        let rect = sample.pixels.bounds;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        bgra.clear();
        bgra.extend_from_slice(&sample.pixels.pixels);
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        self.paint(|dc| unsafe {
            let mut client = RECT::default();
            GetClientRect(self.0, &mut client).unwrap();
            let bitmap = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(
                StretchDIBits(
                    dc,
                    0,
                    0,
                    client.right,
                    client.bottom,
                    0,
                    0,
                    width,
                    height,
                    Some(bgra.as_ptr().cast()),
                    &bitmap,
                    DIB_RGB_COLORS,
                    SRCCOPY
                ) > 0
            );
        });
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0).unwrap();
        }
    }
}

fn scale_rect(rect: RECT, dpi: u32) -> RECT {
    RECT {
        left: scale(rect.left, dpi),
        top: scale(rect.top, dpi),
        right: scale(rect.right, dpi),
        bottom: scale(rect.bottom, dpi),
    }
}

fn fill(dc: HDC, rect: RECT, rgb: u32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(rgb));
        assert_ne!(FillRect(dc, &rect, brush), 0);
        assert!(DeleteObject(brush.into()).as_bool());
    }
}

fn frame_id(sample: &Sample, dpi: u32) -> Option<u32> {
    let width = sample.pixels.bounds.right - sample.pixels.bounds.left;
    let height = sample.pixels.bounds.bottom - sample.pixels.bounds.top;
    if (width, height)
        != (
            scale(ROI.right, dpi) - scale(ROI.left, dpi),
            scale(ROI.bottom, dpi) - scale(ROI.top, dpi),
        )
    {
        return None;
    }
    let pixel = |x, y| {
        let index = ((scale(y, dpi) - scale(ROI.top, dpi)) * width + scale(x, dpi)
            - scale(ROI.left, dpi)) as usize
            * 4;
        &sample.pixels.pixels[index..index + 4]
    };
    let id = pixel(100, 110);
    (id[2..] == [0x40, 255]
        && pixel(60, 60) == [0xd0, 0x90, 0x20, 255]
        && pixel(300, 200) == [0xe3, 0x3f, 0x8d, 255]
        && pixel(800, 480) == [0x5b, 0xc4, 0x3f, 255])
    .then(|| u32::from(id[0]) | (u32::from(id[1]) << 8))
}

fn pump() {
    let mut message = MSG::default();
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
fn await_sample(worker: &Worker, source: &Window, id: u32) -> Sample {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        pump();
        source.source(id);
        if let Some(sample) = worker.take()
            && frame_id(&sample, source.dpi()) == Some(id)
        {
            return sample;
        }
        assert!(Instant::now() < end, "no fresh frame for {id}");
        thread::sleep(Duration::from_millis(16));
    }
}

fn screen_color(point: windows::Win32::Foundation::POINT) -> COLORREF {
    unsafe {
        let dc = GetDC(None);
        let color = GetPixel(dc, point.x, point.y);
        ReleaseDC(None, dc);
        color
    }
}

fn show_and_verify(preview: &Window, sample: &Sample, dpi: u32, pixels: &mut Vec<u8>) {
    let id = frame_id(sample, dpi).expect("verified source markers");
    preview.present(sample, pixels);
    unsafe {
        DwmFlush().unwrap();
        let mut rect = RECT::default();
        GetClientRect(preview.0, &mut rect).unwrap();
        let mut point = windows::Win32::Foundation::POINT {
            x: rect.right / 16,
            y: rect.bottom / 10,
        };
        assert!(ClientToScreen(preview.0, &mut point).as_bool());
        assert_eq!(screen_color(point), COLORREF(0x400000 | id));
    }
}

fn verify_exact(sample: &Sample, dpi: u32, id: u32) {
    let width = (sample.pixels.bounds.right - sample.pixels.bounds.left) as usize;
    for (index, actual) in sample.pixels.pixels.as_chunks::<4>().0.iter().enumerate() {
        let x = (index % width) as i32 + scale(ROI.left, dpi);
        let y = (index / width) as i32 + scale(ROI.top, dpi);
        let inside = |l, t, r, b| {
            x >= scale(l, dpi) && x < scale(r, dpi) && y >= scale(t, dpi) && y < scale(b, dpi)
        };
        let color = if inside(80, 88, 160, 160) {
            id | 0x400000
        } else if inside(240, 128, 620, 412) {
            0x8d3fe3
        } else if inside(720, 360, 1230, 600) {
            0x3fc45b
        } else {
            0x2090d0
        };
        assert_eq!(
            *actual,
            [color as u8, (color >> 8) as u8, (color >> 16) as u8, 255],
            "crop pixel {x},{y}"
        );
    }
}

fn resource_counts() -> (u32, u32, u32) {
    use windows::Win32::System::Threading::*;
    unsafe {
        let process = GetCurrentProcess();
        let mut handles = 0;
        GetProcessHandleCount(process, &mut handles).unwrap();
        (
            handles,
            GetGuiResources(process, GR_USEROBJECTS),
            GetGuiResources(process, GR_GDIOBJECTS),
        )
    }
}

fn observer(stage: &str) {
    let Some(directory) = std::env::var_os("LEXIFT_WINDOW_PIN_OBSERVER") else {
        return;
    };
    let ack = std::path::PathBuf::from(directory).join(format!("{stage}.ack"));
    println!("window-pin-sample {stage}");
    let deadline = Instant::now() + Duration::from_secs(12);
    while !ack.is_file() {
        pump();
        assert!(Instant::now() < deadline, "observer did not sample {stage}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop and two owned synthetic windows"]
fn window_pin_gate() {
    use windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext;
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    struct Dpi(DPI_AWARENESS_CONTEXT);
    impl Drop for Dpi {
        fn drop(&mut self) {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
    let _dpi = Dpi(previous);
    let lefts = std::env::var("LEXIFT_WINDOW_PIN_LEFTS").unwrap_or_else(|_| "0,2560".into());
    let lefts: Vec<i32> = lefts.split(',').map(|part| part.parse().unwrap()).collect();
    assert_eq!(lefts.len(), 2);
    let mode = std::env::var("LEXIFT_WINDOW_PIN_MODE").unwrap_or_else(|_| "matrix".into());
    let rounds = if mode == "smoke" { 1 } else { 10 };
    let mut fps_failures = 0;
    let mut latencies = Vec::new();
    let mut retired = Vec::new();
    observer("before");
    for (&left, &other) in lefts.iter().zip(lefts.iter().rev()) {
        let foreground = unsafe { GetForegroundWindow() };
        let source = Window::new(left, 40, 1920, 1080, false);
        let preview = Window::new(other + 24, 60, 320, 160, true);
        let dpi = source.dpi();
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        let style = unsafe { GetWindowLongPtrW(preview.0, GWL_EXSTYLE) } as u32;
        assert_ne!(style & WS_EX_TOOLWINDOW.0, 0);
        assert_eq!(style & WS_EX_APPWINDOW.0, 0);
        let mut pixels = Vec::new();
        let mut firsts = Vec::new();
        let mut stops = Vec::new();
        let mut restores = Vec::new();

        // Geometry, lifecycle and independent presentation check before the timed trials.
        let mut worker = Worker::start(source.0);
        println!("geometry start dpi={dpi}, left={left}");
        let mut last = await_sample(&worker, &source, 1);
        verify_exact(&last, dpi, 1);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        preview.place(left + 1500, 90, 320, 160);
        last = await_sample(&worker, &source, 2);
        verify_exact(&last, dpi, 2);
        preview.place(left, 40, 1920, 1080);
        last = await_sample(&worker, &source, 3);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        preview.place(other + 24, 60, 320, 160);
        source.place(left + 10, 48, 1920, 1080);
        verify_exact(&await_sample(&worker, &source, 4), dpi, 4);
        println!("geometry cross-dpi from {dpi}");
        source.place(other, 48, 1920, 1080);
        let deadline = Instant::now() + Duration::from_secs(2);
        while source.dpi() == dpi {
            pump();
            assert!(Instant::now() < deadline, "source did not change DPI");
        }
        let _ = worker.take();
        verify_exact(&await_sample(&worker, &source, 5), source.dpi(), 5);
        source.place(left, 40, 1920, 1080);
        pump();
        let _ = worker.take();
        last = await_sample(&worker, &source, 6);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        println!("geometry out of bounds at {dpi}");
        source.place(left, 40, 300, 250);
        pump();
        let _ = worker.take();
        source.source(7);
        thread::sleep(Duration::from_millis(160));
        assert!(worker.take().is_none(), "out-of-range crop published");
        source.place(left, 40, 1920, 1080);
        verify_exact(&await_sample(&worker, &source, 8), dpi, 8);
        last = await_sample(&worker, &source, 9);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        unsafe {
            let _ = ShowWindow(source.0, SW_MINIMIZE);
        }
        pump();
        thread::sleep(Duration::from_millis(160));
        assert!(worker.take().is_none(), "minimized source published");
        let recovered = Instant::now();
        unsafe {
            let _ = ShowWindow(source.0, SW_SHOWNOACTIVATE);
        }
        last = await_sample(&worker, &source, 10);
        restores.push(recovered.elapsed().as_secs_f64() * 1000.);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        let freeze = last.pixels.pixels.clone();
        worker.stop();
        assert!(worker.take().is_none());
        verify_exact(&last, dpi, 10);
        if let Some(directory) = std::env::var_os("LEXIFT_WINDOW_PIN_EVIDENCE") {
            let path = std::path::PathBuf::from(directory).join(format!("frozen-{dpi}.png"));
            let bounds = last.pixels.bounds;
            image::save_buffer(
                path,
                &last.pixels.pixels,
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        assert_eq!(last.pixels.pixels, freeze);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        drop(last);
        drop(freeze);

        for round in 0..rounds {
            let start = Instant::now();
            let mut worker = Worker::start(source.0);
            let first = await_sample(&worker, &source, 20);
            show_and_verify(&preview, &first, dpi, &mut pixels);
            firsts.push(start.elapsed().as_secs_f64() * 1000.);
            drop(first);
            unsafe {
                let _ = ShowWindow(source.0, SW_MINIMIZE);
            }
            pump();
            thread::sleep(Duration::from_millis(80));
            assert!(worker.take().is_none());
            unsafe {
                let _ = ShowWindow(source.0, SW_SHOWNOACTIVATE);
            }
            let resume = Instant::now();
            let first = await_sample(&worker, &source, 21);
            show_and_verify(&preview, &first, dpi, &mut pixels);
            restores.push(resume.elapsed().as_secs_f64() * 1000.);
            drop(first);
            for covered in [false, true] {
                let _cover = covered.then(|| {
                    let cover = Window::new(left, 40, 1920, 1080, true);
                    cover.paint(|dc| {
                        fill(
                            dc,
                            RECT {
                                left: 0,
                                top: 0,
                                right: 1920,
                                bottom: 1080,
                            },
                            0xaa2244,
                        )
                    });
                    unsafe {
                        DwmFlush().unwrap();
                    }
                    assert_eq!(
                        screen_color(windows::Win32::Foundation::POINT {
                            x: left + 300,
                            y: 250
                        }),
                        COLORREF(0xaa2244)
                    );
                    cover
                });
                let first_id = 1000 + round * 120 + u32::from(covered) * 60;
                let mut ids = 0;
                let mut prior = None;
                let mut readback = Vec::new();
                let mut present = Vec::new();
                let mut paint = Vec::new();
                let mut wgc_clock_delta = Vec::new();
                let begin = Instant::now();
                for step in 0..60 {
                    let due = begin + Duration::from_nanos(step * 1_000_000_000 / 30);
                    loop {
                        pump();
                        if let Some(sample) = worker.take()
                            && let Some(id) = frame_id(&sample, dpi)
                            && (first_id..first_id + 60).contains(&id)
                            && prior != Some(id)
                        {
                            prior = Some(id);
                            ids += 1;
                            readback.push(sample.readback.as_secs_f64() * 1000.);
                            let painting = Instant::now();
                            preview.present(&sample, &mut pixels);
                            unsafe {
                                DwmFlush().unwrap();
                            }
                            paint.push(painting.elapsed().as_secs_f64() * 1000.);
                            wgc_clock_delta.push((clock() - sample.time) as f64 / 10_000.);
                            let latency = sample.acquired.elapsed().as_secs_f64() * 1000.;
                            latencies.push(latency);
                            present.push(latency);
                        }
                        if Instant::now() >= due {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    source.source(first_id + step as u32);
                }
                let fps = f64::from(ids) / begin.elapsed().as_secs_f64();
                fps_failures += usize::from(fps < 27.);
                println!(
                    "{}",
                    serde_json::json!({"case":"dynamic", "dpi":dpi, "round":round, "covered":covered, "fps":fps, "readback_ms":readback, "capture_to_present_ms":present,"paint_and_flush_ms":paint,"wgc_timestamp_to_present_ms":wgc_clock_delta})
                );
            }
            let stop = Instant::now();
            worker.stop();
            stops.push(stop.elapsed().as_secs_f64() * 1000.);
            let state = worker.mailbox.lock().unwrap();
            assert!(state.stopped && state.pending.is_none());
            println!(
                "{}",
                serde_json::json!({"case":"buffers","dpi":dpi,"round":round,"gpu_estimate_bytes":state.gpu_peak_estimate,"crop_bytes":state.crop_peak,"preview_bgra_bytes":pixels.capacity(),"cpu_upper_bytes":3*state.crop_peak+pixels.capacity(),"resources":resource_counts()})
            );
        }
        println!(
            "{}",
            serde_json::json!({"case":"timing","dpi":dpi,"first_ms":firsts,"restore_ms":restores,"retire_ms":stops})
        );
        preview.place(other + 24, 60, 320, 160);
        let mut worker = Worker::start(source.0);
        let last = await_sample(&worker, &source, 90);
        show_and_verify(&preview, &last, dpi, &mut pixels);
        for step in 0..14 {
            source.source(91 + step);
            thread::sleep(Duration::from_millis(34));
        }
        assert!(worker.mailbox.lock().unwrap().overwritten > 0);
        let token = source.0;
        drop(source);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !worker.mailbox.lock().unwrap().stopped {
            assert!(Instant::now() < deadline, "closed capture did not retire");
            thread::sleep(Duration::from_millis(2));
        }
        worker.stop();
        assert!(!unsafe { IsWindow(Some(token)) }.as_bool());
        show_and_verify(&preview, &last, dpi, &mut pixels);
        verify_exact(&last, dpi, 90);
        drop(preview);
        pump();
        retired.push(resource_counts());
        observer(&format!("display-{dpi}"));
    }
    let warmed = resource_counts();
    for round in 0..if mode == "smoke" { 2 } else { 20 } {
        let source = Window::new(lefts[round % 2], 40, 1920, 1080, false);
        let hwnd = source.0;
        let mut worker = Worker::start(source.0);
        if round % 4 >= 2 {
            let preview = Window::new(lefts[(round + 1) % 2] + 24, 60, 320, 160, true);
            let frame = await_sample(&worker, &source, 55);
            let mut pixels = Vec::new();
            show_and_verify(&preview, &frame, source.dpi(), &mut pixels);
            worker.stop();
            verify_exact(&frame, source.dpi(), 55);
        } else if round % 4 == 0 {
            worker.stop();
        } else {
            // Destroy before painting any usable fixture pixels.
            drop(source);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !worker.mailbox.lock().unwrap().stopped {
                assert!(
                    Instant::now() < deadline,
                    "closed without a frame did not retire"
                );
                thread::sleep(Duration::from_millis(2));
            }
            worker.stop();
            let state = worker.mailbox.lock().unwrap();
            assert!(state.pending.is_none() && state.error.is_some() && worker.thread.is_none());
            assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
            println!(
                "{}",
                serde_json::json!({"case":"early_close","round":round,"resources":resource_counts()})
            );
            continue;
        }
        assert!(worker.thread.is_none() && worker.mailbox.lock().unwrap().pending.is_none());
        drop(source);
        pump();
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        let resources = resource_counts();
        // Process-wide counts can fall when unrelated native helpers retire.
        // The fixture separately checks its exact HWND and joins its worker.
        assert!(resources.1 <= warmed.1 && resources.2 <= warmed.2);
        println!(
            "{}",
            serde_json::json!({"case":"cycle","round":round,"resources":resources})
        );
    }
    observer("cycles-done");
    println!(
        "{}",
        serde_json::json!({"case":"retired","resources":retired,"capture_to_present_ms":latencies,"fps_failures":fps_failures})
    );
    assert_eq!(fps_failures, 0, "dynamic rounds below 27fps");
}
