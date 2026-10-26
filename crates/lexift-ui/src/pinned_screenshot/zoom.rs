//! Instance-owned zoom and bounded raster viewport. It never changes image documents.
use super::*;
mod raster;
use crate::annotation::image_viewport::PreciseViewport;
use lexift_core::domain::geometry::{Point, Rect};
use raster::raster;

#[derive(Default)]
pub(super) struct View {
    normal: Option<Normal>,
    pub edit: Option<Edit>,
    wheel: f64,
}
#[derive(Clone, Copy)]
struct Normal {
    source: Rect,
    display: Rect,
    center: [f64; 2],
    scale: f64,
}
pub(super) struct Edit {
    pub view: PreciseViewport,
    returning: Normal,
    host_position: slint::PhysicalPosition,
    source: Rect,
    frame: Option<Arc<CapturedRegion>>,
    controls: Option<tiny_skia::Pixmap>,
    crop: Option<PreciseViewport>,
    cancelled: bool,
    over: bool,
    cursor_active: bool,
    pub space: bool,
    pub last_tool: Option<usize>,
}
fn extent(r: Rect) -> [f64; 2] {
    [f64::from(r.right - r.left), f64::from(r.bottom - r.top)]
}
pub(crate) fn active(id: u64) -> bool {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .is_some_and(|p| p.view.edit.is_some())
    })
}
fn content(r: Rect) -> [f64; 4] {
    [r.left, r.top, r.right, r.bottom].map(f64::from)
}
fn center(r: Rect) -> [f64; 2] {
    [
        f64::from(r.left) + extent(r)[0] / 2.,
        f64::from(r.top) + extent(r)[1] / 2.,
    ]
}
fn normal(p: &Pin, display: Rect) -> Normal {
    p.view
        .normal
        .filter(|v| v.source == p.visible)
        .map(|mut v| {
            v.center[0] += f64::from(display.left - v.display.left);
            v.center[1] += f64::from(display.top - v.display.top);
            v.display = display;
            v
        })
        .unwrap_or(Normal {
            source: p.visible,
            display,
            center: center(display),
            scale: extent(display)[0] / extent(p.visible)[0],
        })
}
pub(crate) fn normal_transform(id: u64, source: Rect) -> Option<bool> {
    let (_, display) = super::geometry(id)?;
    let ctx = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        if p.view.edit.is_some() {
            return None;
        }
        let w = p.window.get()?;
        let old = normal(p, display);
        let inset = (w.get_frame_inset() * native_scale(w.window())).round() as i32;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: old.center[0] as i32,
            y: old.center[1] as i32,
        })?;
        let rect = rectangle(source, old.scale, old.center)?;
        Some((
            old,
            inset,
            area,
            rect,
            r.lifecycle.pin_window_budget,
            f64::from(native_scale(w.window())),
        ))
    })?;
    let (old, inset, area, rect, budget, dpi) = ctx;
    if !within(rect, inset, budget, area) {
        // Unverified renderers retain the existing work-area-fitting rotation.
        return if budget.is_none() { None } else { Some(false) };
    }
    let rect = accessible(rect, area, dpi);
    let success = super::set_geometry(
        id,
        source,
        slint::PhysicalPosition::new(rect.left - inset, rect.top - inset),
        slint::PhysicalSize::new(
            (rect.right - rect.left + 2 * inset) as u32,
            (rect.bottom - rect.top + 2 * inset) as u32,
        ),
    );
    if success {
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
                p.view.normal = Some(Normal {
                    source,
                    display: rect,
                    center: [
                        old.center[0] + f64::from(rect.left - ctx.3.left),
                        old.center[1] + f64::from(rect.top - ctx.3.top),
                    ],
                    scale: old.scale,
                });
            }
        });
    }
    Some(success)
}
/// A window may cross work-area edges, but an accessible grip always remains.
fn accessible(mut rect: Rect, area: Rect, dpi: f64) -> Rect {
    let size = extent(rect);
    let grip = (64. * dpi).round().max(1.);
    let visible =
        std::array::from_fn::<_, 2, _>(|i| size[i].min(grip).min(extent(area)[i].max(1.)));
    let x = (f64::from(rect.left)).clamp(
        f64::from(area.left) - size[0] + visible[0],
        f64::from(area.right) - visible[0],
    );
    let y = (f64::from(rect.top)).clamp(
        f64::from(area.top) - size[1] + visible[1],
        f64::from(area.bottom) - visible[1],
    );
    rect.left = x.round() as i32;
    rect.top = y.round() as i32;
    rect.right = rect.left + size[0] as i32;
    rect.bottom = rect.top + size[1] as i32;
    rect
}
fn rectangle(source: Rect, scale: f64, center: [f64; 2]) -> Option<Rect> {
    let s = extent(source).map(|v| (v * scale).round().max(1.));
    if !scale.is_finite()
        || scale <= 0.
        || s.iter()
            .any(|v| !v.is_finite() || *v > i32::MAX as f64 / 4.)
    {
        return None;
    }
    let x = (center[0] - s[0] / 2.).round();
    let y = (center[1] - s[1] / 2.).round();
    if [x, y]
        .iter()
        .any(|v| !v.is_finite() || v.abs() > i32::MAX as f64 / 4.)
    {
        return None;
    }
    Some(Rect {
        left: x as i32,
        top: y as i32,
        right: x as i32 + s[0] as i32,
        bottom: y as i32 + s[1] as i32,
    })
}
fn within(rect: Rect, inset: i32, budget: Option<(u32, u64)>, area: Rect) -> bool {
    let [w, h] = extent(rect).map(|v| v as u64 + 2 * inset as u64);
    budget.map_or(
        w <= extent(area)[0] as u64 && h <= extent(area)[1] as u64,
        |(edge, pixels)| w <= u64::from(edge) && h <= u64::from(edge) && w * h <= pixels,
    )
}
/// Initial fit is not an explicit requested multiplier, so it may reduce to the native budget.
pub(super) fn initial_size(
    size: slint::PhysicalSize,
    inset: i32,
    budget: Option<(u32, u64)>,
) -> slint::PhysicalSize {
    let Some(_) = budget else { return size };
    let source = Rect {
        left: 0,
        top: 0,
        right: size.width as i32,
        bottom: size.height as i32,
    };
    if within(source, inset, budget, source) {
        return size;
    }
    let mut lo = 0.;
    let mut hi = 1.;
    for _ in 0..48 {
        let mid = (lo + hi) / 2.;
        if rectangle(source, mid, [0., 0.]).is_some_and(|r| within(r, inset, budget, source)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let r = rectangle(source, lo, [0., 0.]).unwrap_or(Rect {
        left: 0,
        top: 0,
        right: 1,
        bottom: 1,
    });
    slint::PhysicalSize::new((r.right - r.left) as u32, (r.bottom - r.top) as u32)
}
fn allowed(id: u64) -> bool {
    visibility::operable(id)
        && crate::annotation::pin_editor::interaction_idle(id)
        && STATE.with(|s| {
            s.borrow().as_ref().is_some_and(|r| {
                r.interactive
                    && r.windows.get(&id).is_some_and(|p| {
                        p.phase == Phase::Visible
                            && !p.native_drag
                            && !p.gesture.pressed.get()
                            && !p.interaction.applied.locked
                            && !p.interaction.applied.passthrough
                            && p.interaction.pending.is_none()
                            && p.window.get().is_some_and(|w| {
                                w.get_zoom_enabled()
                                    && !w.get_cropping()
                                    && !w.get_output_busy()
                                    && !w.get_recognizing()
                                    && !w.get_transform_busy()
                            })
                    })
            })
        })
}
pub(super) fn sync(state: &lexift_core::AppState) {
    let values = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| {
                r.windows
                    .iter()
                    .filter_map(|(&id, p)| p.window.get().map(|w| (id, w.clone_strong())))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    for (id, w) in values {
        w.set_zoom_enabled(lexift_core::domain::pinned_image::transform::available(
            state, id,
        ));
        refresh_menu(id);
    }
}
fn refresh_menu(id: u64) {
    let Some((source, display)) = super::geometry(id) else {
        return;
    };
    STATE.with(|s| {
        let s = s.borrow();
        let Some(r) = s.as_ref() else { return };
        let Some(p) = r.windows.get(&id) else { return };
        let Some(w) = p.window.get() else { return };
        let n = normal(p, display);
        let scale = p.view.edit.as_ref().map_or(n.scale, |e| e.view.scale);
        w.set_zoom_percent((scale * 100.) as f32);
        let inset = (w.get_frame_inset() * native_scale(w.window())).round() as i32;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: n.center[0] as i32,
            y: n.center[1] as i32,
        })
        .unwrap_or(display);
        let flags = [0.25, 0.5, 0.75, 1., 1.5, 2.].map(|z| {
            if let Some(e) = &p.view.edit {
                let mut v = e.view.clone();
                v.zoom_at(v.size.map(|x| x / 2.), z)
            } else {
                rectangle(source, z, n.center)
                    .is_some_and(|rect| within(rect, inset, r.lifecycle.pin_window_budget, area))
            }
        });
        w.set_zoom_presets(slint::ModelRc::new(slint::VecModel::from(flags.to_vec())));
    });
}
pub(super) fn request(id: u64, percent: i32) {
    if !allowed(id) || !crate::annotation::pin_editor::prepare_interaction(id) {
        return;
    }
    if STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .is_some_and(|p| p.view.edit.is_some())
    }) {
        let mut failed = false;
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
                && let Some(e) = p.view.edit.as_mut()
            {
                if percent == 0 {
                    e.view.fit(1.);
                } else {
                    failed = !e
                        .view
                        .zoom_at(e.view.size.map(|x| x / 2.), f64::from(percent) / 100.);
                }
            }
        });
        if failed {
            precision_limit(id);
        } else {
            redraw(id);
        }
    } else {
        resize_normal(id, percent, None);
    }
    refresh_menu(id);
}
fn precision_limit(id: u64) {
    show_feedback(
        id,
        "Zoom limit reached for accurate annotation coordinates.",
        0,
        None,
    );
}
fn limit(id: u64) {
    show_feedback(
        id,
        "Zoom limit reached. Use the editor to view details.",
        0,
        None,
    );
}
fn resize_normal(id: u64, percent: i32, anchor: Option<([f64; 2], f64)>) {
    let Some((source, display)) = super::geometry(id) else {
        return;
    };
    let proposal = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        let w = p.window.get()?;
        let n = normal(p, display);
        let dpi = f64::from(native_scale(w.window()));
        let inset = (f64::from(w.get_frame_inset()) * dpi).round() as i32;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: n.center[0] as i32,
            y: n.center[1] as i32,
        })?;
        let mut c = n.center;
        let z = if let Some((point, factor)) = anchor {
            let z = n.scale * factor;
            c = std::array::from_fn(|i| point[i] + (n.center[i] - point[i]) * z / n.scale);
            z
        } else if percent == 0 {
            c = center(area);
            ((extent(area)[0] - 2. * f64::from(inset) - 16. * dpi) / extent(source)[0])
                .min((extent(area)[1] - 2. * f64::from(inset) - 16. * dpi) / extent(source)[1])
                .clamp(f64::MIN_POSITIVE, 1.)
        } else {
            f64::from(percent) / 100.
        };
        let z = z.max(1. / extent(source).into_iter().fold(1., f64::max));
        let rect = rectangle(source, z, c)?;
        if !within(rect, inset, r.lifecycle.pin_window_budget, area) {
            return None;
        }
        Some((
            Normal {
                source,
                display: accessible(rect, area, dpi),
                center: c,
                scale: z,
            },
            inset,
        ))
    });
    let Some((mut n, inset)) = proposal else {
        if anchor.is_none() {
            limit(id);
        }
        return;
    };
    let unclamped = rectangle(source, n.scale, n.center).unwrap();
    n.center[0] += f64::from(n.display.left - unclamped.left);
    n.center[1] += f64::from(n.display.top - unclamped.top);
    if super::set_geometry(
        id,
        source,
        slint::PhysicalPosition::new(n.display.left - inset, n.display.top - inset),
        slint::PhysicalSize::new(
            (n.display.right - n.display.left + 2 * inset) as u32,
            (n.display.bottom - n.display.top + 2 * inset) as u32,
        ),
    ) {
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
                p.view.normal = Some(n);
            }
        });
    } else {
        show_feedback(id, "Operation failed. Try again.", 0, None);
    }
}
pub(super) fn wheel(id: u64, delta: f32, x: f32, y: f32) {
    if !allowed(id) || !delta.is_finite() || delta == 0. {
        return;
    }
    let editing = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .is_some_and(|p| p.view.edit.is_some())
    });
    if editing {
        if !crate::annotation::pin_editor::browse(id) {
            return;
        }
        let mut limited = false;
        let mut changed = false;
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
                && let (Some(e), Some(w)) = (&mut p.view.edit, p.window.get())
            {
                let dpi = f64::from(native_scale(w.window()));
                changed = e.view.wheel(
                    [
                        (f64::from(x) - f64::from(w.get_frame_inset())) * dpi,
                        (f64::from(y) - f64::from(w.get_frame_inset())) * dpi,
                    ],
                    f64::from(delta),
                );
                limited = e.view.limited;
            }
        });
        if changed {
            redraw(id);
        }
        if limited {
            precision_limit(id);
        }
    } else {
        let ctx = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let p = s.as_mut()?.windows.get_mut(&id)?;
            let w = p.window.get()?;
            p.view.wheel += f64::from(delta);
            let steps = (p.view.wheel / 60.).trunc().clamp(-128., 128.);
            p.view.wheel -= steps * 60.;
            let dpi = f64::from(native_scale(w.window()));
            let pos = w.window().position();
            Some((
                [
                    f64::from(pos.x) + f64::from(x) * dpi,
                    f64::from(pos.y) + f64::from(y) * dpi,
                ],
                steps,
            ))
        });
        if let Some((anchor, steps)) = ctx
            && steps != 0.
        {
            resize_normal(id, -1, Some((anchor, 1.1_f64.powf(steps))));
        }
    }
    refresh_menu(id);
}
pub(super) fn begin(id: u64) {
    let Some((source, display)) = super::geometry(id) else {
        return;
    };
    let ctx = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        let w = p.window.get()?;
        let n = normal(p, display);
        let inset = (w.get_frame_inset() * native_scale(w.window())).round() as i32;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: n.center[0] as i32,
            y: n.center[1] as i32,
        })?;
        let size = [
            extent(display)[0]
                .min(extent(area)[0] * 0.9 - 2. * f64::from(inset))
                .max(1.),
            extent(display)[1]
                .min(extent(area)[1] * 0.9 - 2. * f64::from(inset))
                .max(1.),
        ];
        let rect = rectangle(
            Rect {
                left: 0,
                top: 0,
                right: size[0] as i32,
                bottom: size[1] as i32,
            },
            1.,
            n.center,
        )?;
        let rect = Rect {
            left: rect.left.clamp(
                area.left + inset,
                (area.right - inset - size[0] as i32).max(area.left + inset),
            ),
            top: rect.top.clamp(
                area.top + inset,
                (area.bottom - inset - size[1] as i32).max(area.top + inset),
            ),
            ..rect
        };
        Some((n, inset, size, rect, w.clone_strong()))
    });
    let Some((returning, inset, size, rect, w)) = ctx else {
        return;
    };
    if !super::set_geometry(
        id,
        source,
        slint::PhysicalPosition::new(rect.left - inset, rect.top - inset),
        slint::PhysicalSize::new(
            size[0] as u32 + 2 * inset as u32,
            size[1] as u32 + 2 * inset as u32,
        ),
    ) {
        return;
    }
    let mut view = PreciseViewport::new(content(source), size);
    view.fit(returning.scale);
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
            let frame = Some(p.image.clone());
            view.precision = p.document.as_ref().and_then(|d| {
                d.snapshot
                    .session
                    .image_source
                    .map(|full| (full, d.snapshot.session.image_orientation))
            });
            p.view.edit = Some(Edit {
                view,
                returning,
                host_position: w.window().position(),
                source,
                frame,
                controls: None,
                crop: None,
                cancelled: false,
                over: false,
                cursor_active: false,
                space: false,
                last_tool: None,
            });
        }
    });
    w.set_viewport_active(true);
    w.set_navigating(true);
    w.set_interactive(true);
    redraw(id);
}
pub(super) fn end(id: u64) {
    let ctx = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        let p = r.windows.get_mut(&id)?;
        let w = p.window.get()?.clone_strong();
        let edit = p.view.edit.take()?;
        Some((edit, w, r.lifecycle.image_pan_cursor.clone()))
    });
    if let Some((edit, w, cursor)) = ctx {
        cursor(w.window(), None);
        if let Some(frame) = edit.frame {
            w.set_picture(display_picture(
                &w,
                &frame.pixels,
                (frame.bounds.right - frame.bounds.left) as u32,
                (frame.bounds.bottom - frame.bounds.top) as u32,
            ));
        }
        let n = edit.returning;
        w.set_viewport_active(false);
        w.set_navigating(false);
        let inset = (w.get_frame_inset() * native_scale(w.window())).round() as i32;
        if super::set_geometry(
            id,
            n.source,
            slint::PhysicalPosition::new(n.display.left - inset, n.display.top - inset),
            slint::PhysicalSize::new(
                (n.display.right - n.display.left + 2 * inset) as u32,
                (n.display.bottom - n.display.top + 2 * inset) as u32,
            ),
        ) {
            STATE.with(|s| {
                if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
                    p.view.normal = Some(n);
                }
            });
        }
    }
}
/// New document geometry is installed only on prepared preview completion.
pub(crate) fn reframe(id: u64, source: Rect, rotated: bool) -> Option<bool> {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        let e = p.view.edit.as_ref()?;
        let w = p.window.get()?;
        Some((
            e.returning,
            r.lifecycle.popup_work_area.clone(),
            r.lifecycle.pin_window_budget,
            (w.get_frame_inset() * native_scale(w.window())).round() as i32,
            f64::from(native_scale(w.window())),
        ))
    })?;
    let (old, work_area, budget, inset, dpi) = context;
    let area = work_area(Point {
        x: old.center[0] as i32,
        y: old.center[1] as i32,
    })?;
    // A crop can expand from a small selection to a complete 32768px source.
    // Returning from local editing must still honor the native allocation budget.
    let mut z = old.scale;
    let mut bounds = rectangle(source, z, old.center)?;
    if !within(bounds, inset, budget, area) {
        let mut lo = 0.;
        let mut hi = z;
        for _ in 0..48 {
            let mid = (lo + hi) / 2.;
            if rectangle(source, mid, old.center).is_some_and(|r| within(r, inset, budget, area)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        z = lo;
        bounds = rectangle(source, z, old.center)?;
        if !within(bounds, inset, budget, area) {
            return Some(false);
        }
    }
    let c = if rotated {
        old.center
    } else {
        [
            f64::from(old.display.left) + extent(source)[0] * z / 2.,
            f64::from(old.display.top) + extent(source)[1] * z / 2.,
        ]
    };
    let Some(rect) = rectangle(source, z, c) else {
        return Some(false);
    };
    let display = accessible(rect, area, dpi);
    let returning = Normal {
        source,
        display,
        center: [
            c[0] + f64::from(display.left - rect.left),
            c[1] + f64::from(display.top - rect.top),
        ],
        scale: z,
    };
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
            && let Some(e) = p.view.edit.as_mut()
        {
            e.returning = returning;
            e.source = source;
            e.view.content = content(source);
            e.view.fit(1.);
            e.crop = None;
            p.visible = source;
        }
    });
    Some(true)
}
pub(crate) fn crop_begin(id: u64, full: Rect) -> Option<bool> {
    let mut active = false;
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
            && let Some(e) = p.view.edit.as_mut()
        {
            active = true;
            if e.crop.is_none() {
                e.crop = Some(e.view.clone());
            }
            e.source = full;
            e.view.content = content(full);
            e.view.fit(1.);
            p.visible = full;
        }
    });
    active.then_some(true)
}
pub(crate) fn crop_cancel(id: u64) {
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
            && let Some(e) = p.view.edit.as_mut()
            && let Some(v) = e.crop.take()
        {
            e.view = v;
            e.source = e.returning.source;
            p.visible = e.source;
        }
    });
    redraw(id);
}
pub(crate) fn crop_feedback(id: u64, b: Rect) -> Option<([f64; 2], [f64; 2], [f64; 2])> {
    STATE.with(|s| {
        let s = s.borrow();
        let e = s.as_ref()?.windows.get(&id)?.view.edit.as_ref()?;
        Some((
            e.view.screen([f64::from(b.left), f64::from(b.top)]),
            e.view.screen([f64::from(b.right), f64::from(b.bottom)]),
            e.view.size,
        ))
    })
}
pub(crate) fn precision(
    id: u64,
    full: Rect,
    orientation: lexift_core::domain::image_transform::Orientation,
) {
    STATE.with(|s| {
        if let Some(e) = s
            .borrow_mut()
            .as_mut()
            .and_then(|r| r.windows.get_mut(&id))
            .and_then(|p| p.view.edit.as_mut())
        {
            e.view.precision = Some((full, orientation));
        }
    });
}
pub(crate) fn scale(id: u64) -> Option<f64> {
    STATE.with(|s| {
        s.borrow()
            .as_ref()?
            .windows
            .get(&id)?
            .view
            .edit
            .as_ref()
            .map(|e| e.view.scale)
    })
}
pub(crate) fn map(id: u64, point: [f64; 2], reverse: bool) -> Option<[f64; 2]> {
    STATE.with(|s| {
        let s = s.borrow();
        let p = s.as_ref()?.windows.get(&id)?;
        let w = p.window.get()?;
        let e = p.view.edit.as_ref()?;
        let pos = w.window().position();
        let inset = (w.get_frame_inset() * native_scale(w.window())).round();
        let origin = [
            f64::from(pos.x) + f64::from(inset),
            f64::from(pos.y) + f64::from(inset),
        ];
        Some(if reverse {
            e.view.source(std::array::from_fn(|i| point[i] - origin[i]))
        } else {
            let q = e.view.screen(point);
            std::array::from_fn(|i| q[i] + origin[i])
        })
    })
}
pub(crate) fn local(id: u64, p: [f64; 2]) -> Option<[f64; 2]> {
    STATE.with(|s| {
        s.borrow()
            .as_ref()?
            .windows
            .get(&id)?
            .view
            .edit
            .as_ref()
            .map(|e| e.view.source(p))
    })
}
pub(crate) fn contains(id: u64, p: [f64; 2]) -> bool {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .and_then(|p| p.view.edit.as_ref())
            .is_none_or(|e| e.view.contains(p))
    })
}
pub(crate) fn pointer(id: u64, kind: i32, point: [f64; 2]) -> bool {
    let result = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.as_mut()?.windows.get_mut(&id)?;
        let e = p.view.edit.as_mut()?;
        let old = e.view.offset;
        match kind {
            0 => {
                e.view.begin_pan(point);
                e.cancelled = false;
            }
            1 if !e.cancelled => e.view.pan_to(point),
            2 | 3 => {
                e.view.finish_pan();
                e.cancelled = false;
            }
            _ => {}
        }
        e.over = kind != 4 && (0..2).all(|i| point[i] >= 0. && point[i] < e.view.size[i]);
        Some(old != e.view.offset)
    });
    if let Some(changed) = result {
        if changed {
            redraw(id);
        } else {
            refresh_cursor(id);
        }
        true
    } else {
        false
    }
}
pub(super) fn refresh_cursor(id: u64) {
    let target = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        let p = r.windows.get_mut(&id)?;
        let w = p.window.get()?;
        let e = p.view.edit.as_mut()?;
        let active = w.get_navigating()
            && !w.get_cropping()
            && !w.get_recognizing()
            && (e.over || e.view.panning());
        let update = active || e.cursor_active;
        e.cursor_active = active;
        update.then(|| {
            (
                r.lifecycle.image_pan_cursor.clone(),
                w.clone_strong(),
                active.then_some(e.view.panning()),
            )
        })
    });
    if let Some((cursor, w, shape)) = target {
        cursor(w.window(), shape);
    }
}
/// Release our hand before the annotation cursor bridge installs a drawing shape.
pub(crate) fn release_cursor(id: u64) {
    let target = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        let p = r.windows.get_mut(&id)?;
        let e = p.view.edit.as_mut()?;
        if !e.cursor_active {
            return None;
        }
        e.cursor_active = false;
        Some((
            r.lifecycle.image_pan_cursor.clone(),
            p.window.get()?.clone_strong(),
        ))
    });
    if let Some((cursor, w)) = target {
        cursor(w.window(), None);
    }
}
pub(crate) fn cancel_pan(id: u64) -> bool {
    let cancelled = STATE
        .with(|s| {
            let mut s = s.borrow_mut();
            let e = s.as_mut()?.windows.get_mut(&id)?.view.edit.as_mut()?;
            if e.cancelled {
                return Some(true);
            }
            let changed = e.view.cancel_pan();
            e.cancelled = changed;
            Some(changed)
        })
        .unwrap_or(false);
    if cancelled {
        redraw(id);
    }
    cancelled
}
pub(crate) fn lost_focus(id: u64) {
    STATE.with(|s| {
        if let Some(e) = s
            .borrow_mut()
            .as_mut()
            .and_then(|r| r.windows.get_mut(&id))
            .and_then(|p| p.view.edit.as_mut())
        {
            e.view.finish_pan();
            e.cancelled = false;
            e.space = false;
        }
    });
    redraw(id);
}
pub(crate) fn panning(id: u64) -> bool {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .and_then(|p| p.view.edit.as_ref())
            .is_some_and(|e| e.view.panning() || e.cancelled)
    })
}
pub(crate) fn mode(id: u64, browse: bool, tool: Option<usize>) {
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
            if let Some(e) = p.view.edit.as_mut()
                && let Some(t) = tool
            {
                e.last_tool = Some(t);
            }
            if let Some(w) = p.window.get() {
                w.set_navigating(browse);
            }
        }
    });
    refresh_cursor(id);
}
pub(crate) fn space(id: u64, down: bool) -> Option<Option<usize>> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.as_mut()?.windows.get_mut(&id)?;
        let e = p.view.edit.as_mut()?;
        if !down {
            e.space = false;
            return None;
        }
        if e.space {
            return None;
        }
        e.space = true;
        Some(if p.window.get()?.get_navigating() {
            Some(e.last_tool.unwrap_or(usize::MAX))
        } else {
            None
        })
    })
}
/// Install candidate content without resetting the instance-owned view or controls.
pub(super) fn replace_content(id: u64, frame: Arc<CapturedRegion>) -> bool {
    let installed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(e) = s
            .as_mut()
            .and_then(|r| r.windows.get_mut(&id))
            .and_then(|p| p.view.edit.as_mut())
        else {
            return false;
        };
        if e.source != frame.bounds {
            return false;
        }
        e.frame = Some(frame);
        true
    });
    if installed {
        redraw(id);
    }
    installed
}
pub(super) fn present(id: u64, frame: tiny_skia::Pixmap, controls: tiny_skia::Pixmap) -> bool {
    let active = STATE.with(|s| {
        if let Some(e) = s
            .borrow_mut()
            .as_mut()
            .and_then(|r| r.windows.get_mut(&id))
            .and_then(|p| p.view.edit.as_mut())
        {
            if frame.width() != (e.source.right - e.source.left) as u32
                || frame.height() != (e.source.bottom - e.source.top) as u32
            {
                return false;
            }
            e.frame = Some(Arc::new(CapturedRegion {
                bounds: e.source,
                pixels: frame.take(),
            }));
            e.controls = Some(controls);
            true
        } else {
            false
        }
    });
    if active {
        redraw(id);
    }
    active
}
pub(super) fn redraw(id: u64) {
    let ready = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        let p = r.windows.get_mut(&id)?;
        let w = p.window.get()?;
        let e = p.view.edit.as_mut()?;
        let dpi = f64::from(native_scale(w.window()));
        let inset = (f64::from(w.get_frame_inset()) * dpi).round();
        let size = w.window().size();
        e.view.resize([
            (f64::from(size.width) - 2. * inset).max(1.),
            (f64::from(size.height) - 2. * inset).max(1.),
        ]);
        let origin = [
            f64::from(e.frame.as_ref().map_or(e.source, |f| f.bounds).left),
            f64::from(e.frame.as_ref().map_or(e.source, |f| f.bounds).top),
        ];
        if let Some(frame) = &e.frame
            && let Some(image) = raster(
                tiny_skia::PixmapRef::from_bytes(
                    &frame.pixels,
                    (frame.bounds.right - frame.bounds.left) as u32,
                    (frame.bounds.bottom - frame.bounds.top) as u32,
                )
                .unwrap(),
                &e.view,
                origin,
                Some(w.global::<crate::Colors>().get_surface()),
            )
        {
            w.set_picture(image);
        }
        if let Some(controls) = &e.controls
            && let Some(image) = raster(controls.as_ref(), &e.view, origin, None)
        {
            w.set_decoration(image);
        }
        w.set_zoom_percent((e.view.scale * 100.) as f32);
        Some(())
    });
    if ready.is_some() {
        refresh_cursor(id);
    }
}
pub(super) fn settled(id: u64) -> bool {
    if STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .is_some_and(|p| p.view.edit.is_some())
    }) {
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
                && let (Some(e), Some(w)) = (&mut p.view.edit, p.window.get())
            {
                let pos = w.window().position();
                let delta = [pos.x - e.host_position.x, pos.y - e.host_position.y];
                e.returning.display.left += delta[0];
                e.returning.display.right += delta[0];
                e.returning.display.top += delta[1];
                e.returning.display.bottom += delta[1];
                e.returning.center[0] += f64::from(delta[0]);
                e.returning.center[1] += f64::from(delta[1]);
                e.host_position = pos;
            }
        });
        redraw(id);
        return true;
    }
    let Some((source, display)) = super::geometry(id) else {
        return false;
    };
    let ctx = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        let w = p.window.get()?;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: center(display)[0] as i32,
            y: center(display)[1] as i32,
        })?;
        let target = accessible(display, area, f64::from(native_scale(w.window())));
        Some((
            target,
            (w.get_frame_inset() * native_scale(w.window())).round() as i32,
        ))
    });
    if let Some((target, inset)) = ctx {
        if target != display
            && !super::set_geometry(
                id,
                source,
                slint::PhysicalPosition::new(target.left - inset, target.top - inset),
                slint::PhysicalSize::new(
                    (target.right - target.left + 2 * inset) as u32,
                    (target.bottom - target.top + 2 * inset) as u32,
                ),
            )
        {
            return false;
        }
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
                p.view.normal = Some(normal(p, target));
            }
        });
        refresh_menu(id);
        return true;
    }
    false
}

/// Winit may preserve logical size on a monitor transition. Restore physical image scale.
pub(super) fn dpi_changed(id: u64) {
    let candidate = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        let w = p.window.get()?;
        if p.phase != Phase::Visible {
            return None;
        }
        let inset = (w.get_frame_inset() * native_scale(w.window())).round() as i32;
        if let Some(e) = &p.view.edit {
            return Some((
                p.visible,
                w.window().position(),
                slint::PhysicalSize::new(
                    e.view.size[0] as u32 + 2 * inset as u32,
                    e.view.size[1] as u32 + 2 * inset as u32,
                ),
                None,
            ));
        }
        let mut n = p.view.normal?;
        let pos = w.window().position();
        n.center[0] += f64::from(pos.x + p.prepared_inset - n.display.left);
        n.center[1] += f64::from(pos.y + p.prepared_inset - n.display.top);
        n.display = rectangle(n.source, n.scale, n.center)?;
        Some((
            n.source,
            slint::PhysicalPosition::new(n.display.left - inset, n.display.top - inset),
            slint::PhysicalSize::new(
                (n.display.right - n.display.left + 2 * inset) as u32,
                (n.display.bottom - n.display.top + 2 * inset) as u32,
            ),
            Some(n),
        ))
    });
    if let Some((source, pos, size, n)) = candidate
        && super::set_geometry(id, source, pos, size)
    {
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
                && let Some(n) = n
            {
                p.view.normal = Some(n);
            }
        });
    }
    settled(id);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }
    #[test]
    fn native_budget_does_not_silently_fit_requested_zoom() {
        let fitted = initial_size(
            slint::PhysicalSize::new(16000, 8000),
            10,
            Some((8192, 16_000_000)),
        );
        assert!(within(
            rect(0, 0, fitted.width as i32, fitted.height as i32),
            10,
            Some((8192, 16_000_000)),
            rect(0, 0, 16000, 8000)
        ));
        let source = rect(-300, -40, 4000, 2000);
        let area = rect(-1920, 0, 1920, 1080);
        let big = rectangle(source, 1., [-900., 400.]).unwrap();
        assert!(within(big, 10, Some((8192, 16_000_000)), area));
        assert!(!within(big, 10, None, area));
        assert!(!within(
            rect(0, 0, 8000, 2000),
            10,
            Some((8192, 16_000_000)),
            area
        ));
        assert!(!within(
            rect(0, 0, 9000, 1),
            10,
            Some((8192, 16_000_000)),
            area
        ));
        assert_eq!(extent(big), [4000., 2000.]);
        assert!(rectangle(source, f64::NAN, [0., 0.]).is_none());
        assert!(rectangle(source, 1e20, [0., 0.]).is_none());
    }
    #[test]
    fn center_odd_sizes_and_offscreen_access_are_stable() {
        let small = accessible(rect(-100, -100, 200, 200), rect(0, 0, 40, 40), 1.25);
        assert!(small.left <= 0 && small.top <= 0 && small.right >= 40 && small.bottom >= 40);
        let area = rect(-1920, -100, 1920, 1080);
        let c = [-800.5, 400.];
        for scale in [0.25, 0.5, 0.75, 1., 1.5, 2.] {
            let a = rectangle(rect(0, 0, 101, 63), scale, c).unwrap();
            for _ in 0..20 {
                let _ = rectangle(rect(0, 0, 63, 101), scale, c);
                assert_eq!(rectangle(rect(0, 0, 101, 63), scale, c), Some(a));
            }
        }
        let b = accessible(rect(-10000, -10000, 4000, 2000), area, 1.25);
        assert_eq!(extent(b), [4000., 2000.]);
        assert_eq!(b.right, area.left + 80);
        assert_eq!(b.bottom, area.top + 80);
        let tiny = accessible(rect(500, 500, 1, 1), area, 1.);
        assert!(tiny.right <= area.right && tiny.bottom <= area.bottom);
    }
    #[test]
    fn raster_is_bounded_at_extreme_zoom_and_respects_source_crop() {
        let mut source = tiny_skia::Pixmap::new(400, 1200).unwrap();
        source.fill(tiny_skia::Color::from_rgba8(80, 120, 200, 255));
        let mut view = PreciseViewport::new([-100., -30., 300., 1170.], [320., 240.]);
        assert!(view.zoom_at([160., 120.], 100.));
        let image = raster(source.as_ref(), &view, [-100., -30.], None).unwrap();
        assert_eq!((image.size().width, image.size().height), (320, 240));
        view.content = [-20., -10., 40., 100.];
        view.fit(1.);
        assert_eq!(
            raster(source.as_ref(), &view, [-100., -30.], None)
                .unwrap()
                .size(),
            image.size()
        );
        view.resize([8192., 8192.]);
        assert!(raster(source.as_ref(), &view, [-100., -30.], None).is_none());
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct Backend(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for Backend {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(w.clone());
            Ok(w)
        }
    }
    #[test]
    fn viewport_workspace_and_catalog_render_all_languages_themes_and_dpis() {
        let captured = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(Backend(captured.clone()))).unwrap();
        let w = PinnedScreenshotWindow::new().unwrap();
        let adapter = captured.borrow().as_ref().unwrap().clone();
        w.set_editing(true);
        w.set_viewport_active(true);
        w.set_navigating(true);
        w.set_zoom_enabled(true);
        w.set_zoom_percent(200.);
        let mut source = tiny_skia::Pixmap::new(400, 1200).unwrap();
        for (n, p) in source.pixels_mut().iter_mut().enumerate() {
            let x = n % 400;
            let y = n / 400;
            *p = tiny_skia::PremultipliedColorU8::from_rgba(
                (x % 200 + 30) as u8,
                (y % 160 + 50) as u8,
                180,
                255,
            )
            .unwrap();
        }
        let mut view = PreciseViewport::new([-100., -30., 300., 1170.], [320., 240.]);
        view.zoom_at([160., 120.], 2.);
        view.begin_pan([0., 0.]);
        view.pan_to([350., 30.]);
        view.finish_pan();
        for lang in lexift_core::domain::ui_language::UiLanguage::ALL {
            crate::i18n::select(lang);
            for key in [
                "Zoom: {}%",
                "{}% (window limit)",
                "Fit to work area",
                "Wheel: zoom · Drag: pan · Space: edit/pan",
            ] {
                assert!(!crate::i18n::tr(key).is_empty());
            }
            for dark in [false, true] {
                for dpi in [1., 1.25] {
                    w.global::<crate::Colors>().set_preference(i32::from(dark));
                    adapter
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    let width = 320 + (16. * dpi) as u32;
                    let height = 240 + (16. * dpi) as u32;
                    w.window().set_size(slint::PhysicalSize::new(width, height));
                    w.set_picture(
                        raster(
                            source.as_ref(),
                            &view,
                            [-100., -30.],
                            Some(w.global::<crate::Colors>().get_surface()),
                        )
                        .unwrap(),
                    );
                    w.show().unwrap();
                    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                    adapter.request_redraw();
                    adapter.draw_if_needed(|renderer| {
                        renderer.render(&mut pixels, width as usize);
                    });
                    assert!(pixels.iter().any(|p| i32::from(p.b) - i32::from(p.r) > 10));
                    if let Ok(dir) = std::env::var("LEXIFT_PIN_ZOOM_RENDER_DIR") {
                        crate::i18n::write_test_bitmap(
                            &dir,
                            &format!("viewport-{}-{dark}-{dpi}", lang.code()),
                            width,
                            height,
                            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                }
            }
        }
    }
}
