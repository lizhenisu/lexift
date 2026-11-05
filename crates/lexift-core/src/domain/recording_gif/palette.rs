//! Stable spatial ordered dithering for an arbitrary, shared GIF palette.
use crate::{Error, Result};
use std::collections::HashMap;

pub struct Mapping {
    nearest: Vec<u8>,
    exact: HashMap<[u8; 3], u8>,
    colors: Vec<[u8; 3]>,
}
impl Mapping {
    /// A bounded RGB666 lookup avoids searching 256 colors for every video pixel.
    pub fn new(colors: &[[u8; 3]], cancelled: impl Fn() -> bool) -> Result<Self> {
        if colors.is_empty() || colors.len() > 256 {
            return Err(Error::new("Invalid GIF palette"));
        }
        let mut nearest = Vec::new();
        nearest
            .try_reserve_exact(262144)
            .map_err(|_| Error::new("GIF palette buffer unavailable"))?;
        nearest.resize(262144, 0);
        let exact = colors
            .iter()
            .enumerate()
            .map(|(i, c)| (*c, i as u8))
            .collect();
        for (key, slot) in nearest.iter_mut().enumerate() {
            if key % 256 == 0 && cancelled() {
                return Err(Error::new("GIF export cancelled"));
            }
            let rgb = [
                ((key >> 12) * 4 + 2) as i32,
                (((key >> 6) & 63) * 4 + 2) as i32,
                ((key & 63) * 4 + 2) as i32,
            ];
            *slot = colors
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| {
                    [54, 183, 19]
                        .iter()
                        .enumerate()
                        .map(|(ch, w)| {
                            let d = rgb[ch] - i32::from(c[ch]);
                            d * d * w
                        })
                        .sum::<i32>()
                })
                .unwrap()
                .0 as u8;
        }
        Ok(Self {
            nearest,
            exact,
            colors: colors.to_vec(),
        })
    }
    pub fn row(&self, bgra: &[u8], y: usize, output: &mut [u8]) -> Result<()> {
        if bgra.len() != output.len().checked_mul(4).unwrap_or(0) {
            return Err(Error::new("Invalid GIF pixel row"));
        }
        const BAYER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        for (x, (pixel, to)) in bgra.as_chunks::<4>().0.iter().zip(output).enumerate() {
            let rgb = [pixel[2], pixel[1], pixel[0]];
            *to = if let Some(index) = self.exact.get(&rgb) {
                *index
            } else {
                let key = |rgb: [u8; 3]| {
                    ((rgb[0] as usize >> 2) << 12)
                        | ((rgb[1] as usize >> 2) << 6)
                        | (rgb[2] as usize >> 2)
                };
                let nearby = self.nearest[key(rgb)];
                if rgb
                    .iter()
                    .zip(self.colors[nearby as usize])
                    .all(|(a, b)| a.abs_diff(b) <= 8)
                {
                    // Colors within one palette quantization step do not need dithering. Preserve quiet
                    // backgrounds instead of adding an artificial checkerboard.
                    nearby
                } else {
                    let offset = BAYER[y % 4][x % 4] - 7;
                    let shifted = rgb.map(|n| (i32::from(n) + offset).clamp(0, 255) as u8);
                    self.nearest[key(shifted)]
                }
            };
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_colors_survive_and_pattern_is_spatial_not_frame_relative() {
        let colors = [[0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 128, 255]];
        let map = Mapping::new(&colors, || false).unwrap();
        for y in 0..8 {
            let mut row = [0; 4];
            map.row(
                &[
                    0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 255, 255, 255, 128, 0, 255,
                ],
                y,
                &mut row,
            )
            .unwrap();
            assert_eq!(row, [0, 1, 2, 3]);
        }
        let quiet = Mapping::new(&[[33, 58, 74], [0, 0, 0], [255, 255, 255]], || false).unwrap();
        let flat = [77, 52, 32, 255].repeat(16);
        let mut flat_indices = [0; 16];
        for y in 0..4 {
            quiet.row(&flat, y, &mut flat_indices).unwrap();
            assert_eq!(flat_indices, [0; 16]);
        }
        let row = vec![127; 64];
        let (mut a, mut b) = ([0; 16], [0; 16]);
        map.row(&row, 5, &mut a).unwrap();
        map.row(&row, 9, &mut b).unwrap();
        assert_eq!(a, b);
        assert!(Mapping::new(&colors, || true).is_err());
        assert!(map.row(&[], 0, &mut a).is_err());
    }
}
