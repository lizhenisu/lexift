//! View-only crop drafts in source pixel coordinates.
use super::*;

pub(super) struct Draft {
    full: FrameCrop,
    area: FrameCrop,
    drag: Option<Drag>,
}

struct Drag {
    original: FrameCrop,
    origin: (u32, u32),
    // -1 and 1 identify the near and far edges on each axis.
    edges: Option<(i8, i8)>,
}

fn label(w: &RecordingPreview, crop: &Draft, failed: bool) {
    let (width, height) = crop.area.size();
    w.set_crop_valid(crop.area.valid(crop.full.right, crop.full.bottom));
    let size = format!("{width} × {height} px");
    w.set_crop_label(if !w.get_crop_valid() {
        format!(
            "{size} · {} 16 × 16 px",
            crate::i18n::tr("Minimum crop size")
        )
        .into()
    } else if failed {
        format!("{size} · {}", crate::i18n::tr("Could not apply video crop")).into()
    } else {
        size.into()
    });
}

pub(super) fn sync(state: &p::State) {
    let guide = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        let w = v.window.get()?.clone_strong();
        w.set_crop_editing(state.frame_crop_editing);
        if !state.frame_crop_editing {
            v.crop = None;
            return None;
        }
        let fresh = v.crop.is_none();
        let full = FrameCrop::full(state.source_size.0, state.source_size.1);
        let draft = v.crop.get_or_insert_with(|| Draft {
            full,
            area: state.frame_crop.unwrap_or(full),
            drag: None,
        });
        label(&w, draft, state.frame_crop_error);
        fresh.then_some(draft.area)
    });
    if let Some(guide) = guide {
        let identity = state.identity;
        slint::Timer::single_shot(Duration::ZERO, move || {
            if valid(identity) {
                input(p::Input::CropGuide(guide));
            }
        });
    }
}

pub(super) fn action(action: i32) {
    tracing::debug!(action, "Recording frame crop action");
    let event = match action {
        13 => p::Input::BeginFrameCrop,
        14 => p::Input::CancelFrameCrop,
        15 | 16 => {
            let Some(event) = VIEWS.with(|v| {
                let mut v = v.borrow_mut();
                let w = v.window.get()?.clone_strong();
                let draft = v.crop.as_mut()?;
                if action == 15 {
                    draft.area = draft.full;
                    draft.drag = None;
                    label(&w, draft, false);
                    Some(p::Input::CropGuide(draft.area))
                } else {
                    draft.drag = None;
                    Some(p::Input::SetFrameCrop(
                        (draft.area != draft.full).then_some(draft.area),
                    ))
                }
            }) else {
                return;
            };
            event
        }
        _ => return,
    };
    input(event);
}

fn edge(value: f32, near: u32, far: u32, hit: f32) -> i8 {
    let distance = (value - near as f32, far as f32 - value);
    if distance.0.abs() <= hit && distance.0.abs() <= distance.1.abs() {
        -1
    } else if distance.1.abs() <= hit {
        1
    } else {
        0
    }
}

fn axis(v: u32, origin: u32, near: u32, far: u32, limit: u32, edge: i8) -> (u32, u32) {
    match edge {
        -1 => (v.min(far - 2), far),
        1 => (near, v.max(near + 2)),
        _ => {
            let span = far - near;
            let left = (i64::from(near) + i64::from(v) - i64::from(origin))
                .clamp(0, i64::from(limit - span)) as u32;
            (left, left + span)
        }
    }
}

pub(super) fn pointer(kind: i32, x: f32, y: f32) {
    if kind == 3 {
        return;
    }
    let guide = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        let w = v.window.get()?.clone_strong();
        let draft = v.crop.as_mut()?;
        let (sw, sh) = draft.full.size();
        if sw < 2 || sh < 2 {
            return None;
        }
        let dpi = w.window().scale_factor();
        let (vw, vh) = (
            (w.get_video_width() * dpi).round(),
            (w.get_video_height() * dpi).round(),
        );
        let scale = (vw / sw as f32).min(vh / sh as f32);
        let (rw, rh) = ((sw as f32 * scale).round(), (sh as f32 * scale).round());
        if rw < 1. || rh < 1. {
            return None;
        }
        let px = (x * dpi - ((vw - rw) / 2.).floor()) * sw as f32 / rw;
        let py = (y * dpi - ((vh - rh) / 2.).floor()) * sh as f32 / rh;
        let snap = |value: f32, max: u32| ((value / 2.).round() * 2.).clamp(0., max as f32) as u32;
        let (x, y) = (snap(px, sw), snap(py, sh));
        if kind == 0 {
            if px < 0. || py < 0. || px > sw as f32 || py > sh as f32 {
                return None;
            }
            let r = draft.area;
            let hit = 7. * dpi / scale;
            let edges = (px >= r.left as f32 - hit
                && px <= r.right as f32 + hit
                && py >= r.top as f32 - hit
                && py <= r.bottom as f32 + hit
                && r != draft.full)
                .then(|| {
                    (
                        edge(px, r.left, r.right, hit),
                        edge(py, r.top, r.bottom, hit),
                    )
                });
            draft.drag = Some(Drag {
                original: r,
                origin: (x, y),
                edges,
            });
            return None;
        }
        let Drag {
            original: r,
            origin: (ox, oy),
            edges,
        } = *draft.drag.as_ref()?;
        draft.area = if let Some((a, b)) = edges {
            let (left, right) = axis(
                if a == 0 && b != 0 { ox } else { x },
                ox,
                r.left,
                r.right,
                sw,
                a,
            );
            let (top, bottom) = axis(
                if b == 0 && a != 0 { oy } else { y },
                oy,
                r.top,
                r.bottom,
                sh,
                b,
            );
            FrameCrop {
                left,
                top,
                right,
                bottom,
            }
        } else if x != ox && y != oy {
            FrameCrop {
                left: x.min(ox),
                top: y.min(oy),
                right: x.max(ox),
                bottom: y.max(oy),
            }
        } else {
            r
        };
        if kind == 2 {
            draft.drag = None;
        }
        label(&w, draft, false);
        Some(draft.area)
    });
    if let Some(guide) = guide {
        input(p::Input::CropGuide(guide));
    }
}
