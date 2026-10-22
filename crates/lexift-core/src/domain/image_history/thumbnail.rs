//! Bounded premultiplied-RGBA previews prepared off the UI thread.
use crate::{Error, Result, domain::geometry::Rect, ports::screen_capture::CapturedRegion};

pub fn create(image: &CapturedRegion, max_width: u32, max_height: u32) -> Result<CapturedRegion> {
    let width = i64::from(image.bounds.right) - i64::from(image.bounds.left);
    let height = i64::from(image.bounds.bottom) - i64::from(image.bounds.top);
    if width <= 0
        || height <= 0
        || max_width == 0
        || max_height == 0
        || width.checked_mul(height).and_then(|n| n.checked_mul(4))
            != Some(image.pixels.len() as i64)
    {
        return Err(Error::new("Invalid history image dimensions"));
    }
    let (width, height) = (width as u64, height as u64);
    let (w, h) = if width <= u64::from(max_width) && height <= u64::from(max_height) {
        (width, height)
    } else if u64::from(max_width) * height <= u64::from(max_height) * width {
        (
            u64::from(max_width),
            (height * u64::from(max_width) / width).max(1),
        )
    } else {
        (
            (width * u64::from(max_height) / height).max(1),
            u64::from(max_height),
        )
    };
    let len = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Error::new("History preview is too large"))?;
    let mut pixels = vec![0; len];
    for y in 0..h {
        let source_y = ((2 * y + 1) * height / (2 * h)).min(height - 1);
        for x in 0..w {
            let source_x = ((2 * x + 1) * width / (2 * w)).min(width - 1);
            let source = ((source_y * width + source_x) * 4) as usize;
            let dest = ((y * w + x) * 4) as usize;
            pixels[dest..dest + 4].copy_from_slice(&image.pixels[source..source + 4]);
        }
    }
    Ok(CapturedRegion {
        bounds: Rect {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        },
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_preview_preserves_source_pixels_and_nonzero_origin() {
        let source = CapturedRegion {
            bounds: Rect {
                left: -30,
                top: 9,
                right: -28,
                bottom: 13,
            },
            pixels: (0..8).flat_map(|v| [v, 2, 3, 255]).collect(),
        };
        let same = create(&source, 20, 20).unwrap();
        assert_eq!(same.pixels, source.pixels);
        let small = create(&source, 1, 2).unwrap();
        assert_eq!((small.bounds.right, small.bounds.bottom), (1, 2));
        assert_eq!((small.pixels[0], small.pixels[4]), (3, 7));
        let mut invalid = source.clone();
        invalid.pixels.pop();
        assert!(create(&invalid, 1, 2).is_err());
        assert!(create(&source, 0, 1).is_err());
    }
}
