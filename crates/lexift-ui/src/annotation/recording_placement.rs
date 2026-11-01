//! Place the two independent bars together once; subsequent progress never moves them.
use lexift_core::domain::geometry::{Point, Rect};
pub(super) fn bars(
    source: Rect,
    area: Rect,
    control: (i32, i32),
    annotation: (i32, i32),
    gap: i32,
) -> (Point, Point) {
    let row = control.0 + annotation.0 + gap <= area.right - area.left;
    let width = if row {
        control.0 + annotation.0 + gap
    } else {
        control.0.max(annotation.0)
    };
    let height = if row {
        control.1.max(annotation.1)
    } else {
        control.1 + annotation.1 + gap
    };
    let x = source
        .left
        .clamp(area.left, (area.right - width).max(area.left));
    let y = if source.bottom + gap + height <= area.bottom {
        source.bottom + gap
    } else if source.top - gap - height >= area.top {
        source.top - gap - height
    } else {
        (area.bottom - height - gap).max(area.top)
    };
    (
        Point { x, y },
        if row {
            Point {
                x: x + control.0 + gap,
                y,
            }
        } else {
            Point {
                x,
                y: y + control.1 + gap,
            }
        },
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_and_fullscreen_regions_keep_bars_separate_on_negative_monitors() {
        let area = Rect {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: 0,
        };
        for source in [
            area,
            Rect {
                left: -1800,
                top: -900,
                right: -1500,
                bottom: -700,
            },
        ] {
            for (control, annotation) in [
                ((380, 40), (900, 54)),
                ((475, 50), (1125, 68)),
                ((500, 50), (1600, 68)),
            ] {
                let (a, b) = bars(source, area, control, annotation, 10);
                assert!(a.x >= area.left && a.y >= area.top && b.x >= area.left && b.y >= area.top);
                assert!(a.x + control.0 <= area.right && a.y + control.1 <= area.bottom);
                assert!(b.x + annotation.0 <= area.right && b.y + annotation.1 <= area.bottom);
                assert!(
                    a.x + control.0 <= b.x
                        || a.y + control.1 <= b.y
                        || b.x + annotation.0 <= a.x
                        || b.y + annotation.1 <= a.y
                );
            }
        }
    }
}
