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
