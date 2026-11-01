//! Shared opaque BGRA to limited-range BT.709 NV12 conversion.
/// Limited-range BT.709, averaging chroma over each 2x2 block before rounding.
/// Input is the opaque physical desktop in BGRA order, with no alpha unpremultiply.
pub fn nv12(bgra: &[u8], w: usize, h: usize, out: &mut [u8]) {
    assert!(w.is_multiple_of(2) && h.is_multiple_of(2));
    assert_eq!(bgra.len(), w * h * 4);
    assert_eq!(out.len(), w * h * 3 / 2);
    let (y, uv) = out.split_at_mut(w * h);
    for row in 0..h {
        for x in 0..w {
            let p = &bgra[(row * w + x) * 4..];
            y[row * w + x] = (16
                + ((47 * i32::from(p[2]) + 157 * i32::from(p[1]) + 16 * i32::from(p[0]) + 128)
                    >> 8))
                .clamp(16, 235) as u8;
        }
    }
    for row in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut r = 0;
            let mut g = 0;
            let mut b = 0;
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let p = &bgra[((row + dy) * w + x + dx) * 4..];
                r += i32::from(p[2]);
                g += i32::from(p[1]);
                b += i32::from(p[0]);
            }
            let offset = row / 2 * w + x;
            uv[offset] = (128 + ((-26 * r - 86 * g + 112 * b + 512) >> 10)).clamp(16, 240) as u8;
            uv[offset + 1] =
                (128 + ((112 * r - 102 * g - 10 * b + 512) >> 10)).clamp(16, 240) as u8;
        }
    }
}

#[test]
fn nv12_known_bt709_blocks_and_dimensions() {
    for (bgr, expected) in [
        ([0, 0, 0], [16, 128, 128]),
        ([255, 255, 255], [235, 128, 128]),
        ([0, 0, 255], [63, 102, 240]),
        ([255, 0, 0], [32, 240, 118]),
    ] {
        let src: Vec<_> = (0..4).flat_map(|_| [bgr[0], bgr[1], bgr[2], 255]).collect();
        let mut out = [0; 6];
        nv12(&src, 2, 2, &mut out);
        assert_eq!(&out[..4], &[expected[0]; 4]);
        assert_eq!(&out[4..], &expected[1..]);
    }
}
