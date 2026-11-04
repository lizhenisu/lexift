use super::{engine::Engine, surface::Surface};
use lexift_core::{
    Error, Result,
    domain::recording_trim::Range,
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
use windows::{
    Win32::{Media::MediaFoundation::IMFMediaEngineEx, System::Com::*, UI::WindowsAndMessaging::*},
    core::Interface,
};
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
/// A retiring producer cannot unregister a replacement using the same HWND.
fn unregister(window: WindowToken, owner: &Arc<Shared>) {
    let mut targets = targets().lock().unwrap();
    if targets
        .get(&window)
        .is_some_and(|registered| registered.ptr_eq(&Arc::downgrade(owner)))
    {
        targets.remove(&window);
    }
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
        initial_range: Option<(Range, u32)>,
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
            .spawn(move || run(id, target, file, last_frame, initial_range, listener, s))
            .map_err(|_| {
                unregister(target.window, &shared);
                Error::new("Recording preview unavailable")
            })?;
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
    initial_range: Option<(Range, u32)>,
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
        #[cfg(test)]
        if std::env::var_os("LEXIFT_TRIM_PREVIEW_PROBE").is_some() {
            surface.test_probe = Some((500, 66));
        }
        let mut range = initial_range;
        let mut initial_seek = range.is_some_and(|(r, _)| r.start > 0);
        let mut ending = false;
        let mut presented_pts: Option<f64> = None;
        let mut dirty = false;
        let mut first = true;
        let mut visible = false;
        let mut ended_notified = false;
        let mut waiting = true;
        let mut seek_target: Option<f64> = None;
        let mut seek_steps = 0;
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
                            if let Some((r, fps)) = range {
                                engine.marker_reached = false;
                                engine
                                    .media
                                    .cast::<IMFMediaEngineEx>()?
                                    .SetTimelineMarkerTimer(
                                        Range::time(r.end, fps).as_secs_f64(),
                                    )?;
                            }
                            engine.media.Play()?;
                        }
                        Action::Pause => {
                            engine.media.Pause()?;
                            listener(id, Notice::Paused);
                        }
                        Action::SetRange {
                            range: r,
                            fps,
                            version: _,
                            position,
                        } => {
                            engine.media.Pause()?;
                            engine
                                .media
                                .cast::<IMFMediaEngineEx>()?
                                .CancelTimelineMarkerTimer()?;
                            range = Some((r, fps));
                            response = Some(Instant::now());
                            ended_notified = false;
                            engine.marker_reached = false;
                            ending = false;
                            engine
                                .media
                                .cast::<IMFMediaEngineEx>()?
                                .CancelTimelineMarkerTimer()?;
                            engine.seek(position.as_secs_f64())?;
                            seek_target = Some(position.as_secs_f64());
                            seek_steps = 0;
                            waiting = true;
                            deadline = Instant::now() + Duration::from_secs(5);
                        }
                        Action::Seek(position) => {
                            // AAC padding can extend container duration beyond
                            // the last video frame. Never seek into that tail.
                            let position = range.map_or(position.min(last_frame), |(r, fps)| {
                                r.clamp_time(position, fps)
                            });
                            engine.marker_reached = false;
                            ending = false;
                            engine
                                .media
                                .cast::<IMFMediaEngineEx>()?
                                .CancelTimelineMarkerTimer()?;
                            response = Some(Instant::now());
                            ended_notified = false;
                            engine.seek(position.as_secs_f64())?;
                            seek_target = Some(position.as_secs_f64());
                            seek_steps = 0;
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
            unsafe {
                if waiting
                    && engine.seeked
                    && engine.frame_step_done
                    && seek_target
                        .zip(presented_pts)
                        .is_some_and(|(target, shown)| {
                            (target - shown).abs()
                                < range.map_or(0.000_001, |(_, fps)| 0.5 / f64::from(fps))
                        })
                {
                    // A retained complete frame is also a valid result of a
                    // seek to that same frame; MF need not emit a new tick.
                    surface.present(&engine)?;
                    waiting = false;
                    seek_target = None;
                    listener(
                        id,
                        Notice::Frame {
                            position: Duration::from_secs_f64(presented_pts.unwrap().max(0.)),
                            duration: Duration::from_secs_f64(engine.media.GetDuration().max(0.)),
                            has_audio: engine.media.HasAudio().as_bool(),
                        },
                    );
                }
            }
            let decoded_frame = engine.tick()?;
            #[cfg(test)]
            if std::env::var_os("LEXIFT_TRIM_PREVIEW_TRACE").is_some()
                && (decoded_frame.is_some() || engine.marker_reached)
            {
                eprintln!(
                    "TRIM_TICK request={} pts={decoded_frame:?} target={seek_target:?} waiting={waiting} marker={} current={} ended={}",
                    id.request,
                    engine.marker_reached,
                    unsafe { engine.media.GetCurrentTime() },
                    unsafe { engine.media.IsEnded().as_bool() }
                );
            }
            if initial_seek && decoded_frame.is_some() {
                let (r, fps) = range.unwrap();
                let time = Range::time(r.start, fps).as_secs_f64();
                engine.seek(time)?;
                initial_seek = false;
                seek_target = Some(time);
                seek_steps = 0;
                deadline = Instant::now() + Duration::from_secs(5);
                continue;
            }
            if let (Some(target), Some(pts), Some((_, fps))) = (seek_target, decoded_frame, range)
                && engine.seeked
                && seek_steps < 8
                && engine.frame_step_done
                && unsafe { !engine.media.IsSeeking().as_bool() }
                && (pts - target).abs() >= 0.5 / f64::from(fps)
                && (pts - target).abs() < 8.5 / f64::from(fps)
            {
                // A paused MF seek can settle on the adjacent frame, notably
                // frame one when replaying from EOF. Correct using bounded frame steps
                // asynchronously; keep validating the actual presented PTS.
                unsafe {
                    engine
                        .media
                        .cast::<IMFMediaEngineEx>()?
                        .FrameStep(pts < target)?;
                }
                engine.frame_step_done = false;
                seek_steps += 1;
                continue;
            }
            let mut frame = decoded_frame.filter(|p| {
                seek_target.is_none_or(|t| {
                    engine.seeked
                        && engine.frame_step_done
                        && (t - *p).abs() < range.map_or(0.1, |(_, fps)| 0.5 / f64::from(fps))
                })
            });
            if let Some((r, fps)) = range {
                let start = Range::time(r.start, fps).as_secs_f64();
                let end = Range::time(r.end, fps).as_secs_f64();
                if !ended_notified
                    && !ending
                    && (!waiting
                        && engine.marker_reached
                        && unsafe { engine.media.GetCurrentTime() } >= end - 0.000_001
                        || frame.is_some_and(|p| p >= end - 0.000_001)
                        || !waiting && unsafe { engine.media.GetCurrentTime() } >= end - 0.000_001)
                {
                    unsafe {
                        engine.media.Pause()?;
                        engine
                            .media
                            .cast::<IMFMediaEngineEx>()?
                            .CancelTimelineMarkerTimer()?;
                    }
                    engine.marker_reached = false;
                    frame = None;
                    let last = Range::time(r.end - 1, fps).as_secs_f64();
                    if presented_pts.is_some_and(|p| (p - last).abs() < 0.5 / f64::from(fps)) {
                        // A paused seek to the already displayed frame may not
                        // produce another tick. Keep that complete final frame.
                        ending = false;
                        waiting = false;
                        seek_target = None;
                        ended_notified = true;
                        listener(id, Notice::Ended);
                    } else {
                        engine.seek(last)?;
                        ending = true;
                        waiting = true;
                        seek_target = Some(last);
                        seek_steps = 0;
                        deadline = Instant::now() + Duration::from_secs(5);
                    }
                }
                frame = frame.filter(|p| *p >= start - 0.000_001 && *p < end - 0.000_001);
            }
            if frame.is_some() || dirty && engine.texture.is_some() {
                if frame.is_some() {
                    engine.accept_frame()?;
                }
                surface.present(&engine)?;
                if frame.is_some() {
                    presented_pts = frame;
                }
                #[cfg(test)]
                if std::env::var_os("LEXIFT_TRIM_PREVIEW_PROBE").is_some()
                    && let Some(pts) = frame
                {
                    eprintln!(
                        "TRIM_PRESENT request={} pts={pts} pixel={:?}",
                        id.request, surface.test_pixel
                    );
                }
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
            if ending && !waiting {
                ending = false;
                ended_notified = true;
                listener(id, Notice::Ended);
            }
            if waiting && Instant::now() > deadline {
                return Err(windows::core::Error::from_hresult(
                    windows::core::HRESULT::from_win32(windows::Win32::Foundation::ERROR_TIMEOUT.0),
                ));
            }
            let paused = unsafe { engine.media.IsPaused().as_bool() };
            if !waiting && !ended_notified && unsafe { engine.media.IsEnded().as_bool() } {
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
        #[cfg(test)]
        eprintln!(
            "TRIM_NATIVE_ERROR request={} code={:?}",
            id.request,
            error.code()
        );
        tracing::warn!(session=id.session,instance=id.instance,request=id.request,code=?error.code(),"Recording preview native failed");
        listener(id, Notice::Failed("Recording preview unavailable".into()));
        let mut p = shared.pending.lock().unwrap();
        while !p.stop {
            p = shared.wake.wait(p).unwrap();
        }
    }
    let close = shared.pending.lock().unwrap().close;
    unregister(target.window, &shared);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_preserves_replacement_surface_registration() {
        let shared = || {
            Arc::new(Shared {
                pending: Mutex::new(Default::default()),
                wake: Condvar::new(),
            })
        };
        let old = shared();
        let replacement = shared();
        let window = WindowToken(isize::MAX);
        targets()
            .lock()
            .unwrap()
            .insert(window, Arc::downgrade(&old));
        targets()
            .lock()
            .unwrap()
            .insert(window, Arc::downgrade(&replacement));
        unregister(window, &old);
        assert!(position(
            window,
            Rect {
                left: 0,
                top: 0,
                right: 10,
                bottom: 10
            }
        ));
        unregister(window, &replacement);
        assert!(!position(
            window,
            Rect {
                left: 0,
                top: 0,
                right: 10,
                bottom: 10
            }
        ));
    }
}
