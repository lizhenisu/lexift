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
#[test]
#[ignore = "synthetic delayed-audio interruption; writes owned partial MP4"]
fn audio_backlog_can_seal_a_marked_partial_without_reentering_wait() {
    let folder = PathBuf::from(std::env::var_os("LEXIFT_RECORDING_NATIVE_DIR").unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
    let _media = encoder::Media::new().unwrap();
    let bounds = lexift_core::domain::geometry::Rect {
        left: 0,
        top: 0,
        right: 320,
        bottom: 180,
    };
    let id = Identity {
        session: 95,
        operation: 1,
        request: 1,
    };
    let (tx, rx) = mpsc::channel();
    let mut job = Job::prepare(
        PrepareRequest {
            video: Default::default(),
            identity: id,
            bounds,
            monitor_bounds: bounds,
            excluded_windows: vec![],
        },
        Arc::new(move |n| tx.send(n).unwrap()),
    );
    job.file = Some(FileArtifact::new().unwrap());
    job.encoder = Some(
        encoder::Encoder::new_av(
            &job.file.as_ref().unwrap().path,
            320,
            180,
            true,
            Default::default(),
        )
        .unwrap(),
    );
    job.clock = Clock::new(30, Duration::ZERO);
    job.origin = Instant::now() - Duration::from_secs(1);
    job.pending = vec![128; 320 * 180 * 3 / 2];
    job.pending[..320 * 180].fill(16);
    job.pixels = job.pending.clone();
    // Inject accepted video slots while audio is stalled. This is an internal
    // backlog fixture, not a claim that the real microphone was disconnected.
    for _ in 0..30 {
        job.write_pixels().unwrap();
    }
    job.phase = Phase::Recording;
    job.audio.mode = lexift_core::domain::recording_audio::Mode::Microphone;
    let error = job.write_ready_audio().unwrap_err();
    assert!(std::mem::take(&mut job.audio_wait_failed));
    job.audio_interrupted(error).unwrap();
    assert!(matches!(rx.recv().unwrap(), Notice::Interrupted { .. }));
    job.control(Identity { request: 2, ..id }, Control::Finish)
        .unwrap();
    assert!(matches!(job.phase, Phase::Finishing));
    job.tick().unwrap();
    assert!(
        matches!(rx.recv().unwrap(), Notice::Finished { artifact, .. }
        if artifact.frames == 30 && artifact.interrupted == Some(Interruption::AudioUnavailable))
    );
    assert_eq!(job.audio.written, 0, "sealed job releases audio state");
    assert!(job.encoder.is_none());
    std::fs::copy(
        &job.file.as_ref().unwrap().path,
        folder.join("backlog-partial.mp4"),
    )
    .unwrap();
}
#[test]
#[ignore = "writes synthetic audio/video to LEXIFT_RECORDING_NATIVE_DIR"]
fn aac_mux_synthetic_stereo() {
    let folder = PathBuf::from(
        std::env::var_os("LEXIFT_RECORDING_NATIVE_DIR").expect("owned evidence directory"),
    );
    std::fs::create_dir_all(&folder).unwrap();
    let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
    let _media = encoder::Media::new().unwrap();
    let encoder = encoder::Encoder::new_av(
        &folder.join("synthetic-aac.mp4"),
        320,
        180,
        true,
        Default::default(),
    )
    .unwrap();
    let pixels = vec![128; 320 * 180 * 3 / 2];
    for frame in 0..60u64 {
        let pcm: Vec<[f32; 2]> = (frame * 1600..(frame + 1) * 1600)
            .map(|i| {
                let t = i as f32 / 48000.;
                [
                    (t * 440. * std::f32::consts::TAU).sin() * 0.25,
                    (t * 880. * std::f32::consts::TAU).sin() * 0.125,
                ]
            })
            .collect();
        encoder.write_audio(&pcm, frame * 1600).unwrap();
        let start = (frame * 10_000_000 / 30) as i64;
        let end = ((frame + 1) * 10_000_000 / 30) as i64;
        encoder.write(&pixels, start, end - start).unwrap();
    }
    encoder.finish().unwrap();
}
#[test]
#[ignore = "opens actual default audio endpoints without persisting microphone PCM"]
fn audio_endpoints_prepare_start_stop() {
    for system in [true, false] {
        let source = audio_source::Source::prepare(system).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !source.status().unwrap().0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!source.status().unwrap().1, "preparation must not record");
        assert!(source.drain().unwrap().is_empty());
        source.start();
        while !source.status().unwrap().1 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let end = Instant::now() + Duration::from_millis(200);
        let mut frames = 0;
        while Instant::now() < end {
            frames += source
                .drain()
                .unwrap()
                .iter()
                .map(|p| p.frames.len())
                .sum::<usize>();
            thread::sleep(Duration::from_millis(5));
        }
        source.stop();
        while source.status().unwrap().1 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        eprintln!("endpoint system={system} copied_frames={frames}; PCM discarded");
    }
}
struct Source(HWND);
#[test]
fn missing_audio_source_cannot_silently_turn_into_a_silent_recording() {
    use lexift_core::domain::recording_audio::Mode;
    let mut sources = audio::Sources::default();
    for mode in [Mode::System, Mode::Microphone, Mode::Mixed] {
        assert!(sources.prepared(mode).is_err());
        sources.mode = mode;
        assert!(sources.running().is_err());
    }
    sources.mode = Mode::Silent;
    assert!(sources.prepared(Mode::Silent).unwrap());
    assert!(sources.running().unwrap());
}
#[test]
#[ignore = "native WGC/QPC metadata only; never writes captured pixels"]
fn capture_video_clock_anchor_diagnostics() {
    let b = lexift_core::domain::geometry::Rect {
        left: 320,
        top: 200,
        right: 2240,
        bottom: 1280,
    };
    let fresh = qpc_100ns().unwrap();
    let capture =
        Capture::new(b, super::super::screen_capture::arrival::Notice::new(|| {})).unwrap();
    let mut pixels = vec![0; 1920 * 1080 * 4];
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut seen = 0;
    while Instant::now() < deadline && seen < 10 {
        if let Some(timing) = capture.read(&mut pixels, fresh).unwrap() {
            let now = qpc_100ns().unwrap();
            eprintln!(
                "video clock source={} received={} difference_100ns={}",
                timing.source_100ns,
                now,
                now - timing.source_100ns
            );
            seen += 1;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(seen > 0);
}
#[test]
#[ignore = "native Media Foundation resampler; synthetic PCM only"]
fn audio_resampler_native_rates_and_drain() {
    let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
    let _media = encoder::Media::new().unwrap();
    for rate in [16000u32, 44100, 48000, 96000] {
        for channels in [1u16, 2] {
            let format = windows::Win32::Media::Audio::WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: channels,
                nSamplesPerSec: rate,
                nAvgBytesPerSec: rate * u32::from(channels) * 2,
                nBlockAlign: channels * 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let converter = unsafe { audio_convert::Converter::new(&format) }.unwrap();
            let mut result = Vec::new();
            for start in (0..rate).step_by((rate / 100) as usize) {
                let end = (start + rate / 100).min(rate);
                let bytes: Vec<u8> = (start..end)
                    .flat_map(|_| (0..channels).flat_map(|_| 8192i16.to_le_bytes()))
                    .collect();
                for (_, frames) in converter
                    .input(
                        &bytes,
                        end - start,
                        10_000_000 + u64::from(start) * 10_000_000 / u64::from(rate),
                    )
                    .unwrap()
                {
                    result.extend(frames);
                }
            }
            for (_, frames) in converter.finish().unwrap() {
                result.extend(frames);
            }
            // MF's interpolation tail can end before the nominal interval by
            // fewer than one input sample. The recording writer zero-pads that
            // bounded remainder to the final video coverage, never shifts PCM.
            assert!(
                result.len() <= 48000 && 48000 - result.len() < 48000u32.div_ceil(rate) as usize,
                "rate={rate} channels={channels} frames={}",
                result.len()
            );
            for frame in &result[100..result.len() - 100] {
                assert!(
                    (frame[0] - 0.25).abs() < 0.001 && (frame[1] - 0.25).abs() < 0.001,
                    "rate={rate} channels={channels} frame={frame:?}"
                );
            }
            eprintln!(
                "resampler rate={rate} channels={channels} frames={}",
                result.len()
            );
        }
    }
}
#[test]
#[ignore = "synthetic pulse/flash MP4 for independent AAC latency measurement"]
fn aac_mux_pulse_flash_sync() {
    let folder = PathBuf::from(
        std::env::var_os("LEXIFT_RECORDING_NATIVE_DIR").expect("owned evidence directory"),
    );
    std::fs::create_dir_all(&folder).unwrap();
    let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
    let _media = encoder::Media::new().unwrap();
    let encoder = encoder::Encoder::new_av(
        &folder.join("pulse-aac.mp4"),
        320,
        180,
        true,
        Default::default(),
    )
    .unwrap();
    for frame in 0..180u64 {
        let pulse = [15, 75, 135]
            .iter()
            .any(|&start| (start..start + 3).contains(&frame));
        let pcm: Vec<[f32; 2]> = (frame * 1600..(frame + 1) * 1600)
            .map(|i| {
                let value = if pulse {
                    (i as f32 / 48000. * 1000. * std::f32::consts::TAU).sin() * 0.5
                } else {
                    0.
                };
                [value; 2]
            })
            .collect();
        encoder.write_audio(&pcm, frame * 1600).unwrap();
        let mut pixels = vec![128; 320 * 180 * 3 / 2];
        pixels[..320 * 180].fill(if pulse { 235 } else { 16 });
        let start = (frame * 10_000_000 / 30) as i64;
        let end = ((frame + 1) * 10_000_000 / 30) as i64;
        encoder.write(&pixels, start, end - start).unwrap();
    }
    encoder.finish().unwrap();
}
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
                eprintln!("native fixture notice: {n:?}");
                if let Notice::Failed { error, .. } = &n {
                    panic!("native service failed: {error}");
                }
                if let Notice::StartRejected { error, .. } = &n {
                    panic!("native start rejected: {error}");
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
                video: Default::default(),
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
        assert_eq!(
            super::super::recording_gate::resources::ENCODER.load(Ordering::Acquire),
            0,
            "preparation must not create an encoding file"
        );
        if let Ok(mode) = std::env::var("LEXIFT_RECORDING_AUDIO_MODE") {
            use lexift_core::domain::recording_audio::Mode;
            let mode = match mode.as_str() {
                "system" => Mode::System,
                "microphone" => Mode::Microphone,
                "mixed" => Mode::Mixed,
                _ => panic!("invalid native fixture audio mode"),
            };
            id.request += 1;
            port.control(id, Control::ConfigureAudio(mode)).unwrap();
            let Notice::AudioConfigured { result, .. } =
                wait(&rx, |n| matches!(n, Notice::AudioConfigured { .. }))
            else {
                unreachable!()
            };
            assert_eq!(result.unwrap(), mode);
        }
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

/// Uses the production service and an actual 60Hz native source; decode markers independently.
#[test]
#[ignore = "native dynamic 1080p video configuration gate; owns visible test windows"]
fn production_video_configuration_dynamic_gate() {
    let delay: u64 = std::env::var("LEXIFT_RECORDING_TEST_SUBMIT_DELAY_MS")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .unwrap();
    use lexift_core::domain::screen_recording::{FrameRate, Quality, Video};
    let folder = PathBuf::from(std::env::var_os("LEXIFT_RECORDING_NATIVE_DIR").unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    let old = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let roi = lexift_core::domain::geometry::Rect {
        left: 320,
        top: 200,
        right: 2240,
        bottom: 1280,
    };
    let scene = super::super::recording_gate::fixture::Scene::start(roi, true);
    let port = WindowsScreenRecordingPort::new().unwrap();
    for (round, (rate, quality)) in [
        (FrameRate::Fps30, Quality::High),
        (FrameRate::Fps60, Quality::High),
        (FrameRate::Fps30, Quality::Standard),
        (FrameRate::Fps60, Quality::Standard),
    ]
    .into_iter()
    .filter(|(_, quality)| delay == 0 || *quality == Quality::High)
    .enumerate()
    {
        let (tx, rx) = mpsc::channel();
        let mut id = Identity {
            session: 92,
            operation: round as u64 + 1,
            request: 1,
        };
        let video = Video {
            frame_rate: rate,
            quality,
        };
        port.prepare(
            PrepareRequest {
                identity: id,
                bounds: roi,
                monitor_bounds: roi,
                excluded_windows: vec![],
                video: Default::default(),
            },
            Arc::new(move |n| {
                let _ = tx.send(n);
            }),
        )
        .unwrap();
        wait(&rx, |n| matches!(n, Notice::Prepared(_)));
        if video != Video::default() {
            id.request += 1;
            port.control(id, Control::ConfigureVideo(video)).unwrap();
            let Notice::VideoConfigured { result, .. } =
                wait(&rx, |n| matches!(n, Notice::VideoConfigured { .. }))
            else {
                unreachable!()
            };
            assert_eq!(result.unwrap(), video);
        }
        id.request += 1;
        port.control(id, Control::Start).unwrap();
        wait(&rx, |n| matches!(n, Notice::Started(_)));
        dwell(6000);
        id.request += 1;
        port.control(id, Control::Finish).unwrap();
        let Notice::Finished { artifact, .. } = wait(&rx, |n| matches!(n, Notice::Finished { .. }))
        else {
            unreachable!()
        };
        assert_eq!(
            artifact.interrupted.is_some(),
            delay >= 50 || (delay >= 20 && rate == FrameRate::Fps60)
        );
        assert_eq!(artifact.video, video);
        let (tx, result) = mpsc::channel();
        port.send(Message::ExportForTest(
            folder.join(format!("video-{round}.mp4")),
            tx,
        ))
        .unwrap();
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        std::fs::write(folder.join(format!("video-{round}.json")), serde_json::to_vec_pretty(&serde_json::json!({"fps":rate.get(),"quality":format!("{quality:?}"),"frames":artifact.frames,"duration_ms":artifact.duration.as_secs_f64()*1000.})).unwrap()).unwrap();
        id.request += 1;
        port.release(id).unwrap();
        wait(&rx, |n| matches!(n, Notice::Retired(_)));
    }
    let (tx, rx) = mpsc::channel();
    port.shutdown(Box::new(move || {
        let _ = tx.send(());
    }))
    .unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(port);
    drop(scene);
    unsafe {
        SetThreadDpiAwarenessContext(old);
    }
    super::super::recording_gate::resources::assert_retired();
}
