//! Cursor feedback shares the canvas hit test and locks to an active drag's operation.
use super::{Gesture, InteractionMode, Registry};
use crate::AnnotationCursor;
use lexift_core::domain::{
    annotation::{Hit, Session},
    geometry::Rect,
};
use slint::ComponentHandle;

pub(super) fn screen_point(bounds: Rect, point: (f32, f32), scale: f32) -> (f32, f32) {
    (
        bounds.left as f32 + point.0 * scale,
        bounds.top as f32 + point.1 * scale,
    )
}

fn handle_cursor(handle: usize) -> AnnotationCursor {
    match handle {
        0 | 7 => AnnotationCursor::DiagonalDown,
        2 | 5 => AnnotationCursor::DiagonalUp,
        1 | 6 => AnnotationCursor::Vertical,
        3 | 4 => AnnotationCursor::Horizontal,
        _ => AnnotationCursor::Drawing,
    }
}

fn resolve(mode: InteractionMode, gesture: Option<Gesture>, hit: Option<Hit>) -> AnnotationCursor {
    if mode.is_mouse() {
        return AnnotationCursor::Idle;
    }
    match gesture {
        Some(Gesture::Draw { .. }) => AnnotationCursor::Drawing,
        Some(Gesture::Move { .. }) => AnnotationCursor::Moving,
        Some(Gesture::Resize { handle, .. }) => handle_cursor(handle),
        None => match hit {
            Some(Hit::Handle(handle)) => handle_cursor(handle),
            Some(Hit::Object(_)) => AnnotationCursor::Moving,
            None => AnnotationCursor::Drawing,
        },
    }
}

fn at_point(
    session: &Session,
    mode: InteractionMode,
    gesture: Option<Gesture>,
    point: Option<(f32, f32)>,
    scale: f32,
) -> AnnotationCursor {
    let hit = if gesture.is_none() && !mode.is_mouse() {
        point.and_then(|p| session.hit(p, scale))
    } else {
        None
    };
    resolve(mode, gesture, hit)
}

/// Updating feedback does not enqueue annotation rasterization. Slint applies it only
/// to the hovered canvas TouchArea, leaving tool windows and click-through apps alone.
pub(super) fn refresh(r: &Registry) {
    for (index, layer) in r.canvases.iter().enumerate() {
        let cursor = at_point(
            &r.session,
            r.mode,
            r.gesture,
            layer.last_pointer,
            layer.window.window().scale_factor().max(0.1),
        );
        if layer.window.get_cursor() != cursor {
            layer.window.set_cursor(cursor);
            // Slint samples TouchArea.mouse-cursor before calling pointer-event.
            // Re-evaluate the stationary pointer after this event, only on a change.
            if let Some(point) = layer.last_pointer {
                let generation = r.generation;
                super::later(move || refresh_stationary_pointer(generation, index, point));
            }
        }
    }
}

fn refresh_stationary_pointer(generation: u64, index: usize, point: (f32, f32)) {
    let target = super::REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot
            .as_ref()
            .filter(|r| r.generation == generation && !r.mode.is_mouse())?;
        let layer = r.canvases.get(index)?;
        // Do not replay a stale position or change a tool window's cursor.
        let cursor = (r.lifecycle.toolbar_cursor_position)()?;
        if layer.last_pointer != Some(point)
            || (cursor.x as f32 - point.0).abs() > 0.5
            || (cursor.y as f32 - point.1).abs() > 0.5
        {
            return None;
        }
        let over_window = |window: &slint::Window| {
            let p = window.position();
            let size = window.size();
            cursor.x >= p.x
                && cursor.y >= p.y
                && cursor.x < p.x + size.width as i32
                && cursor.y < p.y + size.height as i32
        };
        if r.main.as_ref().is_some_and(|w| over_window(w.window()))
            || r.panel.as_ref().is_some_and(|w| over_window(w.window()))
            || r.choice
                .as_ref()
                .is_some_and(|m| over_window(m.window.window()))
        {
            return None;
        }
        let scale = layer.window.window().scale_factor().max(0.1);
        Some((
            layer.window.clone_strong(),
            slint::LogicalPosition::new(
                (point.0 - layer.bounds.left as f32) / scale,
                (point.1 - layer.bounds.top as f32) / scale,
            ),
        ))
    });
    if let Some((window, position)) = target {
        let _ = window
            .window()
            .dispatch_event_with_result(slint::platform::WindowEvent::PointerMoved { position });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::annotation::{Bounds, Kind, Object, Style};
    const TOOL: InteractionMode = InteractionMode::Tool(0);

    #[test]
    fn all_eight_handles_have_the_correct_resize_axis() {
        let expected = [
            AnnotationCursor::DiagonalDown,
            AnnotationCursor::Vertical,
            AnnotationCursor::DiagonalUp,
            AnnotationCursor::Horizontal,
            AnnotationCursor::Horizontal,
            AnnotationCursor::DiagonalUp,
            AnnotationCursor::Vertical,
            AnnotationCursor::DiagonalDown,
        ];
        for (handle, cursor) in expected.into_iter().enumerate() {
            assert_eq!(resolve(TOOL, None, Some(Hit::Handle(handle))), cursor);
        }
        assert_eq!(resolve(TOOL, None, None), AnnotationCursor::Drawing);
        assert_eq!(
            resolve(TOOL, None, Some(Hit::Object(0))),
            AnnotationCursor::Moving
        );
    }

    #[test]
    fn active_drag_keeps_its_cursor_when_crossing_other_hit_regions() {
        let initial = Bounds::from_corners((0., 0.), (200., 100.));
        let gestures = [
            (
                Gesture::Draw {
                    start: (0., 0.),
                    kind: Kind::Rectangle,
                    style: Style::default(),
                },
                AnnotationCursor::Drawing,
            ),
            (
                Gesture::Move {
                    start: (0., 0.),
                    initial,
                    index: 0,
                },
                AnnotationCursor::Moving,
            ),
            (
                Gesture::Resize {
                    initial,
                    index: 0,
                    handle: 7,
                },
                AnnotationCursor::DiagonalDown,
            ),
        ];
        for (gesture, expected) in gestures {
            for hit in [None, Some(Hit::Object(0)), Some(Hit::Handle(1))] {
                assert_eq!(resolve(TOOL, Some(gesture), hit), expected);
                assert_eq!(
                    resolve(InteractionMode::Mouse, Some(gesture), hit),
                    AnnotationCursor::Idle
                );
            }
        }
        assert_eq!(
            resolve(TOOL, None, Some(Hit::Handle(1))),
            AnnotationCursor::Vertical
        );
    }

    #[test]
    fn cursor_uses_the_existing_hit_regions_at_each_dpi_and_negative_origin() {
        let display = Rect {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: 0,
        };
        for scale in [1., 1.25, 1.5, 2.] {
            for kind in [
                Kind::Rectangle,
                Kind::Ellipse,
                Kind::SpotlightRectangle,
                Kind::SpotlightEllipse,
            ] {
                let mut session = Session::new();
                let bounds = Bounds::from_corners(
                    screen_point(display, (100., 100.), scale),
                    screen_point(display, (300., 200.), scale),
                );
                session.add(Object {
                    bounds,
                    kind,
                    style: Style::default(),
                });
                for (handle, point) in bounds.handles().into_iter().enumerate() {
                    assert_eq!(
                        at_point(&session, TOOL, None, Some(point), scale),
                        handle_cursor(handle)
                    );
                }
                let point = screen_point(display, (150., 100.), scale);
                // Rectangle's upper edge is draggable; an ellipse does not share that edge.
                if kind == Kind::Rectangle || kind.is_spotlight() {
                    assert_eq!(
                        at_point(&session, TOOL, None, Some(point), scale),
                        AnnotationCursor::Moving
                    );
                }
                let blank = screen_point(display, (400., 300.), scale);
                assert_eq!(
                    at_point(&session, TOOL, None, Some(blank), scale),
                    AnnotationCursor::Drawing
                );
                session.delete_selected();
                assert_eq!(
                    at_point(&session, TOOL, None, Some(point), scale),
                    AnnotationCursor::Drawing
                );
                assert!(session.undo());
                assert_eq!(
                    at_point(&session, TOOL, None, Some(bounds.handles()[0]), scale),
                    AnnotationCursor::DiagonalDown
                );
            }
        }
    }
}
