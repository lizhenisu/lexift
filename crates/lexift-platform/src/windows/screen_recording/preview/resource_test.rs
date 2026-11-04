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

#[test]
#[ignore = "explicit owned native trim playback interval"]
fn native_trim_playback_gate() {
    use super::super::FileArtifact;
    use lexift_core::{
        domain::{
            geometry::Rect,
            recording_preview::{Action, Command, Identity, Notice, Target},
            recording_trim::Range,
            screen_recording::Artifact,
        },
        ports::magnifier::WindowToken,
    };
    use std::sync::{Arc, mpsc};
    use windows::{Win32::UI::WindowsAndMessaging::*, core::w};
    let source = PathBuf::from(std::env::var_os("LEXIFT_TRIM_SOURCE").unwrap());
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
    }
    let parent = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("Owned trim interval probe"),
            WS_POPUP,
            40,
            40,
            640,
            360,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    };
    let file = FileArtifact::new().unwrap();
    std::fs::copy(source, &file.path).unwrap();
    let path = file.path.clone();
    let fps = std::env::var("LEXIFT_TRIM_PREVIEW_FPS")
        .ok()
        .map(|s| s.parse::<u32>().unwrap())
        .unwrap_or(60);
    let frames = std::env::var("LEXIFT_TRIM_PREVIEW_FRAMES")
        .ok()
        .map(|s| s.parse::<u64>().unwrap())
        .unwrap_or(205);
    let replay = std::env::var_os("LEXIFT_TRIM_PREVIEW_REPLAY").is_some();
    let (mut id, range) = (
        Identity {
            session: 991,
            operation: 1,
            artifact: Artifact(1),
            instance: 1,
            request: 1,
        },
        if replay {
            Range::full(frames)
        } else {
            Range {
                start: 17,
                end: 103,
            }
        },
    );
    let (tx, rx) = mpsc::channel();
    let worker = super::Worker::open(
        id,
        Target {
            window: WindowToken(parent.0 as isize),
            bounds: Rect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 360,
            },
        },
        file,
        Range::time(frames - 1, fps),
        Some((range, fps)),
        Arc::new(move |id, n| {
            let _ = tx.send((id, n));
        }),
    )
    .unwrap();
    let audio_probe = std::env::var_os("LEXIFT_TRIM_AUDIO_PROBE")
        .map(|_| super::audio_test::Probe::new().unwrap());
    let transitions = std::cell::RefCell::new(Vec::<(usize, bool, f64)>::new());
    let active = std::cell::Cell::new([false, false]);
    let collect = || {
        if let Some(probe) = &audio_probe {
            while let Some(packet) = probe.read().unwrap() {
                for (index, block) in packet.frames.chunks(48).enumerate() {
                    let mut was = active.get();
                    for ch in 0..2 {
                        let rms = (block.iter().map(|s| s[ch] * s[ch]).sum::<f32>()
                            / block.len() as f32)
                            .sqrt();
                        let on = rms > 0.015;
                        if on != was[ch] {
                            transitions.borrow_mut().push((
                                ch,
                                on,
                                packet.qpc as f64 / 10_000_000. + index as f64 / 1000.,
                            ));
                            was[ch] = on;
                        }
                    }
                    active.set(was);
                }
            }
        }
    };
    let drain = || {
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            collect();
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    let wait = |request: u64, ended: bool| {
        collect();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            collect();
            unsafe {
                let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&message);
                }
            }
            let (i, n) = match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(n) => n,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(e) => panic!("request {request} timed out: {e}"),
            };
            if let Notice::Failed(e) = n {
                panic!("{e}");
            }
            if i.request == request
                && (ended && n == Notice::Ended || !ended && matches!(n, Notice::Frame { .. }))
            {
                return n;
            }
        }
    };
    wait(1, false);
    id.request = 2;
    worker
        .control(Command {
            identity: id,
            action: Action::SetRange {
                range,
                fps,
                version: 1,
                position: Range::time(range.start, fps),
            },
        })
        .unwrap();
    let first = wait(2, false);
    assert!(
        matches!(first,Notice::Frame{position,..} if (position.as_secs_f64()-range.start as f64/fps as f64).abs()<0.5/fps as f64)
    );
    id.request = 3;
    worker
        .control(Command {
            identity: id,
            action: Action::Play,
        })
        .unwrap();
    wait(3, true);
    drain();
    id.request = 4;
    worker
        .control(Command {
            identity: id,
            action: Action::Seek(Range::time(range.start, fps)),
        })
        .unwrap();
    wait(4, false);
    id.request = 5;
    worker
        .control(Command {
            identity: id,
            action: Action::Play,
        })
        .unwrap();
    wait(5, true);
    drain();
    if replay {
        for round in 0..20 {
            id.request += 1;
            worker
                .control(Command {
                    identity: id,
                    action: Action::Seek(Duration::ZERO),
                })
                .unwrap();
            wait(id.request, false);
            id.request += 1;
            worker
                .control(Command {
                    identity: id,
                    action: Action::Play,
                })
                .unwrap();
            wait(id.request, true);
            eprintln!("FULL_REPLAY round={round} passed");
        }
    }
    drop(audio_probe);
    let retired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = retired.clone();
    let retire = std::thread::spawn(move || {
        drop(worker);
        done.store(true, std::sync::atomic::Ordering::Release);
    });
    while !retired.load(std::sync::atomic::Ordering::Acquire) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    retire.join().unwrap();
    if let Some(path) = std::env::var_os("LEXIFT_TRIM_AUDIO_PROBE") {
        let rows = transitions.borrow();
        std::fs::write(path, serde_json::to_vec_pretty(&*rows).unwrap()).unwrap();
        let on: Vec<_> = rows
            .iter()
            .filter(|x| x.0 == 0 && x.1)
            .map(|x| x.2)
            .collect();
        let off: Vec<_> = rows
            .iter()
            .filter(|x| x.0 == 1 && !x.1)
            .map(|x| x.2)
            .collect();
        assert_eq!((on.len(), off.len()), (2, 2), "{rows:?}");
        for (a, b) in on.iter().zip(off) {
            assert!(
                ((b - a) - (103. / 60. - 0.5)).abs() < 0.05,
                "audio cutoff delta {} expected {}",
                b - a,
                103. / 60. - 0.5
            );
        }
    }
    assert!(!path.exists());
    super::super::super::recording_gate::resources::assert_retired();
    unsafe {
        DestroyWindow(parent).unwrap();
        CoUninitialize();
    }
}
