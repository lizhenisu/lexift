//! One MTA worker for asynchronous point-scoped screenshot candidate queries.
use lexift_core::{
    Result,
    domain::{
        element_candidate::{ElementCandidate, ElementQuery, ElementQueryResult},
        window_candidate::WindowCandidate,
    },
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::Dwm::*,
        System::{Com::*, Ole::*},
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::Interface,
};
const BUDGET: Duration = Duration::from_millis(80);
type Completed = Box<dyn FnOnce(ElementQueryResult) + Send>;
struct Job {
    query: ElementQuery,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    retired: Arc<AtomicBool>,
    completed: Completed,
}
impl Job {
    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self.retired.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
    }
}
#[derive(Default)]
struct State {
    pending: Option<Job>,
    active: Option<(u64, Arc<AtomicBool>)>,
    stop: bool,
}
struct Worker {
    state: Mutex<State>,
    wake: Condvar,
}
static WORKER: OnceLock<Option<Arc<Worker>>> = OnceLock::new();
pub(super) fn initialize() {
    let _ = service();
}
fn service() -> Option<Arc<Worker>> {
    WORKER
        .get_or_init(|| {
            let w = Arc::new(Worker {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            });
            let owned = w.clone();
            std::thread::Builder::new()
                .name("screenshot-elements".into())
                .spawn(move || run(owned))
                .ok()?;
            Some(w)
        })
        .clone()
}
pub(super) fn query(
    query: ElementQuery,
    cancelled: Arc<AtomicBool>,
    completed: Completed,
) -> Result<()> {
    let Some(w) = service() else {
        completed(ElementQueryResult {
            query,
            elements: vec![],
            leaf: false,
        });
        return Ok(());
    };
    let job = Job {
        query,
        deadline: Instant::now() + BUDGET,
        cancelled,
        retired: Arc::new(AtomicBool::new(false)),
        completed,
    };
    enqueue(&w, job);
    Ok(())
}
fn enqueue(w: &Worker, job: Job) {
    let mut state = w.state.lock().unwrap_or_else(|p| p.into_inner());
    if state.stop {
        return;
    }
    if let Some((_, active)) = &state.active {
        active.store(true, Ordering::Release)
    }
    state.pending = Some(job);
    w.wake.notify_one();
}
pub(super) fn cancel(session: u64) {
    if let Some(Some(w)) = WORKER.get() {
        let mut state = w.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((id, active)) = &state.active
            && *id == session
        {
            active.store(true, Ordering::Release)
        }
        if state
            .pending
            .as_ref()
            .is_some_and(|j| j.query.session == session)
        {
            state.pending = None;
        }
    }
}
pub(crate) fn shutdown() {
    if let Some(Some(w)) = WORKER.get() {
        let mut s = w.state.lock().unwrap_or_else(|p| p.into_inner());
        s.stop = true;
        if let Some((_, a)) = &s.active {
            a.store(true, Ordering::Release)
        }
        s.pending = None;
        w.wake.notify_one();
    }
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
fn client() -> windows::core::Result<(Apartment, IUIAutomation)> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let apartment = Apartment;
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
        let bounded: IUIAutomation2 = automation.cast()?;
        bounded.SetConnectionTimeout(50)?;
        bounded.SetTransactionTimeout(50)?;
        Ok((apartment, automation))
    }
}
fn window_pid(w: &WindowCandidate) -> Option<u32> {
    let hwnd = HWND(w.window.0 as *mut _);
    let mut pid = 0;
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let mut bounds = windows::Win32::Foundation::RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut bounds as *mut windows::Win32::Foundation::RECT).cast(),
            std::mem::size_of_val(&bounds) as u32,
        )
        .ok()?;
        (pid != 0
            && (w.process_id == 0 || pid == w.process_id)
            && (bounds.left, bounds.top, bounds.right, bounds.bottom)
                == (w.bounds.left, w.bounds.top, w.bounds.right, w.bounds.bottom))
            .then_some(pid)
    }
}
fn identity(element: &IUIAutomationElement, pid: u32) -> Option<Vec<i32>> {
    use windows::Win32::System::Variant::{VARIANT, VT_ARRAY, VT_I4, VariantClear};
    struct Cached(VARIANT);
    impl Drop for Cached {
        fn drop(&mut self) {
            unsafe {
                let _ = VariantClear(&mut self.0);
            }
        }
    }
    unsafe {
        let cached = Cached(
            element
                .GetCachedPropertyValue(UIA_RuntimeIdPropertyId)
                .ok()?,
        );
        if cached.0.Anonymous.Anonymous.vt.0 != (VT_ARRAY.0 | VT_I4.0) {
            return None;
        }
        let array = cached.0.Anonymous.Anonymous.Anonymous.parray;
        if array.is_null() {
            return None;
        }
        if SafeArrayGetDim(array) != 1 || SafeArrayGetElemsize(array) != 4 {
            return None;
        }
        let lower = SafeArrayGetLBound(array, 1).ok()?;
        let upper = SafeArrayGetUBound(array, 1).ok()?;
        if upper < lower || i64::from(upper) - i64::from(lower) > 128 {
            return None;
        }
        let mut id = vec![pid as i32];
        for index in lower..=upper {
            let mut value = 0i32;
            SafeArrayGetElement(array, &index, (&mut value as *mut i32).cast()).ok()?;
            id.push(value);
        }
        Some(id)
    }
}

fn run(worker: Arc<Worker>) {
    unsafe {
        windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    let client = client().ok();
    let mut cooldown = HashMap::new();
    let mut identities = HashMap::new();
    let mut session = 0;
    loop {
        let job = {
            let mut state = worker.state.lock().unwrap_or_else(|p| p.into_inner());
            while state.pending.is_none() && !state.stop {
                state = worker.wake.wait(state).unwrap_or_else(|p| p.into_inner())
            }
            if state.stop {
                return;
            }
            let job = state.pending.take().unwrap();
            state.active = Some((job.query.session, job.retired.clone()));
            job
        };
        if session != job.query.session {
            session = job.query.session;
            cooldown.clear();
            identities.clear();
        }
        let key = job.query.window.window;
        let started = Instant::now();
        let mut elements = vec![];
        let mut leaf = false;
        if !job.stopped()
            && !cooldown.get(&key).is_some_and(|until| started < *until)
            && let Some((_, automation)) = &client
            && let Some(pid) = window_pid(&job.query.window)
        {
            let original = *identities.entry(key).or_insert(pid);
            if original == pid {
                (elements, leaf) = collect(automation, &job);
            }
            if started.elapsed() >= BUDGET {
                cooldown.insert(key, Instant::now() + Duration::from_secs(1));
            }
        }
        // Never call an external completion under the worker state lock.
        if !job.cancelled.load(Ordering::Acquire) && !job.retired.load(Ordering::Acquire) {
            (job.completed)(ElementQueryResult {
                query: job.query,
                elements,
                leaf,
            });
        }
        worker
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active = None;
    }
}
fn contains(
    r: lexift_core::domain::geometry::Rect,
    p: lexift_core::domain::geometry::Point,
) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}
fn collect(automation: &IUIAutomation, job: &Job) -> (Vec<ElementCandidate>, bool) {
    let window = &job.query.window;
    let point = job.query.point;
    let Some(pid) = window_pid(window) else {
        return (vec![], false);
    };
    let Ok(walker) = (unsafe { automation.ControlViewWalker() }) else {
        return (vec![], false);
    };
    let Ok(cache) = (unsafe { automation.CreateCacheRequest() }) else {
        return (vec![], false);
    };
    if unsafe { cache.SetTreeScope(TreeScope_Element) }.is_err()
        || unsafe { cache.AddProperty(UIA_BoundingRectanglePropertyId) }.is_err()
        || unsafe { cache.AddProperty(UIA_IsOffscreenPropertyId) }.is_err()
        || unsafe { cache.AddProperty(UIA_RuntimeIdPropertyId) }.is_err()
    {
        return (vec![], false);
    }
    let Ok(root) = (unsafe {
        automation.ElementFromHandleBuildCache(HWND(window.window.0 as *mut _), &cache)
    }) else {
        return (vec![], false);
    };
    let mut stack = vec![(root, None, 0usize)];
    let mut output = vec![];
    let mut examined = 0;
    let mut leaf = false;
    while let Some((element, parent, depth)) = stack.pop() {
        if job.stopped() || examined >= 128 {
            leaf = false;
            break;
        }
        examined += 1;
        if depth > 0
            && let Ok(sibling) = unsafe { walker.GetNextSiblingElementBuildCache(&element, &cache) }
        {
            stack.push((sibling, parent.clone(), depth));
        }
        if unsafe { element.CachedIsOffscreen() }.map_or(true, |v| v.as_bool()) {
            continue;
        }
        let Some(id) = identity(&element, pid) else {
            continue;
        };
        let Ok(r) = (unsafe { element.CachedBoundingRectangle() }) else {
            continue;
        };
        let bounds = lexift_core::domain::geometry::Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        if !contains(bounds, point) || bounds.right <= bounds.left || bounds.bottom <= bounds.top {
            continue;
        }
        output.push(ElementCandidate {
            window: window.window,
            id: id.clone(),
            parent,
            bounds,
            order: examined,
        });
        if depth < 16 {
            match unsafe { walker.GetFirstChildElementBuildCache(&element, &cache) } {
                Ok(child) => {
                    leaf = false;
                    stack.push((child, Some(id), depth + 1));
                }
                Err(_) => {
                    leaf = true;
                }
            }
        } else {
            leaf = false;
        }
    }
    if job.stopped() || window_pid(window) != Some(pid) {
        return (vec![], false);
    }
    (output, leaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::geometry::{Point, Rect},
        ports::magnifier::WindowToken,
    };
    #[test]
    fn a_blocked_provider_does_not_block_submit_and_only_latest_pending_survives() {
        let retired = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            state: Mutex::new(State {
                pending: None,
                active: Some((1, retired.clone())),
                stop: false,
            }),
            wake: Condvar::new(),
        };
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocked = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for request in 1..=32 {
            let count = callbacks.clone();
            enqueue(
                &worker,
                Job {
                    query: ElementQuery {
                        session: 1,
                        request,
                        host: 2,
                        point: Point { x: 1, y: 1 },
                        window: WindowCandidate {
                            process_id: 0,
                            window: WindowToken(3),
                            bounds: Rect {
                                left: 0,
                                top: 0,
                                right: 10,
                                bottom: 10,
                            },
                            order: 0,
                        },
                    },
                    deadline: Instant::now() + BUDGET,
                    cancelled: Arc::new(AtomicBool::new(false)),
                    retired: Arc::new(AtomicBool::new(false)),
                    completed: Box::new(move |_| {
                        count.fetch_add(1, Ordering::Relaxed);
                    }),
                },
            );
        }
        assert!(retired.load(Ordering::Acquire));
        let latest = worker.state.lock().unwrap().pending.take().unwrap();
        assert_eq!(latest.query.request, 32);
        assert_eq!(callbacks.load(Ordering::Relaxed), 0);
        release_tx.send(()).unwrap();
        blocked.join().unwrap();
        (latest.completed)(ElementQueryResult {
            query: latest.query,
            elements: vec![],
            leaf: false,
        });
        assert_eq!(callbacks.load(Ordering::Relaxed), 1);
    }
}
