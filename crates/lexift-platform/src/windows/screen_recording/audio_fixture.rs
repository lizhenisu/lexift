//! Explicit test renderer. A shared playback clock drives the owned visual
//! fixture, avoiding legacy waveOut position emulation in sync measurements.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Media::Audio::*,
        System::{Com::*, Memory::*, Threading::*},
    },
    core::*,
};
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Mapping(MEMORY_MAPPED_VIEW_ADDRESS);
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.0);
        }
    }
}
struct Playing(IAudioClient);
impl Drop for Playing {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}
#[test]
#[ignore = "raw loopback timing, bypassing MFT/mixer/encoder; owned fixture required"]
fn raw_loopback_playback_clock_diagnostics() {
    unsafe {
        let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
        let name = HSTRING::from(
            std::env::var("LEXIFT_AUDIO_FIXTURE_SHARED").expect("owned shared mapping"),
        );
        let mapping = Handle(OpenFileMappingW(FILE_MAP_READ.0, false, &name).unwrap());
        let view = Mapping(MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, 4096));
        assert!(!view.0.Value.is_null());
        let shared = std::slice::from_raw_parts(view.0.Value.cast::<AtomicU64>(), 4);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .unwrap();
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();
        let format = client.GetMixFormat().unwrap();
        let channels = (*format).nChannels;
        let rate = (*format).nSamplesPerSec;
        let bits = (*format).wBitsPerSample;
        assert_eq!(channels, 2);
        assert_eq!(rate, 48000);
        assert_eq!(bits, 32);
        let initialized = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK,
            0,
            0,
            format,
            None,
        );
        CoTaskMemFree(Some(format.cast()));
        initialized.unwrap();
        let event = Handle(CreateEventW(None, false, false, None).unwrap());
        client.SetEventHandle(event.0).unwrap();
        let capture: IAudioCaptureClient = client.GetService().unwrap();
        eprintln!(
            "raw loopback stream latency_100ns={}",
            client.GetStreamLatency().unwrap()
        );
        client.Start().unwrap();
        let _playing = Playing(client.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut active = false;
        while Instant::now() < deadline {
            while capture.GetNextPacketSize().unwrap() > 0 {
                let mut pointer = std::ptr::null_mut();
                let (mut count, mut flags, mut pos, mut qpc) = (0, 0, 0, 0);
                capture
                    .GetBuffer(
                        &mut pointer,
                        &mut count,
                        &mut flags,
                        Some(&mut pos),
                        Some(&mut qpc),
                    )
                    .unwrap();
                let first = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    None
                } else {
                    (0..count as usize).find(|&i| {
                        (0..2).any(|ch| {
                            pointer
                                .add((i * 2 + ch) * 4)
                                .cast::<f32>()
                                .read_unaligned()
                                .abs()
                                > 0.01
                        })
                    })
                };
                capture.ReleaseBuffer(count).unwrap();
                if let Some(index) = first
                    && !active
                {
                    let before = shared[0].load(Ordering::Acquire);
                    let played = shared[1].load(Ordering::Relaxed);
                    let clock_qpc = shared[2].load(Ordering::Relaxed);
                    let after = shared[0].load(Ordering::Acquire);
                    if before == after && before.is_multiple_of(2) {
                        let onset = qpc as f64 + index as f64 * 10_000_000. / 48000.;
                        let playback =
                            played as f64 + (onset - clock_qpc as f64) * 48000. / 10_000_000.;
                        eprintln!(
                            "raw loopback pulse qpc={onset:.0} device_position={pos} phase_ms={:.3}",
                            playback.rem_euclid(96000.) / 48.
                        );
                    }
                }
                active = first.is_some();
            }
            WaitForSingleObject(event.0, 20);
        }
    }
}
#[test]
#[ignore = "owned playback/capture QPC comparison; logs timing only, discards PCM"]
fn loopback_playback_clock_diagnostics() {
    unsafe {
        let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
        let _media = super::encoder::Media::new().unwrap();
        let name = HSTRING::from(
            std::env::var("LEXIFT_AUDIO_FIXTURE_SHARED").expect("owned shared mapping"),
        );
        let mapping = Handle(OpenFileMappingW(FILE_MAP_READ.0, false, &name).unwrap());
        let view = Mapping(MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, 4096));
        assert!(!view.0.Value.is_null());
        let shared = std::slice::from_raw_parts(view.0.Value.cast::<AtomicU64>(), 4);
        let mut endpoint = super::audio_capture::Endpoint::prepare(true).unwrap();
        endpoint.start().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut active = false;
        while Instant::now() < deadline {
            while let Some(packet) = endpoint.read().unwrap() {
                let first = packet
                    .frames
                    .iter()
                    .position(|s| s[0].abs().max(s[1].abs()) > 0.01);
                if let Some(index) = first
                    && !active
                {
                    let before = shared[0].load(Ordering::Acquire);
                    let played = shared[1].load(Ordering::Relaxed);
                    let qpc = shared[2].load(Ordering::Relaxed);
                    let after = shared[0].load(Ordering::Acquire);
                    if before == after && before.is_multiple_of(2) {
                        let onset = packet.qpc as f64 + index as f64 * 10_000_000. / 48000.;
                        let playback = played as f64 + (onset - qpc as f64) * 48000. / 10_000_000.;
                        eprintln!(
                            "loopback pulse qpc={onset:.0} renderer_clock_qpc={qpc} renderer_played={played} inferred_phase_ms={:.3}",
                            playback.rem_euclid(96000.) / 48.
                        );
                    }
                }
                active = first.is_some();
            }
            WaitForSingleObject(endpoint.event(), 20);
        }
        endpoint.stop().unwrap();
    }
}

#[test]
#[ignore = "owned test playback clock bridge; LEXIFT_AUDIO_FIXTURE_SHARED required"]
fn wasapi_playback_clock_bridge() {
    unsafe {
        let _apartment = super::super::screen_capture::capture::Apartment::new().unwrap();
        let name = HSTRING::from(
            std::env::var("LEXIFT_AUDIO_FIXTURE_SHARED").expect("owned shared mapping"),
        );
        let mapping = Handle(
            CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, 4096, &name).unwrap(),
        );
        let view = Mapping(MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, 4096));
        assert!(!view.0.Value.is_null());
        let pointer = view.0.Value.cast::<AtomicU64>();
        for i in 0..4 {
            pointer.add(i).write(AtomicU64::new(0));
        }
        let shared = std::slice::from_raw_parts(pointer, 4);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .unwrap();
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();
        let format = WAVEFORMATEX {
            wFormatTag: 3,
            nChannels: 2,
            nSamplesPerSec: 48000,
            nAvgBytesPerSec: 384000,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                0,
                0,
                &format,
                None,
            )
            .unwrap();
        let event = Handle(CreateEventW(None, false, false, None).unwrap());
        client.SetEventHandle(event.0).unwrap();
        let render: IAudioRenderClient = client.GetService().unwrap();
        let clock: IAudioClock = client.GetService().unwrap();
        let frequency = clock.GetFrequency().unwrap();
        let capacity = client.GetBufferSize().unwrap();
        let mut written = 0u64;
        let fill = |count: u32, written: &mut u64| {
            let buffer = render.GetBuffer(count).unwrap();
            for i in 0..count as usize {
                let sample = *written + i as u64;
                let pulse = (24000..29760).contains(&(sample % 96000));
                for (channel, hz) in [880., 1760.].into_iter().enumerate() {
                    let value = if pulse {
                        (sample as f64 / 48000. * std::f64::consts::TAU * hz).sin() as f32 * 0.15
                    } else {
                        0.
                    };
                    buffer
                        .add((i * 2 + channel) * 4)
                        .cast::<f32>()
                        .write_unaligned(value);
                }
            }
            render.ReleaseBuffer(count, 0).unwrap();
            *written += u64::from(count);
        };
        fill(capacity, &mut written);
        client.Start().unwrap();
        let _playing = Playing(client.clone());
        eprintln!("owned WASAPI playback bridge ready; frequency={frequency}");
        let deadline = Instant::now() + Duration::from_secs(2700);
        while Instant::now() < deadline && shared[3].load(Ordering::Acquire) == 0 {
            let wait = WaitForSingleObject(event.0, 50);
            assert_ne!(wait, WAIT_FAILED);
            let (mut position, mut qpc) = (0, 0);
            clock.GetPosition(&mut position, Some(&mut qpc)).unwrap();
            shared[0].fetch_add(1, Ordering::AcqRel);
            shared[1].store(
                (u128::from(position) * 48000 / u128::from(frequency)) as u64,
                Ordering::Relaxed,
            );
            shared[2].store(qpc, Ordering::Relaxed);
            shared[0].fetch_add(1, Ordering::Release);
            let padding = client.GetCurrentPadding().unwrap();
            if padding < capacity {
                fill(capacity - padding, &mut written);
            }
        }
        eprintln!("owned WASAPI playback bridge stopped; submitted_samples={written}");
    }
}
