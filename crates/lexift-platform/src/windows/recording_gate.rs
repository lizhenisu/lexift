//! Explicit native technology gate, compiled only into the platform test executable.
//! It is deliberately not a public recording API or a user-facing workflow.
mod capture;
mod encoder;
mod fault;
pub(super) mod fixture;
mod hardware;
mod layers;
pub(super) mod resources;
mod run;

use lexift_core::domain::geometry::Rect;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(serde::Deserialize)]
struct Plan {
    roi: [i32; 4],
    rounds: u32,
    #[serde(default)]
    spec: run::Spec,
    #[serde(default)]
    expected_error: Option<String>,
    #[serde(default)]
    mixed: bool,
    #[serde(default)]
    probe_reopen: bool,
    #[serde(default)]
    recover_last: bool,
}

/// Explicit entry point: all files are generated under the caller's ignored gate directory.
#[test]
#[ignore = "requires an interactive Windows desktop and LEXIFT_RECORDING_GATE_DIR"]
fn native_recording_matrix() {
    let folder =
        PathBuf::from(std::env::var_os("LEXIFT_RECORDING_GATE_DIR").expect("owned gate directory"));
    let plan: Plan =
        serde_json::from_slice(&std::fs::read(folder.join("plan.json")).unwrap()).unwrap();
    let [left, top, right, bottom] = plan.roi;
    let roi = Rect {
        left,
        top,
        right,
        bottom,
    };
    assert!(plan.rounds > 0 && plan.rounds <= 100);
    let ended = Arc::new(AtomicBool::new(false));
    let end = ended.clone();
    let memory_folder = folder.clone();
    let surface = Arc::new(Mutex::new(0usize));
    let observed = surface.clone();
    let sampler = std::thread::spawn(move || {
        use windows::Win32::{
            Foundation::FILETIME,
            System::{ProcessStatus::*, Threading::*},
        };
        let start = Instant::now();
        use std::io::Write;
        let mut output = std::io::BufWriter::new(
            std::fs::File::create(memory_folder.join("process-metrics.jsonl")).unwrap(),
        );
        while !end.load(Ordering::Acquire) {
            unsafe {
                let h = GetCurrentProcess();
                let mut m = PROCESS_MEMORY_COUNTERS_EX {
                    cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                    ..Default::default()
                };
                GetProcessMemoryInfo(h, (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), m.cb)
                    .unwrap();
                let mut handles = 0;
                GetProcessHandleCount(h, &mut handles).unwrap();
                let mut a = FILETIME::default();
                let mut b = a;
                let mut kernel = a;
                let mut user = a;
                GetProcessTimes(h, &mut a, &mut b, &mut kernel, &mut user).unwrap();
                let cpu =
                    |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
                let surface = observed.lock().unwrap();
                let mut reply = 0;
                let ui = if *surface != 0 {
                    use windows::Win32::{
                        Foundation::{HWND, LPARAM, WPARAM},
                        UI::WindowsAndMessaging::*,
                    };
                    let now = Instant::now();
                    let delivered = SendMessageTimeoutW(
                        HWND(*surface as *mut _),
                        WM_NULL,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG | SMTO_BLOCK,
                        100,
                        Some(&mut reply),
                    );
                    Some(
                        serde_json::json!({"delivered":delivered.0!=0,"ms":now.elapsed().as_secs_f64()*1000.}),
                    )
                } else {
                    None
                };
                let row = serde_json::json!({"ms":start.elapsed().as_secs_f64()*1000.,"private_bytes":m.PrivateUsage,"working_set":m.WorkingSetSize,"handles":handles,"cpu_100ns":cpu(kernel)+cpu(user),"ui":ui});
                writeln!(output, "{row}").unwrap();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        output.flush().unwrap();
    });
    // One MTA service owns encoding across recordings. Recreating native worker
    // threads per request can mix driver TLS caches into lifecycle measurements;
    // use the intended bounded service lifetime and never replace a blocked worker.
    let (requests, pending) =
        std::sync::mpsc::sync_channel::<(PathBuf, Arc<fixture::Scene>, run::Spec)>(1);
    let (replies, completed) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let _apartment = super::screen_capture::capture::Apartment::new().unwrap();
        let _media = encoder::Media::new().unwrap();
        while let Ok((file, scene, spec)) = pending.recv() {
            let result = run::run(&file, roi, &scene, spec);
            drop(scene);
            if replies.send(result).is_err() {
                break;
            }
        }
    });
    let mut failure = None;
    for round in 0..plan.rounds {
        let scene = Arc::new(fixture::Scene::start(roi, round.is_multiple_of(2)));
        scene.check_input(roi);
        let windows = scene.windows.clone();
        *surface.lock().unwrap() = windows["source"].as_u64().unwrap() as usize;
        let file = folder.join(format!("round-{round:02}.mp4"));
        let mut spec = plan.spec;
        let recovering = plan.recover_last && round + 1 == plan.rounds;
        let expected_error = if recovering {
            spec.fault = None;
            spec.cancel_ms = None;
            None
        } else {
            plan.expected_error.as_deref()
        };
        if plan.mixed {
            spec.cancel_ms = match round % 4 {
                0 => None,
                1 => Some(0),
                2 => Some(200),
                _ => Some(spec.dynamic_ms + spec.static_ms + 100),
            };
        }
        let at = Instant::now();
        requests.send((file.clone(), scene.clone(), spec)).unwrap();
        if plan.probe_reopen {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !run::active() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(run::active());
            let rejected = run::run(&folder.join("rejected.mp4"), roi, &scene, spec);
            assert!(
                rejected
                    .unwrap_err()
                    .to_string()
                    .contains("resources have not retired")
            );
        }
        let result = completed.recv().unwrap();
        let report = match result {
            Ok(mut r) => {
                if expected_error.is_some() {
                    failure = Some("Expected failure did not occur".into());
                }
                r["windows"] = windows.clone();
                r["wall_ms"] = serde_json::json!(at.elapsed().as_secs_f64() * 1000.);
                r["source_clicks"] = serde_json::json!(scene.clicks.load(Ordering::Relaxed));
                r["source_wheel"] = serde_json::json!(scene.wheel.load(Ordering::Relaxed));
                r["source_drag"] = serde_json::json!(scene.drag.load(Ordering::Relaxed));
                r
            }
            Err(e) => {
                let expected =
                    expected_error.is_some_and(|message| e.to_string().contains(message));
                if !expected {
                    failure = Some(e.to_string());
                }
                serde_json::json!({"error":e.to_string(),"expected_failure":expected,"windows":windows})
            }
        };
        std::fs::write(
            folder.join(format!("round-{round:02}.json")),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        *surface.lock().unwrap() = 0;
        drop(scene);
        assert!(!run::active());
        resources::assert_retired();
        // Sample after all owned joins, separately from in-flight peaks. COM
        // lease counters cannot reveal a driver retaining native handles.
        let mut retired_handles = 0;
        unsafe {
            windows::Win32::System::Threading::GetProcessHandleCount(
                windows::Win32::System::Threading::GetCurrentProcess(),
                &mut retired_handles,
            )
            .unwrap();
        }
        std::fs::write(
            folder.join(format!("native-retired-{round:02}.json")),
            serde_json::to_vec(&serde_json::json!({"handles":retired_handles})).unwrap(),
        )
        .unwrap();
        std::fs::write(
            folder.join(format!("retired-{round:02}.json")),
            serde_json::to_vec(&resources::snapshot()).unwrap(),
        )
        .unwrap();
        for key in ["source", "control", "hint"] {
            let hwnd = windows::Win32::Foundation::HWND(windows[key].as_u64().unwrap() as *mut _);
            assert!(
                !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)) }.as_bool(),
                "fixture HWND leaked"
            );
        }
        if failure.is_some() {
            break;
        }
    }
    drop(requests);
    worker.join().unwrap();
    ended.store(true, Ordering::Release);
    sampler.join().unwrap();
    if let Some(e) = failure {
        panic!("recording gate failed: {e}");
    }
}

/// Existing screenshot/effect sampling must remain independent of the monitor recorder.
#[test]
#[ignore = "requires an interactive synthetic desktop and owned evidence directory"]
fn existing_capture_remains_independent() {
    use super::screen_capture::WindowsScreenCapturePort;
    use lexift_core::ports::{
        magnifier::WindowToken,
        screen_capture::{ScreenCapturePort, ScreenCaptureRequest},
    };
    let folder = PathBuf::from(std::env::var_os("LEXIFT_RECORDING_GATE_DIR").unwrap());
    let roi = Rect {
        left: 320,
        top: 200,
        right: 2240,
        bottom: 1280,
    };
    let scene = fixture::Scene::start(roi, true);
    let bounds = Rect {
        left: 370,
        top: 250,
        right: 1020,
        bottom: 450,
    };
    let mut excluded = vec![
        WindowToken(scene.windows["control"].as_i64().unwrap() as isize),
        WindowToken(scene.windows["hint"].as_i64().unwrap() as isize),
    ];
    excluded.extend(
        scene.windows["borders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| WindowToken(id.as_i64().unwrap() as isize)),
    );
    let request = |revision| ScreenCaptureRequest {
        revision,
        regions: vec![bounds],
        paused_regions: vec![],
        excluded_windows: excluded.clone(),
    };
    let port = WindowsScreenCapturePort::new().unwrap();
    assert!(
        port.try_freeze_screenshot(&request(1), &AtomicBool::new(false))
            .unwrap()
            .is_none()
    );
    let wait = |revision| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(frame) = port.latest().unwrap()
                && frame.revision == revision
            {
                assert_eq!(
                    &frame.regions[0].pixels[frame.regions[0].pixels.len() - 4..],
                    &[32, 52, 76, 255]
                );
                return frame.serial;
            }
            assert!(
                Instant::now() < deadline,
                "Existing adapter did not publish the requested frame"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    port.request(request(1)).unwrap();
    let first = wait(1);
    port.clear();
    assert!(port.latest().unwrap().is_none());
    let spec = run::Spec {
        dynamic_ms: 2000,
        static_ms: 1000,
        pause_ms: 500,
        resume_ms: 1000,
        ..Default::default()
    };
    let file = folder.join("independent.mp4");
    let result = std::thread::scope(|scope| {
        let recording = scope.spawn(|| run::run(&file, roi, &scene, spec));
        port.request(request(2)).unwrap();
        assert!(wait(2) > first);
        port.clear();
        recording.join().unwrap().unwrap()
    });
    assert!(port.latest().unwrap().is_none());
    port.request(request(3)).unwrap();
    wait(3);
    port.clear();
    drop(port);
    drop(scene);
    resources::assert_retired();
    std::fs::write(
        folder.join("independent.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
