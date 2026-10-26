//! Bounded viewport pixels from immutable, original-resolution preview buffers.
use crate::annotation::image_viewport::PreciseViewport;

/// Sampling touches only the bounded destination; zoom never allocates scaled-source pixels.
pub(super) fn raster(
    source: tiny_skia::PixmapRef<'_>,
    view: &PreciseViewport,
    origin: [f64; 2],
    fill: Option<slint::Color>,
) -> Option<slint::Image> {
    let (width, height) = (
        view.size[0].round().max(1.) as u32,
        view.size[1].round().max(1.) as u32,
    );
    if u64::from(width) * u64::from(height) > 16_000_000 {
        return None;
    }
    let mut out = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    let xs = (0..width)
        .map(|x| {
            let q = (f64::from(x) + 0.5 - view.offset[0]) / view.scale;
            (q >= view.content[0]
                && q < view.content[2]
                && q >= origin[0]
                && q < origin[0] + f64::from(source.width()))
            .then_some((q - origin[0]).floor().max(0.) as usize)
        })
        .collect::<Vec<_>>();
    for (y, row) in out
        .make_mut_slice()
        .chunks_exact_mut(width as usize)
        .enumerate()
    {
        let q = (y as f64 + 0.5 - view.offset[1]) / view.scale;
        if q < view.content[1]
            || q >= view.content[3]
            || q < origin[1]
            || q >= origin[1] + f64::from(source.height())
        {
            continue;
        }
        let sy = (q - origin[1]).floor() as usize;
        for (dst, x) in row.iter_mut().zip(&xs) {
            if let Some(x) = x {
                let i = (sy * source.width() as usize + x) * 4;
                let c = &source.data()[i..i + 4];
                let bg = fill.unwrap_or(slint::Color::from_argb_u8(0, 0, 0, 0));
                let blend = |a: u8, b: u8| {
                    (u16::from(a) + (u16::from(b) * u16::from(255 - c[3]) + 127) / 255).min(255)
                        as u8
                };
                *dst = slint::Rgba8Pixel::new(
                    blend(c[0], bg.red()),
                    blend(c[1], bg.green()),
                    blend(c[2], bg.blue()),
                    if fill.is_some() { 255 } else { c[3] },
                );
            }
        }
    }
    Some(slint::Image::from_rgba8_premultiplied(out))
}
