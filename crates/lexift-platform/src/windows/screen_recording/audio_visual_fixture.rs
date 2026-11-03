//! Owned native visual source for sync metrology. Flush completion bounds a
//! presentation; it is deliberately not advertised as a scanout timestamp.
use std::{
    io::{BufWriter, Write},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::DwmFlush,
        System::Memory::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::*,
};

struct Source(HWND);
static CLICKS: AtomicU64 = AtomicU64::new(0);
static WHEELS: AtomicU64 = AtomicU64::new(0);
static DRAGS: AtomicU64 = AtomicU64::new(0);
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_APP + 11 {
            // UpdateLayeredWindow also sets position. Serialize movement with
            // that presentation on the owner instead of racing a foreign call.
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                w.0 as i32,
                l.0 as i32,
                1920,
                1080,
                SWP_NOACTIVATE,
            )
            .unwrap();
            return LRESULT(0);
        }
        match message {
            WM_LBUTTONUP => {
                CLICKS.fetch_add(1, Ordering::Relaxed);
            }
            WM_MOUSEWHEEL => {
                WHEELS.fetch_add(1, Ordering::Relaxed);
            }
            WM_MOUSEMOVE if w.0 & 1 != 0 => {
                DRAGS.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
struct Shared(HANDLE, MEMORY_MAPPED_VIEW_ADDRESS);
impl Drop for Shared {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.1);
            let _ = CloseHandle(self.0);
        }
    }
}

#[test]
#[ignore = "owned native visual source; playback bridge and evidence directory required"]
fn native_playback_clock_visual_source() {
    unsafe {
        let folder =
            std::path::PathBuf::from(std::env::var_os("LEXIFT_RECORDING_NATIVE_DIR").unwrap());
        std::fs::create_dir_all(&folder).unwrap();
        let mut log = BufWriter::new(
            std::fs::File::create(folder.join("native-presentations.jsonl")).unwrap(),
        );
        let name = HSTRING::from(std::env::var("LEXIFT_AUDIO_FIXTURE_SHARED").unwrap());
        let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, &name).unwrap();
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 4096);
        let shared_owner = Shared(mapping, view);
        assert!(!shared_owner.1.Value.is_null());
        let shared = std::slice::from_raw_parts(shared_owner.1.Value.cast::<AtomicU64>(), 4);
        let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
        let old_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let class = w!("LexiftNativeAudioVisualFixture");
        assert_ne!(
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                lpszClassName: class,
                ..Default::default()
            }),
            0
        );
        let position: Vec<i32> = std::env::var("LEXIFT_AUDIO_VISUAL_POSITION")
            .unwrap_or_else(|_| "320,200".into())
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        let source = Source(
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                class,
                w!("Lexift owned native audio visual fixture"),
                WS_POPUP,
                position[0],
                position[1],
                1920,
                1080,
                None,
                None,
                None,
                None,
            )
            .unwrap(),
        );
        // This offset is an independently measured input-fixture calibration,
        // never an adjustment inferred from the recording under acceptance.
        let offset: f64 = std::env::var("LEXIFT_AUDIO_FIXTURE_OFFSET_SAMPLES")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .unwrap();
        let mut pixels = vec![0u8; 1920 * 1080 * 4];
        for p in pixels.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[32, 52, 76, 255]);
        }
        let start = Instant::now();
        let mut frame = 0u64;
        let mut shown = false;
        while start.elapsed() < Duration::from_secs(2700) && !folder.join("stop-visual").exists() {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            let seq = shared[0].load(Ordering::Acquire);
            let played = shared[1].load(Ordering::Relaxed);
            let anchor = shared[2].load(Ordering::Relaxed);
            if seq != shared[0].load(Ordering::Acquire) || !seq.is_multiple_of(2) || anchor == 0 {
                std::thread::yield_now();
                continue;
            }
            if shared[3].load(Ordering::Acquire) != 0 {
                break;
            }
            let before = super::super::recording_capture::qpc_100ns().unwrap();
            let sample =
                played as f64 + (before as f64 - anchor as f64) * 48000. / 10_000_000. - offset;
            let flash = (24000. ..29760.).contains(&sample.rem_euclid(96000.));
            for y in 100..300 {
                for x in 1400..1600 {
                    let i = (y * 1920 + x) * 4;
                    pixels[i..i + 3].fill(if flash { 255 } else { 0 });
                }
            }
            for bit in 0..12 {
                for y in 100..150 {
                    for x in 700 + bit * 24..720 + bit * 24 {
                        let i = (y * 1920 + x) * 4;
                        pixels[i..i + 3].fill(if frame & (1 << bit) != 0 { 255 } else { 0 });
                    }
                }
            }
            super::super::popup::present_recording_content_hwnd(source.0, &pixels, 1920, 1080)
                .unwrap();
            if !shown {
                let _ = ShowWindow(source.0, SW_SHOWNOACTIVATE);
                shown = true;
            }
            DwmFlush().unwrap();
            let after = super::super::recording_capture::qpc_100ns().unwrap();
            writeln!(log, "{}", serde_json::json!({"frame":frame,"event":(sample / 96000.).floor() as i64,"flash":flash,"submit_qpc":before,"flush_qpc":after,"playback_sample":played,"playback_qpc":anchor,"offset_samples":offset,"clicks":CLICKS.load(Ordering::Relaxed),"wheels":WHEELS.load(Ordering::Relaxed),"drags":DRAGS.load(Ordering::Relaxed)})).unwrap();
            frame += 1;
        }
        log.flush().unwrap();
        drop(source);
        SetThreadDpiAwarenessContext(old_dpi);
        UnregisterClassW(class, None).unwrap();
    }
}
