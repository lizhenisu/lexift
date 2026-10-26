//! Install a direction and its input document only with the matching prepared pixels.
use super::*;
use lexift_core::domain::{
    annotation_export::ExportRequest,
    geometry::{Point, Rect},
    pinned_image::{Event, States, transform as domain},
};

struct Pending {
    revision: u64,
    instance: u64,
    document: ExportRequest,
    before: ExportRequest,
    image: Arc<CapturedRegion>,
    position: slint::PhysicalPosition,
    size: slint::PhysicalSize,
    installed: bool,
}
#[derive(Clone, Copy)]
struct Placement {
    rect: Rect,
    source: Rect,
    center: [f64; 2],
    zoom: f64,
}
thread_local! {
    static PENDING: RefCell<BTreeMap<u64, Pending>> = const { RefCell::new(BTreeMap::new()) };
    static PLACEMENT: RefCell<BTreeMap<u64, Placement>> = const { RefCell::new(BTreeMap::new()) };
    static SETTLED: RefCell<BTreeMap<u64, u64>> = const { RefCell::new(BTreeMap::new()) };
}
pub(super) fn forget(id: u64) {
    PENDING.with(|s| s.borrow_mut().remove(&id));
    PLACEMENT.with(|s| s.borrow_mut().remove(&id));
    SETTLED.with(|s| s.borrow_mut().remove(&id));
}
pub(super) fn color_request(id: u64, index: i32) {
    let Some(action) = [
        lexift_core::domain::image_color::Action::Grayscale,
        lexift_core::domain::image_color::Action::Invert,
    ]
    .get(index as usize)
    .copied() else {
        return;
    };
    request_color_action(id, action);
}
pub(super) fn brightness_request(id: u64, value: i32) {
    let Some(level) = lexift_core::domain::image_color::Brightness::new(value) else {
        return;
    };
    request_color_action(
        id,
        lexift_core::domain::image_color::Action::Brightness(level),
    );
}
fn request_color_action(id: u64, action: lexift_core::domain::image_color::Action) {
    let allowed = visibility::operable(id)
        && !super::zoom::panning(id)
        && STATE.with(|s| {
            s.borrow().as_ref().is_some_and(|r| {
                r.windows.get(&id).is_some_and(|p| {
                    !p.native_drag
                        && !p.gesture.pressed.get()
                        && p.window
                            .get()
                            .is_some_and(|w| w.get_color_enabled() && !w.get_output_busy())
                })
            })
        });
    if allowed && crate::annotation::pin_editor::prepare_recognition(id) {
        emit_pin(Event::ColorRequested { id, action });
    }
}
pub(super) fn request(id: u64, index: i32) {
    let action = [
        lexift_core::domain::image_transform::Action::Left,
        lexift_core::domain::image_transform::Action::Right,
        lexift_core::domain::image_transform::Action::Horizontal,
        lexift_core::domain::image_transform::Action::Vertical,
    ]
    .get(index as usize)
    .copied();
    let allowed = visibility::operable(id)
        && STATE.with(|s| {
            s.borrow().as_ref().is_some_and(|r| {
                r.windows.get(&id).is_some_and(|p| {
                    !p.native_drag
                        && !p.gesture.pressed.get()
                        && p.window.get().is_some_and(|w| {
                            w.get_transform_enabled() && !w.get_output_busy() && !w.get_locked()
                        })
                })
            })
        });
    if !allowed || !crate::annotation::pin_editor::prepare_recognition(id) {
        return;
    }
    if let Some(action) = action {
        emit_pin(Event::TransformRequested { id, action });
    }
}
pub(super) fn sync(states: &States) {
    let retired = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let ids: Vec<_> = pending
            .iter()
            .filter_map(|(&id, p)| {
                (!states.get(&id).is_some_and(|s| {
                    s.revision < p.revision || (s.transforming && s.revision == p.revision)
                }))
                .then_some(id)
            })
            .collect();
        ids.into_iter()
            .filter_map(|id| pending.remove(&id).map(|p| (id, p)))
            .collect::<Vec<_>>()
    });
    for (id, p) in retired {
        SETTLED.with(|s| {
            let mut s = s.borrow_mut();
            let v = s.entry(id).or_default();
            *v = (*v).max(p.revision);
        });
        if p.installed
            && states
                .get(&id)
                .is_some_and(|s| s.document.as_ref() == Some(&p.before))
        {
            let current = STATE.with(|s| {
                s.borrow()
                    .as_ref()
                    .and_then(|r| r.windows.get(&id))
                    .is_some_and(|w| w.phase == Phase::Visible && w.window.revision() == p.instance)
            });
            if current {
                if p.document.snapshot.session.image_orientation
                    != p.before.snapshot.session.image_orientation
                    || p.document.snapshot.session.image_crop
                        != p.before.snapshot.session.image_crop
                {
                    super::set_geometry(id, p.image.bounds, p.position, p.size);
                }
                crate::annotation::pin_editor::install_direction(id, &p.before);
                let viewport_installed = super::zoom::replace_content(id, p.image.clone());
                STATE.with(|s| {
                    if let Some(pin) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
                    {
                        if let Some(w) = pin.window.get() {
                            if !viewport_installed {
                                set_picture(w, &p.image);
                            }
                            w.set_decoration(slint::Image::default());
                        }
                        pin.visible = p.image.bounds;
                        pin.image = p.image;
                    }
                });
                PLACEMENT.with(|s| s.borrow_mut().remove(&id));
            }
        }
    }
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        STATE.with(|slot| {
            if let Some(r) = slot.borrow().as_ref() {
                for (&id, p) in &r.windows {
                    let Some(s) = states.get(&id) else { continue };
                    if s.revision < p.revision {
                        continue;
                    }
                    if let Some(w) = p.window.get() {
                        let color = s
                            .document
                            .as_ref()
                            .map_or(Default::default(), |d| d.snapshot.session.image_color);
                        w.set_grayscale(color.grayscale);
                        w.set_inverted(color.inverted);
                        w.set_brightness(i32::from(color.brightness.value()));
                        w.set_transform_busy(s.transforming);
                        w.set_transform_enabled(
                            !p.user_hidden
                                && !r.frozen
                                && !s.busy
                                && !s.transforming
                                && s.proposed.is_none()
                                && s.recognition.is_none()
                                && s.interaction.pending.is_none()
                                && !s.interaction.applied.locked
                                && !s.interaction.applied.passthrough,
                        );
                    }
                    if s.transforming
                        && let Some(document) = &s.proposed
                        && SETTLED.with(|s| s.borrow().get(&id).is_none_or(|v| *v < p.revision))
                    {
                        pending.entry(id).or_insert_with(|| Pending {
                            revision: s.revision,
                            instance: p.window.revision(),
                            document: document.clone(),
                            before: s.document.clone().unwrap(),
                            image: p.image.clone(),
                            position: p.window.get().unwrap().window().position(),
                            size: p.window.get().unwrap().window().size(),
                            installed: false,
                        });
                    }
                }
            }
        });
    });
}
pub(crate) fn project(state: &lexift_core::AppState) {
    super::zoom::sync(state);
    STATE.with(|s| {
        if let Some(r) = s.borrow().as_ref() {
            for (&id, p) in &r.windows {
                if let Some(w) = p.window.get().filter(|_| {
                    state
                        .pinned_images
                        .get(&id)
                        .is_some_and(|s| s.revision >= p.revision)
                }) {
                    w.set_transform_enabled(domain::available(state, id));
                    w.set_color_enabled(domain::color_available(state, id));
                }
            }
        }
    });
}
/// Pixel dimensions are physical; retaining the fractional center avoids odd/even drift.
fn centered(center: [f64; 2], zoom: f64, bounds: Rect, area: Rect) -> (Rect, f64) {
    let width = f64::from(bounds.right - bounds.left);
    let height = f64::from(bounds.bottom - bounds.top);
    let zoom = zoom
        .min(f64::from(area.right - area.left) / width)
        .min(f64::from(area.bottom - area.top) / height);
    let w = (width * zoom).round().max(1.) as i32;
    let h = (height * zoom).round().max(1.) as i32;
    let x = ((center[0] - f64::from(w) / 2.).round() as i32)
        .clamp(area.left, (area.right - w).max(area.left));
    let y = ((center[1] - f64::from(h) / 2.).round() as i32)
        .clamp(area.top, (area.bottom - h).max(area.top));
    (
        Rect {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        },
        zoom,
    )
}
pub(crate) fn geometry(id: u64, bounds: Rect) -> bool {
    if let Some(result) = super::zoom::reframe(id, bounds, true) {
        return result;
    }
    if let Some(result) = super::zoom::normal_transform(id, bounds) {
        return result;
    }
    let Some((source, display)) = super::geometry(id) else {
        return false;
    };
    let old = PLACEMENT.with(|s| {
        s.borrow()
            .get(&id)
            .copied()
            .filter(|p| p.rect == display && p.source == source)
    });
    let center = old.map_or(
        [
            f64::from(display.left) + f64::from(display.right - display.left) / 2.,
            f64::from(display.top) + f64::from(display.bottom - display.top) / 2.,
        ],
        |p| p.center,
    );
    let zoom = old.map_or(
        f64::from(display.right - display.left) / f64::from(source.right - source.left),
        |p| p.zoom,
    );
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let w = r.windows.get(&id)?.window.get()?;
        let inset = (w.get_frame_inset() * w.window().scale_factor()).round() as i32;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: center[0].round() as i32,
            y: center[1].round() as i32,
        })?;
        Some((
            inset,
            Rect {
                left: area.left + inset,
                top: area.top + inset,
                right: area.right - inset,
                bottom: area.bottom - inset,
            },
        ))
    });
    let Some((inset, area)) = context.filter(|(_, a)| a.right > a.left && a.bottom > a.top) else {
        return false;
    };
    let (rect, zoom) = centered(center, zoom, bounds, area);
    if !super::set_geometry(
        id,
        bounds,
        slint::PhysicalPosition::new(rect.left - inset, rect.top - inset),
        slint::PhysicalSize::new(
            (rect.right - rect.left + 2 * inset) as u32,
            (rect.bottom - rect.top + 2 * inset) as u32,
        ),
    ) {
        return false;
    }
    PLACEMENT.with(|s| {
        s.borrow_mut().insert(
            id,
            Placement {
                rect,
                source: bounds,
                center,
                zoom,
            },
        )
    });
    true
}
pub(super) fn present(id: u64, revision: u64, image: &CapturedRegion) -> Option<bool> {
    let pending = PENDING.with(|s| s.borrow_mut().remove(&id));
    let Some(mut p) = pending else {
        return SETTLED
            .with(|s| s.borrow().get(&id).is_some_and(|v| *v >= revision))
            .then_some(false);
    };
    let current = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .is_some_and(|w| {
                w.phase == Phase::Visible
                    && w.revision == revision
                    && w.window.revision() == p.instance
            })
    });
    if p.revision != revision || !current {
        PENDING.with(|s| s.borrow_mut().insert(id, p));
        return Some(false);
    }
    if p.installed {
        PENDING.with(|s| s.borrow_mut().insert(id, p));
        return Some(false);
    }
    SETTLED.with(|s| s.borrow_mut().insert(id, revision));
    if p.document.snapshot.session.image_source.and_then(|full| {
        p.document
            .snapshot
            .session
            .image_orientation
            .rect(p.document.snapshot.monitor, full)
            .ok()
    }) != Some(image.bounds)
    {
        emit_pin(Event::PreviewFailed {
            id,
            revision,
            error: crate::i18n::tr("Operation failed. Try again."),
        });
        return Some(false);
    }
    let geometry_changed = p.document.snapshot.session.image_orientation
        != p.before.snapshot.session.image_orientation
        || p.document.snapshot.session.image_crop != p.before.snapshot.session.image_crop;
    if geometry_changed && !geometry(id, image.bounds) {
        emit_pin(Event::PreviewFailed {
            id,
            revision,
            error: crate::i18n::tr("Operation failed. Try again."),
        });
        return Some(false);
    }
    crate::annotation::pin_editor::install_direction(id, &p.document);
    p.installed = true;
    let request = p.document.request;
    PENDING.with(|s| s.borrow_mut().insert(id, p));
    slint::Timer::single_shot(Duration::ZERO, move || {
        emit_pin(Event::TransformAccepted {
            id,
            revision,
            request,
        })
    });
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn odd_sizes_keep_the_fractional_center_across_repeated_axis_swaps() {
        let area = Rect {
            left: -1920,
            top: -20,
            right: 2560,
            bottom: 1400,
        };
        let wide = Rect {
            left: -10,
            top: 30,
            right: 91,
            bottom: 92,
        };
        let tall = Rect {
            left: -10,
            top: 30,
            right: 52,
            bottom: 131,
        };
        for scale in [0.37, 0.8, 1., 1.25] {
            let center = [-400.5, 500.];
            let (initial, _) = centered(center, scale, wide, area);
            for _ in 0..40 {
                let (turned, z) = centered(center, scale, tall, area);
                assert_eq!(turned.right - turned.left, (62. * scale).round() as i32);
                assert_eq!(centered(center, z, wide, area).0, initial);
            }
        }
    }
    #[test]
    fn work_area_limits_only_reduce_scale_and_include_negative_origins() {
        let area = Rect {
            left: -1910,
            top: 10,
            right: -10,
            bottom: 1070,
        };
        let big = Rect {
            left: 50,
            top: 90,
            right: 3050,
            bottom: 3090,
        };
        let (r, z) = centered([-20., 50.], 0.8, big, area);
        assert!(z <= 0.8);
        assert!(
            r.left >= area.left
                && r.top >= area.top
                && r.right <= area.right
                && r.bottom <= area.bottom
        );
        let tiny = Rect {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        };
        let (r, z) = centered([-300., 500.], 0.8, tiny, area);
        assert_eq!(z, 0.8);
        assert_eq!((r.right - r.left, r.bottom - r.top), (1, 1));
    }
}
