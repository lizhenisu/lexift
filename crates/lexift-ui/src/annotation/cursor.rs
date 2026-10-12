//! Cursor feedback shares the canvas hit test and locks to an active drag's operation.
use super::{Gesture, InteractionMode, Registry};
use crate::AnnotationCursor;
use lexift_core::domain::annotation::{Extra, Hit, Kind, Object, Session};
#[cfg(test)]
use lexift_core::domain::geometry::Rect;
use slint::ComponentHandle;

pub(super) use crate::screen_interaction::screen_point;

fn handle_cursor(handle: usize) -> AnnotationCursor {
    match handle % 8 {
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
        Some(Gesture::PlacePoint | Gesture::FinishArrow | Gesture::PlaceSequence) => {
            AnnotationCursor::Drawing
        }
        Some(
            Gesture::DeleteText | Gesture::ToggleTextArrow { .. } | Gesture::SequenceControl { .. },
        ) => AnnotationCursor::Idle,
        Some(Gesture::CreateText { .. }) => AnnotationCursor::Drawing,
        Some(Gesture::Erase | Gesture::Draw { .. }) => AnnotationCursor::Drawing,
        Some(Gesture::Move) => AnnotationCursor::Moving,
        Some(Gesture::TextSelection { .. }) => AnnotationCursor::Text,
        Some(Gesture::Resize { handle, .. }) => handle_cursor(handle),
        Some(Gesture::TextHandle { handle, .. }) => {
            if handle == 5 {
                AnnotationCursor::Vertical
            } else {
                AnnotationCursor::Idle
            }
        }
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
    if mode == InteractionMode::Tool(6)
        && gesture.is_none()
        && let Some(hit) = point.and_then(|p| {
            session
                .rectangle_erase_geometry()
                .and_then(|g| g.hit(p, scale))
        })
    {
        return match hit {
            lexift_core::domain::annotation_erase::RectangleEraseHit::Body => {
                AnnotationCursor::Moving
            }
            lexift_core::domain::annotation_erase::RectangleEraseHit::Handle(h) if h >= 8 => {
                AnnotationCursor::CornerRadius
            }
            lexift_core::domain::annotation_erase::RectangleEraseHit::Handle(h) => handle_cursor(h),
        };
    }
    let hit = if gesture.is_none() && !mode.is_mouse() {
        point.and_then(|p| session.hit(p, scale))
    } else {
        None
    };
    if let Some(index) = session.selected {
        let object = &session.objects[index];
        let handle = match gesture {
            Some(Gesture::Resize { handle, .. }) => Some(handle),
            None => match hit {
                Some(Hit::Handle(h)) => Some(h),
                _ => None,
            },
            _ => None,
        };
        if let Some(handle) = handle {
            if object.kind == Kind::Sequence {
                return if handle == 6 {
                    AnnotationCursor::Vertical
                } else {
                    AnnotationCursor::Idle
                };
            }
            if object.kind == Kind::Text && handle < 4 {
                return AnnotationCursor::Idle;
            }
            if object.kind == Kind::Text && handle == 5 {
                return AnnotationCursor::Vertical;
            }
            if matches!(
                object.kind,
                Kind::HighlightRectangle | Kind::MosaicArea | Kind::StickerArea
            ) && handle >= 8
            {
                return AnnotationCursor::CornerRadius;
            }
            if object.kind == Kind::HighlightLine {
                return AnnotationCursor::Vertical;
            }
            if object.kind == Kind::Arrow {
                return AnnotationCursor::Vertical;
            }
            if object.kind == Kind::Polyline {
                return AnnotationCursor::Moving;
            }
        }
    }
    resolve(mode, gesture, hit)
}

/// Drafts are outside the Core session, but their visible controls still take priority.
fn editing_control_cursor(
    object: &Object,
    point: (f32, f32),
    scale: f32,
) -> Option<AnnotationCursor> {
    let tolerance = 9. * scale;
    object
        .edit_handles(scale)
        .into_iter()
        .take(4)
        .find_map(|(x, y)| {
            ((point.0 - x).abs() <= tolerance && (point.1 - y).abs() <= tolerance)
                .then_some(AnnotationCursor::Idle)
        })
}

/// Updating feedback does not enqueue annotation rasterization. Slint applies it only
/// to the hovered canvas TouchArea, leaving tool windows and click-through apps alone.
pub(super) fn refresh(r: &Registry) {
    let editing = if r.gesture().is_none() && !r.mode.is_mouse() {
        super::editing_text_object(r)
    } else {
        None
    };
    for (index, layer) in r.canvas_state.canvases.iter().enumerate() {
        let editing_control = editing
            .as_ref()
            .filter(|_| !super::editing_sequence(r))
            .and_then(|object| {
                layer.last_pointer.and_then(|point| {
                    editing_control_cursor(
                        object,
                        point,
                        layer.window.window().scale_factor().max(0.1),
                    )
                })
            });
        let erase_cursor = match &r.gesture {
            Some(super::gestures::Active::Erasing(press)) => {
                press.edit_hit().map(|hit| match hit {
                    lexift_core::domain::annotation_erase::RectangleEraseHit::Body => {
                        AnnotationCursor::Moving
                    }
                    lexift_core::domain::annotation_erase::RectangleEraseHit::Handle(h)
                        if h >= 8 =>
                    {
                        AnnotationCursor::CornerRadius
                    }
                    lexift_core::domain::annotation_erase::RectangleEraseHit::Handle(h) => {
                        handle_cursor(h)
                    }
                })
            }
            _ => None,
        };
        let desired = if let Some(cursor) = erase_cursor.or(editing_control) {
            cursor
        } else if r.arrow_pending().is_some() {
            AnnotationCursor::Drawing
        } else {
            at_point(
                &r.session,
                r.mode,
                r.gesture(),
                layer.last_pointer,
                layer.window.window().scale_factor().max(0.1),
            )
        };
        let hovered_text = r.editor.window().is_some()
            && r.gesture().is_none()
            && layer.last_pointer.is_some_and(|point| {
                matches!(r.session.hit(point, layer.window.window().scale_factor().max(0.1)),
                Some(Hit::Object(index)) if r.session.objects[index].kind == Kind::Text)
            });
        let desired = if editing_control.is_none()
            && (hovered_text
                || (r.mode == InteractionMode::Tool(3)
                    && r.selected[3] == 8
                    && desired == AnnotationCursor::Drawing))
        {
            AnnotationCursor::Text
        } else {
            desired
        };
        let was_custom = layer.window.get_cursor() == AnnotationCursor::CornerRadius;
        let brush = desired == AnnotationCursor::Drawing
            && (matches!(
                super::active_kind(r),
                Some(Kind::MosaicBrush | Kind::StickerBrush)
            ) || (r.mode == InteractionMode::Tool(6) && r.values(13).shape == 1))
            && layer.last_pointer.is_some();
        if !brush && layer.window.get_cursor() == AnnotationCursor::Brush {
            (r.lifecycle.annotation_brush_cursor)(layer.window.window(), 0.);
        }
        let cursor = if brush {
            let diameter = if r.mode == InteractionMode::Tool(6) {
                if let Some(super::gestures::Active::Erasing(press)) = &r.gesture {
                    press.diameter
                } else {
                    r.values(13).size.clamp(1, 100) as f32 * layer.window.window().scale_factor()
                }
            } else {
                super::gestures::display_draft(&r.gesture, &r.pending_drawing)
                    .and_then(|object| match &object.extra {
                        Extra::Mosaic(value) if object.kind == Kind::MosaicBrush => {
                            Some(value.diameter)
                        }
                        Extra::Sticker(value) if object.kind == Kind::StickerBrush => {
                            Some(value.diameter)
                        }
                        _ => None,
                    })
                    .unwrap_or(
                        r.values(if super::active_kind(r) == Some(Kind::StickerBrush) {
                            12
                        } else {
                            11
                        })
                        .size
                        .clamp(1, 100) as f32
                            * layer.window.window().scale_factor(),
                    )
            };
            if (r.lifecycle.annotation_brush_cursor)(layer.window.window(), diameter) {
                AnnotationCursor::Brush
            } else {
                AnnotationCursor::Drawing
            }
        } else if desired == AnnotationCursor::CornerRadius {
            if layer.corner_cursor_failed.get() {
                AnnotationCursor::Moving
            } else if was_custom
                || (r.lifecycle.annotation_corner_cursor)(layer.window.window(), true)
            {
                AnnotationCursor::CornerRadius
            } else {
                layer.corner_cursor_failed.set(true);
                AnnotationCursor::Moving
            }
        } else {
            if was_custom {
                (r.lifecycle.annotation_corner_cursor)(layer.window.window(), false);
            }
            desired
        };
        if layer.window.get_cursor() != cursor {
            layer.window.set_cursor(cursor);
            // Slint samples TouchArea.mouse-cursor before calling pointer-event.
            // Re-evaluate the stationary pointer after this event, only on a change.
            if let Some(point) = layer.last_pointer {
                let generation = r.generation;
                let revision = layer.revision();
                layer
                    .input()
                    .post(move || refresh_stationary_pointer(generation, index, revision, point));
            }
        }
    }
}

fn refresh_stationary_pointer(generation: u64, index: usize, revision: u64, point: (f32, f32)) {
    let target = super::REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot
            .as_ref()
            .filter(|r| r.generation == generation && !r.mode.is_mouse())?;
        let layer = r
            .canvas_state
            .canvases
            .get(index)
            .filter(|layer| layer.revision() == revision)?;
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
        if r.toolbar_state
            .main
            .get()
            .is_some_and(|w| over_window(w.window()))
            || r.toolbar_state
                .panel
                .get()
                .is_some_and(|w| over_window(w.window()))
            || r.toolbar_state
                .menus
                .current
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
        // Slint can apply its fallback cursor after dispatching the pointer event.
        // Reapply the native shape after that update, for a stationary first hover.
        super::REGISTRY.with(|slot| {
            if let Some(r) = slot
                .borrow()
                .as_ref()
                .filter(|r| r.generation == generation)
            {
                refresh(r);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_controls_use_arrow_for_drafts_hover_and_drag_at_both_dpi_scales() {
        use lexift_core::domain::annotation::TextAnnotation;
        for scale in [1., 1.25] {
            for rotation in [0., 0.6] {
                let bounds = Bounds::from_corners((-240., -100.), (-208., -72.));
                let object = Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::Text,
                    bounds,
                    points: Vec::new(),
                    style: Style::default(),
                    extra: Extra::Text(TextAnnotation {
                        content: "短".into(),
                        font_family: "Microsoft YaHei".into(),
                        font_size: 22.,
                        bold: false,
                        italic: false,
                        outline: None,
                        background: None,
                        rotation,
                        layout_scale: 1.,
                        linked_arrow_control: None,
                        linked_arrow: None,
                    }),
                };
                let mut session = Session::new();
                session.add(object.clone());
                for (handle, point) in object.edit_handles(scale).into_iter().take(4).enumerate() {
                    assert_eq!(
                        editing_control_cursor(&object, point, scale),
                        Some(AnnotationCursor::Idle)
                    );
                    assert_eq!(
                        at_point(&session, TOOL, None, Some(point), scale),
                        AnnotationCursor::Idle
                    );
                    assert_eq!(
                        at_point(
                            &session,
                            TOOL,
                            Some(Gesture::Resize { handle }),
                            Some((400., 400.)),
                            scale
                        ),
                        AnnotationCursor::Idle
                    );
                }
                assert_eq!(
                    editing_control_cursor(&object, bounds.center(), scale),
                    None
                );
                assert_eq!(
                    at_point(&session, TOOL, None, Some(bounds.center()), scale),
                    AnnotationCursor::Moving
                );
                session.objects[0].toggle_text_arrow(scale);
                let midpoint = session.objects[0].edit_handles(scale)[5];
                assert_eq!(
                    at_point(&session, TOOL, None, Some(midpoint), scale),
                    AnnotationCursor::Vertical
                );
                assert_eq!(
                    at_point(
                        &session,
                        TOOL,
                        Some(Gesture::Move),
                        Some(bounds.center()),
                        scale
                    ),
                    AnnotationCursor::Moving
                );
            }
        }
    }

    use super::*;
    use lexift_core::domain::annotation::{Bounds, Endpoint, Extra, Kind, Object, Style};
    const TOOL: InteractionMode = InteractionMode::Tool(0);

    #[test]
    fn foreign_objects_and_handles_keep_the_current_drawing_cursor() {
        for scale in [1., 1.25] {
            let mut session = Session::new();
            session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((-200., -100.), (-40., 20.)),
                points: Vec::new(),
                style: Style {
                    fill: true,
                    ..Style::default()
                },
                extra: Extra::None,
            });
            let center = session.objects[0].bounds.center();
            let handle = session.objects[0].edit_handles(scale)[0];
            session.set_edit_scope(lexift_core::domain::annotation::EditScope::Magnifier);
            for point in [center, handle] {
                assert_eq!(
                    at_point(&session, InteractionMode::Tool(2), None, Some(point), scale),
                    AnnotationCursor::Drawing
                );
            }
            session.set_edit_scope(lexift_core::domain::annotation::EditScope::Geometry);
            assert_eq!(
                at_point(&session, TOOL, None, Some(center), scale),
                AnnotationCursor::Moving
            );
        }
    }

    #[test]
    fn highlighter_handles_keep_their_edit_cursor_during_drag() {
        for kind in [Kind::HighlightRectangle, Kind::HighlightLine] {
            let mut session = Session::new();
            let bounds = Bounds::from_corners((20., 20.), (220., 120.));
            let points = if kind == Kind::HighlightLine {
                vec![(20., 20.), (220., 120.)]
            } else {
                Vec::new()
            };
            session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind,
                bounds,
                points,
                style: Style::default(),
                extra: Default::default(),
            });
            for (handle, point) in session.objects[0].edit_handles(1.).into_iter().enumerate() {
                if kind == Kind::HighlightRectangle && handle < 8 {
                    continue;
                }
                let expected = if kind == Kind::HighlightLine {
                    AnnotationCursor::Vertical
                } else {
                    AnnotationCursor::CornerRadius
                };
                assert_eq!(at_point(&session, TOOL, None, Some(point), 1.), expected);
                assert_eq!(
                    at_point(
                        &session,
                        TOOL,
                        Some(Gesture::Resize { handle }),
                        Some((500., 500.)),
                        1.
                    ),
                    expected
                );
            }
            let body = if kind == Kind::HighlightLine {
                (120., 70.)
            } else {
                (120., 60.)
            };
            assert_eq!(
                at_point(&session, TOOL, None, Some(body), 1.),
                AnnotationCursor::Moving
            );
            assert_eq!(
                at_point(&session, TOOL, Some(Gesture::Move), Some((500., 500.)), 1.),
                AnnotationCursor::Moving
            );
        }
    }

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
        assert_eq!(
            resolve(
                InteractionMode::Tool(2),
                Some(Gesture::FinishArrow),
                Some(Hit::Object(0))
            ),
            AnnotationCursor::Drawing
        );
        let gestures = [
            (
                Gesture::Draw {
                    kind: Kind::Rectangle,
                },
                AnnotationCursor::Drawing,
            ),
            (Gesture::Move, AnnotationCursor::Moving),
            (
                Gesture::Resize { handle: 7 },
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
    fn arrow_midpoint_uses_vertical_cursor_on_hover_and_during_drag() {
        for points in [
            vec![(0., 0.), (100., 0.)],
            vec![(0., 0.), (100., 100.)],
            vec![(0., 0.), (100., 0.), (50., -100.)],
            vec![(0., 0.), (100., 0.), (50., 100.)],
        ] {
            let mut session = Session::new();
            let curved = points.len() == 3;
            let mut arrow = Object {
                erased: Vec::new(),
                id: Default::default(),
                bounds: Bounds::from_corners(points[0], points[1]),
                points,
                kind: Kind::Arrow,
                style: Style::default(),
                extra: Extra::Arrow {
                    curved,
                    head: true,
                    start: Endpoint::None,
                    end: Endpoint::FilledArrow,
                },
            };
            arrow.refresh_bounds();
            session.add(arrow);
            let handles = session.objects[0].edit_handles(1.);
            for point in handles {
                assert_eq!(
                    at_point(&session, TOOL, None, Some(point), 1.),
                    AnnotationCursor::Vertical
                );
            }
            assert_eq!(
                at_point(
                    &session,
                    TOOL,
                    Some(Gesture::Resize { handle: 2 }),
                    Some((500., 500.)),
                    1.,
                ),
                AnnotationCursor::Vertical
            );
            assert_eq!(
                at_point(&session, TOOL, None, Some((500., 500.)), 1.),
                AnnotationCursor::Drawing
            );
            session.selected = None;
            assert_eq!(
                at_point(&session, TOOL, None, Some((500., 500.)), 1.),
                AnnotationCursor::Drawing
            );
        }
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
                    erased: Vec::new(),
                    id: Default::default(),
                    points: Vec::new(),
                    bounds,
                    kind,
                    style: Style::default(),
                    extra: Default::default(),
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
