//! Client-capture dragging shares the toolbar's physical/DPI-aware grip transform.
use super::*;

#[derive(Default)]
pub(super) struct State {
    current: Option<toolbar::Drag>,
    escape_guard: bool,
    escape_held: bool,
    escape_timer: slint::Timer,
}
impl State {
    pub fn active(&self) -> bool {
        self.current.is_some()
    }
}
fn popup(r: &mut Registry) -> &mut Popup {
    r.screenshot
        .as_mut()
        .unwrap()
        .size_input
        .popup
        .as_mut()
        .unwrap()
}
pub(super) fn begin(q: Request, revision: u64, x: f32, y: f32) {
    REGISTRY.with(|s| {
        let Ok(mut slot) = s.try_borrow_mut() else {
            return;
        };
        let Some(r) = slot.as_mut().filter(|r| valid(r, q, revision)) else {
            return;
        };
        let p = popup(r);
        if !p.shown.get() || p.drag.active() {
            return;
        }
        let window = p.window.window();
        let scale = window.scale_factor().max(0.1);
        p.drag.current = Some(toolbar::Drag::new(
            window.position(),
            x * scale,
            y * scale,
            scale,
        ));
        r.hints.clear();
    });
}
fn area(r: &Registry, point: slint::PhysicalPosition) -> Option<Rect> {
    (r.lifecycle.popup_work_area)(Point {
        x: point.x,
        y: point.y,
    })
}
fn fit(r: &mut Registry, constrain: bool) {
    let p = popup(r);
    let w = p.window.window();
    let pos = w.position();
    let size = w.size();
    let center = slint::PhysicalPosition::new(
        pos.x.saturating_add((size.width / 2) as i32),
        pos.y.saturating_add((size.height / 2) as i32),
    );
    let area = area(r, center);
    let p = popup(r);
    let w = p.window.window();
    let size = panel_size(&p.window, w.scale_factor().max(0.1), area);
    if w.size() != size {
        w.set_size(size);
    }
    if constrain && let Some(a) = area {
        w.set_position(clamp(w.position(), size, a));
    }
}
fn clamp(
    pos: slint::PhysicalPosition,
    size: slint::PhysicalSize,
    a: Rect,
) -> slint::PhysicalPosition {
    slint::PhysicalPosition::new(
        pos.x
            .clamp(a.left, (a.right - size.width as i32).max(a.left)),
        pos.y
            .clamp(a.top, (a.bottom - size.height as i32).max(a.top)),
    )
}
fn move_to_pointer(r: &mut Registry, fallback: Option<(f32, f32)>) {
    let window = popup(r).window.window();
    let pos = window.position();
    let cursor = (r.lifecycle.toolbar_cursor_position)()
        .map(|p| slint::PhysicalPosition::new(p.x, p.y))
        .or_else(|| {
            fallback.map(|(x, y)| {
                slint::PhysicalPosition::new(
                    pos.x.saturating_add(x.round() as i32),
                    pos.y.saturating_add(y.round() as i32),
                )
            })
        });
    let Some(cursor) = cursor else { return };
    let a = area(r, cursor);
    let p = popup(r);
    let w = p.window.window();
    let size = panel_size(&p.window, w.scale_factor().max(0.1), a);
    if w.size() != size {
        w.set_size(size);
    }
    let Some(drag) = p.drag.current else { return };
    let target = drag.position(cursor, w.scale_factor().max(0.1));
    if w.position() != target {
        w.set_position(target);
    }
    sync_owner(
        r,
        Point {
            x: cursor.x,
            y: cursor.y,
        },
    );
}
fn sync_owner(r: &Registry, point: Point) {
    if let Some(index) = canvas_at(r.canvas_state.canvases.iter().map(|c| c.bounds), point) {
        let p = r
            .screenshot
            .as_ref()
            .unwrap()
            .size_input
            .popup
            .as_ref()
            .unwrap();
        (r.lifecycle.attach_tool_window)(
            p.window.window(),
            r.canvas_state.canvases[index].window.window(),
        );
    }
}

pub(super) fn pointer(
    q: Request,
    revision: u64,
    event: PopupPointerInput,
) -> Option<PopupPointerInput> {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| valid(r, q, revision))?;
        if !popup(r).drag.active() {
            return Some(event);
        }
        let grip = popup(r).drag.current.unwrap();
        match event {
            PopupPointerInput::Moved { x, y } => move_to_pointer(r, Some((x, y))),
            PopupPointerInput::LeftReleased { x, y } => {
                move_to_pointer(r, Some((x, y)));
                popup(r).drag.current = None;
                fit(r, true);
            }
            PopupPointerInput::LeftCancelled { .. } => {
                popup(r).drag.current = None;
                fit(r, true);
            }
            _ => {}
        }
        let (x, y) = grip.client_position(popup(r).window.window().scale_factor());
        Some(match event {
            PopupPointerInput::Moved { .. } => PopupPointerInput::Moved { x, y },
            PopupPointerInput::LeftReleased { .. } => PopupPointerInput::LeftReleased { x, y },
            PopupPointerInput::LeftCancelled { .. } => PopupPointerInput::LeftCancelled { x, y },
            other => other,
        })
    })
}
/// DPI notification is event-driven. A settled/moved panel never returns to its label anchor.
pub(super) fn settle(q: Request, revision: u64) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| valid(r, q, revision)) else {
            return;
        };
        if !popup(r).shown.get() {
            return;
        }
        if popup(r).drag.active() {
            move_to_pointer(r, None);
        } else {
            fit(r, true);
        }
    });
}
fn clear_escape_later(r: &mut Registry, q: Request, revision: u64) {
    popup(r).drag.escape_timer.start(
        slint::TimerMode::SingleShot,
        Duration::from_millis(16),
        move || {
            REGISTRY.with(|s| {
                if let Some(r) = s.borrow_mut().as_mut().filter(|r| valid(r, q, revision)) {
                    let p = popup(r);
                    if !p.drag.escape_held {
                        p.drag.escape_guard = false;
                    }
                }
            });
        },
    );
}
/// Returns true when this Escape belongs to the current drag (including duplicate routes).
pub(super) fn escape(q: Request, revision: u64, from_key: bool) -> bool {
    let (consumed, cancel) = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| valid(r, q, revision)) else {
            return (true, None);
        };
        let p = popup(r);
        if from_key {
            p.drag.escape_held = true;
        }
        if let Some(drag) = p.drag.current.take() {
            p.drag.escape_guard = true;
            p.window.window().set_position(drag.origin);
            let cancel = Some((p.window.clone_strong(), p.cancel_interaction.clone()));
            sync_owner(
                r,
                Point {
                    x: drag.origin.x,
                    y: drag.origin.y,
                },
            );
            clear_escape_later(r, q, revision);
            (true, cancel)
        } else {
            (p.drag.escape_guard, None)
        }
    });
    // Native cancellation may reenter input delivery; the Registry is free here.
    if let Some((window, cancel)) = cancel {
        cancel(window.window());
    }
    consumed
}
pub(super) fn released(q: Request, revision: u64) {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut().filter(|r| valid(r, q, revision)) {
            popup(r).drag.escape_held = false;
            if popup(r).drag.escape_guard {
                clear_escape_later(r, q, revision);
            }
        }
    });
}
