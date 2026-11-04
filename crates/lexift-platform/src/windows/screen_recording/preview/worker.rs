use super::{engine::Engine, surface::Surface};
use lexift_core::{
    Error, Result,
    domain::{
        geometry::Rect,
        recording_preview::{Action, Command, Identity, Notice, Target},
    },
    ports::{magnifier::WindowToken, screen_recording::PreviewListener},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};
use windows::Win32::{System::Com::*, UI::WindowsAndMessaging::*};
#[derive(Default)]
struct Pending {
    commands: VecDeque<Command>,
    close: Option<Identity>,
    stop: bool,
    bounds: Option<Rect>,
}
struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
}
type Targets = Mutex<HashMap<WindowToken, std::sync::Weak<Shared>>>;
fn targets() -> &'static Targets {
    static TARGETS: OnceLock<Targets> = OnceLock::new();
    TARGETS.get_or_init(Default::default)
}
pub(crate) fn position(window: WindowToken, bounds: Rect) -> bool {
    let slot = targets()
        .lock()
        .unwrap()
        .get(&window)
        .and_then(std::sync::Weak::upgrade);
    if let Some(s) = slot {
        s.pending.lock().unwrap().bounds = Some(bounds);
        s.wake.notify_one();
        true
    } else {
        false
    }
}
pub(crate) struct Worker {
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
    instance: Identity,
}
impl Worker {
    pub fn open(
        id: Identity,
        target: Target,
        file: Arc<super::super::FileArtifact>,
        last_frame: Duration,
        listener: PreviewListener,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Default::default()),
            wake: Condvar::new(),
        });
        let s = shared.clone();
        targets()
            .lock()
            .unwrap()
            .insert(target.window, Arc::downgrade(&shared));
        let thread = thread::Builder::new()
            .name("lexift-recording-preview".into())
            .spawn(move || run(id, target, file, last_frame, listener, s))
            .map_err(|_| Error::new("Recording preview unavailable"))?;
        Ok(Self {
            shared,
            thread: Some(thread),
            instance: id,
        })
    }
    pub fn control(&self, command: Command) -> Result<()> {
        let id = command.identity;
        if id.session != self.instance.session
            || id.operation != self.instance.operation
            || id.artifact != self.instance.artifact
            || id.instance != self.instance.instance
        {
            return Err(Error::new("Recording preview request is stale"));
        }
        let mut p = self.shared.pending.lock().unwrap();
        if command.action == Action::Close {
            p.close = Some(id);
            p.stop = true;
        } else {
            if p.stop {
                return Err(Error::new("Recording preview is retiring"));
            }
            if matches!(command.action, Action::Seek(_)) {
                p.commands.retain(|c| !matches!(c.action, Action::Seek(_)));
            }
            if p.commands.len() >= 16 {
                return Err(Error::new("Recording preview is busy"));
            }
            p.commands.push_back(command);
        }
        self.shared.wake.notify_one();
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.pending.lock().unwrap().stop = true;
        self.shared.wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn run(
    mut id: Identity,
    target: Target,
    file: Arc<super::super::FileArtifact>,
    last_frame: Duration,
    listener: PreviewListener,
    shared: Arc<Shared>,
) {
    let result = (|| -> windows::core::Result<()> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() }
            }
        }
        let _apartment = Apartment;
        use windows::Win32::UI::HiDpi::*;
        struct Dpi(DPI_AWARENESS_CONTEXT);
        impl Drop for Dpi {
            fn drop(&mut self) {
                unsafe {
                    SetThreadDpiAwarenessContext(self.0);
                }
            }
        }
        let _dpi = Dpi(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        });
        let mut surface = Surface::new(target)?;
        let mut engine = Engine::open(&file.path)?;
        let mut dirty = false;
        let mut first = true;
        let mut visible = false;
        let mut ended_notified = false;
        let mut waiting = true;
        let mut seek_target: Option<f64> = None;
        let mut deadline = Instant::now() + Duration::from_secs(10);
        let mut notify = Instant::now();
        let mut response = Some(Instant::now());
        loop {
            let (commands, bounds, stop) = {
                let mut p = shared.pending.lock().unwrap();
                (
                    p.commands.drain(..).collect::<Vec<_>>(),
                    p.bounds.take(),
                    p.stop,
                )
            };
            if stop {
                break;
            }
            for command in commands {
                if command.identity.request < id.request {
                    continue;
                }
                id = command.identity;
                unsafe {
                    match command.action {
                        Action::Play => {
                            response = Some(Instant::now());
                            ended_notified = false;
                            engine.media.Play()?;
                        }
                        Action::Pause => {
                            engine.media.Pause()?;
                            listener(id, Notice::Paused);
                        }
                        Action::Seek(position) => {
                            // AAC padding can extend container duration beyond
                            // the last video frame. Never seek into that tail.
                            let position = position.min(last_frame);
                            response = Some(Instant::now());
                            ended_notified = false;
                            engine.media.SetCurrentTime(position.as_secs_f64())?;
                            seek_target = Some(position.as_secs_f64());
                            waiting = true;
                            deadline = Instant::now() + Duration::from_secs(5);
                        }
                        Action::Mute(muted) => engine.media.SetMuted(muted)?,
                        _ => {}
                    }
                }
            }
            if let Some(b) = bounds {
                surface.resize(b)?;
                dirty = true;
            }
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            engine.check_events()?;
            let frame = engine
                .tick()?
                .filter(|p| seek_target.is_none_or(|t| (t - *p).abs() < 0.1));
            if frame.is_some() || dirty && engine.texture.is_some() {
                if frame.is_some() {
                    engine.accept_frame()?;
                }
                surface.present(&engine)?;
                if frame.is_some()
                    && let Some(start) = response.take()
                {
                    tracing::debug!(
                        session = id.session,
                        instance = id.instance,
                        request = id.request,
                        elapsed_ms = start.elapsed().as_secs_f64() * 1000.,
                        position_ms = frame.unwrap_or_default() * 1000.,
                        "Recording preview operation frame presented"
                    );
                }
                dirty = false;
                if first {
                    surface.flush()?;
                    first = false;
                }
                if let Some(frame) = frame
                    && (waiting || notify.elapsed() >= Duration::from_millis(100))
                {
                    waiting = false;
                    seek_target = None;
                    notify = Instant::now();
                    listener(
                        id,
                        Notice::Frame {
                            position: Duration::from_secs_f64(frame.max(0.)),
                            duration: Duration::from_secs_f64(
                                unsafe { engine.media.GetDuration() }.max(0.),
                            ),
                            has_audio: unsafe { engine.media.HasAudio().as_bool() },
                        },
                    );
                }
                if !visible
                    && unsafe {
                        IsWindowVisible(windows::Win32::Foundation::HWND(target.window.0 as *mut _))
                            .as_bool()
                    }
                {
                    surface.flush()?;
                    visible = true;
                    tracing::debug!(
                        session = id.session,
                        instance = id.instance,
                        request = id.request,
                        "Recording preview child visible frame presented"
                    );
                    listener(id, Notice::Visible);
                }
            }
            if waiting && Instant::now() > deadline {
                return Err(windows::core::Error::from_hresult(
                    windows::core::HRESULT::from_win32(windows::Win32::Foundation::ERROR_TIMEOUT.0),
                ));
            }
            let paused = unsafe { engine.media.IsPaused().as_bool() };
            if !ended_notified && unsafe { engine.media.IsEnded().as_bool() } {
                ended_notified = true;
                listener(id, Notice::Ended);
            }
            if paused && !waiting {
                let p = shared.pending.lock().unwrap();
                if p.commands.is_empty() && !p.stop && p.bounds.is_none() {
                    drop(
                        shared
                            .wake
                            .wait_timeout(p, Duration::from_millis(100))
                            .unwrap(),
                    );
                }
            } else if frame.is_none() {
                surface.flush()?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(session=id.session,instance=id.instance,request=id.request,code=?error.code(),"Recording preview native failed");
        listener(id, Notice::Failed("Recording preview unavailable".into()));
        let mut p = shared.pending.lock().unwrap();
        while !p.stop {
            p = shared.wake.wait(p).unwrap();
        }
    }
    let close = shared.pending.lock().unwrap().close;
    targets().lock().unwrap().remove(&target.window);
    drop(file);
    if let Some(id) = close {
        tracing::debug!(
            session = id.session,
            instance = id.instance,
            request = id.request,
            "Recording preview resources retired"
        );
        listener(id, Notice::Retired);
    }
}
