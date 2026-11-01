//! Real monitor capture of transparent ink underneath excluded interaction chrome.
use super::{capture::Capture, fixture::Scene};
use crate::RecordingWindowRole;
use lexift_core::domain::geometry::Rect;
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::DwmFlush,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::*,
};

struct Layer(HWND);
impl Drop for Layer {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
struct Cursor {
    point: POINT,
    dpi: DPI_AWARENESS_CONTEXT,
}
impl Drop for Cursor {
    fn drop(&mut self) {
        unsafe {
            let _ = SetCursorPos(self.point.x, self.point.y);
            SetThreadDpiAwarenessContext(self.dpi);
        }
    }
}
fn layer(region: Rect, role: RecordingWindowRole) -> Layer {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            w!("LexiftRecordingGateFixture"),
            w!("Recording layer isolation gate"),
            WS_POPUP,
            region.left,
            region.top,
            200,
            100,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let owned = Layer(hwnd);
        super::super::recording_window::configure(hwnd, role).unwrap();
        if role == RecordingWindowRole::Content {
            // A restored interaction surface must not inherit a passive subclass.
            super::super::recording_window::configure(hwnd, RecordingWindowRole::Interaction)
                .unwrap();
            assert!(windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd).as_bool());
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            assert_eq!(
                style & (WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_APPWINDOW).0 as isize,
                0
            );
            super::super::recording_window::configure(hwnd, role).unwrap();
        }
        let mut rgba = vec![0; 200 * 100 * 4];
        for y in 20..80 {
            for x in 20..80 {
                rgba[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4].copy_from_slice(
                    if role == RecordingWindowRole::Content {
                        &[240, 40, 40, 255]
                    } else {
                        &[255, 0, 255, 255]
                    },
                );
            }
        }
        super::super::popup::present_recording_content_hwnd(hwnd, &rgba, 200, 100).unwrap();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        owned
    }
}

#[test]
#[ignore = "opens owned transparent content layers; requires interactive Windows WGC"]
fn transparent_content_is_captured_but_controls_are_not() {
    let folder = std::path::PathBuf::from(
        std::env::var_os("LEXIFT_RECORDING_GATE_DIR").expect("owned evidence directory"),
    );
    let coordinates: Vec<i32> = std::env::var("LEXIFT_RECORDING_LAYER_ROI")
        .unwrap_or_else(|_| "320,200,2240,1280".into())
        .split(',')
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(coordinates.len(), 4);
    let roi = Rect {
        left: coordinates[0],
        top: coordinates[1],
        right: coordinates[2],
        bottom: coordinates[3],
    };
    let old = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).unwrap();
        SetCursorPos(roi.left + 10, roi.top + 10).unwrap();
    }
    let _cursor = Cursor { point, dpi: old };
    let scene = Scene::start(roi, true);
    let foreground = unsafe { GetForegroundWindow() };
    let position = Rect {
        left: roi.left + 100,
        top: roi.top + 700,
        right: roi.left + 300,
        bottom: roi.top + 800,
    };
    let base = layer(position, RecordingWindowRole::Content);
    let background: Vec<u8> = (0..200 * 100).flat_map(|_| [32, 52, 76, 255]).collect();
    super::super::popup::present_recording_content_hwnd(base.0, &background, 200, 100).unwrap();
    let content = layer(position, RecordingWindowRole::Content);
    let controls = layer(position, RecordingWindowRole::Feedback);
    unsafe {
        DwmFlush().unwrap();
    }
    assert_eq!(
        foreground,
        unsafe { GetForegroundWindow() },
        "passive layers stole focus"
    );
    unsafe {
        for (label, hwnd) in [
            (
                "source",
                HWND(scene.windows["source"].as_u64().unwrap() as *mut _),
            ),
            ("content", content.0),
            ("controls", controls.0),
        ] {
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).unwrap();
            eprintln!(
                "{label}: hwnd={} rect={r:?} style={} ex={}",
                hwnd.0 as usize,
                GetWindowLongPtrW(hwnd, GWL_STYLE),
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
            );
        }
    }
    let underlying = unsafe {
        WindowFromPoint(POINT {
            x: position.left + 30,
            y: position.top + 30,
        })
    };
    assert_ne!(underlying, content.0, "content intercepted native input");
    assert_ne!(underlying, controls.0, "controls intercepted native input");
    assert_ne!(
        underlying, base.0,
        "synthetic base intercepted native input"
    );
    assert!(!underlying.is_invalid(), "no underlying application found");
    let notice = super::super::screen_capture::arrival::Notice::new(|| {});
    let capture = Capture::new(roi, notice).unwrap();
    let mut pixels = vec![0; 1920 * 1080 * 4];
    let deadline = Instant::now() + Duration::from_secs(5);
    let pixel =
        |data: &[u8], x: usize, y: usize| data[(y * 1920 + x) * 4..(y * 1920 + x) * 4 + 4].to_vec();
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if capture.read(&mut pixels, 0).unwrap().is_some()
            && pixel(&pixels, 130, 730) == [40, 40, 240, 255]
            && pixel(&pixels, 110, 710) == [76, 52, 32, 255]
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "content/chrome separation failed: ink={:?}, transparent={:?}",
            pixel(&pixels, 130, 730),
            pixel(&pixels, 110, 710)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // Only the owned synthetic patch is persisted, not the surrounding desktop.
    let mut patch = Vec::with_capacity(200 * 100 * 4);
    for row in 700..800 {
        patch.extend_from_slice(&pixels[(row * 1920 + 100) * 4..(row * 1920 + 300) * 4]);
    }
    std::fs::write(folder.join("content-layer.bgra"), &patch).unwrap();
    std::fs::write(folder.join("content-layer.json"),serde_json::to_vec_pretty(&serde_json::json!({"passed":true,"width":200,"height":100,"ink_pixel":pixel(&pixels,130,730),"transparent_pixel":pixel(&pixels,110,710),"foreground_unchanged":true,"input_pass_through":true})).unwrap()).unwrap();
    drop(capture);
    drop(controls);
    drop(content);
    drop(base);
    drop(scene);
    super::resources::assert_retired();
}
