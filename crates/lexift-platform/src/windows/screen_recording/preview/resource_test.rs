//! Isolated repeated decoder ownership probe; no user files are modified.
use super::engine::Engine;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use windows::Win32::{Graphics::Dwm::DwmFlush, System::Com::*};

#[test]
#[ignore = "native repeated decoder ownership; explicit fixture and evidence directory required"]
fn native_preview_resource_cycles() {
    use lexift_core::{
        domain::{geometry::Rect, recording_preview::Target},
        ports::magnifier::WindowToken,
    };
    use windows::{Win32::UI::WindowsAndMessaging::*, core::w};
    let source = PathBuf::from(std::env::var_os("LEXIFT_PREVIEW_GATE_FILE").unwrap());
    let folder = PathBuf::from(std::env::var_os("LEXIFT_PREVIEW_GATE_DIR").unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
    let parent = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("Owned hidden playback resource probe"),
            WS_POPUP,
            0,
            0,
            640,
            360,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    };
    for round in 0..=20 {
        let copy = folder.join(format!("owned-{round}.mp4"));
        std::fs::copy(&source, &copy).unwrap();
        let mut engine = Engine::open(&copy).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < until && !ready {
            engine.check_events().unwrap();
            ready = engine.frame().unwrap().is_some();
            unsafe { DwmFlush().unwrap() };
        }
        assert!(ready, "paused first frame missing in round {round}");
        let mut surface = super::surface::Surface::new(Target {
            window: WindowToken(parent.0 as isize),
            bounds: Rect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 360,
            },
        })
        .unwrap();
        surface.present(&engine).unwrap();
        unsafe { engine.media.Play().unwrap() };
        let until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < until {
            engine.check_events().unwrap();
            if engine.frame().unwrap().is_some() {
                surface.present(&engine).unwrap();
            }
            unsafe { DwmFlush().unwrap() };
        }
        drop(surface);
        assert!(
            unsafe { GetWindow(parent, GW_CHILD) }.is_err(),
            "video child survived retirement"
        );
        drop(engine);
        super::super::super::recording_gate::resources::assert_retired();
        std::fs::remove_file(copy).unwrap();
        eprintln!("RESOURCE_SAMPLE round={round} pid={}", std::process::id());
        std::thread::sleep(Duration::from_millis(400));
    }
    std::thread::sleep(Duration::from_secs(120));
    eprintln!("RESOURCE_SAMPLE idle pid={}", std::process::id());
    std::thread::sleep(Duration::from_secs(2));
    unsafe {
        DestroyWindow(parent).unwrap();
        CoUninitialize()
    };
}

#[test]
#[ignore = "native WGC recorder isolation; explicit owned evidence directory required"]
fn native_recorder_resource_cycles() {
    use super::super::*;
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    let port = WindowsScreenRecordingPort::new().unwrap();
    let roi = lexift_core::domain::geometry::Rect {
        left: 320,
        top: 200,
        right: 2240,
        bottom: 1280,
    };
    for round in 0..=20 {
        let (tx, rx) = mpsc::channel();
        let mut id = Identity {
            session: 991,
            operation: round + 1,
            request: 1,
        };
        port.prepare(
            PrepareRequest {
                identity: id,
                bounds: roi,
                monitor_bounds: roi,
                excluded_windows: vec![],
                video: Default::default(),
            },
            Arc::new(move |notice| {
                let _ = tx.send(notice);
            }),
        )
        .unwrap();
        let wait = |predicate: fn(&Notice) -> bool| {
            loop {
                let notice = rx.recv_timeout(Duration::from_secs(10)).unwrap();
                assert!(
                    !matches!(notice, Notice::Failed { .. } | Notice::StartRejected { .. }),
                    "{notice:?}"
                );
                if predicate(&notice) {
                    break;
                }
            }
        };
        wait(|n| matches!(n, Notice::Prepared(_)));
        id.request += 1;
        port.control(id, Control::Start).unwrap();
        wait(|n| matches!(n, Notice::Started(_)));
        std::thread::sleep(Duration::from_millis(600));
        id.request += 1;
        port.control(id, Control::Finish).unwrap();
        wait(|n| matches!(n, Notice::Finished { .. }));
        id.request += 1;
        port.release(id).unwrap();
        wait(|n| matches!(n, Notice::Retired(_)));
        super::super::super::recording_gate::resources::assert_retired();
        eprintln!("RESOURCE_SAMPLE round={round} pid={}", std::process::id());
        std::thread::sleep(Duration::from_millis(400));
    }
    std::thread::sleep(Duration::from_secs(120));
    eprintln!("RESOURCE_SAMPLE idle pid={}", std::process::id());
    std::thread::sleep(Duration::from_secs(2));
    let (tx, rx) = mpsc::channel();
    port.shutdown(Box::new(move || {
        let _ = tx.send(());
    }))
    .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
}

#[test]
#[ignore = "native WGC-only broker isolation; captures are discarded without encoding"]
fn native_capture_resource_cycles() {
    use super::super::super::{
        recording_capture::Capture,
        screen_capture::{arrival::Notice, capture::Apartment},
    };
    use lexift_core::domain::geometry::Rect;
    let _apartment = Apartment::new().unwrap();
    let mut pixels = vec![0; 1920 * 1080 * 4];
    for round in 0..=40 {
        let capture = Capture::new(
            Rect {
                left: 320,
                top: 200,
                right: 2240,
                bottom: 1280,
            },
            Notice::new(|| {}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while capture.read(&mut pixels, 0).unwrap().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(capture);
        super::super::super::recording_gate::resources::assert_retired();
        eprintln!("RESOURCE_SAMPLE round={round} pid={}", std::process::id());
        std::thread::sleep(Duration::from_millis(400));
        if round == 20 || round == 40 {
            std::thread::sleep(Duration::from_secs(120));
            eprintln!(
                "RESOURCE_SAMPLE idle-after-{round} pid={}",
                std::process::id()
            );
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

#[test]
#[ignore = "native decoder failure recovery; explicit owned fixture and evidence directory required"]
fn native_preview_failed_open_recovers() {
    let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
    let source = PathBuf::from(std::env::var_os("LEXIFT_PREVIEW_GATE_FILE").unwrap());
    let folder = PathBuf::from(std::env::var_os("LEXIFT_PREVIEW_GATE_DIR").unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    for (round, contents) in [
        None,
        Some(&[][..]),
        Some(&bytes[..128.min(bytes.len())]),
        Some(&b"invalid owned fixture"[..]),
    ]
    .into_iter()
    .enumerate()
    {
        let path = folder.join(format!("invalid-{round}.mp4"));
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        if let Ok(mut engine) = Engine::open(&path) {
            let limit = Instant::now() + Duration::from_secs(5);
            loop {
                if engine.check_events().is_err() {
                    break;
                }
                assert!(Instant::now() < limit, "malformed source was not rejected");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
        let mut valid = Engine::open(&source).unwrap();
        let limit = Instant::now() + Duration::from_secs(5);
        while valid.frame().unwrap().is_none() {
            valid.check_events().unwrap();
            assert!(
                Instant::now() < limit,
                "failed source blocked subsequent playback"
            );
            unsafe { DwmFlush().unwrap() };
        }
        assert!(unsafe { valid.media.IsPaused().as_bool() });
        drop(valid);
        super::super::super::recording_gate::resources::assert_retired();
        eprintln!("decoder failure recovery round={round} passed");
    }
}
