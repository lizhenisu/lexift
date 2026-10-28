//! Shared frozen-screen rectangle host; consumers own OCR/screenshot business sessions.
use crate::screen_interaction::Host;
use crate::{
    OcrSelectionWindow, PassiveWindowPreparation, PopupPointerInput, WindowLifecycleCallbacks,
};
use lexift_core::{
    domain::{
        geometry::{Point, Rect},
        region_selection::{RegionSelection, SelectionContext},
    },
    ports::screen_capture::CapturedRegion,
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostPhase {
    Idle,
    Capturing,
}
/// Captured before input retirement; authorizes only this live selection-to-editor handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubmissionFocus {
    pub session: u64,
    pub host: u64,
    pub input_host: u64,
    pub screen: Rect,
    pub source: Option<lexift_core::ports::magnifier::WindowToken>,
    pub observed: bool,
    pub started: std::time::Instant,
}
pub(crate) enum SelectionEvent {
    /// Every host has presented complete feedback; initial activation was already attempted.
    Ready {
        generation: u64,
    },
    ReuseRegion {
        session: u64,
        host: u64,
        request: u64,
    },
    Cancelled,
    QueryElements(lexift_core::domain::element_candidate::ElementQuery),
    CancelElements {
        session: u64,
        request: u64,
    },
    Failed {
        generation: u64,
        error: String,
    },
    SelectionSubmitted {
        selection: lexift_core::domain::region_selection::RegionSelectionResult,
        focus: Option<SubmissionFocus>,
    },
}

pub(crate) fn submission_live(focus: SubmissionFocus) -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            r.generation == focus.session
                && r.screenshot
                && r.submitted
                && r.windows.iter().any(|h| {
                    h.revision() == focus.host
                        && (r.lifecycle.annotation_window_token)(h.window.window()) == focus.source
                })
                && r.windows
                    .iter()
                    .any(|h| h.revision() == focus.input_host && h.bounds == focus.screen)
        })
    })
}
thread_local! { static STATE: RefCell<Option<Registry>> = const { RefCell::new(None) }; }
struct Registry {
    activation_origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    activation_done: bool,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    handler: Option<Rc<dyn Fn(SelectionEvent)>>,
    generation: u64,
    screenshot: bool,
    submitted: bool,
    reusable: bool,
    reuse_sequence: u64,
    reuse_pending: Option<u64>,
    candidates: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
    hover: Option<Rect>,
    presented_hover: Option<Rect>,
    hover_revision: u64,
    scheduled_hover: Option<Rect>,
    hover_paints: Vec<Rect>,
    query_host: u64,
    query_sequence: u64,
    query_point: Option<Point>,
    query_pending: Option<lexift_core::domain::element_candidate::ElementQuery>,
    query_cache: Option<(std::time::Instant, Rect)>,
    elements: lexift_core::domain::element_candidate::Graph,
    navigation: lexift_core::domain::element_candidate::Navigation,
    modified: Rc<Cell<bool>>,
    help: super::screenshot_help::State,
    phase: HostPhase,
    windows: Vec<Host<OcrSelectionWindow>>,
    gesture: RegionSelection,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    native_handoff: Vec<bool>,
    presented_hosts: Vec<Rect>,
    feedback_hosts: Vec<Rect>,
    timer: slint::Timer,
    attempts: u32,
}
pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(Registry {
            activation_origin: None,
            activation_done: false,
            prepare,
            lifecycle,
            handler: None,
            generation: 0,
            screenshot: false,
            submitted: false,
            reusable: false,
            reuse_sequence: 0,
            reuse_pending: None,
            candidates: Vec::new(),
            hover: None,
            presented_hover: None,
            scheduled_hover: None,
            hover_revision: 0,
            hover_paints: vec![],
            query_host: 0,
            query_sequence: 0,
            query_point: None,
            query_pending: None,
            query_cache: None,
            elements: Default::default(),
            navigation: Default::default(),
            modified: Rc::new(Cell::new(false)),
            help: Default::default(),
            phase: HostPhase::Idle,
            windows: vec![],
            gesture: RegionSelection::default(),
            frames: None,
            native_handoff: vec![],
            presented_hosts: vec![],
            feedback_hosts: vec![],
            timer: Default::default(),
            attempts: 0,
        })
    });
}
pub(crate) fn set_handler(handler: Rc<dyn Fn(SelectionEvent)>) {
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.handler = Some(handler);
        }
    });
}
#[cfg(test)]
pub(crate) fn set_candidates(
    generation: u64,
    candidates: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
    elements: Vec<lexift_core::domain::element_candidate::ElementCandidate>,
) {
    let point = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.screenshot)?;
        r.elements = lexift_core::domain::element_candidate::Graph::new(elements, &candidates);
        r.navigation = Default::default();
        r.candidates = candidates;
        let p = (r.lifecycle.toolbar_cursor_position)()?;
        let b = r
            .windows
            .iter()
            .find(|h| {
                p.x >= h.bounds.left
                    && p.x < h.bounds.right
                    && p.y >= h.bounds.top
                    && p.y < h.bounds.bottom
            })?
            .bounds;
        Some((p, b))
    });
    if let Some((p, b)) = point {
        pointer(
            generation,
            b,
            1.,
            PopupPointerInput::Moved {
                x: (p.x - b.left) as f32,
                y: (p.y - b.top) as f32,
            },
        );
    }
}

/// Only a current point query may update a live, unpressed host.
pub(crate) fn set_elements(result: lexift_core::domain::element_candidate::ElementQueryResult) {
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.screenshot
                && !r.submitted
                && !r.gesture.active()
                && r.reuse_pending.is_none()
                && r.generation == result.query.session
                && r.query_host == result.query.host
                && r.query_pending.as_ref() == Some(&result.query)
                && r.query_point == Some(result.query.point)
        }) else {
            return;
        };
        r.query_pending = None;
        r.elements =
            lexift_core::domain::element_candidate::Graph::new(result.elements, &r.candidates);
        r.hover = candidate_at(r, result.query.point);
        r.query_cache = if result.leaf {
            r.elements
                .leaf_bounds(
                    &r.candidates,
                    result.query.window.bounds,
                    result.query.point,
                )
                .map(|b| (std::time::Instant::now(), b))
        } else {
            None
        };
        sync_region(r);
    });
}
/// Cache only confirmed leaves. Pointer work never waits on the platform worker.
fn point_query(r: &mut Registry, p: Point) -> Option<SelectionEvent> {
    if !r.screenshot || r.gesture.active() {
        return None;
    }
    r.query_point = Some(p);
    if r.query_cache
        .is_some_and(|(t, b)| t.elapsed() < Duration::from_millis(150) && inside(b, p))
    {
        return None;
    }
    if r.query_pending.as_ref().is_some_and(|q| q.point == p) {
        return None;
    }
    let retain_navigation = r.query_cache.is_some_and(|(_, b)| inside(b, p));
    let window = r
        .candidates
        .iter()
        .filter(|w| inside(w.bounds, p))
        .min_by_key(|w| w.order)
        .copied();
    r.query_sequence += 1;
    r.query_pending = window.map(
        |window| lexift_core::domain::element_candidate::ElementQuery {
            session: r.generation,
            request: r.query_sequence,
            host: r.query_host,
            point: p,
            window,
        },
    );
    if !retain_navigation {
        r.query_cache = None;
        r.elements = Default::default();
        r.navigation = Default::default();
    }
    match r.query_pending.clone() {
        Some(q) => Some(SelectionEvent::QueryElements(q)),
        None => Some(SelectionEvent::CancelElements {
            session: r.generation,
            request: r.query_sequence,
        }),
    }
}
fn inside(b: Rect, p: Point) -> bool {
    p.x >= b.left && p.x < b.right && p.y >= b.top && p.y < b.bottom
}
fn cancel_query(r: &mut Registry) -> Option<SelectionEvent> {
    if !r.screenshot {
        return None;
    }
    r.query_sequence += 1;
    r.query_pending = None;
    r.query_cache = None;
    Some(SelectionEvent::CancelElements {
        session: r.generation,
        request: r.query_sequence,
    })
}
fn hover_snapshot(generation: u64, bounds: Rect) -> Option<(u64, Rect, Option<Rect>)> {
    STATE.with(|s| {
        let slot = s.try_borrow().ok()?;
        let r = slot
            .as_ref()
            .filter(|r| r.generation == generation && !r.gesture.active())?;
        Some((r.hover_revision, bounds, r.hover))
    })
}
fn present_hover(generation: u64, snapshot: Option<(u64, Rect, Option<Rect>)>) {
    let Some((revision, bounds, hover)) = snapshot else {
        return;
    };
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation && r.hover_revision == revision && !r.gesture.active()
        }) else {
            return;
        };
        if !r.hover_paints.contains(&bounds) {
            r.hover_paints.push(bounds)
        }
        let intersects = |a: Rect, b: Rect| {
            a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
        };
        let ready = r
            .windows
            .iter()
            .filter(|h| {
                hover.is_some_and(|b| intersects(b, h.bounds))
                    || r.presented_hover.is_some_and(|b| intersects(b, h.bounds))
            })
            .all(|h| r.hover_paints.contains(&h.bounds));
        if ready {
            r.presented_hover = hover;
        }
    });
}

pub(crate) fn finish_selection(generation: u64) {
    if STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.generation == generation && r.submitted)
    }) {
        close();
    }
}

fn candidate_at(r: &mut Registry, point: Point) -> Option<Rect> {
    if !r.screenshot {
        return None;
    }
    let first = r.windows.first()?.bounds;
    let desktop = r.windows.iter().fold(first, |a, h| Rect {
        left: a.left.min(h.bounds.left),
        top: a.top.min(h.bounds.top),
        right: a.right.max(h.bounds.right),
        bottom: a.bottom.max(h.bounds.bottom),
    });
    r.navigation
        .point(&r.elements, &r.candidates, desktop, point)
}

pub(crate) fn active() -> bool {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| !r.submitted && !r.windows.is_empty())
    })
}
fn emit(event: SelectionEvent) {
    let handler = STATE.with(|s| s.borrow().as_ref().and_then(|r| r.handler.clone()));
    if let Some(h) = handler {
        h(event);
    }
}
pub(crate) fn close() {
    STATE.with(|s| {
        if let Some(r) = s
            .borrow()
            .as_ref()
            .filter(|r| r.screenshot && !r.windows.is_empty())
        {
            tracing::debug!(
                session = r.generation,
                submitted = r.submitted,
                "Screenshot selection hosts closing"
            );
        }
    });
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.timer.stop();
            r.query_host = r.query_host.wrapping_add(1);
            r.query_pending = None;
            r.query_point = None;
            r.query_cache = None;
            r.presented_hover = None;
            r.scheduled_hover = None;
            r.hover_revision = r.hover_revision.wrapping_add(1);
            r.hover_paints.clear();
            r.submitted = false;
            r.reusable = false;
            r.reuse_pending = None;
            r.gesture.cancel();
            r.candidates.clear();
            r.elements = Default::default();
            r.navigation = Default::default();
            r.modified = Rc::new(Cell::new(false));
            r.hover = None;
            r.help = Default::default();
            r.frames = None;
            r.native_handoff.clear();
            r.presented_hosts.clear();
            r.feedback_hosts.clear();
            r.windows.clear();
        }
    });
}
pub(crate) fn sync(generation: u64, phase: HostPhase) {
    let retire = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.generation != generation || phase != HostPhase::Capturing)
    });
    if retire {
        close();
    }
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.generation = generation;
            r.phase = phase;
        }
    });
}
pub(crate) fn show(generation: u64, frames: Arc<Vec<CapturedRegion>>) {
    show_with_origin(generation, frames, None);
}
pub(crate) fn show_with_origin(
    generation: u64,
    frames: Arc<Vec<CapturedRegion>>,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
) {
    show_prepared(generation, frames, origin, None);
}
pub(crate) fn show_screenshot_with_candidates(
    generation: u64,
    frames: Arc<Vec<CapturedRegion>>,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    windows: Vec<lexift_core::domain::window_candidate::WindowCandidate>,
    reusable: bool,
) {
    show_prepared(generation, frames, origin, Some((windows, reusable)));
}
fn show_prepared(
    generation: u64,
    frames: Arc<Vec<CapturedRegion>>,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    candidates: Option<(
        Vec<lexift_core::domain::window_candidate::WindowCandidate>,
        bool,
    )>,
) {
    let started = std::time::Instant::now();
    if !STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.generation == generation && r.phase == HostPhase::Capturing)
    }) {
        return;
    }
    close();
    if let Some((candidates, reusable)) = candidates {
        let event = STATE.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut()?;
            r.generation = generation;
            r.candidates = candidates;
            r.reusable = reusable;
            let p = (r.lifecycle.toolbar_cursor_position)()?;
            point_query(r, p)
        });
        if let Some(event) = event {
            emit(event);
        }
    }

    crate::bridge::cancel_idle_memory_trim();
    let result = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().ok_or(())?;
        r.generation = generation;
        r.activation_origin = origin;
        r.activation_done = false;
        r.attempts = 0;
        r.frames = Some(frames.clone());
        for frame in frames.iter() {
            let window = crate::screen_interaction::create(&r.lifecycle, OcrSelectionWindow::new)
                .map_err(|_| ())?;
            window.set_screenshot_mode(r.screenshot);
            window.set_reuse_available(r.reusable);
            let b = frame.bounds;
            let key_weak = window.as_weak();
            let repeat = Rc::new(std::cell::Cell::new(false));
            let native_repeat = repeat.clone();
            let modified = r.modified.clone();
            use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
            window.window().on_winit_window_event(move |_, event| {
                match event {
                    winit::event::WindowEvent::KeyboardInput { event, .. } => {
                        native_repeat.set(event.repeat)
                    }
                    winit::event::WindowEvent::ModifiersChanged(m) => {
                        modified.set(!m.state().is_empty())
                    }
                    winit::event::WindowEvent::Focused(false) => modified.set(false),
                    _ => {}
                }
                EventResult::Propagate
            });
            let reuse_weak = window.as_weak();
            let reuse_repeat = repeat.clone();
            window.on_reuse_region(move || {
                request_reuse(generation, &reuse_weak, reuse_repeat.get())
            });
            window.on_color_copy(move || {
                let current = STATE.with(|s| {
                    let slot = s.borrow();
                    let Some(r) = slot.as_ref() else {
                        return false;
                    };
                    let Some(w) = key_weak.upgrade() else {
                        return false;
                    };
                    r.generation == generation
                        && r.screenshot
                        && !repeat.get()
                        && r.windows
                            .iter()
                            .any(|h| h.bounds == b && std::ptr::eq(h.window.window(), w.window()))
                        && r.lifecycle.annotation_foreground.as_ref().is_none_or(|q| {
                            q().is_some_and(|token| {
                                r.windows.iter().any(|h| {
                                    (r.lifecycle.annotation_window_token)(h.window.window())
                                        == Some(token)
                                })
                            })
                        })
                });
                current && super::pixel_picker::copy()
            });
            let cancel_window = window.as_weak();
            window.on_cancelled(move || {
                let Some(window) = cancel_window.upgrade() else {
                    return;
                };
                let current = STATE.with(|s| {
                    s.borrow().as_ref().is_some_and(|r| {
                        r.generation == generation
                            && r.windows.iter().any(
                                |Host {
                                     window: w, bounds, ..
                                 }| {
                                    *bounds == b && std::ptr::eq(w.window(), window.window())
                                },
                            )
                    })
                });
                if current {
                    close();
                    emit(SelectionEvent::Cancelled);
                }
            });
            let width = (b.right - b.left) as u32;
            let height = (b.bottom - b.top) as u32;
            let copy_started = std::time::Instant::now();
            let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &frame.pixels,
                width,
                height,
            );
            window.set_frozen_image(slint::Image::from_rgba8(pixels));
            tracing::debug!(
                elapsed_us = copy_started.elapsed().as_micros(),
                "Screenshot Slint pixel copy completed"
            );
            window
                .window()
                .set_position(slint::PhysicalPosition::new(b.left, b.top));
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, height));
            let weak = window.as_weak();
            let finished = Arc::new(AtomicBool::new(false));
            let done = finished.clone();
            let feedback_done = Arc::new(AtomicBool::new(false));
            let notifier_weak = weak.clone();
            let installed = window
                .window()
                .set_rendering_notifier(move |state, _| {
                    if matches!(state, slint::RenderingState::AfterRendering)
                        && !done.load(Ordering::Relaxed)
                    {
                        schedule_handoff(generation, b, notifier_weak.clone(), done.clone());
                    } else if matches!(state, slint::RenderingState::AfterRendering) {
                        let weak = notifier_weak.clone();
                        let complete = feedback_done.clone();
                        let hover = hover_snapshot(generation, b);
                        let _ = slint::invoke_from_event_loop(move || {
                            present_hover(generation, hover);
                            if feedback_presented(generation, b, weak) {
                                complete.store(true, Ordering::Relaxed);
                            }
                        });
                    }
                })
                .is_ok();
            // Software renderers have no rendering notifier; native WM_PAINT still
            // brackets their redraw dispatch and defers handoff until it returns.
            r.native_handoff.push(!installed);
            r.windows.push(Host::new(window, b));
        }
        r.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || prepare(generation),
        );
        Ok::<(), ()>(())
    });
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "Region hosts constructed"
    );
    if result.is_err() {
        close();
        emit(SelectionEvent::Failed {
            generation,
            error: "Could not prepare OCR selection".into(),
        });
    } else {
        // Try the already-created native handles now; retain the existing
        // bounded timer only for backends whose handles are not ready yet.
        prepare(generation);
    }
}

fn prepare(generation: u64) {
    let started = std::time::Instant::now();
    let result = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if r.generation != generation {
            return None;
        }
        r.attempts += 1;
        for Host { window, .. } in &r.windows {
            match (r.prepare)(window.window()) {
                PassiveWindowPreparation::Ready => {}
                PassiveWindowPreparation::Pending if r.attempts < 20 => return None,
                _ => return Some(false),
            }
        }
        let frames = r.frames.as_ref()?;
        for (host, frame) in r.windows.iter().zip(frames.iter()) {
            match host.prepare(&r.lifecycle, Some(frame)) {
                PassiveWindowPreparation::Ready => {}
                PassiveWindowPreparation::Pending if r.attempts < 20 => return None,
                _ => return Some(false),
            }
        }
        // All native windows must be ready before any screen is covered.
        for host in &r.windows {
            let window = &host.window;
            let bounds = &host.bounds;
            let bounds = *bounds;
            let weak = window.as_weak();
            let sink =
                host.input().pointer_sink(move |event| {
                    let Some(window) = weak.upgrade() else { return };
                    let weak = window.as_weak();
                    let hover = (event == PopupPointerInput::FeedbackFramePresented)
                        .then(|| hover_snapshot(generation, bounds))
                        .flatten();
                    let _ =
                        slint::invoke_from_event_loop(move || {
                            let Some(window) = weak.upgrade() else { return };
                            let current =
                                STATE.with(|s| {
                                    s.borrow().as_ref().is_some_and(|r| {
                                        r.generation == generation
                                && r.windows.iter().any(|Host {window: w, bounds: b, ..}| {
                                    *b == bounds && std::ptr::eq(w.window(), window.window())
                                })
                                    })
                                });
                            if current {
                                if event == PopupPointerInput::FeedbackFramePresented {
                                    present_hover(generation, hover);
                                }
                                pointer(generation, bounds, window.window().scale_factor(), event);
                            }
                        });
                });
            if !(r.lifecycle.complete_toolbar_show)(window.window(), sink)
                || !(r.lifecycle.ocr_dismissal)(window.window(), true)
            {
                return Some(false);
            }
        }
        if r.screenshot
            && let Some(point) = (r.lifecycle.toolbar_cursor_position)()
            && let Some(host) = r.windows.iter().find(|h| {
                point.x >= h.bounds.left
                    && point.x < h.bounds.right
                    && point.y >= h.bounds.top
                    && point.y < h.bounds.bottom
            })
        {
            r.help.pointer(host.bounds, Some(point));
            super::pixel_picker::point(generation, Some(point));
        }
        if r.screenshot {
            sync_help(r);
            sync_picker(r);
        }
        for (index, host) in r.windows.iter().enumerate() {
            let window = &host.window;
            let bounds = &host.bounds;
            // Reapply physical geometry after native non-client sizing was removed.
            window
                .window()
                .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
            window.window().set_size(slint::PhysicalSize::new(
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
            ));
            if host.show().is_err() {
                return Some(false);
            }
            if r.native_handoff[index] {
                schedule_handoff(
                    generation,
                    *bounds,
                    window.as_weak(),
                    Arc::new(AtomicBool::new(false)),
                );
            }
        }
        r.frames = None;
        r.timer.stop();
        Some(true)
    });
    if result == Some(false) {
        close();
        emit(SelectionEvent::Failed {
            generation,
            error: "Could not prepare OCR selection".into(),
        });
    } else if result == Some(true) {
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "Region hosts prepared and shown"
        );
    }
}
/// AfterRendering runs before the renderer's present returns. Queue the native
/// handoff for a later event-loop turn and validate the owning window instance.
fn schedule_handoff(
    generation: u64,
    bounds: Rect,
    weak: slint::Weak<OcrSelectionWindow>,
    done: Arc<AtomicBool>,
) {
    let _ = slint::invoke_from_event_loop(move || finish_handoff(generation, bounds, weak, done));
}
fn finish_handoff(
    generation: u64,
    bounds: Rect,
    weak: slint::Weak<OcrSelectionWindow>,
    done: Arc<AtomicBool>,
) {
    if done.load(Ordering::Relaxed) {
        return;
    }
    let Some(window) = weak.upgrade() else {
        return;
    };
    let callback = STATE.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        (r.generation == generation
            && r.phase == HostPhase::Capturing
            && r.windows.iter().any(
                |Host {
                     window: current,
                     bounds: b,
                     ..
                 }| {
                    *b == bounds && std::ptr::eq(current.window(), window.window())
                },
            ))
        .then(|| r.lifecycle.complete_region_frame.clone())
    });
    if let Some(callback) = callback
        && window.window().is_visible()
        && callback(window.window())
    {
        STATE.with(|s| {
            if let Some(r) = s.borrow().as_ref() {
                for host in &r.windows {
                    if std::ptr::eq(host.window.window(), window.window()) {
                        host.presented();
                    }
                }
            }
        });
        done.store(true, Ordering::Relaxed);
        crate::bridge::repair_window_paint(window.window());
    }
}

fn warm_assets(generation: u64, weak: slint::Weak<OcrSelectionWindow>, step: u8) {
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let Some(window) = weak.upgrade() else { return };
        let context = STATE.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref()?;
            let host = r
                .windows
                .iter()
                .find(|h| std::ptr::eq(h.window.window(), window.window()))?;
            (r.generation == generation && r.screenshot && !r.submitted)
                .then(|| (r.lifecycle.clone(), host.bounds))
        });
        let Some((lifecycle, bounds)) = context else {
            return;
        };
        let scale = window.window().scale_factor();
        let result = match step {
            0 => super::size_label::prepare(scale, &lifecycle),
            1 => super::pixel_picker::prepare(scale, &lifecycle),
            _ => super::screenshot_help::prepare(
                scale,
                (lifecycle.popup_work_area)(Point {
                    x: bounds.left,
                    y: bounds.top,
                })
                .unwrap_or(bounds),
                &lifecycle,
            ),
        };
        if result.is_ok() && step < 2 {
            warm_assets(generation, weak, step + 1);
        }
    });
}

/// The native bootstrap handoff requests another paint. Warm editor resources
/// only after that Slint feedback frame has returned from present on every host.
fn feedback_presented(
    generation: u64,
    bounds: Rect,
    weak: slint::Weak<OcrSelectionWindow>,
) -> bool {
    let Some(window) = weak.upgrade() else {
        return true;
    };
    let (ready, warm) = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && !r.submitted)
        else {
            return (true, vec![]);
        };
        if !r
            .windows
            .iter()
            .any(|h| h.bounds == bounds && std::ptr::eq(h.window.window(), window.window()))
        {
            return (true, vec![]);
        }
        if !r.presented_hosts.contains(&bounds) {
            return (false, vec![]);
        }
        if r.feedback_hosts.contains(&bounds) {
            return (true, vec![]);
        }
        r.feedback_hosts.push(bounds);
        tracing::debug!(?bounds, "Region feedback frame presented");
        if r.screenshot && r.feedback_hosts.len() == r.windows.len() {
            (true, r.windows.iter().map(|h| h.window.as_weak()).collect())
        } else {
            (true, vec![])
        }
    });
    if !warm.is_empty() {
        // Hiding a foreground pin before activation invalidates the original
        // foreground identity. Retire pins only after this complete host handoff.
        emit(SelectionEvent::Ready { generation });
    }
    for weak in warm {
        warm_assets(generation, weak, 0);
    }
    ready
}

pub(crate) fn pointer(generation: u64, bounds: Rect, _scale: f32, event: PopupPointerInput) {
    if event == PopupPointerInput::FramePresented {
        let current = STATE.with(|s| {
            let mut slot = s.borrow_mut();
            let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
                return false;
            };
            if let Some(host) = r.windows.iter().find(|h| h.bounds == bounds) {
                host.presented();
                let new = !r.presented_hosts.contains(&bounds);
                if new {
                    r.presented_hosts.push(bounds);
                }
                true
            } else {
                false
            }
        });
        if current {
            activate(generation, bounds, false);
            let redraw = STATE.with(|s| {
                let slot = s.borrow();
                let r = slot.as_ref()?;
                (!r.feedback_hosts.contains(&bounds))
                    .then(|| {
                        r.windows
                            .iter()
                            .find(|h| h.bounds == bounds)
                            .map(|h| h.window.as_weak())
                    })
                    .flatten()
            });
            if let Some(window) = redraw.and_then(|w| w.upgrade()) {
                crate::bridge::repair_window_paint(window.window());
            }
        }
        return;
    }
    if event == PopupPointerInput::FeedbackFramePresented {
        let weak = STATE.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref().filter(|r| r.generation == generation)?;
            r.windows
                .iter()
                .enumerate()
                .find(|(i, h)| h.bounds == bounds && r.native_handoff[*i])
                .map(|(_, h)| h.window.as_weak())
        });
        if let Some(weak) = weak {
            feedback_presented(generation, bounds, weak);
        }
        return;
    }
    if matches!(event, PopupPointerInput::LeftPressed { .. }) {
        slint::Timer::single_shot(Duration::ZERO, move || activate(generation, bounds, true));
    }
    // Native passive input owns capture; forward hover only so Slint can select its crosshair.
    if let Some(window) = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|r| r.generation == generation)
            .and_then(|r| {
                r.windows
                    .iter()
                    .find(|host| host.bounds == bounds)
                    .map(|host| host.window.clone_strong())
            })
    }) {
        let scale = window.window().scale_factor();
        match event {
            PopupPointerInput::Moved { x, y } => {
                window
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(x / scale, y / scale),
                    })
            }
            PopupPointerInput::Exited => window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerExited),
            _ => {}
        }
    }
    let mut outcome = None;
    let mut input_screen = bounds;
    let mut query_event = None;
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.generation != generation
            || r.submitted
            || r.windows.is_empty()
            || (r.reuse_pending.is_some()
                && !matches!(
                    event,
                    PopupPointerInput::RightPressed { .. }
                        | PopupPointerInput::LeftCancelled { .. }
                        | PopupPointerInput::DismissRequested { .. }
                ))
        {
            return;
        }
        let point = |x: f32, y: f32| {
            let (x, y) = crate::screen_interaction::screen_point(bounds, (x, y), 1.);
            Point {
                x: x.round() as i32,
                y: y.round() as i32,
            }
        };
        match event {
            PopupPointerInput::Moved { x, y }
            | PopupPointerInput::LeftPressed { x, y }
            | PopupPointerInput::LeftReleased { x, y } => {
                let p = point(x, y);
                if !r.gesture.active() && matches!(event, PopupPointerInput::Moved { .. }) {
                    query_event = point_query(r, p);
                    r.hover = candidate_at(r, p);
                }
                let host = r
                    .windows
                    .iter()
                    .find(|h| {
                        p.x >= h.bounds.left
                            && p.x < h.bounds.right
                            && p.y >= h.bounds.top
                            && p.y < h.bounds.bottom
                    })
                    .map_or(bounds, |h| h.bounds);
                input_screen = host;
                r.help.pointer(host, Some(p));
                if r.screenshot {
                    let (px, py) = crate::screen_interaction::screen_point(bounds, (x, y), 1.);
                    super::pixel_picker::point(
                        generation,
                        Some(Point {
                            x: px.floor() as i32,
                            y: py.floor() as i32,
                        }),
                    );
                }
            }
            PopupPointerInput::Exited => {
                r.help.pointer(bounds, None);
                // Cross-monitor delivery may report the old host's exit after the
                // new host's move. Only retire a query still owned by this host.
                if r.query_point.is_some_and(|p| inside(bounds, p)) {
                    query_event = cancel_query(r);
                    r.query_point = None;
                }
                if r.screenshot
                    && super::pixel_picker::location().is_some_and(|p| {
                        p.x >= bounds.left
                            && p.x < bounds.right
                            && p.y >= bounds.top
                            && p.y < bounds.bottom
                    })
                {
                    super::pixel_picker::point(generation, None);
                    r.hover = None;
                }
            }
            _ => {}
        }
        match event {
            PopupPointerInput::Scrolled {
                x,
                y,
                delta_x,
                delta_y,
            } if r.screenshot && !r.gesture.active() && !r.modified.get() && delta_x == 0. => {
                let scale = r
                    .windows
                    .iter()
                    .find(|h| h.bounds == bounds)
                    .map_or(1., |h| h.window.window().scale_factor());
                candidate_at(r, point(x, y));
                r.hover = r.navigation.scroll(delta_y / (60. * scale));
            }
            PopupPointerInput::LeftPressed { x, y } => {
                let scale = r
                    .windows
                    .iter()
                    .find(|h| h.bounds == bounds)
                    .map_or(1., |h| h.window.window().scale_factor());
                query_event = cancel_query(r);
                r.gesture.begin_candidate(
                    point(x, y),
                    if r.screenshot {
                        r.presented_hover
                    } else {
                        r.hover
                    },
                    scale,
                );
            }
            PopupPointerInput::LeftReleased { x, y } => {
                if let Some(selection) = r
                    .gesture
                    .finish(point(x, y), SelectionContext::standalone(generation))
                {
                    outcome = Some(SelectionEvent::SelectionSubmitted {
                        selection,
                        focus: None,
                    });
                } else {
                    for Host { window, .. } in &r.windows {
                        window.set_selecting(false);
                        window.set_size_visible(false);
                    }
                }
            }
            PopupPointerInput::RightPressed { .. }
            | PopupPointerInput::LeftCancelled { .. }
            | PopupPointerInput::DismissRequested { .. } => {
                outcome = Some(SelectionEvent::Cancelled)
            }
            PopupPointerInput::Moved { x, y } => {
                r.gesture.update(point(x, y));
            }
            _ => {}
        }
        sync_region(r);
        if r.screenshot {
            sync_help(r);
            sync_picker(r);
        }
    });
    if let Some(event) = query_event {
        emit(event);
    }
    if let Some(event) = outcome {
        finish_submission_from(event, Some(input_screen));
    }
}

#[cfg(test)]
pub(crate) fn diagnostic_feedback(generation: u64) {
    let hosts = STATE.with(|s| {
        s.borrow().as_ref().map_or_else(Vec::new, |r| {
            r.windows
                .iter()
                .map(|h| (h.bounds, h.window.as_weak()))
                .collect()
        })
    });
    for (bounds, weak) in hosts {
        pointer(generation, bounds, 1., PopupPointerInput::FramePresented);
        feedback_presented(generation, bounds, weak);
    }
}
#[cfg(test)]
pub(crate) fn diagnostic_reuse(generation: u64) -> bool {
    let weak = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.first())
            .map(|h| h.window.as_weak())
    });
    weak.is_some_and(|weak| request_reuse(generation, &weak, false))
}

#[cfg(test)]
fn set_reusable(generation: u64, available: bool) {
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        if let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.screenshot)
        {
            r.reusable = available;
            for h in &r.windows {
                h.window.set_reuse_available(available);
            }
            sync_help(r);
        }
    });
}
fn request_reuse(generation: u64, weak: &slint::Weak<OcrSelectionWindow>, repeat: bool) -> bool {
    let event = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        let window = weak.upgrade()?;
        if !repeat {
            tracing::debug!(
                generation,
                host = r.query_host,
                available = r.reusable,
                submitted = r.submitted,
                pending = r.reuse_pending.is_some(),
                ready = r.feedback_hosts.len() == r.windows.len(),
                dragging = r.gesture.active(),
                modified = r.modified.get(),
                "Screenshot region reuse input"
            );
        }
        if r.generation != generation
            || !r.screenshot
            || !r.reusable
            || r.submitted
            || repeat
            || r.modified.get()
            || r.gesture.active()
            || r.reuse_pending.is_some()
            || r.windows.is_empty()
            || r.feedback_hosts.len() != r.windows.len()
            || !r
                .windows
                .iter()
                .any(|h| std::ptr::eq(h.window.window(), window.window()))
            || r.lifecycle.annotation_foreground.as_ref().is_some_and(|q| {
                q().is_none_or(|token| {
                    !r.windows.iter().any(|h| {
                        (r.lifecycle.annotation_window_token)(h.window.window()) == Some(token)
                    })
                })
            })
        {
            return None;
        }
        r.reuse_sequence = r
            .reuse_sequence
            .checked_add(1)
            .expect("selection reuse exhausted");
        r.reuse_pending = Some(r.reuse_sequence);
        tracing::debug!(
            generation,
            host = r.query_host,
            request = r.reuse_sequence,
            "Screenshot region reuse requested"
        );
        Some(SelectionEvent::ReuseRegion {
            session: generation,
            host: r.query_host,
            request: r.reuse_sequence,
        })
    });
    let accepted = event.is_some();
    if let Some(event) = event {
        emit(event);
    }
    accepted
}
/// Apply only to the idle host that issued this request, retaining the complete
/// selection picture until the editor's first frame retires it.
pub(crate) fn apply_reused_region(generation: u64, host: u64, request: u64, bounds: Option<Rect>) {
    let selection = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.query_host == host
                && r.reuse_pending == Some(request)
                && r.screenshot
                && !r.submitted
        })?;
        r.reuse_pending = None;
        if r.gesture.active() {
            return None;
        }
        let bounds = bounds?;
        r.hover = Some(bounds);
        sync_region(r);
        Some(
            lexift_core::domain::region_selection::RegionSelectionResult {
                context: SelectionContext::standalone(generation),
                bounds,
            },
        )
    });
    tracing::debug!(
        generation,
        host,
        request,
        applied = selection.is_some(),
        available = bounds.is_some(),
        "Screenshot region reuse result"
    );
    if let Some(selection) = selection {
        finish_submission(SelectionEvent::SelectionSubmitted {
            selection,
            focus: None,
        });
    }
}

fn finish_submission(event: SelectionEvent) {
    finish_submission_from(event, None);
}

fn finish_submission_from(mut event: SelectionEvent, input_screen: Option<Rect>) {
    if matches!(event, SelectionEvent::SelectionSubmitted { .. }) {
        let cancelled = STATE.with(|s| s.borrow_mut().as_mut().and_then(cancel_query));
        if let Some(cancelled) = cancelled {
            emit(cancelled);
        }
    }
    let retained = matches!(event, SelectionEvent::SelectionSubmitted { .. })
        && STATE.with(|s| {
            let mut slot = s.borrow_mut();
            let Some(r) = slot.as_mut().filter(|r| r.screenshot) else {
                return false;
            };
            r.submitted = true;
            if let SelectionEvent::SelectionSubmitted { focus, .. } = &mut event {
                let source = r.lifecycle.annotation_foreground.as_ref().and_then(|q| q());
                let host = r
                    .windows
                    .iter()
                    .find(|h| {
                        source.is_some()
                            && (r.lifecycle.annotation_window_token)(h.window.window()) == source
                    })
                    .or_else(|| r.windows.iter().find(|h| h.revision() == r.query_host))
                    .or_else(|| r.windows.first());
                *focus = host.map(|h| SubmissionFocus {
                    session: r.generation,
                    host: h.revision(),
                    input_host: r
                        .windows
                        .iter()
                        .find(|input| Some(input.bounds) == input_screen)
                        .map_or(h.revision(), |input| input.revision()),
                    screen: input_screen.unwrap_or(h.bounds),
                    source: (r.lifecycle.annotation_window_token)(h.window.window()),
                    observed: r.lifecycle.annotation_foreground.is_some(),
                    started: std::time::Instant::now(),
                });
                tracing::debug!(
                    session = r.generation,
                    ?source,
                    ?focus,
                    "Screenshot selection focus captured before retirement"
                );
            }
            r.reuse_pending = None;
            r.timer.stop();
            r.candidates.clear();
            r.elements = Default::default();
            r.navigation = Default::default();
            r.hover = None;
            r.modified = Rc::new(Cell::new(false));
            for h in &r.windows {
                h.retire_input();
            }
            true
        });
    if !retained {
        close();
    }
    emit(event);
}

fn sync_region(r: &mut Registry) {
    let region = r.gesture.bounds().or(r.hover);
    if r.scheduled_hover != region {
        r.scheduled_hover = region;
        r.hover_revision = r.hover_revision.wrapping_add(1);
        r.hover_paints.clear();
    }
    let label_host = region
        .filter(|_| r.screenshot)
        .and_then(|crop| {
            r.windows
                .iter()
                .find(|h| {
                    crop.left >= h.bounds.left
                        && crop.left < h.bounds.right
                        && crop.top >= h.bounds.top
                        && crop.top < h.bounds.bottom
                })
                .or_else(|| {
                    r.windows.iter().find(|h| {
                        crop.left < h.bounds.right
                            && crop.right > h.bounds.left
                            && crop.top < h.bounds.bottom
                            && crop.bottom > h.bounds.top
                    })
                })
        })
        .map(|h| h.bounds);
    for Host {
        window, bounds: b, ..
    } in &r.windows
    {
        window.set_size_visible(false);
        if let Some(crop) = region.filter(|_| label_host == Some(*b)) {
            let scale = window.window().scale_factor();
            window.set_size_text(
                format!("{} × {} px", crop.right - crop.left, crop.bottom - crop.top).into(),
            );
            let width = (window.get_measured_size_width() * scale).ceil() as u32;
            let height = (window.get_measured_size_height() * scale).ceil() as u32;
            let work = (r.lifecycle.popup_work_area)(Point {
                x: crop.left,
                y: crop.top,
            })
            .unwrap_or(*b);
            let work = Rect {
                left: work.left.max(b.left),
                top: work.top.max(b.top),
                right: work.right.min(b.right),
                bottom: work.bottom.min(b.bottom),
            };
            let p = super::size_label::label_position(
                crop,
                (width, height),
                work,
                (8. * scale).round() as i32,
            );
            window.set_size_left((p.x - b.left) as f32 / scale);
            window.set_size_top((p.y - b.top) as f32 / scale);
            window.set_size_width(width as f32 / scale);
            window.set_size_height(height as f32 / scale);
            window.set_size_visible(true);
        }

        window.set_selecting(false);
        if let Some(preview) = region
            .map(|region| Rect {
                left: region.left.max(b.left),
                top: region.top.max(b.top),
                right: region.right.min(b.right),
                bottom: region.bottom.min(b.bottom),
            })
            .filter(|p| p.right > p.left && p.bottom > p.top)
        {
            let scale = window.window().scale_factor();
            window.set_selecting(true);
            let (left, top) = crate::screen_interaction::local_point(
                *b,
                (preview.left as f32, preview.top as f32),
                scale,
            );
            window.set_selection_left(left);
            window.set_selection_top(top);
            window.set_selection_width((preview.right - preview.left) as f32 / scale);
            window.set_selection_height((preview.bottom - preview.top) as f32 / scale);
        }
    }
}

fn activate(generation: u64, bounds: Rect, explicit: bool) {
    let action = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        tracing::debug!(generation,explicit,done=r.activation_done,screenshot=r.screenshot,?bounds,"Screenshot input activation considered");
        if r.generation != generation || !r.screenshot || r.submitted || (!explicit && r.activation_done) {
            return None;
        }
        let target =
            (r.lifecycle.toolbar_cursor_position)().map_or(r.windows.first()?.bounds, |p| {
                r.windows
                    .iter()
                    .find(|h| {
                        p.x >= h.bounds.left
                            && p.x < h.bounds.right
                            && p.y >= h.bounds.top
                            && p.y < h.bounds.bottom
                    })
                    .map_or(bounds, |h| h.bounds)
            });
        if !explicit && bounds != target {
            tracing::debug!(?target,"Screenshot input awaits its pointer host frame");
            return None;
        }
        r.activation_done = true;
        let current = r.lifecycle.annotation_foreground.as_ref().and_then(|q| q());
        let permitted = explicit
            || r.activation_origin.is_none_or(|origin| {
                current.is_some()
                    && (current == origin
                        || r.windows.iter().any(|h| {
                            (r.lifecycle.annotation_window_token)(h.window.window()) == current
                        }))
            });
        tracing::debug!(?current,origin=?r.activation_origin,permitted,"Screenshot input activation eligibility");
        if !permitted {
            return None;
        }
        let w = r
            .windows
            .iter()
            .find(|h| h.bounds == bounds)?
            .window
            .clone_strong();
        Some((w, r.lifecycle.activate_user_requested_window))
    });
    if let Some((w, activate)) = action {
        activate(w.window());
    }
}
pub(crate) fn refresh_picker() {
    STATE.with(|s| {
        if let Some(r) = s.borrow().as_ref().filter(|r| r.screenshot) {
            sync_picker(r);
        }
    });
}
fn sync_picker(r: &Registry) {
    let screens = r
        .windows
        .iter()
        .map(|h| (h.bounds, h.window.window().scale_factor()))
        .collect::<Vec<_>>();
    let feedback = super::pixel_picker::location().and_then(|p| {
        let h = r.windows.iter().find(|h| {
            p.x >= h.bounds.left
                && p.x < h.bounds.right
                && p.y >= h.bounds.top
                && p.y < h.bounds.bottom
        })?;
        let scale = h.window.window().scale_factor();
        let size = (
            (h.window.get_measured_picker_width() * scale).ceil() as u32,
            (h.window.get_measured_picker_height() * scale).ceil() as u32,
        );
        super::pixel_picker::live_feedback(&screens, (r.lifecycle.popup_work_area)(p), size)
    });
    for h in &r.windows {
        let w = &h.window;
        w.set_picker_visible(false);
        if let Some(f) = feedback.as_ref().filter(|f| f.host == h.bounds) {
            let scale = f.scale();
            w.set_picker_grid(f.image());
            w.set_picker_coordinates(f.coordinates().into());
            w.set_picker_hex(f.sample.color.hex().into());
            w.set_picker_rgb(f.rgb().into());
            w.set_picker_status(f.status);
            w.set_picker_left((f.bounds.left - h.bounds.left) as f32 / scale);
            w.set_picker_top((f.bounds.top - h.bounds.top) as f32 / scale);
            w.set_picker_width((f.bounds.right - f.bounds.left) as f32 / scale);
            w.set_picker_height((f.bounds.bottom - f.bounds.top) as f32 / scale);
            w.set_picker_visible(true);
        }
    }
}

fn sync_help(r: &Registry) {
    let screens = r
        .windows
        .iter()
        .map(|h| (h.bounds, h.window.window().scale_factor()))
        .collect::<Vec<_>>();
    let host = r.help.host().or_else(|| screens.first().map(|s| s.0));
    let area = host.and_then(|h| {
        (r.lifecycle.popup_work_area)(Point {
            x: h.left,
            y: h.top,
        })
    });
    let help = host.and_then(|bounds| {
        let h = r.windows.iter().find(|h| h.bounds == bounds)?;
        let scale = h.window.window().scale_factor();
        let size = (
            (h.window.get_measured_help_width() * scale).ceil() as u32,
            (h.window.get_measured_help_height() * scale).ceil() as u32,
        );
        r.help.live_feedback(
            bounds,
            area.unwrap_or(bounds),
            size,
            scale,
            r.gesture.bounds(),
        )
    });
    for host in &r.windows {
        let window = &host.window;
        window.set_help_visible(false);
        if let Some(help) = help.as_ref().filter(|h| h.host == host.bounds) {
            let scale = window.window().scale_factor();
            window.set_help_left((help.bounds.left - host.bounds.left) as f32 / scale);
            window.set_help_top((help.bounds.top - host.bounds.top) as f32 / scale);
            window.set_help_width((help.bounds.right - help.bounds.left) as f32 / scale);
            window.set_help_height((help.bounds.bottom - help.bounds.top) as f32 / scale);
            window.set_help_visible(true);
        }
    }
}

pub(crate) fn set_screenshot(value: bool) {
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.screenshot = value;
        }
    });
}

#[cfg(any(test, debug_assertions))]
pub(crate) fn diagnostic_visible() -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            !r.windows.is_empty() && r.windows.iter().all(|w| w.window.window().is_visible())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    fn lifecycle() -> WindowLifecycleCallbacks {
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        lifecycle.complete_toolbar_show = Rc::new(|_, _| true);
        lifecycle.ocr_dismissal = Rc::new(|_, _| true);
        lifecycle.prepare_screen_window = Rc::new(|_, _, _| PassiveWindowPreparation::Ready);
        lifecycle.complete_region_frame = Rc::new(|_| true);
        lifecycle
    }
    #[test]
    fn fast_cross_screen_submission_preserves_input_screen_and_cancels_queued_activation() {
        use lexift_core::ports::magnifier::WindowToken;
        thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let mut l = lifecycle();
        l.annotation_window_token = Rc::new(|_| Some(WindowToken(101)));
        l.annotation_foreground = Some(Rc::new(|| Some(WindowToken(101))));
        l.activate_user_requested_window = |_| {
            CALLS.set(CALLS.get() + 1);
            true
        };
        init(|_| PassiveWindowPreparation::Ready, l);
        sync(501, HostPhase::Capturing);
        set_screenshot(true);
        let output = Rc::new(RefCell::new(None));
        let recorded = output.clone();
        set_handler(Rc::new(move |event| {
            if let SelectionEvent::SelectionSubmitted { focus, .. } = event {
                *recorded.borrow_mut() = focus;
            }
        }));
        show(501, frames());
        let screen = frames()[1].bounds;
        for input in [
            PopupPointerInput::LeftPressed { x: 20., y: 20. },
            PopupPointerInput::Moved { x: 60., y: 60. },
            PopupPointerInput::LeftReleased { x: 60., y: 60. },
        ] {
            pointer(501, screen, 1., input);
        }
        let focus = output.borrow().unwrap();
        assert_eq!(focus.screen, screen);
        assert_ne!(
            focus.host, focus.input_host,
            "source still owns the first screen before queued native activation"
        );
        assert!(submission_live(focus));
        CALLS.set(0);
        activate(501, screen, true);
        assert_eq!(
            CALLS.get(),
            0,
            "retired source cannot perform its queued pointer activation"
        );
        close();
        assert!(!submission_live(focus));
        // Mouse capture still delivers the release to the first host after crossing displays.
        sync(502, HostPhase::Capturing);
        show(502, frames());
        let capture_screen = frames()[0].bounds;
        for input in [
            PopupPointerInput::LeftPressed { x: 20., y: 20. },
            PopupPointerInput::Moved { x: 160., y: 60. },
            PopupPointerInput::LeftReleased { x: 160., y: 60. },
        ] {
            pointer(502, capture_screen, 1., input);
        }
        let crossed = output.borrow().unwrap();
        assert_eq!(crossed.screen, screen);
        assert_ne!(crossed.host, crossed.input_host);
        assert!(submission_live(crossed));
        close();
    }
    fn frames() -> Arc<Vec<CapturedRegion>> {
        Arc::new(vec![
            CapturedRegion {
                bounds: Rect {
                    left: -100,
                    top: 0,
                    right: 0,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
        ])
    }
    #[test]
    fn pin_retirement_follows_activation_and_does_not_authorize_external_foregrounds() {
        use lexift_core::ports::magnifier::WindowToken;
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        thread_local! {
            static FRONT: Cell<isize> = const { Cell::new(17) };
            static ACTIVATIONS: Cell<u32> = const { Cell::new(0) };
        }
        let prepare_case = |session, current| {
            FRONT.set(current);
            ACTIVATIONS.set(0);
            let mut l = lifecycle();
            l.toolbar_cursor_position = Rc::new(|| Some(Point { x: 20, y: 20 }));
            l.annotation_foreground = Some(Rc::new(|| Some(WindowToken(FRONT.get()))));
            l.annotation_window_token = Rc::new(|_| Some(WindowToken(101)));
            l.activate_user_requested_window = |_| {
                FRONT.set(101);
                ACTIVATIONS.set(ACTIVATIONS.get() + 1);
                true
            };
            init(|_| PassiveWindowPreparation::Ready, l);
            sync(session, HostPhase::Capturing);
            set_screenshot(true);
            show_with_origin(session, frames(), Some(Some(WindowToken(17))));
        };
        let retired = Rc::new(Cell::new(0));
        for (session, current, expected) in [(501, 17, 1), (502, 900, 0)] {
            prepare_case(session, current);
            let count = retired.clone();
            set_handler(Rc::new(move |event| {
                if let SelectionEvent::Ready { generation } = event {
                    assert_eq!(generation, session);
                    // Simulate native hide falling back to the application's main
                    // window only if the retired pin still owns the foreground.
                    if FRONT.get() == 17 {
                        FRONT.set(18);
                    }
                    count.set(count.get() + 1);
                }
            }));
            let before = retired.get();
            diagnostic_feedback(session - 1);
            assert_eq!(retired.get(), before);
            diagnostic_feedback(session);
            assert_eq!(ACTIVATIONS.get(), expected);
            assert_eq!(FRONT.get(), if expected == 1 { 101 } else { 900 });
            assert_eq!(retired.get(), before + 1);
            diagnostic_feedback(session);
            assert_eq!(retired.get(), before + 1);
            close();
            diagnostic_feedback(session);
            assert_eq!(retired.get(), before + 1);
        }
    }
    #[test]
    fn reuse_routes_only_idle_ready_owned_hosts_and_retires_through_shared_submit() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(301, HostPhase::Capturing);
        set_screenshot(true);
        show(301, frames());
        let weak = STATE.with(|s| s.borrow().as_ref().unwrap().windows[0].window.as_weak());
        let submissions = Rc::new(RefCell::new(Vec::new()));
        let events = Rc::new(RefCell::new(Vec::new()));
        let out = submissions.clone();
        let requests = events.clone();
        set_handler(Rc::new(move |event| match event {
            SelectionEvent::SelectionSubmitted { selection, .. } => {
                out.borrow_mut().push(selection.bounds)
            }
            SelectionEvent::ReuseRegion {
                session,
                host,
                request,
            } => requests.borrow_mut().push((session, host, request)),
            _ => {}
        }));
        set_reusable(301, true);
        assert!(weak.upgrade().unwrap().get_reuse_available());
        assert!(!request_reuse(301, &weak, false)); // No complete feedback yet.
        STATE.with(|s| {
            s.borrow_mut().as_mut().unwrap().feedback_hosts =
                frames().iter().map(|f| f.bounds).collect()
        });
        assert!(!request_reuse(301, &weak, true));
        STATE.with(|s| s.borrow().as_ref().unwrap().modified.set(true));
        assert!(!request_reuse(301, &weak, false));
        STATE.with(|s| s.borrow().as_ref().unwrap().modified.set(false));
        pointer(
            301,
            frames()[0].bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 10., y: 10. },
        );
        assert!(!request_reuse(301, &weak, false));
        STATE.with(|s| s.borrow_mut().as_mut().unwrap().gesture.cancel());
        set_reusable(301, false);
        assert!(!request_reuse(301, &weak, false));
        set_reusable(301, true);
        assert!(request_reuse(301, &weak, false));
        assert!(!request_reuse(301, &weak, false));
        let (session, host, request) = events.borrow()[0];
        apply_reused_region(session, host, request, None); // Rejection unlocks input.
        assert!(request_reuse(301, &weak, false));
        let (session, host, request) = events.borrow()[1];
        let expected = Rect {
            left: -70,
            top: 10,
            right: 60,
            bottom: 80,
        };
        apply_reused_region(session - 1, host, request, Some(expected));
        apply_reused_region(session, host + 1, request, Some(expected));
        apply_reused_region(session, host, request - 1, Some(expected));
        assert!(submissions.borrow().is_empty());
        pointer(
            301,
            frames()[0].bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 10., y: 10. },
        );
        STATE.with(|s| assert!(!s.borrow().as_ref().unwrap().gesture.active()));
        apply_reused_region(session, host, request, Some(expected));
        apply_reused_region(session, host, request, Some(expected));
        assert_eq!(*submissions.borrow(), vec![expected]);
        assert!(!active());
        assert!(!request_reuse(301, &weak, false));
        STATE.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.submitted && r.query_pending.is_none());
            assert_eq!(r.scheduled_hover, Some(expected));
            assert!(r.windows.iter().all(|h| h.window.get_selecting()));
        });
        finish_selection(301);
        sync(302, HostPhase::Capturing);
        set_screenshot(true);
        show(302, frames());
        apply_reused_region(session, host, request, Some(expected));
        assert!(!request_reuse(301, &weak, false));
        assert_eq!(submissions.borrow().len(), 1);
        close();
    }
    #[test]
    fn presented_frame_activates_only_the_requested_host_once_and_rejects_stale_sessions() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        use lexift_core::ports::magnifier::WindowToken;
        thread_local! { static CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
        let calls = || CALLS.with(|c| c.get());
        let mut l = lifecycle();
        l.toolbar_cursor_position = Rc::new(|| Some(Point { x: 10, y: 20 }));
        l.annotation_foreground = Some(Rc::new(|| Some(WindowToken(17))));
        l.activate_user_requested_window = |_| {
            CALLS.with(|c| c.set(c.get() + 1));
            true
        };
        init(|_| PassiveWindowPreparation::Ready, l);
        sync(31, HostPhase::Capturing);
        set_screenshot(true);
        show_with_origin(31, frames(), Some(Some(WindowToken(17))));
        let weak: Vec<_> = STATE.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .windows
                .iter()
                .map(|h| h.window.as_weak())
                .collect()
        });
        assert!(!feedback_presented(31, frames()[0].bounds, weak[0].clone()));
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().feedback_hosts.is_empty()));
        pointer(
            31,
            frames()[0].bounds,
            1.,
            PopupPointerInput::FramePresented,
        );
        assert_eq!(calls(), 0);
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().presented_hosts.len(), 1));
        feedback_presented(31, frames()[0].bounds, weak[0].clone());
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().feedback_hosts.len(), 1));
        pointer(
            31,
            frames()[1].bounds,
            1.,
            PopupPointerInput::FramePresented,
        );
        pointer(
            31,
            frames()[1].bounds,
            1.,
            PopupPointerInput::FramePresented,
        );
        assert_eq!(calls(), 1);
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().presented_hosts.len(), 2));
        feedback_presented(31, frames()[1].bounds, weak[1].clone());
        feedback_presented(31, frames()[1].bounds, weak[1].clone());
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().feedback_hosts.len(), 2));
        sync(32, HostPhase::Capturing);
        show_with_origin(32, frames(), Some(Some(WindowToken(99))));
        pointer(
            31,
            frames()[1].bounds,
            1.,
            PopupPointerInput::FramePresented,
        );
        pointer(
            32,
            frames()[1].bounds,
            1.,
            PopupPointerInput::FramePresented,
        );
        assert_eq!(calls(), 1);
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().presented_hosts.len(), 1));
        activate(32, frames()[1].bounds, true);
        assert_eq!(calls(), 2);
        set_screenshot(false);
        show(32, frames());
        pointer(
            32,
            frames()[1].bounds,
            1.,
            PopupPointerInput::FramePresented,
        );
        assert_eq!(calls(), 2);
        close();
    }

    #[test]
    fn old_host_exit_does_not_clear_the_new_host_sample() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(43, HostPhase::Capturing);
        set_screenshot(true);
        let captured = frames();
        super::super::pixel_picker::begin(43, captured.clone(), Rc::new(|| {}));
        show(43, captured.clone());
        set_candidates(
            43,
            vec![lexift_core::domain::window_candidate::WindowCandidate {
                process_id: 0,
                window: lexift_core::ports::magnifier::WindowToken(1),
                bounds: Rect {
                    left: captured[0].bounds.left,
                    top: 0,
                    right: captured[1].bounds.right,
                    bottom: 100,
                },
                order: 0,
            }],
            vec![],
        );
        pointer(
            43,
            captured[0].bounds,
            1.,
            PopupPointerInput::Moved { x: 50., y: 20. },
        );
        pointer(
            43,
            captured[1].bounds,
            1.,
            PopupPointerInput::Moved { x: 20., y: 20. },
        );
        let query = STATE.with(|s| s.borrow().as_ref().unwrap().query_pending.clone());
        assert!(query.is_some());
        pointer(43, captured[0].bounds, 1., PopupPointerInput::Exited);
        STATE.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.query_pending, query);
            assert_eq!(r.query_point, Some(Point { x: 20, y: 20 }));
        });
        assert_eq!(
            super::super::pixel_picker::location(),
            Some(Point { x: 20, y: 20 })
        );
        pointer(43, captured[1].bounds, 1., PopupPointerInput::Exited);
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().query_pending.is_none()));
        assert_eq!(super::super::pixel_picker::location(), None);
        close();
        super::super::pixel_picker::clear();
    }

    fn paint_hover(session: u64) {
        let bounds = STATE.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .windows
                .iter()
                .map(|h| h.bounds)
                .collect::<Vec<_>>()
        });
        for b in bounds {
            present_hover(session, hover_snapshot(session, b));
        }
    }
    fn finish_query(
        elements: Vec<lexift_core::domain::element_candidate::ElementCandidate>,
        leaf: bool,
    ) {
        let query = STATE.with(|s| s.borrow().as_ref().unwrap().query_pending.clone().unwrap());
        set_elements(lexift_core::domain::element_candidate::ElementQueryResult {
            query,
            elements,
            leaf,
        });
    }
    #[test]
    fn frozen_windows_hover_across_hosts_click_with_jitter_and_retire_on_close() {
        use lexift_core::{
            domain::window_candidate::WindowCandidate, ports::magnifier::WindowToken,
        };
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(71, HostPhase::Capturing);
        set_screenshot(true);
        show(71, frames());
        let selected = Rc::new(RefCell::new(Vec::new()));
        let results = selected.clone();
        set_handler(Rc::new(move |event| {
            if let SelectionEvent::SelectionSubmitted { selection, .. } = event {
                results.borrow_mut().push(selection.bounds);
            }
        }));
        let expected = Rect {
            left: -50,
            top: 10,
            right: 70,
            bottom: 80,
        };
        set_candidates(
            71,
            vec![WindowCandidate {
                process_id: 0,
                window: WindowToken(1),
                bounds: expected,
                order: 0,
            }],
            Vec::new(),
        );
        pointer(
            71,
            frames()[0].bounds,
            1.,
            PopupPointerInput::Moved { x: 80., y: 30. },
        );
        STATE.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.hover, Some(expected));
            assert!(r.windows.iter().all(|h| h.window.get_selecting()));
            assert!(!r.gesture.active());
        });
        paint_hover(71);
        pointer(
            71,
            frames()[0].bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 80., y: 30. },
        );
        pointer(
            71,
            frames()[0].bounds,
            1.,
            PopupPointerInput::LeftReleased { x: 82., y: 31. },
        );
        assert_eq!(*selected.borrow(), vec![expected]);
        assert!(!active());
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().candidates.is_empty()));
        sync(72, HostPhase::Capturing);
        show(72, frames());
        set_candidates(
            71,
            vec![WindowCandidate {
                process_id: 0,
                window: WindowToken(2),
                bounds: expected,
                order: 0,
            }],
            Vec::new(),
        );
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().candidates.is_empty()));
        set_screenshot(false);
        set_candidates(
            72,
            vec![WindowCandidate {
                process_id: 0,
                window: WindowToken(2),
                bounds: expected,
                order: 0,
            }],
            Vec::new(),
        );
        pointer(
            72,
            frames()[1].bounds,
            1.,
            PopupPointerInput::Moved { x: 20., y: 20. },
        );
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().hover, None));
        close();
    }

    #[test]
    fn asynchronous_elements_wheel_levels_reset_and_press_freezes_the_presented_candidate() {
        use lexift_core::{
            domain::{element_candidate::ElementCandidate, window_candidate::WindowCandidate},
            ports::magnifier::WindowToken,
        };
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(81, HostPhase::Capturing);
        set_screenshot(true);
        show(81, frames());
        let region = |l, t, r, b| Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        let whole = region(-80, 0, 80, 90);
        let parent = region(-70, 5, 70, 85);
        let leaf = region(-60, 10, 20, 50);
        let other = region(30, 10, 60, 50);
        let node = |id, p: Option<i32>, bounds| ElementCandidate {
            window: WindowToken(1),
            id: vec![id],
            parent: p.map(|id| vec![id]),
            bounds,
            order: id as u32,
        };
        set_candidates(
            81,
            vec![WindowCandidate {
                process_id: 0,
                window: WindowToken(1),
                bounds: whole,
                order: 0,
            }],
            vec![
                node(1, None, whole),
                node(2, Some(1), parent),
                node(3, Some(2), leaf),
                node(4, Some(2), other),
            ],
        );
        let send = |event| pointer(81, frames()[1].bounds, 1., event);
        let hover = || STATE.with(|s| s.borrow().as_ref().unwrap().hover);
        let wheel = |x, dx, dy| {
            send(PopupPointerInput::Scrolled {
                x,
                y: 20.,
                delta_x: dx,
                delta_y: dy,
            })
        };
        send(PopupPointerInput::Moved { x: 0., y: 20. });
        finish_query(
            vec![
                node(1, None, whole),
                node(2, Some(1), parent),
                node(3, Some(2), leaf),
            ],
            true,
        );
        assert_eq!(hover(), Some(leaf));
        wheel(0., 0., 30.);
        assert_eq!(hover(), Some(leaf));
        wheel(0., 0., 30.);
        assert_eq!(hover(), Some(parent));
        send(PopupPointerInput::Moved { x: 1., y: 20. });
        assert_eq!(hover(), Some(parent));
        wheel(1., 60., 0.);
        STATE.with(|s| s.borrow().as_ref().unwrap().modified.set(true));
        wheel(1., 0., 60.);
        assert_eq!(hover(), Some(parent));
        STATE.with(|s| s.borrow().as_ref().unwrap().modified.set(false));
        wheel(1., 0., 60.);
        assert_eq!(hover(), Some(whole));
        wheel(1., 0., 600.);
        assert_eq!(hover(), Some(whole));
        send(PopupPointerInput::Moved { x: 40., y: 20. });
        finish_query(
            vec![
                node(1, None, whole),
                node(2, Some(1), parent),
                node(4, Some(2), other),
            ],
            true,
        );
        assert_eq!(hover(), Some(other));
        wheel(40., 0., 60.);
        assert_eq!(hover(), Some(parent));
        let selected = Rc::new(RefCell::new(None));
        let result = selected.clone();
        set_handler(Rc::new(move |event| {
            if let SelectionEvent::SelectionSubmitted { selection, .. } = event {
                *result.borrow_mut() = Some(selection.bounds);
            }
        }));
        paint_hover(81);
        send(PopupPointerInput::LeftPressed { x: 40., y: 20. });
        wheel(40., 0., 60.);
        assert_eq!(hover(), Some(parent));
        set_elements(lexift_core::domain::element_candidate::ElementQueryResult {
            query: lexift_core::domain::element_candidate::ElementQuery {
                session: 81,
                request: 1,
                host: 0,
                point: Point { x: 0, y: 0 },
                window: lexift_core::domain::window_candidate::WindowCandidate {
                    process_id: 0,
                    window: lexift_core::ports::magnifier::WindowToken(1),
                    bounds: frames()[0].bounds,
                    order: 0,
                },
            },
            elements: vec![],
            leaf: false,
        });
        assert_eq!(hover(), Some(parent));
        let old_modifier = STATE.with(|s| s.borrow().as_ref().unwrap().modified.clone());
        send(PopupPointerInput::LeftReleased { x: 42., y: 21. });
        old_modifier.set(true);
        STATE.with(|s| assert!(!s.borrow().as_ref().unwrap().modified.get()));
        assert_eq!(*selected.borrow(), Some(parent));
        assert!(!active());
        assert_eq!(hover(), None);
    }

    #[test]
    fn late_query_cannot_change_a_click_before_the_new_candidate_was_painted() {
        use lexift_core::{
            domain::{
                element_candidate::{ElementCandidate, ElementQueryResult},
                window_candidate::WindowCandidate,
            },
            ports::magnifier::WindowToken,
        };
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(99, HostPhase::Capturing);
        set_screenshot(true);
        show(99, frames());
        let whole = Rect {
            left: -80,
            top: 0,
            right: 80,
            bottom: 90,
        };
        let leaf = Rect {
            left: -10,
            top: 10,
            right: 30,
            bottom: 60,
        };
        set_candidates(
            99,
            vec![WindowCandidate {
                process_id: 0,
                window: WindowToken(1),
                bounds: whole,
                order: 0,
            }],
            vec![],
        );
        pointer(
            99,
            frames()[1].bounds,
            1.,
            PopupPointerInput::Moved { x: 10., y: 20. },
        );
        paint_hover(99);
        let query = STATE.with(|s| s.borrow().as_ref().unwrap().query_pending.clone().unwrap());
        set_elements(ElementQueryResult {
            query: query.clone(),
            elements: vec![ElementCandidate {
                window: WindowToken(1),
                id: vec![7],
                parent: None,
                bounds: leaf,
                order: 0,
            }],
            leaf: true,
        });
        STATE.with(|s| {
            let r = s.borrow();
            assert_eq!(r.as_ref().unwrap().hover, Some(leaf));
            assert_eq!(r.as_ref().unwrap().presented_hover, Some(whole));
        });
        pointer(
            99,
            frames()[1].bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 10., y: 20. },
        );
        set_elements(ElementQueryResult {
            query,
            elements: vec![],
            leaf: false,
        });
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().gesture.bounds(), Some(whole)));
        close();
    }

    #[test]
    fn point_cache_expires_on_input_and_old_host_results_are_ignored() {
        use lexift_core::{
            domain::{
                element_candidate::{ElementCandidate, ElementQueryResult},
                window_candidate::WindowCandidate,
            },
            ports::magnifier::WindowToken,
        };
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(98, HostPhase::Capturing);
        set_screenshot(true);
        show(98, frames());
        let whole = Rect {
            left: -80,
            top: 0,
            right: 80,
            bottom: 90,
        };
        set_candidates(
            98,
            vec![WindowCandidate {
                process_id: 0,
                window: WindowToken(1),
                bounds: whole,
                order: 0,
            }],
            vec![],
        );
        pointer(
            98,
            frames()[1].bounds,
            1.,
            PopupPointerInput::Moved { x: 10., y: 20. },
        );
        let q = STATE.with(|s| s.borrow().as_ref().unwrap().query_pending.clone().unwrap());
        finish_query(
            vec![ElementCandidate {
                window: WindowToken(1),
                id: vec![1],
                parent: None,
                bounds: whole,
                order: 0,
            }],
            true,
        );
        pointer(
            98,
            frames()[1].bounds,
            1.,
            PopupPointerInput::Moved { x: 11., y: 20. },
        );
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            assert!(r.query_pending.is_none());
            r.query_cache = Some((
                std::time::Instant::now() - Duration::from_millis(151),
                whole,
            ));
        });
        pointer(
            98,
            frames()[1].bounds,
            1.,
            PopupPointerInput::Moved { x: 12., y: 20. },
        );
        STATE.with(|s| {
            assert!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .query_pending
                    .as_ref()
                    .unwrap()
                    .request
                    > q.request
            )
        });
        show(98, frames());
        set_elements(ElementQueryResult {
            query: q,
            elements: vec![],
            leaf: false,
        });
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().hover.is_none()));
        close();
    }

    #[test]
    fn first_frame_draws_feedback_without_editor_textures_and_retains_pixels_until_editing() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(91, HostPhase::Capturing);
        set_screenshot(true);
        super::super::pixel_picker::clear();
        super::super::screenshot_help::clear();
        super::super::size_label::clear();
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        let frames = Arc::new(vec![CapturedRegion {
            bounds,
            pixels: vec![255; 1000 * 800 * 4],
        }]);
        super::super::pixel_picker::begin(91, frames.clone(), Rc::new(|| {}));
        show(91, frames);
        prepare(91);
        pointer(
            91,
            bounds,
            1.,
            PopupPointerInput::Moved { x: 400., y: 300. },
        );
        pointer(
            91,
            bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 400., y: 300. },
        );
        pointer(
            91,
            bounds,
            1.,
            PopupPointerInput::Moved { x: 600., y: 500. },
        );
        STATE.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref().unwrap();
            let w = &r.windows[0].window;
            assert!(w.get_size_visible());
            assert_eq!(w.get_size_text(), "200 × 200 px");
            assert!(w.get_picker_visible());
            assert!(w.get_help_visible());
        });
        assert!(super::super::pixel_picker::feedback(&[(bounds, 1.)], None).is_none());
        pointer(
            91,
            bounds,
            1.,
            PopupPointerInput::LeftReleased { x: 600., y: 500. },
        );
        assert!(!active());
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().windows.len(), 1));
        finish_selection(90);
        STATE.with(|s| assert_eq!(s.borrow().as_ref().unwrap().windows.len(), 1));
        finish_selection(91);
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().windows.is_empty()));
        super::super::pixel_picker::clear();
    }

    #[test]
    fn focused_screenshot_escape_retires_once_without_changing_ocr_key_routing() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        let count = Rc::new(Cell::new(0));
        let events = count.clone();
        set_handler(Rc::new(move |event| {
            if matches!(event, SelectionEvent::Cancelled) {
                events.set(events.get() + 1);
            }
        }));
        set_screenshot(true);
        sync(92, HostPhase::Capturing);
        show(92, frames());
        let window = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[0]
                .window
                .clone_strong()
        });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!active());
        assert_eq!(count.get(), 1);
        window.invoke_cancelled();
        assert_eq!(count.get(), 1);
        set_screenshot(false);
        sync(93, HostPhase::Capturing);
        show(93, frames());
        let window = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[0]
                .window
                .clone_strong()
        });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(active());
        assert_eq!(count.get(), 1);
        close();
    }

    #[test]
    fn any_native_preparation_failure_destroys_the_group_before_showing() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let mut l = lifecycle();
        l.prepare_screen_window = Rc::new(|w, _, f| {
            let f = f.unwrap();
            assert!(!w.is_visible());
            if f.bounds.left == 0 {
                PassiveWindowPreparation::Failed
            } else {
                PassiveWindowPreparation::Ready
            }
        });
        init(|_| PassiveWindowPreparation::Ready, l);
        sync(3, HostPhase::Capturing);
        show(3, frames());
        assert!(!active());
        STATE.with(|s| assert!(s.borrow().as_ref().unwrap().windows.is_empty()));
    }
    #[test]
    fn cancelled_preparation_and_old_same_generation_frame_cannot_touch_reopened_windows() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let calls = Rc::new(std::cell::Cell::new(0));
        let c = calls.clone();
        let mut l = lifecycle();
        l.complete_region_frame = Rc::new(move |_| {
            c.set(c.get() + 1);
            true
        });
        init(|_| PassiveWindowPreparation::Ready, l);
        sync(3, HostPhase::Capturing);
        show(3, frames());
        sync(4, HostPhase::Idle);
        prepare(3);
        assert!(!active());
        sync(5, HostPhase::Capturing);
        show(5, frames());
        prepare(5);
        let old = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[0]
                .window
                .clone_strong()
        });
        show(5, frames());
        prepare(5);
        finish_handoff(
            5,
            frames()[0].bounds,
            old.as_weak(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(calls.get(), 0);
        let current = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[0]
                .window
                .clone_strong()
        });
        finish_handoff(
            5,
            frames()[0].bounds,
            current.as_weak(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(calls.get(), 1);
        close();
        finish_handoff(
            5,
            frames()[0].bounds,
            current.as_weak(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(calls.get(), 1);
    }
    #[test]
    fn all_overlays_show_after_preparation_and_stale_frames_cannot_reopen_them() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        lifecycle.complete_toolbar_show = Rc::new(|_, _| true);
        lifecycle.ocr_dismissal = Rc::new(|_, _| true);
        lifecycle.prepare_screen_window = Rc::new(|_, _, _| PassiveWindowPreparation::Ready);
        lifecycle.complete_region_frame = Rc::new(|_| true);
        init(|_| PassiveWindowPreparation::Ready, lifecycle);
        sync(7, HostPhase::Capturing);
        let frames = Arc::new(vec![
            CapturedRegion {
                bounds: Rect {
                    left: -100,
                    top: 0,
                    right: 0,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
        ]);
        show(7, frames.clone());
        prepare(7);
        STATE.with(|s| {
            assert!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .windows
                    .iter()
                    .all(|host| host.window.window().is_visible())
            )
        });
        let events = Rc::new(RefCell::new(vec![]));
        let e = events.clone();
        set_handler(Rc::new(move |event| e.borrow_mut().push(event)));
        pointer(
            7,
            frames[0].bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 90., y: 80. },
        );
        pointer(
            7,
            frames[0].bounds,
            1.,
            PopupPointerInput::LeftReleased { x: 110., y: 20. },
        );
        assert!(!active());
        assert!(matches!(
            &events.borrow()[0],
            SelectionEvent::SelectionSubmitted {
                focus: None,
                selection: lexift_core::domain::region_selection::RegionSelectionResult {
                    context: SelectionContext {
                        source_session: 7,
                        request_id: 7,
                        snapshot_id: 7
                    },
                    bounds: Rect {
                        left: -10,
                        top: 20,
                        right: 10,
                        bottom: 80
                    }
                }
            }
        ));
        sync(8, HostPhase::Idle);
        show(7, frames);
        assert!(!active());
    }
    #[test]
    fn screenshot_help_tracks_physical_input_across_capture_hosts_and_avoids_active_crop() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(41, HostPhase::Capturing);
        set_screenshot(true);
        let a = Rect {
            left: -800,
            top: 0,
            right: 0,
            bottom: 600,
        };
        let b = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        show(
            41,
            Arc::new(vec![
                CapturedRegion {
                    bounds: a,
                    pixels: vec![180; 800 * 600 * 4],
                },
                CapturedRegion {
                    bounds: b,
                    pixels: vec![180; 1000 * 800 * 4],
                },
            ]),
        );
        prepare(41);
        let visible = || {
            STATE.with(|s| {
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .windows
                    .iter()
                    .filter(|h| h.window.get_help_visible())
                    .map(|h| h.bounds)
                    .collect::<Vec<_>>()
            })
        };
        pointer(41, a, 1., PopupPointerInput::Moved { x: 600., y: 100. });
        assert_eq!(visible(), vec![a]);
        // Capture can keep delivering coordinates through A while the physical pointer is on B.
        pointer(41, a, 1., PopupPointerInput::Moved { x: 1000., y: 100. });
        assert_eq!(visible(), vec![b]);
        let (x, y) = STATE.with(|s| {
            let s = s.borrow();
            let h = &s.as_ref().unwrap().windows[1];
            (h.window.get_help_left() + 3., h.window.get_help_top() + 3.)
        });
        pointer(41, b, 1., PopupPointerInput::Moved { x, y });
        assert!(visible().is_empty());
        pointer(41, b, 1., PopupPointerInput::Moved { x: 700., y: 100. });
        assert_eq!(visible(), vec![b]);
        pointer(40, a, 1., PopupPointerInput::Moved { x: 600., y: 100. });
        assert_eq!(visible(), vec![b]);
        pointer(41, b, 1., PopupPointerInput::LeftPressed { x, y });
        pointer(41, b, 1., PopupPointerInput::Moved { x: 700., y: 100. });
        assert!(
            visible().is_empty(),
            "active crop hides help even when the pointer is far away"
        );
        close();
        assert!(!active());
        pointer(41, b, 1., PopupPointerInput::Moved { x: 700., y: 100. });
        assert!(!active());
    }
    #[test]
    fn screenshot_preview_uses_whole_physical_bounds_in_all_directions() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(|_| PassiveWindowPreparation::Ready, lifecycle());
        sync(17, HostPhase::Capturing);
        set_screenshot(true);
        show(17, frames());
        prepare(17);
        let events = Rc::new(RefCell::new(vec![]));
        let recorded = events.clone();
        set_handler(Rc::new(move |event| recorded.borrow_mut().push(event)));
        for (a, b) in [
            ((70., 20.), (130., 80.)),
            ((130., 20.), (70., 80.)),
            ((70., 80.), (130., 20.)),
            ((130., 80.), (70., 20.)),
        ] {
            let monitor = frames()[0].bounds;
            pointer(
                17,
                monitor,
                1.25,
                PopupPointerInput::LeftPressed { x: a.0, y: a.1 },
            );
            pointer(
                17,
                monitor,
                1.25,
                PopupPointerInput::Moved { x: b.0, y: b.1 },
            );
            assert!(active(), "preview must not submit or destroy overlays");
            STATE.with(|s| {
                let s = s.borrow();
                let r = s.as_ref().unwrap();
                let labels = r
                    .windows
                    .iter()
                    .filter(|h| h.window.get_size_visible())
                    .collect::<Vec<_>>();
                assert_eq!(labels.len(), 1);
                assert_eq!(labels[0].window.get_size_text(), "60 × 60 px");
            });
        }
        let count = events.borrow().len();
        pointer(
            16,
            frames()[0].bounds,
            1.,
            PopupPointerInput::Moved { x: 10., y: 10. },
        );
        assert_eq!(events.borrow().len(), count);
        close();
    }
}
