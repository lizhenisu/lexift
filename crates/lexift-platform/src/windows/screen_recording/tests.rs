//! Explicit native contract tests drive the shipping Port, not the old timed gate runner.
use super::*;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::DwmFlush,
        Graphics::Gdi::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::*,
};
struct Source(HWND);
impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
fn paint(hwnd: HWND, rgb: [u8; 3]) {
    let mut rgba = vec![0; 1920 * 1080 * 4];
    for p in rgba.as_chunks_mut::<4>().0 {
        p.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    super::super::popup::present_recording_content_hwnd(hwnd, &rgba, 1920, 1080).unwrap();
    unsafe {
        DwmFlush().unwrap();
    }
}
fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
fn wait(receiver: &Receiver<Notice>, accept: impl Fn(&Notice) -> bool) -> Notice {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        pump();
        match receiver.recv_timeout(Duration::from_millis(5)) {
            Ok(n) => {
                if let Notice::Failed { error, .. } = &n {
                    panic!("native service failed: {error}");
                }
                if accept(&n) {
                    return n;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                assert!(Instant::now() < deadline, "native reply timed out")
            }
            Err(e) => panic!("native listener closed: {e}"),
        }
    }
}
fn dwell(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump();
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "interactive synthetic source; requires LEXIFT_RECORDING_NATIVE_DIR"]
fn production_port_static_pause_resume_and_final_presented_frame() {
    let folder = PathBuf::from(
        std::env::var_os("LEXIFT_RECORDING_NATIVE_DIR").expect("owned native evidence directory"),
    );
    std::fs::create_dir_all(&folder).unwrap();
    let mut roi = lexift_core::domain::geometry::Rect {
        left: 320,
        top: 200,
        right: 2240,
        bottom: 1280,
    };
    if let Ok(value) = std::env::var("LEXIFT_RECORDING_NATIVE_ROI") {
        let coordinates: Vec<i32> = value.split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(coordinates.len(), 4);
        roi = lexift_core::domain::geometry::Rect {
            left: coordinates[0],
            top: coordinates[1],
            right: coordinates[2],
            bottom: coordinates[3],
        };
        assert_eq!((roi.right - roi.left, roi.bottom - roi.top), (1920, 1080));
    }
    let old = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let scene = super::super::recording_gate::fixture::Scene::start(roi, true);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            w!("LexiftRecordingGateFixture"),
            w!("Owned recording source"),
            WS_POPUP,
            roi.left,
            roi.top,
            1920,
            1080,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    let source = Source(hwnd);
    let rectangle = RECT {
        left: roi.left,
        top: roi.top,
        right: roi.right,
        bottom: roi.bottom,
    };
    let monitor = unsafe { MonitorFromRect(&rectangle, MONITOR_DEFAULTTONULL) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    assert!(unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool());
    let monitor_bounds = lexift_core::domain::geometry::Rect {
        left: info.rcMonitor.left,
        top: info.rcMonitor.top,
        right: info.rcMonitor.right,
        bottom: info.rcMonitor.bottom,
    };
    super::super::recording_window::configure(hwnd, crate::RecordingWindowRole::Content).unwrap();
    paint(hwnd, [32, 52, 76]);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    let port = WindowsScreenRecordingPort::new().unwrap();
    for round in 0..2 {
        let (tx, rx) = mpsc::channel();
        let mut id = Identity {
            session: 71,
            operation: round + 1,
            request: 1,
        };
        paint(hwnd, [32, 52, 76]);
        port.prepare(
            PrepareRequest {
                identity: id,
                bounds: roi,
                monitor_bounds,
                excluded_windows: vec![],
            },
            Arc::new(move |n| {
                let _ = tx.send(n);
            }),
        )
        .unwrap();
        wait(&rx, |n| matches!(n, Notice::Prepared(_)));
        id.request += 1;
        port.control(id, Control::Start).unwrap();
        wait(&rx, |n| matches!(n, Notice::Started(_)));
        assert_eq!(
            super::super::recording_gate::resources::ENCODER.load(Ordering::Acquire),
            1
        );
        dwell(600);
        id.request += 1;
        port.control(id, Control::Pause).unwrap();
        wait(&rx, |n| matches!(n, Notice::Paused(_)));
        dwell(250);
        if round == 0 {
            id.request += 1;
            port.control(id, Control::Resume).unwrap();
            wait(&rx, |n| matches!(n, Notice::Resumed(_)));
            dwell(500);
        }
        // Final contents change after a pause, or immediately before Finish. The
        // unsubmitted tail slot must contain these fresh pixels without adding pause time.
        paint(hwnd, [220, 50, 70]);
        id.request += 1;
        port.control(id, Control::Finish).unwrap();
        let Notice::Finished { artifact, .. } = wait(&rx, |n| matches!(n, Notice::Finished { .. }))
        else {
            unreachable!()
        };
        assert_eq!((artifact.width, artifact.height), (1920, 1080));
        assert!(artifact.frames >= 15);
        assert!(artifact.interrupted.is_none());
        for counter in [
            &super::super::recording_gate::resources::ENCODER,
            &super::super::recording_gate::resources::CAPTURE,
        ] {
            assert_eq!(
                counter.load(Ordering::Acquire),
                0,
                "Sealed video retained native resources"
            );
        }
        let (reply, result) = mpsc::channel();
        port.send(Message::ExportForTest(
            folder.join(format!("round-{round}.mp4")),
            reply,
        ))
        .unwrap();
        let private = result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        std::fs::write(folder.join(format!("round-{round}.json")),serde_json::to_vec_pretty(&serde_json::json!({"frames":artifact.frames,"duration_ms":artifact.duration.as_secs_f64()*1000.,"width":artifact.width,"height":artifact.height,"final_rgb":[220,50,70],"static_resume":round==0})).unwrap()).unwrap();
        id.request += 1;
        port.release(id).unwrap();
        wait(&rx, |n| matches!(n, Notice::Retired(_)));
        assert!(!private.exists(), "private artifact survived release");
    }
    let (retired, wait) = mpsc::channel();
    port.shutdown(Box::new(move || {
        let _ = retired.send(());
    }))
    .unwrap();
    wait.recv_timeout(Duration::from_secs(5))
        .expect("native service and its apartment must retire before exit");
    drop(port);
    drop(source);
    drop(scene);
    unsafe {
        SetThreadDpiAwarenessContext(old);
    }
    super::super::recording_gate::resources::assert_retired();
}
