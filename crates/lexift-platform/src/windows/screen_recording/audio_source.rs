//! One event-driven apartment per endpoint. Only owned PCM and commands cross
//! the boundary; cancellation never substitutes another thread for a stuck one.
use super::audio_capture::{Endpoint, Packet};
use lexift_core::{Error, Result, domain::recording_audio::MAX_QUEUE_FRAMES};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use windows::Win32::{Foundation::*, System::Threading::*};

#[derive(Clone, Copy)]
enum Command {
    Start,
    Stop,
    Exit,
}
#[derive(Default)]
struct State {
    ready: bool,
    running: bool,
    error: Option<String>,
    commands: VecDeque<Command>,
    packets: VecDeque<Packet>,
    frames: usize,
}
struct Shared {
    state: Mutex<State>,
    changed: AtomicBool,
    event: Mutex<Option<isize>>,
}
struct AudioPriority(HANDLE);
impl AudioPriority {
    fn acquire() -> Option<Self> {
        match unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Audio"), &mut 0) } {
            Ok(handle) => Some(Self(handle)),
            Err(error) => {
                tracing::debug!(
                    hresult = error.code().0,
                    "MMCSS audio scheduling unavailable"
                );
                None
            }
        }
    }
}
impl Drop for AudioPriority {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}
impl Shared {
    fn signal(&self) {
        let event = self.event.lock().unwrap();
        if let Some(event) = *event {
            unsafe {
                let _ = SetEvent(HANDLE(event as *mut _));
            }
        }
    }
    fn fail(&self, error: windows::core::Error, system: bool) {
        use windows::Win32::Media::{Audio::*, MediaFoundation::*};
        let code = error.code();
        let detail = error.message();
        let message = if code == E_ACCESSDENIED {
            "Recording audio permission denied"
        } else if code == AUDCLNT_E_DEVICE_INVALIDATED || detail.contains("endpoint changed") {
            "Recording audio device changed"
        } else if code == AUDCLNT_E_SERVICE_NOT_RUNNING {
            "Recording audio service unavailable"
        } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT || code == MF_E_INVALIDMEDIATYPE {
            "Recording audio format unsupported"
        } else if detail.contains("500ms") {
            "Recording audio queue is full"
        } else {
            "Recording audio unavailable"
        };
        let diagnostic = if [
            "Native audio",
            "Recording audio",
            "Recording microphone",
            "Unlocated recording",
            "Invalid recording",
            "Invalid converted",
        ]
        .iter()
        .any(|prefix| detail.starts_with(prefix))
        {
            detail.as_str()
        } else {
            "native API failure"
        };
        tracing::warn!(
            hresult = code.0,
            system,
            reason = message,
            diagnostic,
            "Recording audio source failed"
        );
        let mut state = self.state.lock().unwrap();
        state.error = Some(message.into());
        state.running = false;
    }
}
pub(super) struct Source {
    pub token: u64,
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Source {
    pub fn prepare(system: bool) -> Result<Self> {
        let event = unsafe { CreateEventW(None, false, false, None) }
            .map_err(|e| Error::new(e.to_string()))?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: AtomicBool::new(false),
            event: Mutex::new(Some(event.0 as isize)),
        });
        let owner = shared.clone();
        let thread = match thread::Builder::new()
            .name(
                if system {
                    "lexift-loopback"
                } else {
                    "lexift-microphone"
                }
                .into(),
            )
            .spawn(move || run(owner, system))
        {
            Ok(thread) => thread,
            Err(error) => {
                unsafe {
                    let _ = CloseHandle(event);
                }
                return Err(Error::new(error.to_string()));
            }
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Ok(Self {
            token: NEXT.fetch_add(1, Ordering::Relaxed),
            shared,
            thread: Some(thread),
        })
    }
    pub fn status(&self) -> Result<(bool, bool)> {
        let state = self.shared.state.lock().unwrap();
        if let Some(error) = &state.error {
            return Err(Error::new(error.clone()));
        }
        Ok((state.ready, state.running))
    }
    fn command(&self, command: Command) {
        self.shared
            .state
            .lock()
            .unwrap()
            .commands
            .push_back(command);
        self.shared.signal();
    }
    pub fn start(&self) {
        self.command(Command::Start);
    }
    pub fn stop(&self) {
        self.command(Command::Stop);
    }
    pub fn drain(&self) -> Result<Vec<Packet>> {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(error) = &state.error {
            return Err(Error::new(error.clone()));
        }
        state.frames = 0;
        Ok(state.packets.drain(..).collect())
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        self.command(Command::Exit);
        // Only the native encoder owner drops Source; never the UI thread.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // A queued device notification may briefly retain Shared after COM
        // unregisters. It must retain neither PCM nor a usable kernel handle.
        {
            let mut state = self.shared.state.lock().unwrap();
            state.packets = VecDeque::new();
            state.commands = VecDeque::new();
            state.frames = 0;
        }
        if let Some(event) = self.shared.event.lock().unwrap().take() {
            unsafe {
                let _ = CloseHandle(HANDLE(event as *mut _));
            }
        }
    }
}
fn run(shared: Arc<Shared>, system: bool) {
    let result = (|| -> windows::core::Result<()> {
        let _apartment = super::super::screen_capture::capture::Apartment::new()?;
        let _media = super::encoder::Media::new()?;
        let mut endpoint = Endpoint::prepare(system)?;
        let _priority = AudioPriority::acquire();
        let notification = shared.clone();
        endpoint.watch(Arc::new(move || {
            notification.changed.store(true, Ordering::Release);
            notification.signal();
        }))?;
        shared.state.lock().unwrap().ready = true;
        let command_event = shared.event.lock().unwrap().unwrap();
        let events = [HANDLE(command_event as *mut _), endpoint.event()];
        let mut last_packet = std::time::Instant::now();
        loop {
            let running = shared.state.lock().unwrap().running;
            let wait = unsafe {
                WaitForMultipleObjects(&events, false, if running { 500 } else { INFINITE })
            };
            if wait == WAIT_FAILED {
                return Err(windows::core::Error::from_thread());
            }
            if wait == WAIT_TIMEOUT && running {
                endpoint.check_health()?;
                if !system && last_packet.elapsed() >= std::time::Duration::from_millis(500) {
                    return Err(windows::core::Error::new(
                        E_FAIL,
                        "Recording microphone stopped delivering samples",
                    ));
                }
            }
            if shared.changed.load(Ordering::Acquire) {
                return Err(windows::core::Error::new(
                    E_FAIL,
                    "Recording audio endpoint changed",
                ));
            }
            loop {
                let command = shared.state.lock().unwrap().commands.pop_front();
                match command {
                    Some(Command::Exit) => return Ok(()),
                    Some(Command::Start) => {
                        endpoint.start()?;
                        last_packet = std::time::Instant::now();
                        shared.state.lock().unwrap().running = true;
                    }
                    Some(Command::Stop) => {
                        let tail = endpoint.finish()?;
                        let mut state = shared.state.lock().unwrap();
                        for packet in tail {
                            if state.frames.saturating_add(packet.frames.len()) > MAX_QUEUE_FRAMES {
                                return Err(windows::core::Error::new(
                                    E_FAIL,
                                    "Recording audio backlog exceeded 500ms",
                                ));
                            }
                            state.frames += packet.frames.len();
                            state.packets.push_back(packet);
                        }
                        state.running = false;
                    }
                    None => break,
                }
            }
            if !shared.state.lock().unwrap().running {
                continue;
            }
            while let Some(packet) = endpoint.read()? {
                last_packet = std::time::Instant::now();
                let mut state = shared.state.lock().unwrap();
                if state.frames.saturating_add(packet.frames.len()) > MAX_QUEUE_FRAMES {
                    return Err(windows::core::Error::new(
                        E_FAIL,
                        "Recording audio backlog exceeded 500ms",
                    ));
                }
                state.frames += packet.frames.len();
                state.packets.push_back(packet);
            }
        }
    })();
    if let Err(error) = result {
        shared.fail(error, system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_silent_packets_cannot_rewrite_committed_idle_silence() {
        use lexift_core::domain::recording_audio::Mode;
        fn queued(value: f32) -> Source {
            Source {
                token: 0,
                thread: None,
                shared: Arc::new(Shared {
                    event: Mutex::new(None),
                    changed: AtomicBool::new(false),
                    state: Mutex::new(State {
                        ready: true,
                        running: true,
                        frames: 480,
                        packets: VecDeque::from([Packet {
                            silence_gap: false,
                            qpc: 0,
                            position: 0,
                            frames: vec![[value; 2]; 480],
                            discontinuity: false,
                        }]),
                        ..Default::default()
                    }),
                }),
            }
        }
        for value in [0., 0.5] {
            let mut sources = super::super::audio::Sources::default();
            sources.mode = Mode::System;
            sources.segment(0, 0);
            assert_eq!(sources.ready_count(480, 1_100_000), 480);
            assert_eq!(sources.render(480).unwrap(), vec![[0.; 2]; 480]);
            sources.written = 480;
            sources.system = Some(queued(value));
            assert_eq!(sources.collect().is_ok(), value == 0.);
        }
    }
    #[test]
    fn injected_native_failures_keep_localized_result_categories() {
        use windows::Win32::Media::Audio::*;
        for (code, expected) in [
            (E_ACCESSDENIED, "Recording audio permission denied"),
            (
                AUDCLNT_E_SERVICE_NOT_RUNNING,
                "Recording audio service unavailable",
            ),
            (
                AUDCLNT_E_DEVICE_INVALIDATED,
                "Recording audio device changed",
            ),
            (
                AUDCLNT_E_UNSUPPORTED_FORMAT,
                "Recording audio format unsupported",
            ),
        ] {
            let shared = Shared {
                state: Mutex::new(State::default()),
                changed: AtomicBool::new(false),
                event: Mutex::new(None),
            };
            shared.fail(windows::core::Error::from_hresult(code), false);
            assert_eq!(
                shared.state.lock().unwrap().error.as_deref(),
                Some(expected)
            );
        }
    }
    /// Exercise real apartment ownership, including cancellation and a queued
    /// notification. Only counters are retained; captured PCM is discarded.
    #[test]
    #[ignore = "real default endpoints, serial lifecycle validation"]
    fn native_source_lifecycle_releases_callbacks_and_handles() {
        use std::time::{Duration, Instant};
        fn until(mut ready: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !ready() {
                assert!(
                    Instant::now() < deadline,
                    "native source retirement timed out"
                );
                thread::sleep(Duration::from_millis(2));
            }
        }
        fn handles() -> u32 {
            let mut count = 0;
            unsafe {
                GetProcessHandleCount(GetCurrentProcess(), &mut count).unwrap();
            }
            count
        }
        let mut baseline = 0;
        for round in 0..22 {
            let system = Source::prepare(true).unwrap();
            let microphone = Source::prepare(false).unwrap();
            until(|| system.status().unwrap().0 && microphone.status().unwrap().0);
            let weak = [
                Arc::downgrade(&system.shared),
                Arc::downgrade(&microphone.shared),
            ];
            system.start();
            microphone.start();
            until(|| system.status().unwrap().1 && microphone.status().unwrap().1);
            let deadline = Instant::now() + Duration::from_millis(150);
            while Instant::now() < deadline {
                let _ = system.drain().unwrap();
                let _ = microphone.drain().unwrap();
                thread::sleep(Duration::from_millis(5));
            }
            if round % 3 == 0 {
                system.stop();
                microphone.stop();
                until(|| !system.status().unwrap().1 && !microphone.status().unwrap().1);
            } else if round % 3 == 1 {
                // An injected callback exercises error retirement, not a claim
                // that a physical device was disconnected.
                microphone.shared.changed.store(true, Ordering::Release);
                microphone.shared.signal();
                until(|| microphone.status().is_err());
            }
            drop(system);
            drop(microphone);
            until(|| weak.iter().all(|w| w.upgrade().is_none()));
            let count = handles();
            if round == 1 {
                baseline = count;
            }
            if round > 1 {
                assert!(
                    count <= baseline + 2,
                    "owned handles grew: {baseline} -> {count}"
                );
            }
            eprintln!("source lifecycle round={round} handles={count} shared_owners=0");
        }
    }
    #[test]
    fn retired_notification_cannot_signal_a_reused_handle_or_retain_pcm() {
        let event = unsafe { CreateEventW(None, false, false, None).unwrap() };
        let mut state = State {
            frames: 480,
            ..State::default()
        };
        state.packets.push_back(Packet {
            qpc: 1,
            position: 0,
            discontinuity: false,
            silence_gap: false,
            frames: vec![[0.; 2]; 480],
        });
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            changed: AtomicBool::new(false),
            event: Mutex::new(Some(event.0 as isize)),
        });
        let callback = shared.clone();
        drop(Source {
            token: 1,
            shared,
            thread: None,
        });
        let replacement = unsafe { CreateEventW(None, false, false, None).unwrap() };
        callback.changed.store(true, Ordering::Release);
        callback.signal();
        assert!(callback.event.lock().unwrap().is_none());
        assert!(callback.state.lock().unwrap().packets.is_empty());
        assert_eq!(unsafe { WaitForSingleObject(replacement, 0) }, WAIT_TIMEOUT);
        unsafe {
            CloseHandle(replacement).unwrap();
        }
    }
}
