//! Frozen top-level window geometry, independent of live native window queries.
use super::geometry::{Point, Rect};
use crate::ports::magnifier::WindowToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowCandidate {
    pub window: WindowToken,
    pub bounds: Rect,
    /// Lower ranks are closer to the foreground.
    pub order: u32,
}

/// Resolve the frontmost frozen window and clip its complete frame to the desktop.
pub fn at(candidates: &[WindowCandidate], point: Point, desktop: Rect) -> Option<Rect> {
    candidates
        .iter()
        .filter_map(|candidate| {
            let b = candidate.bounds;
            let bounds = Rect {
                left: b.left.max(desktop.left),
                top: b.top.max(desktop.top),
                right: b.right.min(desktop.right),
                bottom: b.bottom.min(desktop.bottom),
            };
            (point.x >= bounds.left
                && point.x < bounds.right
                && point.y >= bounds.top
                && point.y < bounds.bottom)
                .then_some((candidate.order, bounds))
        })
        .min_by_key(|(order, _)| *order)
        .map(|(_, bounds)| bounds)
}

/// Geometry or stacking changes during capture invalidate a candidate's snapshot.
pub fn stable(before: &[WindowCandidate], after: &[WindowCandidate]) -> Vec<WindowCandidate> {
    before
        .iter()
        .copied()
        .filter(|candidate| after.contains(candidate))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreground_clipping_and_capture_changes_are_frozen() {
        let desktop = Rect {
            left: -100,
            top: -50,
            right: 200,
            bottom: 150,
        };
        let back = WindowCandidate {
            window: WindowToken(1),
            bounds: desktop,
            order: 3,
        };
        let front = WindowCandidate {
            window: WindowToken(2),
            bounds: Rect {
                left: -200,
                top: -100,
                right: 50,
                bottom: 70,
            },
            order: 0,
        };
        assert_eq!(
            at(&[back, front], Point { x: -40, y: 10 }, desktop),
            Some(Rect {
                left: -100,
                top: -50,
                right: 50,
                bottom: 70
            })
        );
        assert_eq!(at(&[front], Point { x: 90, y: 10 }, desktop), None);
        assert_eq!(at(&[front], Point { x: 50, y: 10 }, desktop), None);
        let moved = WindowCandidate {
            bounds: desktop,
            ..front
        };
        assert_eq!(stable(&[back, front], &[back, moved]), vec![back]);
        assert!(at(&[], Point { x: 0, y: 0 }, desktop).is_none());
    }
}
