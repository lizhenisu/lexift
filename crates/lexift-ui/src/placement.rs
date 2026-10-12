use lexift_core::domain::geometry::{Point, Rect};

const SELECTION_TOOLBAR_TEXT_GAP: f32 = 12.0;
const SELECTION_TOOLBAR_SCREEN_MARGIN: f32 = 8.0;

pub(crate) const SELECTION_TOOLBAR_FADE_START: f64 = 24.0;
pub(crate) const SELECTION_TOOLBAR_FADE_END: f64 = 220.0;

/// Cursor-to-window edge distance in logical pixels, shared by placement and fading.
pub(crate) fn selection_toolbar_distance(cursor: Point, bounds: Rect, scale: f32) -> f64 {
    let dx = (i64::from(bounds.left) - i64::from(cursor.x))
        .max(0)
        .max(i64::from(cursor.x) - i64::from(bounds.right));
    let dy = (i64::from(bounds.top) - i64::from(cursor.y))
        .max(0)
        .max(i64::from(cursor.y) - i64::from(bounds.bottom));
    let scale = if scale.is_finite() {
        scale.max(0.1)
    } else {
        1.0
    };
    (dx as f64).hypot(dy as f64) / f64::from(scale)
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    fn verify(rects: &[Rect], start: Point, end: Point, scale: f32, area: Rect) -> Point {
        let size = ((96.0 * scale) as u32, (48.0 * scale) as u32);
        let p = place_selection_toolbar(rects, start, end, size, scale, area).unwrap();
        let gap = (12.0 * scale).ceil() as i32;
        let margin = (8.0 * scale).ceil() as i32;
        assert!(p.x >= area.left + margin && p.y >= area.top + margin);
        assert!(i64::from(p.x) + i64::from(size.0) <= i64::from(area.right - margin));
        assert!(i64::from(p.y) + i64::from(size.1) <= i64::from(area.bottom - margin));
        for r in rects {
            assert!(
                p.x + size.0 as i32 <= r.left - gap
                    || p.x >= r.right + gap
                    || p.y + size.1 as i32 <= r.top - gap
                    || p.y >= r.bottom + gap,
                "toolbar {p:?} must not overlap {r:?}"
            );
        }
        p
    }
    #[test]
    fn four_drag_directions_and_multiline_selections_preserve_a_clear_gap() {
        let rows = [
            Rect {
                left: 100,
                top: 100,
                right: 480,
                bottom: 125,
            },
            Rect {
                left: 100,
                top: 130,
                right: 330,
                bottom: 155,
            },
        ];
        let area = Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        for (start, end) in [
            (Point { x: 100, y: 100 }, Point { x: 330, y: 155 }),
            (Point { x: 330, y: 155 }, Point { x: 100, y: 100 }),
            (Point { x: 480, y: 100 }, Point { x: 100, y: 155 }),
            (Point { x: 100, y: 155 }, Point { x: 480, y: 100 }),
        ] {
            let p = verify(&rows, start, end, 1.0, area);
            assert!(
                selection_toolbar_distance(
                    end,
                    Rect {
                        left: p.x,
                        top: p.y,
                        right: p.x + 96,
                        bottom: p.y + 48
                    },
                    1.
                ) < SELECTION_TOOLBAR_FADE_END
            );
        }
    }
    #[test]
    fn all_screen_edges_negative_coordinates_dpi_and_cross_screen_text() {
        for scale in [1.0, 1.25] {
            let area = Rect {
                left: -1200,
                top: -500,
                right: 0,
                bottom: 400,
            };
            for (x, y) in [
                (-1198, -498),
                (-4, -498),
                (-1198, 398),
                (-4, 398),
                (-500, 0),
            ] {
                let r = Rect {
                    left: x - 80,
                    top: y - 12,
                    right: x + 80,
                    bottom: y + 12,
                };
                verify(
                    &[r],
                    Point {
                        x: x + 20,
                        y: y + 10,
                    },
                    Point { x, y },
                    scale,
                    area,
                );
            }
            verify(
                &[Rect {
                    left: -20,
                    top: -50,
                    right: 100,
                    bottom: 40,
                }],
                Point { x: 100, y: 30 },
                Point { x: -10, y: -40 },
                scale,
                area,
            );
        }
    }
    #[test]
    fn uses_gaps_between_discontinuous_ranges_instead_of_union() {
        let area = Rect {
            left: 0,
            top: 0,
            right: 200,
            bottom: 200,
        };
        let rows = [
            Rect {
                left: 0,
                top: 0,
                right: 200,
                bottom: 40,
            },
            Rect {
                left: 0,
                top: 140,
                right: 200,
                bottom: 200,
            },
        ];
        let p = verify(
            &rows,
            Point { x: 20, y: 20 },
            Point { x: 20, y: 35 },
            1.0,
            area,
        );
        assert_eq!(p.y, 52);
    }
    #[test]
    fn nearest_corner_and_below_ties_use_both_axis_gaps() {
        let area = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 700,
        };
        let text = Rect {
            left: 300,
            top: 250,
            right: 500,
            bottom: 300,
        };
        for scale in [1., 1.25] {
            let gap = (12_f32 * scale).ceil() as i32;
            let (w, h) = ((96_f32 * scale) as i32, (48_f32 * scale) as i32);
            for (end, expected) in [
                (
                    Point { x: 500, y: 300 },
                    Point {
                        x: 500 + gap,
                        y: 300 + gap,
                    },
                ),
                (
                    Point { x: 300, y: 300 },
                    Point {
                        x: 300 - gap - w,
                        y: 300 + gap,
                    },
                ),
                (
                    Point { x: 300, y: 250 },
                    Point {
                        x: 300 - gap - w,
                        y: 250 - gap - h,
                    },
                ),
                (
                    Point { x: 500, y: 250 },
                    Point {
                        x: 500 + gap,
                        y: 250 - gap - h,
                    },
                ),
            ] {
                assert_eq!(
                    verify(&[text], Point { x: 400, y: 275 }, end, scale, area),
                    expected
                );
            }
            let tie = verify(
                &[text],
                Point { x: 400, y: 275 },
                Point { x: 400, y: 275 },
                scale,
                area,
            );
            assert_eq!(tie.y, text.bottom + gap);
        }
    }

    #[test]
    fn corner_failure_falls_back_without_increasing_screen_margin() {
        let area = Rect {
            left: 0,
            top: 0,
            right: 112,
            bottom: 200,
        };
        let text = Rect {
            left: 0,
            top: 0,
            right: 112,
            bottom: 20,
        };
        let end = Point { x: 60, y: 20 };
        assert_eq!(verify(&[text], end, end, 1., area), Point { x: 8, y: 32 });
        let area = Rect { right: 800, ..area };
        let text = Rect {
            left: 300,
            top: 0,
            right: 500,
            bottom: 200,
        };
        let p = verify(&[text], end, Point { x: 480, y: 100 }, 1., area);
        assert!(p.x + 96 <= 288 || p.x >= 512);
    }

    #[test]
    fn multiline_edge_prefers_near_fallback_over_remote_union_corner() {
        let area = Rect {
            left: -1000,
            top: -500,
            right: 0,
            bottom: 300,
        };
        let rows = [
            Rect {
                left: -500,
                top: 0,
                right: -20,
                bottom: 20,
            },
            Rect {
                left: -300,
                top: 25,
                right: -200,
                bottom: 50,
            },
        ];
        for scale in [1., 1.25] {
            let p = verify(
                &rows,
                Point { x: -500, y: 0 },
                Point { x: -200, y: 50 },
                scale,
                area,
            );
            let gap = (12_f32 * scale).ceil() as i32;
            assert_eq!(p.y, 50 + gap);
            assert!(p.x <= -200 && p.x + (96_f32 * scale) as i32 >= -200);
        }
    }

    #[test]
    fn real_toolbar_size_uses_diagonal_placement_at_both_dpis() {
        use slint::{
            ComponentHandle,
            platform::{
                Platform, WindowAdapter,
                software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
            },
        };
        use std::{cell::RefCell, rc::Rc};
        struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
                *self.0.borrow_mut() = Some(adapter.clone());
                Ok(adapter)
            }
        }
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let toolbar = crate::SelectionToolbarWindow::new().unwrap();
        toolbar.set_history_supported(true);
        let adapter = last.borrow().as_ref().unwrap().clone();
        for scale in [1_f32, 1.25] {
            toolbar
                .window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            let size = ((138. * scale).ceil() as u32, (48. * scale).ceil() as u32);
            adapter.set_size(slint::PhysicalSize::new(size.0, size.1));
            let text = Rect {
                left: (240. * scale) as i32,
                top: (130. * scale) as i32,
                right: (480. * scale) as i32,
                bottom: (154. * scale) as i32,
            };
            let area = Rect {
                left: 0,
                top: 0,
                right: (800. * scale) as i32,
                bottom: (400. * scale) as i32,
            };
            let end = Point {
                x: text.right,
                y: text.bottom,
            };
            let p = place_selection_toolbar(&[text], end, end, size, scale, area).unwrap();
            assert_eq!(p.x - text.right, (12. * scale).ceil() as i32);
            assert_eq!(p.y - text.bottom, (12. * scale).ceil() as i32);
            for dark in [false, true] {
                toolbar
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                slint::platform::update_timers_and_animations();
                toolbar.window().request_redraw();
                let mut pixels = vec![slint::Rgb8Pixel::default(); (size.0 * size.1) as usize];
                assert!(adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut pixels, size.0 as usize);
                }));
                assert!(
                    pixels
                        .iter()
                        .any(|pixel| pixel.r != 0 || pixel.g != 0 || pixel.b != 0)
                );
                if let Ok(directory) = std::env::var("LEXIFT_SELECTION_PLACEMENT_SNAPSHOT_DIR") {
                    crate::i18n::write_test_bitmap(
                        &directory,
                        &format!("toolbar-{scale}-{dark}"),
                        size.0,
                        size.1,
                        pixels
                            .into_iter()
                            .map(|pixel| [pixel.r, pixel.g, pixel.b, 255]),
                    );
                    std::fs::write(
                        std::path::Path::new(&directory).join(format!("placement-{scale}.txt")),
                        format!(
                            "{} {} {} {} {} {}",
                            text.left, text.top, text.right, text.bottom, p.x, p.y
                        ),
                    )
                    .unwrap();
                    let edge = Rect {
                        left: (6. * scale) as i32,
                        top: (374. * scale) as i32,
                        right: (600. * scale) as i32,
                        bottom: (394. * scale) as i32,
                    };
                    let end = Point {
                        x: edge.left,
                        y: (384. * scale) as i32,
                    };
                    let near = place_selection_toolbar(
                        &[edge],
                        Point {
                            x: edge.right,
                            y: end.y,
                        },
                        end,
                        size,
                        scale,
                        area,
                    )
                    .unwrap();
                    std::fs::write(
                        std::path::Path::new(&directory)
                            .join(format!("edge-placement-{scale}.txt")),
                        format!(
                            "{} {} {} {} {} {}",
                            edge.left, edge.top, edge.right, edge.bottom, near.x, near.y
                        ),
                    )
                    .unwrap();
                }
            }
        }
    }

    #[test]
    fn long_reverse_selections_at_screen_corners_remain_near_release() {
        for scale in [1_f32, 1.25] {
            let area = Rect {
                left: -1200,
                top: -500,
                right: 0,
                bottom: 400,
            };
            for right_edge in [false, true] {
                for bottom_edge in [false, true] {
                    let (left, right) = if right_edge {
                        (-606, -6)
                    } else {
                        (-1194, -594)
                    };
                    let top = if bottom_edge { 374 } else { -494 };
                    let text = Rect {
                        left,
                        top,
                        right,
                        bottom: top + 20,
                    };
                    let end = Point {
                        x: if right_edge { right } else { left },
                        y: top + 10,
                    };
                    let start = Point {
                        x: if right_edge { left } else { right },
                        y: top + 10,
                    };
                    let p = verify(&[text], start, end, scale, area);
                    let toolbar = Rect {
                        left: p.x,
                        top: p.y,
                        right: p.x + (96_f32 * scale) as i32,
                        bottom: p.y + (48_f32 * scale) as i32,
                    };
                    assert!(
                        selection_toolbar_distance(end, toolbar, scale)
                            <= SELECTION_TOOLBAR_FADE_START
                    );
                    if bottom_edge {
                        assert!(toolbar.bottom <= top - (12. * scale).ceil() as i32);
                    } else {
                        assert!(toolbar.top >= text.bottom + (12. * scale).ceil() as i32);
                    }
                }
            }
        }
    }

    #[test]
    fn missing_bounds_double_click_and_no_full_size_space() {
        let area = Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        let click = Point { x: 300, y: 300 };
        let p = verify(&[], click, click, 1.25, area);
        assert!(p.y >= 341 || p.y + 60 <= 260 || p.x >= 311 || p.x + 120 <= 290);
        for (start, end) in [
            (Point { x: 500, y: 400 }, Point { x: 100, y: 100 }),
            (Point { x: 100, y: 100 }, Point { x: 500, y: 400 }),
        ] {
            let p = verify(&[], start, end, 1.0, area);
            assert!(
                selection_toolbar_distance(
                    end,
                    Rect {
                        left: p.x,
                        top: p.y,
                        right: p.x + 96,
                        bottom: p.y + 48
                    },
                    1.
                ) < SELECTION_TOOLBAR_FADE_END
            );
        }
        assert!(place_selection_toolbar(&[area], click, click, (96, 48), 1.0, area).is_none());
        assert!(
            place_selection_toolbar(
                &[],
                click,
                click,
                (96, 48),
                1.0,
                Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 50
                }
            )
            .is_none()
        );
    }
}

/// Prefer a nearby corner, then nearby free spans. A distant valid corner must
/// not outrank a reachable fallback and immediately trigger distance dismissal.
pub(crate) fn place_selection_toolbar(
    bounds: &[Rect],
    start: Point,
    end: Point,
    desired: (u32, u32),
    scale: f32,
    area: Rect,
) -> Option<Point> {
    let scale = if scale.is_finite() {
        scale.max(0.1)
    } else {
        1.0
    };
    let gap = (SELECTION_TOOLBAR_TEXT_GAP * scale).ceil() as i64;
    let margin = (SELECTION_TOOLBAR_SCREEN_MARGIN * scale).ceil() as i64;
    let (width, height) = (i64::from(desired.0), i64::from(desired.1));
    let (left, top, right, bottom) = (
        i64::from(area.left) + margin,
        i64::from(area.top) + margin,
        i64::from(area.right) - margin,
        i64::from(area.bottom) - margin,
    );
    if width <= 0 || height <= 0 || right - left < width || bottom - top < height {
        return None;
    }
    let mut obstacles: Vec<_> = bounds
        .iter()
        .copied()
        .filter(|r| r.left < r.right && r.top < r.bottom)
        .collect();
    if obstacles.is_empty() {
        let near_distance = (6.0 * scale).ceil() as i64;
        let near = (i64::from(start.x) - i64::from(end.x)).abs() <= near_distance
            && (i64::from(start.y) - i64::from(end.y)).abs() <= near_distance;
        let padding = if near {
            (24.0 * scale).ceil() as i32
        } else {
            0
        };
        obstacles.push(Rect {
            left: start.x.min(end.x),
            right: start.x.max(end.x).saturating_add(1),
            top: start.y.min(end.y).saturating_sub(padding),
            bottom: start.y.max(end.y).saturating_add(padding).saturating_add(1),
        });
    }
    let blocked: Vec<_> = obstacles
        .iter()
        .map(|r| {
            (
                i64::from(r.left) - gap,
                i64::from(r.top) - gap,
                i64::from(r.right) + gap,
                i64::from(r.bottom) + gap,
            )
        })
        .collect();
    let (ex, ey) = (i64::from(end.x), i64::from(end.y));
    // Expanded ranges encode the text gap once. Never clamp corners: doing so
    // would silently reduce the requested separation on one of the axes.
    let union = blocked.iter().fold(
        (i64::MAX, i64::MAX, i64::MIN, i64::MIN),
        |(l, t, r, b), &(x1, y1, x2, y2)| (l.min(x1), t.min(y1), r.max(x2), b.max(y2)),
    );
    let mut best = None;
    let mut consider = |x: i64, y: i64| {
        let bounds = Rect {
            left: x as i32,
            top: y as i32,
            right: (x + width) as i32,
            bottom: (y + height) as i32,
        };
        let distance = selection_toolbar_distance(end, bounds, scale);
        let near = distance <= SELECTION_TOOLBAR_FADE_START;
        let diagonal =
            (x + width <= union.0 || x >= union.2) && (y + height <= union.1 || y >= union.3);
        let direction = if y >= ey + gap {
            0
        } else if y + height <= ey - gap {
            1
        } else if x + width <= ex - gap {
            2
        } else {
            3
        };
        // Preserve diagonal spacing while reachable; otherwise distance
        // wins so a far corner cannot outrank a usable near fallback.
        let score = (
            !near,
            near && !diagonal,
            distance,
            !diagonal,
            direction,
            y,
            x,
        );
        if best.as_ref().is_none_or(|(saved, _)| score < *saved) {
            best = Some((
                score,
                Point {
                    x: x as i32,
                    y: y as i32,
                },
            ));
        }
    };
    for y in [union.3, union.1 - height] {
        for x in [union.0 - width, union.2] {
            if x < left || y < top || x + width > right || y + height > bottom {
                continue;
            }
            consider(x, y);
        }
    }
    let mut ys = vec![
        top,
        bottom - height,
        ey + gap,
        ey - gap - height,
        ey,
        ey - height,
    ];
    for &(_, t, _, b) in &blocked {
        ys.extend([t - height, b]);
    }
    for y in &mut ys {
        *y = (*y).clamp(top, bottom - height);
    }
    ys.sort_unstable();
    ys.dedup();
    for y in ys {
        let mut intervals: Vec<_> = blocked
            .iter()
            .filter(|&&(_, t, _, b)| y < b && y + height > t)
            .map(|&(l, _, r, _)| (l.max(left), r.min(right)))
            .filter(|&(l, r)| l < r)
            .collect();
        intervals.sort_unstable();
        let mut free = Vec::new();
        let mut cursor = left;
        for (l, r) in intervals {
            if l > cursor {
                free.push((cursor, l));
            }
            cursor = cursor.max(r);
        }
        if cursor < right {
            free.push((cursor, right));
        }
        for (l, r) in free {
            if r - l < width {
                continue;
            }
            let mut xs = [
                l,
                r - width,
                ex.clamp(l, r - width),
                (ex - gap - width).clamp(l, r - width),
                (ex + gap).clamp(l, r - width),
            ];
            xs.sort_unstable();
            for x in xs {
                if !(y >= ey + gap
                    || y + height <= ey - gap
                    || x + width <= ex - gap
                    || x >= ex + gap)
                {
                    continue;
                }
                if blocked
                    .iter()
                    .any(|&(l, t, r, b)| x < r && x + width > l && y < b && y + height > t)
                {
                    continue;
                }
                consider(x, y);
            }
        }
    }
    best.map(|(_, point)| point)
}

/// Finds unobstructed physical space for annotation controls. Cropping changes
/// the viewport, not the content; callers supply scrolling for reduced sizes.
pub(crate) fn place_annotation_popup(
    anchor: Rect,
    desired: (u32, u32),
    area: Rect,
    obstacles: &[Rect],
    prefer_above: bool,
    gap: i32,
) -> Option<AttachedMenuPlacement> {
    let gap = gap.max(0);
    let blocked: Vec<_> = obstacles
        .iter()
        .map(|r| Rect {
            left: r.left.saturating_sub(gap).max(area.left),
            top: r.top.saturating_sub(gap).max(area.top),
            right: r.right.saturating_add(gap).min(area.right),
            bottom: r.bottom.saturating_add(gap).min(area.bottom),
        })
        .filter(|r| r.left < r.right && r.top < r.bottom)
        .collect();
    let mut xs = vec![area.left, area.right];
    let mut ys = vec![area.top, area.bottom];
    for r in &blocked {
        xs.extend([r.left, r.right]);
        ys.extend([r.top, r.bottom]);
    }
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let (dw, dh) = (i64::from(desired.0.max(1)), i64::from(desired.1.max(1)));
    let mut best = None;
    for (ix, &left) in xs.iter().enumerate() {
        for &right in &xs[ix + 1..] {
            for (iy, &top) in ys.iter().enumerate() {
                for &bottom in &ys[iy + 1..] {
                    if blocked.iter().any(|r| {
                        left < r.right && right > r.left && top < r.bottom && bottom > r.top
                    }) {
                        continue;
                    }
                    let w = dw.min(i64::from(right) - i64::from(left));
                    let h = dh.min(i64::from(bottom) - i64::from(top));
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    for direction in 0..4 {
                        let above =
                            (direction == 0 && prefer_above) || (direction == 1 && !prefer_above);
                        let (x, y) = match direction {
                            0 | 1 => (
                                i64::from(anchor.left),
                                if above {
                                    i64::from(anchor.top) - i64::from(gap) - h
                                } else {
                                    i64::from(anchor.bottom) + i64::from(gap)
                                },
                            ),
                            2 => (
                                i64::from(anchor.left) - i64::from(gap) - w,
                                i64::from(anchor.top),
                            ),
                            _ => (
                                i64::from(anchor.right) + i64::from(gap),
                                i64::from(anchor.top),
                            ),
                        };
                        let x = x.clamp(i64::from(left), i64::from(right) - w);
                        let y = y.clamp(i64::from(top), i64::from(bottom) - h);
                        // A clamped candidate must still lie on its requested side.
                        let side_ok = match direction {
                            0 | 1 if above => y + h <= i64::from(anchor.top),
                            0 | 1 => y >= i64::from(anchor.bottom),
                            2 => x + w <= i64::from(anchor.left),
                            _ => x >= i64::from(anchor.right),
                        };
                        if !side_ok {
                            continue;
                        }
                        let distance =
                            (x - i64::from(anchor.left)).abs() + (y - i64::from(anchor.top)).abs();
                        let score = (w == dw && h == dh, w * h, -direction, -distance);
                        // Full-sized candidates use direction before distance; area
                        // only decides between reduced, scrollable viewports.
                        if best.as_ref().is_none_or(|(s, _)| score > *s) {
                            best = Some((
                                score,
                                AttachedMenuPlacement {
                                    position: Point {
                                        x: x as i32,
                                        y: y as i32,
                                    },
                                    width: w as u32,
                                    height: h as u32,
                                    opens_below: y >= i64::from(anchor.bottom),
                                },
                            ));
                        }
                    }
                }
            }
        }
    }
    best.map(|(_, result)| result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    After,
    Before,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupPlacement {
    pub(crate) position: Point,
    pub(crate) horizontal: Side,
    pub(crate) vertical: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachedMenuPlacement {
    pub(crate) position: Point,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) opens_below: bool,
}

/// Places a menu against a physical-pixel anchor rectangle and constrains it to the work area.
pub(crate) fn place_attached_menu(
    anchor: Rect,
    desired_height: u32,
    work_area: Rect,
    gap: i32,
    margin: i32,
) -> AttachedMenuPlacement {
    let margin = margin.max(0);
    let gap = gap.max(0);
    let usable_left = work_area.left.saturating_add(margin);
    let usable_top = work_area.top.saturating_add(margin);
    let usable_right = work_area.right.saturating_sub(margin);
    let usable_bottom = work_area.bottom.saturating_sub(margin);
    let usable_width = usable_right.saturating_sub(usable_left).max(1) as u32;
    let anchor_width = anchor.right.saturating_sub(anchor.left).max(1) as u32;
    let width = anchor_width.min(usable_width);
    let anchor_bottom = anchor.bottom;
    let below = usable_bottom
        .saturating_sub(anchor_bottom.saturating_add(gap))
        .max(0) as u32;
    let above = anchor
        .top
        .saturating_sub(gap)
        .saturating_sub(usable_top)
        .max(0) as u32;
    let opens_below = below >= desired_height || below >= above;
    let available_height = if opens_below { below } else { above };
    let height = desired_height.min(available_height).max(1);
    let max_x = usable_right.saturating_sub(width as i32).max(usable_left);
    let x = anchor.left.clamp(usable_left, max_x);
    let y = if opens_below {
        anchor_bottom.saturating_add(gap)
    } else {
        anchor.top.saturating_sub(gap).saturating_sub(height as i32)
    };
    AttachedMenuPlacement {
        position: Point { x, y },
        width,
        height,
        opens_below,
    }
}

/// Place a fixed-size tool window above or below its physical toolbar bounds.
/// Oversized windows retain their size and align to the work-area origin.
pub(crate) fn place_toolbar_popup(
    anchor: Rect,
    popup_width: u32,
    popup_height: u32,
    work_area: Option<Rect>,
    gap: i32,
    preferred: Side,
) -> Point {
    let width = i64::from(popup_width);
    let height = i64::from(popup_height);
    let gap = i64::from(gap.max(0));
    let before = i64::from(anchor.top) - gap - height;
    let after = i64::from(anchor.bottom) + gap;
    let (preferred_y, other_y) = match preferred {
        Side::Before => (before, after),
        Side::After => (after, before),
    };
    let Some(area) = work_area else {
        return Point {
            x: anchor.left,
            y: clamp_i32(preferred_y),
        };
    };
    let y = if preferred_y >= i64::from(area.top) && preferred_y + height <= i64::from(area.bottom)
    {
        preferred_y
    } else {
        other_y
    };
    Point {
        x: clamp_i32(clamp_origin(
            i64::from(anchor.left),
            width,
            i64::from(area.left),
            i64::from(area.right),
        )),
        y: clamp_i32(clamp_origin(
            y,
            height,
            i64::from(area.top),
            i64::from(area.bottom),
        )),
    }
}

fn clamp_origin(candidate: i64, size: i64, start: i64, end: i64) -> i64 {
    let latest_start = end - size;
    if latest_start < start {
        start
    } else {
        candidate.clamp(start, latest_start)
    }
}

/// Places a physical-size popup near an anchor and clamps it to the monitor work area.
pub(crate) fn place_popup(
    anchor: Point,
    popup_width: u32,
    popup_height: u32,
    work_area: Rect,
    gap: i32,
    margin: i32,
) -> PopupPlacement {
    let width = i64::from(popup_width);
    let height = i64::from(popup_height);
    let gap = i64::from(gap.max(0));
    let margin = i64::from(margin.max(0));

    let (x, horizontal) = place_axis(
        i64::from(anchor.x),
        width,
        i64::from(work_area.left),
        i64::from(work_area.right),
        gap,
        margin,
    );
    let (y, vertical) = place_axis(
        i64::from(anchor.y),
        height,
        i64::from(work_area.top),
        i64::from(work_area.bottom),
        gap,
        margin,
    );

    PopupPlacement {
        position: Point {
            x: clamp_i32(x),
            y: clamp_i32(y),
        },
        horizontal,
        vertical,
    }
}

fn place_axis(
    anchor: i64,
    size: i64,
    area_start: i64,
    area_end: i64,
    gap: i64,
    margin: i64,
) -> (i64, Side) {
    let usable_start = area_start + margin;
    let usable_end = area_end - margin;
    let after = anchor + gap;
    let (candidate, side) = if after + size <= usable_end {
        (after, Side::After)
    } else {
        (anchor - gap - size, Side::Before)
    };
    let clamped = clamp_origin(candidate, size, usable_start, usable_end);
    (clamped, side)
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_menus_avoid_actual_toolbars_at_all_edges_and_dpis() {
        for scale in [1., 1.25] {
            let rect = |l: f32, t: f32, r: f32, b: f32| Rect {
                left: (l * scale) as i32,
                top: (t * scale) as i32,
                right: (r * scale) as i32,
                bottom: (b * scale) as i32,
            };
            let area = rect(-1200., -300., 0., 600.);
            for (x, y) in [(-1200., -300.), (-900., 0.), (-650., 400.), (-650., -300.)] {
                let main = rect(x, y, x + 650., y + 54.);
                let panel_y = if y > 300. { y - 70. } else { y + 62. };
                let panel = rect(x, panel_y, x + 900., panel_y + 58.);
                let anchor = rect(x + 160., panel_y + 10., x + 198., panel_y + 48.);
                for (w, h) in [
                    (112., 188.),
                    (180., 440.),
                    (240., 188.),
                    (300., 320.),
                    (320., 300.),
                    (330., 80.),
                    (360., 160.),
                    (500., 200.),
                ] {
                    let result = place_annotation_popup(
                        anchor,
                        ((w * scale) as u32, (h * scale) as u32),
                        area,
                        &[main, panel],
                        panel_y < y,
                        4,
                    )
                    .unwrap();
                    let r = Rect {
                        left: result.position.x,
                        top: result.position.y,
                        right: result.position.x + result.width as i32,
                        bottom: result.position.y + result.height as i32,
                    };
                    assert!(
                        r.left >= area.left
                            && r.top >= area.top
                            && r.right <= area.right
                            && r.bottom <= area.bottom
                    );
                    for bar in [main, panel] {
                        assert!(
                            r.right <= bar.left - 4
                                || r.left >= bar.right + 4
                                || r.bottom <= bar.top - 4
                                || r.top >= bar.bottom + 4,
                            "{r:?} intersects {bar:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn annotation_placement_uses_sides_free_gaps_and_scrollable_viewports() {
        let area = Rect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 200,
        };
        let main = Rect {
            left: 0,
            top: 40,
            right: 250,
            bottom: 90,
        };
        let panel = Rect {
            left: 0,
            top: 100,
            right: 250,
            bottom: 160,
        };
        let anchor = Rect {
            left: 10,
            top: 110,
            right: 48,
            bottom: 148,
        };
        let result =
            place_annotation_popup(anchor, (240, 180), area, &[main, panel], false, 4).unwrap();
        assert!(
            result.position.x >= 254,
            "full-height right side beats truncated vertical space"
        );
        let full = Rect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 200,
        };
        assert!(place_annotation_popup(anchor, (300, 300), area, &[full], false, 4).is_none());
        let a = Rect {
            left: 0,
            top: 0,
            right: 500,
            bottom: 70,
        };
        let b = Rect {
            left: 0,
            top: 130,
            right: 500,
            bottom: 200,
        };
        let anchor = Rect {
            left: 10,
            top: 20,
            right: 48,
            bottom: 60,
        };
        let result = place_annotation_popup(anchor, (320, 300), area, &[a, b], false, 4).unwrap();
        assert_eq!(result.height, 52);
        assert_eq!(result.position.y, 74);
        let other_screen = Rect {
            left: 600,
            top: 0,
            right: 1000,
            bottom: 200,
        };
        assert_eq!(
            result,
            place_annotation_popup(anchor, (320, 300), area, &[a, b, other_screen], false, 4)
                .unwrap()
        );
    }

    const WORK_AREA: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn place(anchor: Point, area: Rect) -> PopupPlacement {
        place_popup(anchor, 420, 240, area, 12, 8)
    }

    #[test]
    fn toolbar_placement_preserves_side_preference_and_saturates_extreme_dimensions() {
        let toolbar = Rect {
            left: 50,
            top: 300,
            right: 860,
            bottom: 360,
        };
        let area = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 700,
        };
        assert_eq!(
            place_toolbar_popup(toolbar, 440, 270, Some(area), 15, Side::Before),
            Point { x: 50, y: 15 }
        );
        assert_eq!(
            place_toolbar_popup(toolbar, 440, 270, Some(area), 8, Side::After),
            Point { x: 50, y: 368 }
        );
        let low = Rect {
            top: 640,
            bottom: 700,
            ..toolbar
        };
        assert_eq!(
            place_toolbar_popup(low, 440, 270, Some(area), 8, Side::After),
            Point { x: 50, y: 362 }
        );
        let extreme = Rect {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        };
        assert_eq!(
            place_toolbar_popup(extreme, u32::MAX, u32::MAX, None, i32::MAX, Side::Before),
            Point {
                x: i32::MIN,
                y: i32::MIN
            }
        );
        assert_eq!(
            place_toolbar_popup(
                extreme,
                u32::MAX,
                u32::MAX,
                Some(area),
                i32::MAX,
                Side::After
            ),
            Point { x: 0, y: 0 }
        );
    }

    #[test]
    fn prefers_right_and_below_the_anchor() {
        assert_eq!(
            place(Point { x: 100, y: 100 }, WORK_AREA).position,
            Point { x: 112, y: 112 }
        );
    }

    #[test]
    fn flips_left_at_the_right_edge() {
        let placement = place(Point { x: 1900, y: 100 }, WORK_AREA);
        assert_eq!(placement.position, Point { x: 1468, y: 112 });
        assert_eq!(placement.horizontal, Side::Before);
    }

    #[test]
    fn flips_above_at_the_bottom_edge() {
        let placement = place(Point { x: 100, y: 1060 }, WORK_AREA);
        assert_eq!(placement.position, Point { x: 112, y: 808 });
        assert_eq!(placement.vertical, Side::Before);
    }

    #[test]
    fn flips_both_axes_in_the_bottom_right_corner() {
        assert_eq!(
            place(Point { x: 1900, y: 1060 }, WORK_AREA).position,
            Point { x: 1468, y: 808 }
        );
    }

    #[test]
    fn clamps_an_oversized_popup_to_the_work_area_origin() {
        let area = Rect {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        };
        assert_eq!(
            place(Point { x: 150, y: 100 }, area).position,
            Point { x: 8, y: 8 }
        );
    }

    #[test]
    fn preserves_negative_virtual_screen_coordinates() {
        let area = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(
            place(Point { x: -1000, y: 500 }, area).position,
            Point { x: -988, y: 512 }
        );
    }

    #[test]
    fn attached_menu_prefers_full_height_below() {
        let placement = place_attached_menu(
            Rect {
                left: 1400,
                top: 300,
                right: 1600,
                bottom: 348,
            },
            400,
            WORK_AREA,
            4,
            8,
        );
        assert_eq!(placement.position, Point { x: 1400, y: 352 });
        assert_eq!((placement.width, placement.height), (200, 400));
        assert!(placement.opens_below);
    }

    #[test]
    fn attached_menu_opens_above_and_shrinks_near_bottom() {
        let area = Rect {
            left: 0,
            top: 500,
            right: 800,
            bottom: 900,
        };
        let placement = place_attached_menu(
            Rect {
                left: 700,
                top: 820,
                right: 880,
                bottom: 868,
            },
            400,
            area,
            4,
            8,
        );
        assert_eq!(placement.position, Point { x: 612, y: 508 });
        assert_eq!((placement.width, placement.height), (180, 308));
        assert!(!placement.opens_below);
    }

    #[test]
    fn attached_menu_preserves_negative_monitor_coordinates() {
        let area = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let placement = place_attached_menu(
            Rect {
                left: -220,
                top: 300,
                right: -20,
                bottom: 348,
            },
            400,
            area,
            4,
            8,
        );
        assert_eq!(placement.position, Point { x: -220, y: 352 });
        assert_eq!(placement.width, 200);
    }
}
