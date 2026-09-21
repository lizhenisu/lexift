use lexift_core::domain::geometry::{Point, Rect};

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
    let latest_start = usable_end - size;
    let clamped = if latest_start < usable_start {
        usable_start
    } else {
        candidate.clamp(usable_start, latest_start)
    };
    (clamped, side)
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
