//! Physical-coordinate ROI composition; no UI overlays or capture pixels enter Core.
use lexift_core::{domain::geometry::Rect, ports::screen_capture::CapturedRegion};

pub(super) fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let r = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

pub(super) fn compose(bounds: Rect, layers: &[(&CapturedRegion, u8)]) -> CapturedRegion {
    let width = (bounds.right - bounds.left) as usize;
    let height = (bounds.bottom - bounds.top) as usize;
    let mut pixels = vec![0; width * height * 4];
    for (layer, opacity) in layers {
        let Some(area) = intersection(bounds, layer.bounds) else {
            continue;
        };
        let source_width = (layer.bounds.right - layer.bounds.left) as usize;
        for y in area.top..area.bottom {
            for x in area.left..area.right {
                let src = ((y - layer.bounds.top) as usize * source_width
                    + (x - layer.bounds.left) as usize)
                    * 4;
                let dst = ((y - bounds.top) as usize * width + (x - bounds.left) as usize) * 4;
                let alpha = (layer.pixels[src + 3] as u32 * *opacity as u32 + 127) / 255;
                for c in 0..3 {
                    let color = (layer.pixels[src + c] as u32 * *opacity as u32 + 127) / 255;
                    pixels[dst + c] = (color + (pixels[dst + c] as u32 * (255 - alpha) + 127) / 255)
                        .min(255) as u8;
                }
                pixels[dst + 3] =
                    (alpha + (pixels[dst + 3] as u32 * (255 - alpha) + 127) / 255) as u8;
            }
        }
    }
    CapturedRegion { bounds, pixels }
}

/// Freeze only covered desktop intersections; new geometry can reuse samples
/// from a previous ROI without copying toolbar pixels into the effect.
pub(super) fn retain_covered(
    regions: &mut [CapturedRegion],
    previous: &[CapturedRegion],
    covered: &[Rect],
) {
    for target in regions {
        let width = (target.bounds.right - target.bounds.left) as usize;
        for &cover in covered {
            for source in previous {
                let Some(area) =
                    intersection(target.bounds, cover).and_then(|r| intersection(r, source.bounds))
                else {
                    continue;
                };
                let source_width = (source.bounds.right - source.bounds.left) as usize;
                let length = (area.right - area.left) as usize * 4;
                for y in area.top..area.bottom {
                    let src = ((y - source.bounds.top) as usize * source_width
                        + (area.left - source.bounds.left) as usize)
                        * 4;
                    let dst = ((y - target.bounds.top) as usize * width
                        + (area.left - target.bounds.left) as usize)
                        * 4;
                    target.pixels[dst..dst + length]
                        .copy_from_slice(&source.pixels[src..src + length]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn covered_pixels_freeze_across_roi_changes_and_uncover_immediately() {
        let previous = CapturedRegion {
            bounds: Rect {
                left: -2,
                top: 0,
                right: 2,
                bottom: 1,
            },
            pixels: [10, 20, 30, 255].repeat(4),
        };
        let target = CapturedRegion {
            bounds: Rect {
                left: -1,
                top: 0,
                right: 3,
                bottom: 1,
            },
            pixels: [80, 90, 100, 255].repeat(4),
        };
        let mut current = vec![target.clone()];
        retain_covered(
            &mut current,
            std::slice::from_ref(&previous),
            &[Rect {
                left: 0,
                top: 0,
                right: 3,
                bottom: 1,
            }],
        );
        assert_eq!(&current[0].pixels[..4], &[80, 90, 100, 255]);
        assert_eq!(
            &current[0].pixels[4..12],
            &[10, 20, 30, 255, 10, 20, 30, 255]
        );
        assert_eq!(&current[0].pixels[12..], &[80, 90, 100, 255]);
        let mut current = vec![target.clone()];
        retain_covered(&mut current, &[previous], &[]);
        assert_eq!(current[0].pixels, target.pixels);
    }
    #[test]
    fn cropped_negative_origin_layers_keep_z_order_and_alpha() {
        let bounds = Rect {
            left: -2,
            top: 0,
            right: 0,
            bottom: 1,
        };
        let base = CapturedRegion {
            bounds,
            pixels: vec![0, 100, 0, 255, 0, 100, 0, 255],
        };
        let top = CapturedRegion {
            bounds: Rect {
                left: -1,
                top: 0,
                right: 1,
                bottom: 1,
            },
            pixels: vec![100, 0, 0, 128, 100, 0, 0, 128],
        };
        let result = compose(bounds, &[(&base, 255), (&top, 255)]);
        assert_eq!(result.pixels, [0, 100, 0, 255, 100, 50, 0, 255]);
    }
}
