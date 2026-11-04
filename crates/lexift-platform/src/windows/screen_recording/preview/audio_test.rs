//! Test-only loopback metrology of an owned, prerecorded pulse/flash fixture.
use super::{engine::Engine, surface::Surface};
use lexift_core::{
    domain::{geometry::Rect, recording_preview::Target},
    ports::magnifier::WindowToken,
};
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::DwmFlush,
        System::Com::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::*,
};
/// Independent loopback probe: bypass the recording adapter's resampler and
/// timestamp correction when measuring a playback implementation.
struct Probe {
    client: windows::Win32::Media::Audio::IAudioClient,
    capture: windows::Win32::Media::Audio::IAudioCaptureClient,
}
impl Probe {
    fn new() -> Result<Self> {
        unsafe {
            use windows::Win32::Media::Audio::*;
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            assert_eq!(
                (
                    (*format).nSamplesPerSec,
                    (*format).nChannels,
                    (*format).wBitsPerSample
                ),
                (48000, 2, 32)
            );
            let result = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                0,
                0,
                format,
                None,
            );
            CoTaskMemFree(Some(format.cast()));
            result?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self { client, capture })
        }
    }
    fn read(&self) -> Result<Option<super::super::audio_capture::Packet>> {
        unsafe {
            use windows::Win32::Media::Audio::*;
            if self.capture.GetNextPacketSize()? == 0 {
                return Ok(None);
            }
            let (mut data, mut count, mut flags, mut position, mut qpc) =
                (std::ptr::null_mut(), 0, 0, 0, 0);
            self.capture.GetBuffer(
                &mut data,
                &mut count,
                &mut flags,
                Some(&mut position),
                Some(&mut qpc),
            )?;
            assert_eq!(flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32, 0);
            let frames = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                vec![[0., 0.]; count as usize]
            } else {
                (0..count as usize)
                    .map(|i| {
                        std::array::from_fn(|ch| {
                            data.add((i * 2 + ch) * 4).cast::<f32>().read_unaligned()
                        })
                    })
                    .collect()
            };
            self.capture.ReleaseBuffer(count)?;
            Ok(Some(super::super::audio_capture::Packet {
                silence_gap: false,
                qpc,
                position,
                discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
                frames,
            }))
        }
    }
    fn stop(&self) -> Result<()> {
        unsafe { self.client.Stop() }
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[test]
#[ignore = "native pulse/flash playback; owned LEXIFT_PREVIEW_SYNC_FILE and report directory required"]
fn native_preview_playback_audio_sync() {
    let folder = std::path::PathBuf::from(std::env::var("LEXIFT_PREVIEW_GATE_DIR").unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    let result = std::panic::catch_unwind(measure);
    match result {
        Ok(report) => {
            std::fs::write(
                folder.join("playback-sync.json"),
                serde_json::to_vec_pretty(&report).unwrap(),
            )
            .unwrap();
            assert!(report["passed"].as_bool().unwrap(), "{report}");
        }
        Err(error) => {
            std::fs::write(
                folder.join("playback-sync.json"),
                br#"{"passed":false,"failure":"native test assertion; see harness log"}"#,
            )
            .unwrap();
            std::panic::resume_unwind(error);
        }
    }
}
fn measure() -> serde_json::Value {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() }
            }
        }
        let _apartment = Apartment;
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        struct Dpi(DPI_AWARENESS_CONTEXT);
        impl Drop for Dpi {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
        let _dpi = Dpi(old);
        let origin = std::env::var("LEXIFT_PREVIEW_GATE_ORIGIN").unwrap_or("100,100".into());
        let xy: Vec<i32> = origin.split(',').map(|x| x.parse().unwrap()).collect();
        let parent = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("Lexift owned preview sync fixture"),
            WS_POPUP,
            xy[0],
            xy[1],
            1024,
            640,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        struct Parent(HWND);
        impl Drop for Parent {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let _parent = Parent(parent);
        SetWindowDisplayAffinity(parent, WDA_EXCLUDEFROMCAPTURE).unwrap();
        SetWindowPos(
            parent,
            Some(HWND_TOPMOST),
            xy[0],
            xy[1],
            1024,
            640,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .unwrap();
        let path = std::env::var("LEXIFT_PREVIEW_SYNC_FILE").unwrap();
        let mut engine = Engine::open(std::path::Path::new(&path)).unwrap();
        let mut surface = Surface::new(Target {
            window: WindowToken(parent.0 as isize),
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 640,
            },
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut first = None;
        while first.is_none() && Instant::now() < deadline {
            engine.check_events().unwrap();
            first = engine.frame().unwrap();
            if first.is_some() {
                surface.present(&engine).unwrap();
            } else {
                DwmFlush().unwrap();
            }
        }
        assert!(first.is_some());
        assert!(engine.media.IsPaused().as_bool());
        // Validate the rendered swap-chain image, not just decoder pixels:
        // a 16:9 image in 1024x640 has 32px black bars above and below it.
        let reference = engine.read_pixel(60, 503).unwrap();
        for (point, black) in [
            ((16, 8), true),
            ((16, 24), true),
            ((16, 40), false),
            ((16, 620), true),
        ] {
            surface.test_probe = Some(point);
            surface.present(&engine).unwrap();
            let actual = surface.test_pixel.unwrap();
            if black {
                assert_eq!(&actual[..3], &[0, 0, 0]);
            } else {
                assert!(
                    actual
                        .iter()
                        .zip(reference)
                        .all(|(a, b)| a.abs_diff(b) <= 2)
                );
            }
        }
        surface.test_probe = Some((800, 139));
        let endpoint = Probe::new().unwrap();
        let silent_until = Instant::now() + Duration::from_millis(200);
        let mut ambient = 0f32;
        while Instant::now() < silent_until {
            while let Some(packet) = endpoint.read().unwrap() {
                for s in packet.frames {
                    ambient = ambient.max(s[0].abs()).max(s[1].abs());
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            ambient < 0.0001,
            "loopback has unrelated ambient sound, peak={ambient}"
        );
        let play_qpc = super::super::super::recording_capture::qpc_100ns().unwrap() as f64 / 1e7;
        engine.media.Play().unwrap();
        let seconds: u64 = std::env::var("LEXIFT_PREVIEW_SYNC_SECONDS")
            .ok()
            .map(|value| value.parse().expect("positive validation duration"))
            .unwrap_or(10);
        assert!((1..=610).contains(&seconds));
        let until = Instant::now() + Duration::from_secs(seconds);
        let mut flashes = Vec::new();
        let mut flash_media_pts = Vec::new();
        let mut pulses = Vec::new();
        let (mut bright, mut active) = (false, false);
        let mut channel_active = [false, false];
        let mut channel_pulses = [Vec::new(), Vec::new()];
        let mut block = Vec::with_capacity(48);
        let mut block_time = 0.;
        let mut frames = 0;
        let mut discontinuities = Vec::new();
        while Instant::now() < until {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            engine.check_events().unwrap();
            let frame = engine.frame().unwrap();
            if let Some(pts) = frame {
                frames += 1;
                let before = super::super::super::recording_capture::qpc_100ns().unwrap();
                surface.present(&engine).unwrap();
                let now = surface.test_pixel.unwrap()[0] > 128;
                if now && !bright {
                    surface.flush().unwrap();
                    let after = super::super::super::recording_capture::qpc_100ns().unwrap();
                    flashes.push((before as f64 / 1e7, after as f64 / 1e7));
                    flash_media_pts.push(pts);
                }
                bright = now;
            } else {
                DwmFlush().unwrap();
            }
            while let Some(packet) = endpoint.read().unwrap() {
                if packet.discontinuity {
                    discontinuities.push((packet.position, packet.qpc));
                }
                for (i, sample) in packet.frames.into_iter().enumerate() {
                    if block.is_empty() {
                        block_time = packet.qpc as f64 / 1e7 + i as f64 / 48000.;
                    }
                    block.push(sample);
                    if block.len() == 48 {
                        let rms = (block
                            .iter()
                            .map(|s| f64::from(s[0]).powi(2) + f64::from(s[1]).powi(2))
                            .sum::<f64>()
                            / 96.)
                            .sqrt();
                        let now = rms > 0.01;
                        for ch in 0..2 {
                            let rms = (block.iter().map(|s| f64::from(s[ch]).powi(2)).sum::<f64>()
                                / 48.)
                                .sqrt();
                            let on = rms > 0.01;
                            if on && !channel_active[ch] {
                                channel_pulses[ch].push(block_time - play_qpc);
                            }
                            channel_active[ch] = on;
                        }
                        if now && !active {
                            pulses.push(block_time);
                        }
                        active = now;
                        block.clear();
                    }
                }
            }
        }
        engine.media.Pause().unwrap();
        endpoint.stop().unwrap();
        let mut used = std::collections::HashSet::new();
        let mut errors = Vec::new();
        for (lo, hi) in flashes.iter().copied() {
            let hits: Vec<_> = pulses
                .iter()
                .enumerate()
                .filter(|(i, p)| !used.contains(i) && (**p - (lo + hi) / 2.).abs() < 0.25)
                .collect();
            assert_eq!(hits.len(), 1, "ambiguous or missing playback pulse");
            let (i, p) = hits[0];
            used.insert(i);
            errors.push([(p - hi) * 1000., (p - lo) * 1000.]);
        }
        let bound = errors.iter().flatten().map(|x| x.abs()).fold(0., f64::max);
        serde_json::json!({"passed":errors.len()>=4&&used.len()==pulses.len()&&bound<=50.,"channel_pulses_seconds":channel_pulses,"frames":frames,"ambient_peak":ambient,"duration_seconds":seconds,"discontinuities":discontinuities,"flash_media_pts":flash_media_pts,"flash_count":flashes.len(),"pulse_count":pulses.len(),"pulse_seconds":pulses.iter().map(|t|t-play_qpc).collect::<Vec<_>>(),"flash_seconds":flashes.iter().map(|(lo,hi)|[lo-play_qpc,hi-play_qpc]).collect::<Vec<_>>(),"error_intervals_ms":errors,"maximum_error_ms":bound,"scope":"WASAPI loopback versus native child Present/DWM completion; not acoustic or photon timing"})
    }
}

#[test]
#[ignore = "independent per-channel WASAPI clock calibration; owned shared playback source required"]
fn native_preview_channel_clock_calibration() {
    unsafe {
        use std::sync::atomic::{AtomicU64, Ordering};
        use windows::Win32::System::Memory::*;
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        let name = HSTRING::from(std::env::var("LEXIFT_AUDIO_FIXTURE_SHARED").unwrap());
        let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, &name).unwrap();
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 4096);
        assert!(!view.Value.is_null());
        let shared = std::slice::from_raw_parts(view.Value.cast::<AtomicU64>(), 4);
        let probe = Probe::new().unwrap();
        let mut active = [false, false];
        let mut block = Vec::with_capacity(48);
        let mut time = 0.;
        let mut phases = [Vec::new(), Vec::new()];
        let until = Instant::now() + Duration::from_secs(6);
        while Instant::now() < until {
            while let Some(p) = probe.read().unwrap() {
                for (i, s) in p.frames.into_iter().enumerate() {
                    if block.is_empty() {
                        time = p.qpc as f64 / 1e7 + i as f64 / 48000.;
                    }
                    block.push(s);
                    if block.len() == 48 {
                        for ch in 0..2 {
                            let rms = (block.iter().map(|s| f64::from(s[ch]).powi(2)).sum::<f64>()
                                / 48.)
                                .sqrt();
                            let on = rms > 0.01;
                            if on && !active[ch] {
                                let seq = shared[0].load(Ordering::Acquire);
                                let played = shared[1].load(Ordering::Relaxed);
                                let qpc = shared[2].load(Ordering::Relaxed);
                                if seq == shared[0].load(Ordering::Acquire) && seq.is_multiple_of(2)
                                {
                                    let sample = played as f64 + (time - qpc as f64 / 1e7) * 48000.;
                                    phases[ch].push(sample.rem_euclid(96000.) / 48.);
                                }
                            }
                            active[ch] = on;
                        }
                        block.clear();
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(probe);
        UnmapViewOfFile(view).unwrap();
        CloseHandle(mapping).unwrap();
        CoUninitialize();
        let folder = std::path::PathBuf::from(std::env::var("LEXIFT_PREVIEW_GATE_DIR").unwrap());
        std::fs::create_dir_all(&folder).unwrap();
        let report = serde_json::json!({"passed":phases.iter().all(|p|p.len()>=2),"channel_phases_ms":phases,"source_pulse_phase_ms":500});
        std::fs::write(
            folder.join("channel-calibration.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        eprintln!("{report}");
        assert!(report["passed"].as_bool().unwrap());
    }
}
