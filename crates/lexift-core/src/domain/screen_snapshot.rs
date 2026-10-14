//! Checked physical-pixel operations on stable desktop snapshots.
use super::geometry::Rect;

/// Preserve source monitor DPI when a frozen image crosses display boundaries.
pub fn render_partitions(
    bounds: Rect,
    monitors: &[(Rect, f32)],
    fallback: f32,
) -> Vec<(Rect, f32)> {
    let parts: Vec<_> = monitors
        .iter()
        .filter_map(|(m, scale)| {
            let b = Rect {
                left: bounds.left.max(m.left),
                top: bounds.top.max(m.top),
                right: bounds.right.min(m.right),
                bottom: bounds.bottom.min(m.bottom),
            };
            (b.left < b.right && b.top < b.bottom).then_some((b, *scale))
        })
        .collect();
    if parts.is_empty() {
        vec![(bounds, fallback)]
    } else {
        parts
    }
}

/// Merge frozen monitor pixels in physical coordinates; gaps between monitors remain white.
pub fn crop_frames(
    frames: &[crate::ports::screen_capture::CapturedRegion],
    bounds: Rect,
) -> crate::Result<crate::ports::screen_capture::CapturedRegion> {
    use crate::{Error, ports::screen_capture::CapturedRegion};
    let width = i64::from(bounds.right) - i64::from(bounds.left);
    let height = i64::from(bounds.bottom) - i64::from(bounds.top);
    if width <= 0 || height <= 0 || width.checked_mul(height).is_none_or(|n| n > 40_000_000) {
        return Err(Error::new("OCR selection is too large"));
    }
    let mut pixels = vec![255; width as usize * height as usize * 4];
    let mut copied = false;
    for frame in frames {
        let b = frame.bounds;
        let fw = (i64::from(b.right) - i64::from(b.left)).max(0) as usize;
        let fh = (i64::from(b.bottom) - i64::from(b.top)).max(0) as usize;
        if fw == 0
            || fh == 0
            || fw.checked_mul(fh).and_then(|n| n.checked_mul(4)) != Some(frame.pixels.len())
        {
            return Err(Error::new("Invalid frozen image"));
        }
        let left = b.left.max(bounds.left);
        let right = b.right.min(bounds.right);
        let top = b.top.max(bounds.top);
        let bottom = b.bottom.min(bounds.bottom);
        if left >= right || top >= bottom {
            continue;
        }
        copied = true;
        for y in top..bottom {
            let source = ((i64::from(y) - i64::from(b.top)) as usize * fw
                + (i64::from(left) - i64::from(b.left)) as usize)
                * 4;
            let target = ((i64::from(y) - i64::from(bounds.top)) as usize * width as usize
                + (i64::from(left) - i64::from(bounds.left)) as usize)
                * 4;
            let bytes = (right - left) as usize * 4;
            pixels[target..target + bytes].copy_from_slice(&frame.pixels[source..source + bytes]);
        }
    }
    if !copied {
        return Err(Error::new("Selection is outside the captured desktop"));
    }
    Ok(CapturedRegion { bounds, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::screen_capture::CapturedRegion;
    #[test]
    fn rejects_malformed_buffers_empty_geometry_and_uncovered_selection() {
        let bounds = Rect {
            left: -5,
            top: -5,
            right: 5,
            bottom: 5,
        };
        for frame in [
            CapturedRegion {
                bounds,
                pixels: vec![0; 399],
            },
            CapturedRegion {
                bounds: Rect::default(),
                pixels: vec![],
            },
        ] {
            assert!(crop_frames(&[frame], bounds).is_err());
        }
        assert!(crop_frames(&[], bounds).is_err());
        assert!(crop_frames(&[], Rect::default()).is_err());
        assert!(
            crop_frames(
                &[],
                Rect {
                    left: i32::MIN,
                    top: i32::MIN,
                    right: i32::MAX,
                    bottom: i32::MAX
                }
            )
            .is_err()
        );
    }
    #[test]
    fn exact_pixels_and_monitor_gaps_survive_crop() {
        let frames = [
            CapturedRegion {
                bounds: Rect {
                    left: -2,
                    top: 0,
                    right: 0,
                    bottom: 1,
                },
                pixels: vec![10; 8],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 1,
                    top: 0,
                    right: 2,
                    bottom: 1,
                },
                pixels: vec![20; 4],
            },
        ];
        let result = crop_frames(
            &frames,
            Rect {
                left: -1,
                top: 0,
                right: 2,
                bottom: 1,
            },
        )
        .unwrap();
        assert_eq!(
            result.pixels,
            [10, 10, 10, 10, 255, 255, 255, 255, 20, 20, 20, 20]
        );
    }
}
