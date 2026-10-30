//! Bounded, latest-only source ownership with independently scheduled control commands.
use super::{
    capture::{Capture, qpc_100ns},
    encoder::{Encoder, Media, nv12},
    fixture::{Scene, marker},
};
use lexift_core::domain::{geometry::Rect, recording_clock::Clock};
use std::io::Write;
use std::{
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows::Win32::Foundation::E_FAIL;
use windows::core::{Error, Result};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

struct Image {
    data: Vec<u8>,
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
    time: i64,
    read_ms: f64,
    received: Instant,
    epoch: u64,
}
impl Drop for Image {
    fn drop(&mut self) {
        self.pool
            .lock()
            .unwrap()
            .push(std::mem::take(&mut self.data));
    }
}
#[derive(Default)]
struct Shared {
    frame: Mutex<Option<Arc<Image>>>,
    arrived: Condvar,
    error: Mutex<Option<String>>,
    stop: AtomicBool,
    paused: AtomicBool,
    epoch: AtomicU64,
    fresh: AtomicI64,
    peak_frames: AtomicU64,
    captured: AtomicU64,
    gpu_bytes: AtomicU64,
}
struct Producer {
    shared: Arc<Shared>,
    join: Option<thread::JoinHandle<()>>,
}
impl Drop for Producer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.join.take().unwrap().join().unwrap();
        self.shared.frame.lock().unwrap().take();
    }
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(default)]
pub struct Spec {
    pub fault: Option<super::fault::Fault>,
    pub dynamic_ms: u64,
    pub static_ms: u64,
    pub pause_ms: u64,
    pub resume_ms: u64,
    pub fps: u32,
    pub hardware: bool,
    pub native_controls: bool,
    pub cursor_path: bool,
    pub cancel_ms: Option<u64>,
    pub stall_ms: u64,
}
impl Default for Spec {
    fn default() -> Self {
        Self {
            fault: None,
            dynamic_ms: 6000,
            static_ms: 6000,
            pause_ms: 2000,
            resume_ms: 2000,
            fps: 30,
            hardware: false,
            native_controls: false,
            cursor_path: false,
            cancel_ms: None,
            stall_ms: 0,
        }
    }
}
#[derive(Clone, Copy)]
enum Action {
    Pause,
    Resume,
    Stop,
    Cancel,
}
struct Command {
    id: u64,
    action: Action,
    at: Instant,
}
fn err(text: impl Into<String>) -> Error {
    Error::new(E_FAIL, text.into())
}

pub fn run(path: &Path, roi: Rect, scene: &Scene, spec: Spec) -> Result<serde_json::Value> {
    if ACTIVE.swap(true, Ordering::AcqRel) {
        return Err(err("Previous recording resources have not retired"));
    }
    let _admission = Admission;
    if path.exists() {
        return Err(err("Recording evidence already exists"));
    }
    let temp = path.with_extension("partial.mp4");
    if temp.exists() {
        return Err(err("Recording temporary evidence already exists"));
    }
    let actual_temp = if matches!(spec.fault, Some(super::fault::Fault::Create)) {
        temp.join("missing-parent.mp4")
    } else {
        temp.clone()
    };
    super::encoder::take_stages();
    let result = execute(&actual_temp, roi, scene, spec);
    let stages = super::encoder::take_stages();
    let returned = Instant::now();
    match result {
        Ok(mut report) if report["cancelled"] == false => {
            report["completion_stages"] = serde_json::json!(stages);
            let rename_fault = matches!(spec.fault, Some(super::fault::Fault::Rename));
            if rename_fault {
                std::fs::create_dir(path).map_err(|e| err(e.to_string()))?;
            }
            if let Err(e) = std::fs::rename(&temp, path) {
                if rename_fault {
                    let _ = std::fs::remove_dir(path);
                }
                let _ = std::fs::remove_file(&temp);
                return Err(err(format!("rename: {e}")));
            }
            report["stop_to_ready_ms"] = serde_json::json!(
                report["stop_to_closed_ms"]
                    .as_f64()
                    .map(|elapsed| elapsed + returned.elapsed().as_secs_f64() * 1000.)
            );
            Ok(report)
        }
        other => {
            if temp.exists() {
                std::fs::remove_file(&temp).map_err(|e| err(e.to_string()))?;
            }
            other.map_err(|e| err(format!("{e}; native_stages={}", serde_json::json!(stages))))
        }
    }
}

fn execute(path: &Path, roi: Rect, scene: &Scene, spec: Spec) -> Result<serde_json::Value> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let requested = Instant::now();
    if Clock::new(spec.fps, Duration::ZERO).is_none() {
        return Err(err("Invalid recording frame rate"));
    }
    let width = usize::try_from(i64::from(roi.right) - i64::from(roi.left))
        .map_err(|e| err(e.to_string()))?;
    let height = usize::try_from(i64::from(roi.bottom) - i64::from(roi.top))
        .map_err(|e| err(e.to_string()))?;
    if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(err("Recording dimensions must be positive and even"));
    }
    let bytes = width
        .checked_mul(height)
        .and_then(|p| p.checked_mul(4))
        .filter(|n| *n > 0 && n.is_multiple_of(8))
        .ok_or_else(|| err("Invalid recording extent"))?;
    let controlled = bytes
        .checked_mul(3)
        .and_then(|n| n.checked_add(width * height * 3))
        .ok_or_else(|| err("Recording budget overflow"))?;
    if controlled > 64 * 1024 * 1024 {
        return Err(err("Recording gate pixel budget exceeded"));
    }
    let shared = Arc::new(Shared::default());
    let state = shared.clone();
    let pool = Arc::new(Mutex::new(vec![
        vec![0; bytes],
        vec![0; bytes],
        vec![0; bytes],
    ]));
    let producer_pool = pool.clone();
    let join = thread::spawn(move || {
        let outcome = (|| -> Result<()> {
            let (wake, arrivals) = std::sync::mpsc::sync_channel(1);
            let notice = super::super::screen_capture::arrival::Notice::new(move || {
                let _ = wake.try_send(());
            });
            let mut capture =
                Capture::new(roi, notice).map_err(|e| err(format!("Capture preparation: {e}")))?;
            state
                .gpu_bytes
                .store(capture.texture_bytes(), Ordering::Release);
            while !state.stop.load(Ordering::Acquire) {
                let _ = arrivals.recv_timeout(Duration::from_millis(30));
                if state.paused.load(Ordering::Acquire) {
                    continue;
                }
                let Some(mut data) = producer_pool.lock().unwrap().pop() else {
                    continue;
                };
                if state.captured.load(Ordering::Relaxed) >= 20 {
                    match spec.fault {
                        Some(super::fault::Fault::DeviceLost) => {
                            return Err(err("injected_device_lost"));
                        }
                        Some(super::fault::Fault::CaptureClosed) => {
                            capture.close()?;
                            return Err(err("capture_closed_native"));
                        }
                        _ => {}
                    }
                }
                let epoch = state.epoch.load(Ordering::Acquire);
                match capture.read(&mut data, state.fresh.load(Ordering::Acquire)) {
                    Ok(Some(timing)) => {
                        let image = Arc::new(Image {
                            data,
                            pool: producer_pool.clone(),
                            time: timing.source_100ns,
                            read_ms: timing.read_ms,
                            received: timing.received,
                            epoch,
                        });
                        if !state.paused.load(Ordering::Acquire)
                            && epoch == state.epoch.load(Ordering::Acquire)
                        {
                            *state.frame.lock().unwrap() = Some(image);
                            state.arrived.notify_one();
                            state.captured.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(None) => producer_pool.lock().unwrap().push(data),
                    Err(e) => {
                        producer_pool.lock().unwrap().push(data);
                        return Err(err(format!("Capture readback: {e}")));
                    }
                }
                let in_use = 3 - producer_pool.lock().unwrap().len();
                state
                    .peak_frames
                    .fetch_max(in_use as u64, Ordering::Relaxed);
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            *state.error.lock().unwrap() = Some(e.to_string());
        }
    });
    let producer = Producer {
        shared: shared.clone(),
        join: Some(join),
    };
    let apartment = super::super::screen_capture::capture::Apartment::new()?;
    let media = Media::new()?;
    let encoder = Encoder::new(
        path,
        width as u32,
        height as u32,
        spec.fps,
        spec.hardware,
        spec.fault,
    )?;
    let encoder_identity = encoder.identity.clone();
    if spec.cancel_ms == Some(0) {
        drop(encoder);
        drop(producer);
        assert_eq!(pool.lock().unwrap().len(), 3);
        return Ok(
            serde_json::json!({"cancelled":true,"stage":"before_first_acceptance","task":id,"returned_source_buffers":3}),
        );
    }
    let mut last = loop {
        if let Some(e) = shared.error.lock().unwrap().take() {
            return Err(err(e));
        }
        if let Some(frame) = shared.frame.lock().unwrap().clone() {
            break frame;
        }
        if requested.elapsed() > Duration::from_secs(5) {
            return Err(err("Fresh recording first frame timed out"));
        }
        thread::sleep(Duration::from_millis(1));
    };
    let first_ms = requested.elapsed().as_secs_f64() * 1000.;
    let origin = Instant::now();
    let clock_origin = Duration::ZERO;
    let mut clock = Clock::new(spec.fps, clock_origin).ok_or_else(|| err("Invalid fps"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = cancel.clone();
    let control_state = shared.clone();
    let dynamic = scene.dynamic.clone();
    let user_action = scene.action.clone();
    let control_hwnd = scene.windows["control"].as_u64().unwrap() as usize;
    dynamic.store(true, Ordering::Release);
    let (tx, rx) = std::sync::mpsc::channel();
    let control = thread::spawn(move || {
        let _lease = super::resources::Lease::new(&super::resources::CONTROL);
        let mut actions = vec![
            (0, id.wrapping_sub(1), Action::Stop),
            (spec.dynamic_ms + spec.static_ms, id, Action::Pause),
            (
                spec.dynamic_ms + spec.static_ms + spec.pause_ms,
                id,
                Action::Resume,
            ),
            (
                spec.dynamic_ms + spec.static_ms + spec.pause_ms + spec.resume_ms,
                id,
                Action::Stop,
            ),
        ];
        if let Some(at) = spec.cancel_ms {
            actions.push((at, id, Action::Cancel));
        }
        actions.sort_by_key(|x| x.0);
        let mut pointer = spec.cursor_path.then(super::fixture::Pointer::new);
        let mut next = 0;
        while (next < actions.len() || spec.native_controls)
            && !control_state.stop.load(Ordering::Acquire)
        {
            let requested = user_action.swap(0, Ordering::AcqRel);
            let manual = match requested {
                1 => Some(Action::Pause),
                2 => Some(Action::Resume),
                3 => Some(Action::Stop),
                4 => Some(Action::Cancel),
                _ => None,
            };
            if let Some(action) = manual {
                if matches!(action, Action::Cancel) {
                    cancelled.store(true, Ordering::Release);
                }
                if tx
                    .send(Command {
                        id,
                        action,
                        at: Instant::now(),
                    })
                    .is_err()
                {
                    break;
                }
            }
            let elapsed = origin.elapsed().as_millis();
            if let Some(pointer) = &mut pointer {
                pointer.tick(roi, elapsed as u64);
            }
            dynamic.store(
                elapsed < u128::from(spec.dynamic_ms)
                    || elapsed >= u128::from(spec.dynamic_ms + spec.static_ms + spec.pause_ms),
                Ordering::Release,
            );
            if next >= actions.len() {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let (at, request_id, action) = actions[next];
            if origin.elapsed().as_millis() >= u128::from(at) {
                if matches!(action, Action::Resume) {
                    dynamic.store(true, Ordering::Release);
                }
                if matches!(action, Action::Cancel) && !spec.native_controls {
                    cancelled.store(true, Ordering::Release);
                }
                if spec.native_controls && request_id == id {
                    let button = match action {
                        Action::Pause => 1,
                        Action::Resume => 2,
                        Action::Stop => 3,
                        Action::Cancel => 4,
                    };
                    if let Err(e) = super::fixture::click_control(control_hwnd, button) {
                        *control_state.error.lock().unwrap() = Some(e.to_string());
                        break;
                    }
                    next += 1;
                    continue;
                }
                if tx
                    .send(Command {
                        id: request_id,
                        action,
                        at: Instant::now(),
                    })
                    .is_err()
                {
                    break;
                }
                next += 1;
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
    });
    let mut index = 0;
    let mut duplicate = 0;
    let mut old_marker = u32::MAX;
    let mut changed = 0;
    let mut pixels = vec![0; width * height * 3 / 2];
    let samples_path = path.with_extension("samples.jsonl");
    let mut samples = std::io::BufWriter::new(
        std::fs::File::create(&samples_path).map_err(|e| err(e.to_string()))?,
    );
    let mut last_stage = None;
    let mut paused = false;
    let mut stopped = false;
    let mut stop_at = None;
    let mut resume_deadline = None;
    let outcome = (|| -> Result<()> {
        nv12(&last.data, width, height, &mut pixels);
        loop {
            if let Some(e) = shared.error.lock().unwrap().take() {
                return Err(err(e));
            }
            if cancel.load(Ordering::Acquire) {
                break;
            }
            for cmd in rx.try_iter() {
                if cmd.id != id {
                    continue;
                }
                let now = cmd.at.duration_since(origin);
                match cmd.action {
                    Action::Pause if clock.pause(now) => {
                        paused = true;
                        shared.paused.store(true, Ordering::Release);
                    }
                    Action::Resume if clock.resume(now) => {
                        shared.frame.lock().unwrap().take();
                        shared.fresh.store(
                            if matches!(spec.fault, Some(super::fault::Fault::ResumeStale)) {
                                i64::MAX
                            } else {
                                qpc_100ns()?
                            },
                            Ordering::Release,
                        );
                        shared.epoch.fetch_add(1, Ordering::AcqRel);
                        shared.paused.store(false, Ordering::Release);
                        paused = false;
                        resume_deadline = Some(Instant::now() + Duration::from_secs(5));
                    }
                    Action::Stop => {
                        clock.stop(now);
                        stopped = true;
                        paused = false;
                        stop_at = Some(cmd.at);
                        shared.stop.store(true, Ordering::Release);
                    }
                    _ => {}
                }
            }
            if paused {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let epoch = shared.epoch.load(Ordering::Acquire);
            if last.epoch != epoch {
                match shared
                    .frame
                    .lock()
                    .unwrap()
                    .clone()
                    .filter(|f| f.epoch == epoch)
                {
                    Some(f) => {
                        last = f;
                        resume_deadline = None;
                    }
                    None => {
                        if resume_deadline.is_some_and(|at| Instant::now() >= at) {
                            return Err(err("Fresh resume frame timed out"));
                        }
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                }
                nv12(&last.data, width, height, &mut pixels);
            }
            let now = origin.elapsed();
            if clock.backlog_exceeded(now, index) {
                return Err(err(format!(
                    "Encoding backlog exceeded one second: submitted={index}, due={}, last_stage={:?}",
                    clock.frames(now),
                    last_stage.as_ref()
                )));
            }
            let due = clock.frames(now);
            if index >= due {
                if stopped {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let mut convert_ms = 0.;
            // WGC and output cadence have different phases. Give an unchanged source
            // a bounded half-frame arrival window rather than baking scheduler jitter
            // into duplicate output. Static content still uses the same pixels.
            if !stopped {
                let _arrival = shared
                    .arrived
                    .wait_timeout_while(
                        shared.frame.lock().unwrap(),
                        Duration::from_nanos(500_000_000 / u64::from(spec.fps)),
                        |frame| {
                            !cancel.load(Ordering::Acquire)
                                && !frame
                                    .as_ref()
                                    .is_some_and(|f| f.epoch == epoch && !Arc::ptr_eq(f, &last))
                        },
                    )
                    .unwrap();
            }
            if let Some(f) = shared.frame.lock().unwrap().clone()
                && f.epoch == epoch
                && !Arc::ptr_eq(&f, &last)
            {
                let at = Instant::now();
                if f.data != last.data {
                    nv12(&f.data, width, height, &mut pixels);
                }
                convert_ms = at.elapsed().as_secs_f64() * 1000.;
                last = f;
            }
            let counter = marker(&last.data, width);
            if counter == old_marker {
                duplicate += 1
            } else {
                changed += 1;
                old_marker = counter;
            }
            let (pts, duration) = clock
                .sample(index)
                .ok_or_else(|| err("Recording timestamp overflow"))?;
            if spec.stall_ms > 0 {
                thread::sleep(Duration::from_millis(spec.stall_ms));
            }
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let write_ms = encoder.write(&pixels, pts, duration)?;
            let row = serde_json::json!({"index":index,"pts_100ns":pts,"duration_100ns":duration,"source_100ns":last.time,"source_epoch":last.epoch,"marker":counter,"at_ms":now.as_secs_f64()*1000.,"read_ms":last.read_ms,"convert_ms":convert_ms,"write_ms":write_ms,"source_age_ms":last.received.elapsed().as_secs_f64()*1000.,"backlog":due-index});
            writeln!(samples, "{row}").map_err(|e| err(e.to_string()))?;
            last_stage = Some(row);
            index += 1;
        }
        Ok(())
    })();
    shared.stop.store(true, Ordering::Release);
    control.join().unwrap();
    samples.flush().map_err(|e| err(e.to_string()))?;
    let before_finalize_cancelled = cancel.load(Ordering::Acquire);
    // Exercise cancellation at the irreversible native handoff: complete the
    // native operation, but never publish its now-invalid output.
    let finalize = if !before_finalize_cancelled && outcome.is_ok() {
        if matches!(spec.fault, Some(super::fault::Fault::CancelFinalize)) {
            cancel.store(true, Ordering::Release);
        }
        encoder.finish()
    } else {
        drop(encoder);
        Ok(0.)
    };
    drop(last);
    drop(producer);
    let returned_buffers = pool.lock().unwrap().len();
    drop(media);
    drop(apartment);
    let was_cancelled = cancel.load(Ordering::Acquire);
    outcome.map_err(|e|err(format!("{e}; submitted={index}; returned_source_buffers={returned_buffers}; peak_source_buffers={}; controlled_pixel_bytes={controlled}",shared.peak_frames.load(Ordering::Relaxed))))?;
    if returned_buffers != 3 {
        return Err(err("Recording pixel ownership did not retire"));
    }
    let finalize =
        finalize.map_err(|e| err(format!("{e}; returned_source_buffers={returned_buffers}")))?;
    Ok(
        serde_json::json!({"task":id,"cancelled":was_cancelled,"first_frame_ms":first_ms,"active_ms":clock.active(origin.elapsed()).as_secs_f64()*1000.,"frames":index,"duplicates":duplicate,"marker_changes":changed,"fps":spec.fps,"cursor_path":spec.cursor_path,"controlled_pixel_bytes":controlled,"owned_gpu_texture_bytes_estimate":shared.gpu_bytes.load(Ordering::Acquire),"encoder_internal_bytes":null,"max_source_buffers":shared.peak_frames.load(Ordering::Relaxed),"returned_source_buffers":returned_buffers,"capture_publications":shared.captured.load(Ordering::Relaxed),"finalize_ms":finalize,"stop_to_closed_ms":stop_at.map(|t|t.elapsed().as_secs_f64()*1000.),"encoder":encoder_identity,"samples_file":samples_path.file_name().unwrap().to_string_lossy()}),
    )
}
