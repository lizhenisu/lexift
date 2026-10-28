//! Screenshot-local numeric drafts. Only successful preview frames expose label hits.
use super::*;
use crate::{
    ScreenshotSizeInput,
    bridge::{PopupPointerInput, dispatch_pointer_input},
};
use lexift_core::domain::region_selection::{AspectRatio, SizeAxis};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use std::cell::Cell;
use std::collections::HashMap;
#[path = "screenshot_size_drag.rs"]
mod drag;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::annotation) struct Hit {
    pub crop: Rect,
    pub bounds: Rect,
}
#[derive(Clone, Copy)]
struct Request {
    token: u64,
    generation: u64,
    session: u64,
    canvas: u64,
    index: usize,
    hit: Hit,
    origin: focus::Origin,
}
#[derive(Default)]
pub(in crate::annotation) struct State {
    pub presented: HashMap<u64, Option<Hit>>,
    pending: Option<Request>,
    popup: Option<Popup>,
    swallow: bool,
    suppression: slint::Timer,
}
type CancelInteraction = Rc<dyn Fn(&slint::Window) -> bool>;
type Dismissal = Rc<dyn Fn(&slint::Window, bool) -> bool>;
struct Popup {
    window: ScreenshotSizeInput,
    request: Request,
    revision: u64,
    input: Rc<window::InputWork>,
    preparation: slint::Timer,
    positioned: std::cell::Cell<bool>,
    dismissal: Dismissal,
    cancel_interaction: CancelInteraction,
    shown: std::cell::Cell<bool>,
    drag: drag::State,
    draft: Rc<SizeDraft>,
}
#[derive(Default)]
struct SizeDraft {
    aspect: Cell<Option<AspectRatio>>,
    axis: Cell<SizeAxis>,
}
impl Drop for Popup {
    fn drop(&mut self) {
        self.input.retire();
        self.preparation.stop();
        (self.dismissal)(self.window.window(), false);
        (self.cancel_interaction)(self.window.window());
        let _ = self.window.hide();
    }
}
impl State {
    pub(in crate::annotation) fn window(&self) -> Option<&ScreenshotSizeInput> {
        self.popup.as_ref().map(|p| &p.window)
    }
}
pub(in crate::annotation) fn busy(r: &Registry) -> bool {
    r.screenshot
        .as_ref()
        .is_some_and(|e| e.size_input.pending.is_some() || e.size_input.popup.is_some())
}
pub(in crate::annotation) fn close(r: &mut Registry) -> bool {
    let Some(e) = r.screenshot.as_mut() else {
        return false;
    };
    let pending = e.size_input.pending.take().is_some();
    let popup = e.size_input.popup.take();
    let closed = pending || popup.is_some();
    if closed {
        r.hints.clear();
    }
    drop(popup);
    if closed {
        r.queue_render();
    }
    closed
}
fn allowed(r: &Registry) -> bool {
    if super::long::busy() || super::long::image_editing() {
        return false;
    }
    r.document.mode.is_mouse()
        && r.screenshot.as_ref().is_some_and(|e| {
            e.pin.is_none()
                && !e.region.dragging()
                && FLOW.with(|s| {
                    let s = s.borrow();
                    s.state.session == e.id && s.state.phase == ScreenshotPhase::Editing
                })
        })
        && r.editor.window().is_none()
        && r.toolbar_state.watermark.get().is_none()
        && r.toolbar_state.panel.get().is_none()
        && r.toolbar_state.menus.current.is_none()
        && r.toolbar_state.toolbar_drag.is_none()
}
fn matches(r: &Registry, q: Request) -> bool {
    r.generation == q.generation
        && allowed(r)
        && r.screenshot
            .as_ref()
            .is_some_and(|e| e.id == q.session && e.region.bounds() == Some(q.hit.crop))
        && r.canvas_state
            .canvases
            .get(q.index)
            .is_some_and(|c| c.revision() == q.canvas)
}
fn contains(b: Rect, p: Point) -> bool {
    p.x >= b.left && p.x < b.right && p.y >= b.top && p.y < b.bottom
}
/// Intercept a whole label click before crop handles. No geometry is changed here.
pub(in crate::annotation) fn pointer(
    r: &mut Registry,
    index: usize,
    event: i32,
    x: f32,
    y: f32,
) -> bool {
    let Some(e) = r.screenshot.as_ref().filter(|e| e.pin.is_none()) else {
        return false;
    };
    if e.size_input.swallow {
        if matches!(event, 2 | 3) {
            r.screenshot.as_mut().unwrap().size_input.swallow = false;
        }
        return true;
    }
    if e.size_input.popup.is_some() {
        if event == 0 {
            close(r);
            r.screenshot.as_mut().unwrap().size_input.swallow = true;
        }
        return true;
    }
    if let Some(q) = e.size_input.pending {
        if matches!(event, 2 | 3) {
            if event == 3 {
                close(r);
            } else {
                let layer = &r.canvas_state.canvases[index];
                let (px, py) = crate::screen_interaction::screen_point(
                    layer.bounds,
                    (x, y),
                    layer.input_scale(),
                );
                if contains(
                    q.hit.bounds,
                    Point {
                        x: px.round() as i32,
                        y: py.round() as i32,
                    },
                ) {
                    layer.input().post(move || open(q));
                } else {
                    close(r);
                }
            }
        }
        return true;
    }
    if event != 0 || !allowed(r) {
        return false;
    }
    let Some(layer) = r.canvas_state.canvases.get(index) else {
        return false;
    };
    let Some(hit) = e
        .size_input
        .presented
        .get(&layer.revision())
        .copied()
        .flatten()
    else {
        return false;
    };
    let (px, py) =
        crate::screen_interaction::screen_point(layer.bounds, (x, y), layer.input_scale());
    if !contains(
        hit.bounds,
        Point {
            x: px.round() as i32,
            y: py.round() as i32,
        },
    ) {
        return false;
    }
    let q = Request {
        token: window::next_revision(),
        generation: r.generation,
        session: e.id,
        canvas: layer.revision(),
        index,
        hit,
        origin: focus_origin(&r.lifecycle),
    };
    keyboard::finish(r);
    // A displayed older frame remains clickable, but must not edit a newer crop.
    if matches(r, q) {
        r.screenshot.as_mut().unwrap().size_input.pending = Some(q);
    } else {
        r.screenshot.as_mut().unwrap().size_input.swallow = true;
    }
    true
}
/// Saturation accepts arbitrarily large integers without float rounding or overflow.
fn integer(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(text.bytes().fold(0u64, |n, b| {
        n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    }))
}
fn valid(r: &Registry, q: Request, revision: u64) -> bool {
    matches(r, q)
        && r.screenshot
            .as_ref()
            .and_then(|e| e.size_input.popup.as_ref())
            .is_some_and(|p| p.revision == revision && p.request.token == q.token)
}
/// Only a user edit chooses the driving axis. Programmatic counterpart updates do not.
fn draft_target(q: Request, revision: u64) -> Option<(ScreenshotSizeInput, Rc<SizeDraft>)> {
    REGISTRY.with(|s| {
        let slot = s.try_borrow().ok()?;
        let r = slot.as_ref().filter(|r| valid(r, q, revision))?;
        let p = r.screenshot.as_ref()?.size_input.popup.as_ref()?;
        Some((p.window.clone_strong(), p.draft.clone()))
    })
}
fn edit_dimension(q: Request, revision: u64, axis: SizeAxis) {
    let Some((window, draft)) = draft_target(q, revision) else {
        return;
    };
    draft.axis.set(axis);
    if let Some(ratio) = draft.aspect.get() {
        let source = match axis {
            SizeAxis::Width => window.get_width_text(),
            SizeAxis::Height => window.get_height_text(),
        };
        if let Some(value) = integer(&source) {
            let (w, h) = ratio.dimensions(axis, value);
            match axis {
                SizeAxis::Width => window.set_height_text(h.to_string().into()),
                SizeAxis::Height => window.set_width_text(w.to_string().into()),
            }
        }
    }
    window.set_valid(
        integer(&window.get_width_text()).is_some() && integer(&window.get_height_text()).is_some(),
    );
}
fn lock_draft(q: Request, revision: u64, locked: bool) {
    let Some((window, draft)) = draft_target(q, revision) else {
        return;
    };
    let ratio = if locked {
        let (Some(w), Some(h)) = (
            integer(&window.get_width_text()),
            integer(&window.get_height_text()),
        ) else {
            return;
        };
        Some(AspectRatio::new(w, h))
    } else {
        None
    };
    draft.aspect.set(ratio);
    window.set_locked(locked);
}
fn open(q: Request) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            matches(r, q)
                && r.screenshot
                    .as_ref()
                    .is_some_and(|e| e.size_input.pending.is_some_and(|p| p.token == q.token))
        }) else {
            return;
        };
        let Ok(window) = crate::screen_interaction::create(&r.lifecycle, ScreenshotSizeInput::new)
        else {
            close(r);
            return;
        };
        crate::theme::apply(&window);
        window.set_width_text((q.hit.crop.right - q.hit.crop.left).to_string().into());
        window.set_height_text((q.hit.crop.bottom - q.hit.crop.top).to_string().into());
        let draft = Rc::new(SizeDraft {
            aspect: Cell::new(r.screenshot.as_ref().unwrap().region.aspect()),
            axis: Default::default(),
        });
        window.set_locked(draft.aspect.get().is_some());
        let revision = window::next_revision();
        let input = window::InputWork::new();
        let weak = window.as_weak();
        window.on_draft_changed(move |w, h| {
            if draft_target(q, revision).is_some()
                && let Some(window) = weak.upgrade()
            {
                window.set_valid(integer(w.as_str()).is_some() && integer(h.as_str()).is_some());
            }
        });
        window.on_dimension_edited(move |axis| {
            edit_dimension(
                q,
                revision,
                if axis == 0 {
                    SizeAxis::Width
                } else {
                    SizeAxis::Height
                },
            )
        });
        window.on_lock_requested(move |locked| lock_draft(q, revision, locked));
        let work = input.clone();
        window.on_apply(move |w, h| {
            work.post(move || finish(q, revision, Some((w.to_string(), h.to_string())), true));
        });
        hints::bind_size_input(&window, q.generation, hints::Owner::SizeInput(revision));
        window.on_header_drag_requested(move |x, y| drag::begin(q, revision, x, y));
        let work = input.clone();
        window.on_escape_pressed(move || work.post(move || escape(q, revision, true)));
        let work = input.clone();
        window.on_escape_released(move || work.post(move || drag::released(q, revision)));
        let work = input.clone();
        window.on_cancel(move || {
            work.post(move || finish(q, revision, None, true));
        });
        let work = input.clone();
        window.window().on_close_requested(move || {
            work.post(move || finish(q, revision, None, false));
            slint::CloseRequestResponse::KeepWindowShown
        });
        let work = input.clone();
        let focused = std::cell::Cell::new(false);
        window.window().on_winit_window_event(move |_, event| {
            if let winit::event::WindowEvent::Focused(active) = event
                && focused.replace(*active)
                && !active
            {
                work.post(move || finish(q, revision, None, false));
            }
            if matches!(event, winit::event::WindowEvent::ScaleFactorChanged { .. }) {
                work.post(move || drag::settle(q, revision));
            }
            EventResult::Propagate
        });
        let state = &mut r.screenshot.as_mut().unwrap().size_input;
        state.pending = None;
        state.popup = Some(Popup {
            window,
            request: q,
            revision,
            input,
            preparation: Default::default(),
            positioned: Default::default(),
            dismissal: r.lifecycle.set_popup_dismissal.clone(),
            cancel_interaction: r.lifecycle.cancel_window_interaction.clone(),
            shown: Default::default(),
            drag: Default::default(),
            draft,
        });
        r.hints.clear();
        r.queue_render();
    });
    show(q, 0);
}
fn show(q: Request, attempt: u8) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(revision) = r
            .screenshot
            .as_ref()
            .and_then(|e| e.size_input.popup.as_ref())
            .map(|p| p.revision)
        else {
            return;
        };
        if !valid(r, q, revision) {
            return;
        }
        let popup = r
            .screenshot
            .as_ref()
            .unwrap()
            .size_input
            .popup
            .as_ref()
            .unwrap();
        if popup.shown.get() {
            return;
        }
        let window = popup.window.window();
        let preparation = (r.prepare)(window);
        let ready = preparation == PassiveWindowPreparation::Ready
            && place(r, q)
            && (r.lifecycle.configure_menu_window)(window)
            && (r.lifecycle.attach_tool_window)(
                window,
                r.canvas_state.canvases[q.index].window.window(),
            );
        if ready && !popup.positioned.replace(true) {
            // Moving a hidden HWND across DPI boundaries settles on the next event turn.
            // Reapply its physical size before exposing the input panel.
            popup.preparation.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(16),
                move || show(q, attempt + 1),
            );
            return;
        }
        if ready {
            let input = popup.input.clone();
            let work = input.clone();
            let weak = popup.window.as_weak();
            let sink = registry_pointer(
                input,
                Rc::new(move |event| {
                    if let PopupPointerInput::DismissRequested { cause } = event {
                        work.post(move || dismiss(q, revision, cause));
                    } else if let Some(window) = weak.upgrade()
                        && let Some(event) = drag::pointer(q, revision, event)
                    {
                        dispatch_pointer_input(window.window(), event);
                    }
                }),
            );
            if popup.window.show().is_err()
                || !(r.lifecycle.configure_menu_window)(window)
                || !(r.lifecycle.complete_passive_window_show)(window, sink)
                || !(r.lifecycle.set_popup_dismissal)(window, true)
            {
                close(r);
                return;
            }
            popup.shown.set(true);
            // Recheck foreground immediately before user-requested activation, once only.
            if permits_focus_activation(r, q.origin) {
                (r.lifecycle.activate_user_requested_window)(window);
                popup.window.invoke_focus_width();
            }
        } else if preparation != PassiveWindowPreparation::Failed && attempt < 20 {
            popup.preparation.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(16),
                move || show(q, attempt + 1),
            );
        } else {
            close(r);
        }
    });
}
fn place(r: &Registry, q: Request) -> bool {
    let p = r
        .screenshot
        .as_ref()
        .unwrap()
        .size_input
        .popup
        .as_ref()
        .unwrap();
    let window = p.window.window();
    let Some(area) = (r.lifecycle.popup_work_area)(Point {
        x: q.hit.bounds.left,
        y: q.hit.bounds.top,
    }) else {
        return false;
    };
    let scale = r.canvas_state.canvases[q.index].scale().max(0.1);
    let size = panel_size(&p.window, scale, Some(area));
    window.set_size(size);
    let gap = (8. * scale).round() as i32;
    let x = q
        .hit
        .bounds
        .left
        .clamp(area.left, (area.right - size.width as i32).max(area.left));
    let below = q.hit.bounds.bottom + gap;
    let y = if below + size.height as i32 <= area.bottom {
        below
    } else {
        q.hit.bounds.top - gap - size.height as i32
    };
    window.set_position(slint::PhysicalPosition::new(
        x,
        y.clamp(area.top, (area.bottom - size.height as i32).max(area.top)),
    ));
    true
}
fn dismiss(q: Request, revision: u64, cause: PopupDismissCause) {
    if cause == PopupDismissCause::Escape {
        escape(q, revision, false);
        return;
    }
    if cause == PopupDismissCause::PointerOutside {
        // Native dismissal can precede delivery of the same press to the canvas.
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut().filter(|r| valid(r, q, revision)) {
                r.screenshot.as_mut().unwrap().size_input.swallow = true;
                let generation = q.generation;
                r.screenshot.as_ref().unwrap().size_input.suppression.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(16),
                    move || {
                        REGISTRY.with(|s| {
                            if let Some(r) = s
                                .borrow_mut()
                                .as_mut()
                                .filter(|r| r.generation == generation)
                                && let Some(e) = r.screenshot.as_mut().filter(|e| e.id == q.session)
                            {
                                e.size_input.swallow = false;
                            }
                        })
                    },
                );
            }
        });
    }
    finish(q, revision, None, cause == PopupDismissCause::Escape);
}
/// Native and Slint Escape share one guard, so cancelling a drag cannot close its panel.
fn escape(q: Request, revision: u64, from_key: bool) {
    if !drag::escape(q, revision, from_key) {
        finish(q, revision, None, true);
    }
}
pub(in crate::annotation) fn hint_window(r: &Registry, revision: u64) -> Option<&slint::Window> {
    let p = r.screenshot.as_ref()?.size_input.popup.as_ref()?;
    (p.revision == revision && valid(r, p.request, revision)).then(|| p.window.window())
}
pub(in crate::annotation) fn dragging(r: &Registry) -> bool {
    r.screenshot
        .as_ref()
        .and_then(|e| e.size_input.popup.as_ref())
        .is_some_and(|p| p.drag.active())
}
fn panel_size(window: &ScreenshotSizeInput, scale: f32, area: Option<Rect>) -> slint::PhysicalSize {
    let width = (window.get_content_width() * scale).round().max(1.) as u32;
    let height = (window.get_content_height() * scale).round().max(1.) as u32;
    slint::PhysicalSize::new(
        area.map_or(width, |a| width.min((a.right - a.left).max(1) as u32)),
        area.map_or(height, |a| height.min((a.bottom - a.top).max(1) as u32)),
    )
}
fn finish(q: Request, revision: u64, values: Option<(String, String)>, restore: bool) {
    let result = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| valid(r, q, revision))?;
        let dimensions = match values {
            Some((w, h)) => Some((integer(&w)?, integer(&h)?)),
            None => None,
        };
        let focus = restore && permits_focus_activation(r, q.origin);
        let e = r.screenshot.as_mut().unwrap();
        let changed = if let Some((w, h)) = dimensions {
            let mut desktop = e.frames[0].bounds;
            for f in e.frames.iter().skip(1) {
                desktop.left = desktop.left.min(f.bounds.left);
                desktop.top = desktop.top.min(f.bounds.top);
                desktop.right = desktop.right.max(f.bounds.right);
                desktop.bottom = desktop.bottom.max(f.bounds.bottom);
            }
            let draft = &e.size_input.popup.as_ref().unwrap().draft;
            e.region
                .apply_size(w, h, draft.aspect.get(), draft.axis.get(), desktop)
        } else {
            false
        };
        let bounds = e.region.bounds()?;
        close(r);
        if changed {
            position_region_toolbar(r, true);
        }
        let target = focus.then(|| {
            (
                r.canvas_state.canvases[q.index].window.clone_strong(),
                r.lifecycle.activate_user_requested_window,
            )
        });
        Some((changed.then_some(bounds), target))
    });
    if let Some((bounds, target)) = result {
        if let Some(bounds) = bounds {
            emit(ScreenshotEvent::Selected {
                session: q.session,
                bounds,
            });
        }
        if let Some((window, activate)) = target {
            let current = REGISTRY.with(|s| {
                s.borrow().as_ref().is_some_and(|r| {
                    r.generation == q.generation
                        && allowed(r)
                        && r.screenshot.as_ref().is_some_and(|e| e.id == q.session)
                        && r.canvas_state
                            .canvases
                            .get(q.index)
                            .is_some_and(|c| c.revision() == q.canvas)
                        && permits_focus_activation(r, q.origin)
                })
            });
            if current {
                activate(window.window());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TestPlatform, lifecycle};
    use super::*;
    use lexift_core::ports::screen_capture::{CapturedRegion, ScreenCaptureFrame};
    use std::sync::Arc;
    const ORIGINAL: Rect = Rect {
        left: 60,
        top: 60,
        right: 180,
        bottom: 150,
    };
    struct Fixture {
        q: Request,
        events: Rc<RefCell<Vec<ScreenshotEvent>>>,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_platform(Box::new(TestPlatform))
        }
        fn with_platform(platform: Box<dyn slint::platform::Platform>) -> Self {
            slint::platform::set_platform(platform).unwrap();
            let mut l = lifecycle();
            l.popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: 0,
                    top: 0,
                    right: 800,
                    bottom: 600,
                })
            });
            super::super::super::init(|_| PassiveWindowPreparation::Ready, l.clone());
            crate::screen_interaction::size_label::prepare(1., &l).unwrap();
            let events = Rc::new(RefCell::new(vec![]));
            let recorded = events.clone();
            super::super::super::set_handler(Rc::new(move |event| {
                if let AppEvent::Screenshot(event) = event {
                    recorded.borrow_mut().push(event);
                }
            }));
            FLOW.with(|s| {
                s.borrow_mut().state = ScreenshotState {
                    session: 51,
                    phase: ScreenshotPhase::Editing,
                    ..Default::default()
                }
            });
            let q = REGISTRY.with(|s| {
                let mut slot = s.borrow_mut();
                let r = slot.as_mut().unwrap();
                r.generation = 42;
                r.document.mode = InteractionMode::Mouse;
                r.document
                    .session
                    .add(super::super::tests::object(Bounds::from_corners(
                        (80., 80.),
                        (100., 100.),
                    )));
                r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
                let bounds = Rect {
                    left: 0,
                    top: 0,
                    right: 800,
                    bottom: 600,
                };
                let window = AnnotationCanvas::new().unwrap();
                window.window().set_size(slint::PhysicalSize::new(800, 600));
                r.canvas_state
                    .canvases
                    .push(CanvasLayer::new(window, bounds));
                let frames = Arc::new(vec![CapturedRegion {
                    bounds,
                    pixels: [120, 160, 180, 255].repeat(800 * 600),
                }]);
                r.screenshot = Some(Editing {
                    pin: None,
                    id: 51,
                    capture: Arc::new(ScreenCaptureFrame {
                        revision: 1,
                        serial: 1,
                        regions: frames.as_ref().clone(),
                    }),
                    frames,
                    region: RegionEditor::new(ORIGINAL),
                    preview: None,
                    revision: 0,
                    accepted_after: 0,
                    image_content: None,
                    presented: 0,
                    feedback_placement: None,
                    toolbar_adjustment: 0,
                    keyboard: Default::default(),
                    initial_focus: None,
                    help: Default::default(),
                    size_input: Default::default(),
                });
                let label = label_feedback(r).unwrap();
                let canvas = r.canvas_state.canvases[0].revision();
                let hit = Hit {
                    crop: label.crop,
                    bounds: label.bounds,
                };
                r.screenshot
                    .as_mut()
                    .unwrap()
                    .size_input
                    .presented
                    .insert(canvas, Some(hit));
                Request {
                    token: window::next_revision(),
                    generation: 42,
                    session: 51,
                    canvas,
                    index: 0,
                    hit,
                    origin: focus::Origin::Unobserved,
                }
            });
            Self { q, events }
        }
        fn open(&self) -> (ScreenshotSizeInput, u64) {
            REGISTRY.with(|s| {
                s.borrow_mut()
                    .as_mut()
                    .unwrap()
                    .screenshot
                    .as_mut()
                    .unwrap()
                    .size_input
                    .pending = Some(self.q)
            });
            open(self.q);
            REGISTRY.with(|s| {
                let slot = s.borrow();
                let p = slot
                    .as_ref()
                    .unwrap()
                    .screenshot
                    .as_ref()
                    .unwrap()
                    .size_input
                    .popup
                    .as_ref()
                    .unwrap();
                (p.window.clone_strong(), p.revision)
            })
        }
        fn bounds(&self) -> Rect {
            REGISTRY.with(|s| {
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .screenshot
                    .as_ref()
                    .unwrap()
                    .region
                    .bounds()
                    .unwrap()
            })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            super::super::super::shutdown();
        }
    }
    #[test]
    fn integer_drafts_are_exact_and_overflow_safe() {
        for s in ["", "-1", "+1", "1.0", " 1", "1e3", "１２"] {
            assert_eq!(integer(s), None);
        }
        assert_eq!(integer("00010"), Some(10));
        assert_eq!(integer("0"), Some(0));
        assert_eq!(integer(&"9".repeat(2000)), Some(u64::MAX));
    }
    #[test]
    fn aspect_drafts_link_only_user_edits_and_apply_atomically() {
        let f = Fixture::new();
        let (w, revision) = f.open();
        w.set_width_text("320".into());
        w.invoke_dimension_edited(0);
        assert_eq!(w.get_height_text(), "90");
        w.set_height_text("200".into());
        w.invoke_lock_requested(true);
        assert!(w.get_locked());
        w.set_width_text("400".into());
        w.invoke_dimension_edited(0);
        assert_eq!(w.get_height_text(), "250");
        w.set_height_text("300".into());
        w.invoke_dimension_edited(1);
        assert_eq!(w.get_width_text(), "480");
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.events.borrow().is_empty());
        finish(
            f.q,
            revision,
            Some((
                w.get_width_text().to_string(),
                w.get_height_text().to_string(),
            )),
            true,
        );
        assert_eq!(
            f.bounds(),
            Rect {
                right: 540,
                bottom: 360,
                ..ORIGINAL
            }
        );
        assert_eq!(f.events.borrow().len(), 1);
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(
                r.screenshot.as_ref().unwrap().region.aspect(),
                Some(AspectRatio::new(8, 5))
            );
            assert_eq!(r.document.session.objects.len(), 1);
            assert!(r.document.session.undo());
            assert!(r.document.session.objects.is_empty());
        });
    }
    #[test]
    fn aspect_only_commit_reopen_and_cancel_preserve_the_session_constraint() {
        let f = Fixture::new();
        let (w, revision) = f.open();
        w.invoke_lock_requested(true);
        finish(f.q, revision, Some(("120".into(), "90".into())), true);
        assert!(f.events.borrow().is_empty());
        assert_eq!(f.bounds(), ORIGINAL);
        let (old, revision) = f.open();
        assert!(old.get_locked());
        old.invoke_lock_requested(false);
        old.set_width_text("200".into());
        old.invoke_dimension_edited(0);
        assert_eq!(old.get_height_text(), "90");
        finish(f.q, revision, None, true);
        let (next, _) = f.open();
        assert!(next.get_locked());
        old.invoke_lock_requested(true);
        old.invoke_dimension_edited(0);
        old.invoke_apply("1".into(), "1".into());
        slint::platform::update_timers_and_animations();
        assert_eq!(next.get_width_text(), "120");
        assert_eq!(f.bounds(), ORIGINAL);
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.set_mode(InteractionMode::Tool(0));
            assert_eq!(
                r.screenshot.as_ref().unwrap().region.aspect(),
                Some(AspectRatio::new(4, 3))
            );
        });
    }
    #[test]
    fn aspect_invalid_recovery_and_relocking_do_not_rewrite_the_active_field() {
        let f = Fixture::new();
        let (w, revision) = f.open();
        w.set_width_text("".into());
        w.invoke_dimension_edited(0);
        w.invoke_lock_requested(true);
        assert!(!w.get_locked());
        w.set_width_text("120".into());
        w.invoke_lock_requested(true);
        w.set_width_text("".into());
        w.invoke_dimension_edited(0);
        assert!(!w.get_valid());
        assert_eq!(w.get_height_text(), "90");
        w.set_width_text("00100".into());
        w.invoke_dimension_edited(0);
        assert!(w.get_valid());
        assert_eq!(w.get_width_text(), "00100");
        assert_eq!(w.get_height_text(), "75");
        w.invoke_lock_requested(false);
        w.set_height_text("50".into());
        w.invoke_dimension_edited(1);
        assert_eq!(w.get_width_text(), "00100");
        w.invoke_lock_requested(true);
        w.set_width_text("200".into());
        w.invoke_dimension_edited(0);
        assert_eq!(w.get_height_text(), "100");
        w.set_width_text("9".repeat(2000).into());
        w.invoke_dimension_edited(0);
        assert_eq!(integer(&w.get_height_text()), Some(9223372036854775808));
        finish(
            f.q,
            revision,
            Some((
                w.get_width_text().to_string(),
                w.get_height_text().to_string(),
            )),
            true,
        );
        assert_eq!(
            f.bounds(),
            Rect {
                right: 800,
                bottom: 430,
                ..ORIGINAL
            }
        );
    }
    #[test]
    fn locked_crop_pointer_routes_all_handles_and_output_uses_confirmed_bounds() {
        let f = Fixture::new();
        let (w, revision) = f.open();
        w.invoke_lock_requested(true);
        finish(f.q, revision, Some(("120".into(), "90".into())), true);
        for (handle, (x, y)) in [
            (60., 60.),
            (120., 60.),
            (180., 60.),
            (60., 105.),
            (180., 105.),
            (60., 150.),
            (120., 150.),
            (180., 150.),
        ]
        .into_iter()
        .enumerate()
        {
            REGISTRY.with(|s| {
                let mut slot = s.borrow_mut();
                let r = slot.as_mut().unwrap();
                // Restore the gesture origin after testing a handle with a different anchor.
                r.screenshot.as_mut().unwrap().region = RegionEditor::new(ORIGINAL);
                r.screenshot.as_mut().unwrap().region.apply_size(
                    120,
                    90,
                    Some(AspectRatio::new(4, 3)),
                    SizeAxis::Width,
                    Rect {
                        left: 0,
                        top: 0,
                        right: 800,
                        bottom: 600,
                    },
                );
                let dx = if matches!(handle, 1 | 6) {
                    0.
                } else if matches!(handle, 0 | 3 | 5) {
                    -40.
                } else {
                    40.
                };
                let dy = if matches!(handle, 3 | 4) {
                    0.
                } else if handle <= 2 {
                    -30.
                } else {
                    30.
                };
                assert!(super::super::pointer(r, 0, 0, x, y));
                assert!(r.screenshot.as_ref().unwrap().region.dragging());
                assert!(super::super::pointer(r, 0, 1, x + dx, y + dy));
                assert!(super::super::pointer(r, 0, 2, x + dx, y + dy));
                let b = r.screenshot.as_ref().unwrap().region.bounds().unwrap();
                assert_eq!((b.right - b.left, b.bottom - b.top), (160, 120));
                assert!(!r.screenshot.as_ref().unwrap().region.dragging());
            });
        }
        let expected = f.bounds();
        output(lexift_core::domain::screenshot::ScreenshotOutput::Copy);
        assert!(f.events.borrow().iter().any(|e|matches!(e,ScreenshotEvent::Output{request,..} if request.snapshot.monitor==expected)));
    }
    #[test]
    fn confirm_is_atomic_clamped_and_does_not_add_document_history() {
        let f = Fixture::new();
        let (w, revision) = f.open();
        assert_eq!(w.get_width_text(), "120");
        assert_eq!(w.get_height_text(), "90");
        w.set_width_text("640".into());
        w.set_height_text("80".into());
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.events.borrow().is_empty());
        finish(
            f.q,
            revision,
            Some(("99999999999999999999999".into(), "0".into())),
            true,
        );
        assert_eq!(
            f.bounds(),
            Rect {
                right: 800,
                bottom: 61,
                ..ORIGINAL
            }
        );
        assert_eq!(f.events.borrow().len(), 1);
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(!busy(r));
            assert_eq!(r.document.session.objects.len(), 1);
            assert!(r.document.session.undo());
            assert!(r.document.session.objects.is_empty());
        });
    }
    #[test]
    fn invalid_cancel_noop_and_old_callback_never_apply() {
        let f = Fixture::new();
        let (old, rev) = f.open();
        old.set_width_text("".into());
        old.invoke_draft_changed("".into(), "90".into());
        assert!(!old.get_valid());
        finish(f.q, rev, Some(("".into(), "80".into())), true);
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(REGISTRY.with(|s| busy(s.borrow().as_ref().unwrap())));
        finish(f.q, rev, None, true);
        assert_eq!(f.bounds(), ORIGINAL);
        let (_, new_rev) = f.open();
        finish(f.q, rev, Some(("1".into(), "1".into())), true);
        assert_eq!(f.bounds(), ORIGINAL);
        finish(f.q, new_rev, Some(("120".into(), "90".into())), true);
        assert!(f.events.borrow().is_empty());
        old.invoke_apply("1".into(), "1".into());
        slint::platform::update_timers_and_animations();
        assert_eq!(f.bounds(), ORIGINAL);
    }
    #[test]
    fn outside_press_cancels_without_beginning_crop_drag_and_mode_change_retires_input() {
        let f = Fixture::new();
        let (w, _) = f.open();
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(pointer(r, 0, 0, 100., 100.));
            assert!(pointer(r, 0, 1, 120., 120.));
            assert!(pointer(r, 0, 2, 120., 120.));
            assert!(!r.screenshot.as_ref().unwrap().region.dragging());
            assert!(!busy(r));
        });
        assert_eq!(f.bounds(), ORIGINAL);
        let (_, _) = f.open();
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.set_mode(InteractionMode::Tool(0));
            assert!(!busy(r));
        });
        w.invoke_apply("1".into(), "1".into());
        slint::platform::update_timers_and_animations();
        assert_eq!(f.bounds(), ORIGINAL);
    }
    #[test]
    fn last_presented_hit_consumes_stale_click_without_editing_new_crop() {
        let f = Fixture::new();
        let p = Point {
            x: f.q.hit.bounds.left + 3,
            y: f.q.hit.bounds.top + 3,
        };
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.screenshot.as_mut().unwrap().region.set_size(
                300,
                300,
                Rect {
                    left: 0,
                    top: 0,
                    right: 800,
                    bottom: 600,
                },
            );
            assert!(pointer(r, 0, 0, p.x as f32, p.y as f32));
            assert!(pointer(r, 0, 2, p.x as f32, p.y as f32));
            assert!(!busy(r));
            assert!(!r.screenshot.as_ref().unwrap().region.dragging());
        });
        assert_eq!(
            f.bounds(),
            Rect {
                right: 360,
                bottom: 360,
                ..ORIGINAL
            }
        );
    }
    #[test]
    fn delayed_frame_metadata_tracks_successful_pixels_and_rejects_old_canvas() {
        let f = Fixture::new();
        let image = tiny_skia::Pixmap::new(800, 600).unwrap();
        let hit = Hit {
            bounds: Rect {
                left: 10,
                top: 10,
                right: 100,
                bottom: 38,
            },
            ..f.q.hit
        };
        super::super::super::frozen_editor::present(
            42,
            51,
            2,
            Some(vec![image.clone()]),
            vec![(f.q.canvas, Some(hit))],
        );
        super::super::super::frozen_editor::present(
            42,
            51,
            1,
            Some(vec![image.clone()]),
            vec![(f.q.canvas, Some(f.q.hit))],
        );
        super::super::super::frozen_editor::present(
            42,
            51,
            3,
            Some(vec![image]),
            vec![(f.q.canvas + 1, None)],
        );
        REGISTRY.with(|s| {
            assert_eq!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .screenshot
                    .as_ref()
                    .unwrap()
                    .size_input
                    .presented[&f.q.canvas],
                Some(hit)
            )
        });
    }
    #[test]
    fn pending_preparation_cannot_reopen_after_cancel_and_output_ignores_draft() {
        let f = Fixture::new();
        REGISTRY.with(|s| {
            s.borrow_mut().as_mut().unwrap().prepare = |_| PassiveWindowPreparation::Pending
        });
        let (_, rev) = f.open();
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(busy(r));
            assert!(close(r));
        });
        show(f.q, 1);
        finish(f.q, rev, Some(("1".into(), "1".into())), true);
        assert_eq!(f.bounds(), ORIGINAL);
        REGISTRY.with(|s| {
            s.borrow_mut().as_mut().unwrap().prepare = |_| PassiveWindowPreparation::Ready
        });
        let (w, _) = f.open();
        w.invoke_lock_requested(true);
        w.set_width_text("1".into());
        output(lexift_core::domain::screenshot::ScreenshotOutput::Copy);
        assert_eq!(f.bounds(), ORIGINAL);
        REGISTRY.with(|s| assert!(!busy(s.borrow().as_ref().unwrap())));
        assert!(f.events.borrow().iter().any(|e| matches!(e,ScreenshotEvent::Output{request,..} if request.snapshot.monitor==ORIGINAL)));
    }
    #[test]
    fn cancelled_popup_restores_help_using_the_latest_pointer() {
        let f = Fixture::new();
        let point = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            let screen = r.canvas_state.canvases[0].bounds;
            crate::screen_interaction::screenshot_help::prepare(1., screen, &r.lifecycle).unwrap();
            super::super::pointer(r, 0, 1, 700., 100.);
            let help = help_feedback(r).unwrap();
            Point {
                x: help.bounds.left + 4,
                y: help.bounds.top + 4,
            }
        });
        let (_, rev) = f.open();
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(help_feedback(r).is_none());
            super::super::pointer(r, 0, 1, point.x as f32, point.y as f32);
        });
        finish(f.q, rev, None, false);
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(
                help_feedback(r).is_none(),
                "closing input must not restore a hint under the current cursor"
            );
            super::super::pointer(r, 0, 1, 700., 100.);
            assert!(help_feedback(r).is_some());
        });
    }

    fn drag_fixture() -> (Fixture, ScreenshotSizeInput, u64) {
        let f = Fixture::with_platform(super::super::tests::position_platform());
        let (window, revision) = f.open();
        show(f.q, 1);
        (f, window, revision)
    }
    fn send_pointer(window: &ScreenshotSizeInput, down: bool, x: f32, y: f32) {
        use slint::platform::{PointerEventButton, WindowEvent};
        window.window().dispatch_event(if down {
            WindowEvent::PointerPressed {
                position: slint::LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            }
        } else {
            WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            }
        });
    }
    #[test]
    fn title_drag_keeps_drafts_and_preview_updates_do_not_reanchor() {
        let (f, w, rev) = drag_fixture();
        let origin = w.window().position();
        w.set_width_text("640".into());
        w.set_height_text("360".into());
        send_pointer(&w, true, 80., 14.);
        REGISTRY.with(|s| assert!(dragging(s.borrow().as_ref().unwrap())));
        drag::pointer(f.q, rev, PopupPointerInput::Moved { x: 180., y: 44. });
        assert_eq!(
            w.window().position(),
            slint::PhysicalPosition::new(origin.x + 100, origin.y + 30)
        );
        drag::pointer(f.q, rev, PopupPointerInput::LeftReleased { x: 80., y: 14. });
        let moved = w.window().position();
        show(f.q, 2);
        drag::settle(f.q, rev);
        assert_eq!(w.window().position(), moved);
        assert_eq!(f.bounds(), ORIGINAL);
        assert_eq!(w.get_width_text(), "640");
        assert_eq!(w.get_height_text(), "360");
        assert!(f.events.borrow().is_empty());
    }
    #[test]
    fn fields_footer_and_close_have_separate_hit_regions() {
        let (f, w, rev) = drag_fixture();
        for (x, y) in [(40., 70.), (160., 70.)] {
            send_pointer(&w, true, x, y);
            REGISTRY.with(|s| assert!(!dragging(s.borrow().as_ref().unwrap())));
            send_pointer(&w, false, x, y);
        }
        // Clicking Apply can confirm a no-op, but must never start a window drag.
        send_pointer(
            &w,
            true,
            w.get_content_width() - 45.,
            w.get_content_height() - 26.,
        );
        REGISTRY.with(|s| assert!(!dragging(s.borrow().as_ref().unwrap())));
        send_pointer(
            &w,
            false,
            w.get_content_width() - 45.,
            w.get_content_height() - 26.,
        );
        slint::platform::update_timers_and_animations();
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.events.borrow().is_empty());
        finish(f.q, rev, None, false);
        let (next, _) = f.open();
        show(f.q, 1);
        send_pointer(&next, true, next.get_content_width() - 16., 14.);
        REGISTRY.with(|s| assert!(!dragging(s.borrow().as_ref().unwrap())));
        send_pointer(&next, false, next.get_content_width() - 16., 14.);
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|s| assert!(!busy(s.borrow().as_ref().unwrap())));
    }
    #[test]
    fn cross_dpi_drag_clamps_on_capture_loss_and_preserves_geometry() {
        let (f, w, rev) = drag_fixture();
        let cursor = Rc::new(std::cell::Cell::new(Some(Point { x: 500, y: 220 })));
        let query = cursor.clone();
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .toolbar_cursor_position = Rc::new(move || query.get())
        });
        w.invoke_header_drag_requested(80., 14.);
        drag::pointer(f.q, rev, PopupPointerInput::Moved { x: 0., y: 0. });
        w.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: 1.25,
            });
        drag::settle(f.q, rev);
        assert_eq!(
            w.window().position(),
            slint::PhysicalPosition::new(400, 202)
        );
        assert_eq!(w.window().size(), panel_size(&w, 1.25, None));
        REGISTRY.with(|s| {
            s.borrow_mut().as_mut().unwrap().lifecycle.popup_work_area = Rc::new(|_| {
                Some(Rect {
                    left: -800,
                    top: -100,
                    right: 0,
                    bottom: 600,
                })
            })
        });
        cursor.set(Some(Point { x: -795, y: 100 }));
        drag::pointer(f.q, rev, PopupPointerInput::Moved { x: 0., y: 0. });
        drag::pointer(f.q, rev, PopupPointerInput::LeftCancelled { x: 0., y: 0. });
        assert_eq!(w.window().position().x, -800);
        REGISTRY.with(|s| assert!(!dragging(s.borrow().as_ref().unwrap())));
        assert_eq!(f.bounds(), ORIGINAL);
        assert!(f.events.borrow().is_empty());
    }
    #[test]
    fn escape_routes_cancel_one_drag_then_allow_panel_cancel_after_release() {
        let (f, w, rev) = drag_fixture();
        let origin = w.window().position();
        w.invoke_header_drag_requested(50., 14.);
        drag::pointer(f.q, rev, PopupPointerInput::Moved { x: 150., y: 54. });
        dismiss(f.q, rev, PopupDismissCause::Escape);
        escape(f.q, rev, true);
        assert_eq!(w.window().position(), origin);
        REGISTRY.with(|s| {
            let slot = s.borrow();
            let r = slot.as_ref().unwrap();
            assert!(busy(r));
            assert!(!dragging(r));
        });
        drag::released(f.q, rev);
        std::thread::sleep(Duration::from_millis(25));
        slint::platform::update_timers_and_animations();
        escape(f.q, rev, true);
        REGISTRY.with(|s| assert!(!busy(s.borrow().as_ref().unwrap())));
        assert_eq!(f.bounds(), ORIGINAL);
    }
    #[test]
    fn closing_drag_releases_capture_and_old_components_cannot_move_a_replacement() {
        let f = Fixture::with_platform(super::super::tests::position_platform());
        let cancelled = Rc::new(std::cell::Cell::new(0));
        let count = cancelled.clone();
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .cancel_window_interaction = Rc::new(move |_| {
                count.set(count.get() + 1);
                true
            })
        });
        let (old, rev) = f.open();
        show(f.q, 1);
        old.invoke_header_drag_requested(40., 14.);
        REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(dragging(r));
            close(r);
        });
        assert_eq!(cancelled.get(), 1);
        let (next, _) = f.open();
        show(f.q, 1);
        let pos = next.window().position();
        old.invoke_header_drag_requested(40., 14.);
        assert!(drag::pointer(f.q, rev, PopupPointerInput::Moved { x: 200., y: 200. }).is_none());
        assert_eq!(next.window().position(), pos);
        REGISTRY.with(|s| assert!(!dragging(s.borrow().as_ref().unwrap())));
    }

    #[test]
    fn render_size_panel_in_all_languages_themes_and_scales() {
        use lexift_core::domain::{settings::ThemePreference, ui_language::UiLanguage};
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        for language in UiLanguage::ALL {
            crate::i18n::select(language);
            for (dark, theme) in [
                (false, ThemePreference::Light),
                (true, ThemePreference::Dark),
            ] {
                crate::theme::set_current(theme);
                for scale in [1., 1.25] {
                    let window = ScreenshotSizeInput::new().unwrap();
                    crate::theme::apply(&window);
                    window.set_width_text("1920".into());
                    window.set_height_text("1080".into());
                    window.set_locked(true);
                    window.window().dispatch_event(
                        slint::platform::WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        },
                    );
                    window.window().set_size(panel_size(&window, scale, None));
                    let image = window.window().take_snapshot().unwrap();
                    assert_eq!(
                        image.width(),
                        (window.get_content_width() * scale).round() as u32
                    );
                    assert_eq!(
                        image.height(),
                        (window.get_content_height() * scale).round() as u32
                    );
                    assert!(
                        window.get_content_width() >= 288. && window.get_content_width() <= 340.
                    );
                    assert!(window.get_content_height() <= 174.);
                    assert!(image.as_slice().iter().any(|p| p.a > 0 && p.r != p.g));
                    if std::env::var_os("LEXIFT_SIZE_INPUT_RENDER").is_some() {
                        crate::i18n::write_test_bitmap(
                            concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/../../target/screenshot-aspect/render"
                            ),
                            &format!("{}-{}-{}", language.code(), dark, (scale * 100.) as i32),
                            image.width(),
                            image.height(),
                            image.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                        );
                    }
                }
            }
        }
        crate::i18n::select(UiLanguage::EnglishUs);
        crate::theme::set_current(ThemePreference::Light);
    }
}
