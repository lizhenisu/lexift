//! Client-area toolbar dragging and hidden panel layout convergence.
use lexift_core::domain::geometry::Rect;
use slint::{PhysicalPosition, PhysicalSize};

#[derive(Clone, Copy, Debug)]
pub(super) struct Drag {
    pub origin: PhysicalPosition,
    grab: (f32, f32),
    snap: Snap,
}

impl Drag {
    pub fn new(origin: PhysicalPosition, x: f32, y: f32, scale: f32) -> Self {
        Self {
            origin,
            grab: (x / scale, y / scale),
            snap: Snap::default(),
        }
    }

    /// Keep the logical grip offset when the toolbar crosses a DPI boundary.
    pub fn position(self, cursor: PhysicalPosition, scale: f32) -> PhysicalPosition {
        PhysicalPosition::new(
            cursor
                .x
                .saturating_sub((self.grab.0 * scale).round() as i32),
            cursor
                .y
                .saturating_sub((self.grab.1 * scale).round() as i32),
        )
    }

    pub fn client_position(self, scale: f32) -> (f32, f32) {
        (self.grab.0 * scale, self.grab.1 * scale)
    }

    pub fn snapped_position(
        &mut self,
        cursor: PhysicalPosition,
        scale: f32,
        size: PhysicalSize,
        area: Option<Rect>,
    ) -> PhysicalPosition {
        let raw = self.position(cursor, scale);
        self.snap.position(raw, scale, size, area)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Snap {
    context: Option<(Rect, f32, PhysicalSize)>,
    horizontal: Option<bool>,
    vertical: Option<bool>,
}

impl Snap {
    /// Hysteresis is measured against the unmodified grab position, never against
    /// the previous snapped window. Otherwise the grip drifts or cannot detach.
    fn position(
        &mut self,
        raw: PhysicalPosition,
        scale: f32,
        size: PhysicalSize,
        area: Option<Rect>,
    ) -> PhysicalPosition {
        let context = area.map(|area| (area, scale, size));
        if self.context != context {
            *self = Self {
                context,
                ..Self::default()
            };
        }
        let Some(area) = area else { return raw };
        let enter = (12. * scale).ceil() as i64;
        let release = (20. * scale).ceil() as i64;
        PhysicalPosition::new(
            snap_axis(
                raw.x,
                size.width,
                area.left,
                area.right,
                enter,
                release,
                &mut self.horizontal,
            ),
            snap_axis(
                raw.y,
                size.height,
                area.top,
                area.bottom,
                enter,
                release,
                &mut self.vertical,
            ),
        )
    }
}

fn snap_axis(
    raw: i32,
    size: u32,
    start: i32,
    end: i32,
    enter: i64,
    release: i64,
    side: &mut Option<bool>,
) -> i32 {
    let available = i64::from(end) - i64::from(start);
    if available <= 0 || i64::from(size) > available {
        *side = None;
        return raw;
    }
    let near = i64::from(start);
    let far = i64::from(end) - i64::from(size);
    let distance = |target: i64| (i64::from(raw) - target).abs();
    if let Some(far_side) = *side {
        let target = if far_side { far } else { near };
        if distance(target) <= release {
            return target as i32;
        }
        *side = None;
    }
    let far_side = distance(far) < distance(near);
    let target = if far_side { far } else { near };
    if distance(target) <= enter {
        *side = Some(far_side);
        target as i32
    } else {
        raw
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layout {
    pub token: Option<lexift_core::ports::magnifier::WindowToken>,
    pub scale: f32,
    pub position: PhysicalPosition,
    pub size: PhysicalSize,
    pub content: (i32, bool, f32, f32),
    pub values: crate::AnnotationValues,
    pub language: i32,
}

#[derive(Default)]
pub(super) struct Preparation {
    last: Option<Layout>,
    pub measured: bool,
    pub ready: bool,
}

impl Preparation {
    /// Never reveal the first observation: native resize/DPI events must settle
    /// before a second matching layout can authorize the first visible frame.
    pub fn observe(&mut self, layout: Layout) -> bool {
        let stable = self.last.as_ref() == Some(&layout);
        self.last = Some(layout);
        stable
    }
}

pub(super) fn physical_size(width: f32, height: f32, scale: f32) -> PhysicalSize {
    PhysicalSize::new(
        (width * scale).ceil() as u32,
        (height * scale).ceil() as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_edges_corners_and_hysteresis_use_raw_position_at_both_dpis() {
        let area = Rect {
            left: -1000,
            top: -500,
            right: 1000,
            bottom: 500,
        };
        let size = PhysicalSize::new(200, 100);
        for scale in [1., 1.25] {
            let enter = (12.0_f32 * scale).ceil() as i32;
            let release = (20.0_f32 * scale).ceil() as i32;
            for (raw, expected) in [
                ((-1000 + enter, 0), (-1000, 0)),
                ((800 - enter, 0), (800, 0)),
                ((0, -500 + enter), (0, -500)),
                ((0, 400 - enter), (0, 400)),
                ((-1000 + enter, -500 + enter), (-1000, -500)),
                ((800 - enter, -500 + enter), (800, -500)),
                ((-1000 + enter, 400 - enter), (-1000, 400)),
                ((800 - enter, 400 - enter), (800, 400)),
            ] {
                let mut snap = Snap::default();
                assert_eq!(
                    snap.position(PhysicalPosition::new(raw.0, raw.1), scale, size, Some(area)),
                    PhysicalPosition::new(expected.0, expected.1)
                );
            }
            let mut drag = Drag::new(
                PhysicalPosition::new(-800, 0),
                14. * scale,
                27. * scale,
                scale,
            );
            let cursor = |offset| {
                PhysicalPosition::new(
                    -1000 + offset + (14. * scale).round() as i32,
                    (27. * scale).round() as i32,
                )
            };
            assert_eq!(
                drag.snapped_position(cursor(enter), scale, size, Some(area))
                    .x,
                -1000
            );
            assert_eq!(
                drag.snapped_position(cursor(release), scale, size, Some(area))
                    .x,
                -1000
            );
            assert_eq!(
                drag.snapped_position(cursor(release + 1), scale, size, Some(area))
                    .x,
                -1000 + release + 1
            );
            assert_eq!(
                drag.snapped_position(cursor(enter + 1), scale, size, Some(area))
                    .x,
                -1000 + enter + 1
            );
            assert_eq!(
                drag.snapped_position(cursor(enter), scale, size, Some(area))
                    .x,
                -1000
            );
        }
    }

    #[test]
    fn changing_monitor_dpi_or_size_clears_old_lock_and_missing_area_is_free() {
        let a = Rect {
            left: -1000,
            top: 0,
            right: 0,
            bottom: 1000,
        };
        let b = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 1000,
        };
        let size = PhysicalSize::new(200, 100);
        let mut snap = Snap::default();
        assert_eq!(
            snap.position(PhysicalPosition::new(-990, 300), 1., size, Some(a))
                .x,
            -1000
        );
        assert_eq!(
            snap.position(PhysicalPosition::new(16, 300), 1., size, Some(b))
                .x,
            16
        );
        assert_eq!(
            snap.position(PhysicalPosition::new(10, 300), 1., size, Some(b))
                .x,
            0
        );
        assert_eq!(
            snap.position(PhysicalPosition::new(18, 300), 1.25, size, Some(b))
                .x,
            18
        );
        assert_eq!(
            snap.position(PhysicalPosition::new(10, 300), 1.25, size, Some(b))
                .x,
            0
        );
        assert_eq!(
            snap.position(PhysicalPosition::new(10, 300), 1.25, size, None)
                .x,
            10
        );
        assert_eq!(
            snap.position(PhysicalPosition::new(10, 300), 1., size, Some(b))
                .x,
            0
        );
        assert_eq!(
            snap.position(
                PhysicalPosition::new(18, 300),
                1.,
                PhysicalSize::new(201, 100),
                Some(b)
            )
            .x,
            18
        );
        let small = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 1000,
        };
        assert_eq!(
            snap.position(PhysicalPosition::new(5, 5), 1., size, Some(small)),
            PhysicalPosition::new(5, 0)
        );
        let mut side = None;
        assert_eq!(
            snap_axis(5, 90, 0, 100, 12, 20, &mut side),
            0,
            "ties choose the near edge"
        );
        assert_eq!(
            snap_axis(i32::MIN + 10, 200, i32::MIN, i32::MAX, 12, 20, &mut None),
            i32::MIN
        );
    }

    #[test]
    fn drag_preserves_grip_across_dpi_and_negative_coordinates() {
        let drag = Drag::new(PhysicalPosition::new(-600, 40), 20., 30., 1.);
        assert_eq!(
            drag.position(PhysicalPosition::new(-580, 70), 1.),
            drag.origin
        );
        assert_eq!(
            drag.position(PhysicalPosition::new(100, 100), 1.25),
            PhysicalPosition::new(75, 62)
        );
    }

    #[test]
    fn layout_requires_two_matches_and_resets_on_content_dpi_or_token_change() {
        let mut state = Preparation::default();
        let mut layout = Layout {
            token: None,
            scale: 1.,
            position: PhysicalPosition::new(-500, 10),
            size: PhysicalSize::new(501, 73),
            content: (7, false, 500.1, 72.1),
            values: Default::default(),
            language: 0,
        };
        assert!(!state.observe(layout.clone()));
        assert!(state.observe(layout.clone()));
        layout.token = Some(lexift_core::ports::magnifier::WindowToken(42));
        assert!(!state.observe(layout.clone()));
        layout.values.size += 1;
        assert!(!state.observe(layout.clone()));
        layout.scale = 1.25;
        assert!(!state.observe(layout.clone()));
        layout.content.3 += 1.;
        assert!(!state.observe(layout.clone()));
        assert!(state.observe(layout));
        assert_eq!(physical_size(500.1, 72.1, 1.25), PhysicalSize::new(626, 91));
    }
}
