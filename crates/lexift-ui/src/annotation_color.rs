//! Opaque annotation color conversion and validated HEX parsing.
#[derive(Clone, Copy, Debug)]
pub(super) struct Hsv {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}
impl Hsv {
    pub fn from_rgb(rgb: [u8; 3]) -> Self {
        let [r, g, b] = rgb.map(|v| v as f32 / 255.);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let hue = if d == 0. {
            0.
        } else if max == r {
            60. * ((g - b) / d).rem_euclid(6.)
        } else if max == g {
            60. * ((b - r) / d + 2.)
        } else {
            60. * ((r - g) / d + 4.)
        };
        Self {
            hue,
            saturation: if max == 0. { 0. } else { d / max },
            value: max,
        }
    }
    pub fn rgb(self) -> [u8; 3] {
        let h = self.hue.rem_euclid(360.) / 60.;
        let v = self.value.clamp(0., 1.);
        let c = v * self.saturation.clamp(0., 1.);
        let x = c * (1. - (h.rem_euclid(2.) - 1.).abs());
        let rgb = match h as u32 {
            0 => [c, x, 0.],
            1 => [x, c, 0.],
            2 => [0., c, x],
            3 => [0., x, c],
            4 => [x, 0., c],
            _ => [c, 0., x],
        };
        rgb.map(|n| ((n + v - c) * 255.).round().clamp(0., 255.) as u8)
    }
}
pub(super) fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let text = text.trim();
    let code = text.strip_prefix('#').unwrap_or(text);
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&code[0..2], 16).ok()?,
        u8::from_str_radix(&code[2..4], 16).ok()?,
        u8::from_str_radix(&code[4..6], 16).ok()?,
    ])
}
pub(super) fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hsv_round_trips_chromatic_and_achromatic_colors() {
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    assert_eq!(Hsv::from_rgb([r, g, b]).rgb(), [r, g, b]);
                }
            }
        }
        assert_eq!(
            Hsv {
                hue: 360.,
                saturation: 1.,
                value: 1.
            }
            .rgb(),
            [255, 0, 0]
        );
        assert_eq!(
            Hsv {
                hue: 120.,
                saturation: 1.,
                value: 0.
            }
            .rgb(),
            [0, 0, 0]
        );
        assert_eq!(
            Hsv {
                hue: 240.,
                saturation: 0.,
                value: 1.
            }
            .rgb(),
            [255, 255, 255]
        );
    }
    #[test]
    fn hex_accepts_optional_prefix_and_rejects_incomplete_or_unicode() {
        assert_eq!(parse_hex(" #aBcD09 "), Some([171, 205, 9]));
        assert_eq!(parse_hex("ABCDEF"), Some([171, 205, 239]));
        for invalid in [
            "#FFF",
            "#12345678",
            "gg0000",
            "颜色",
            "#12345",
            "",
            "#12 345",
        ] {
            assert_eq!(parse_hex(invalid), None);
        }
        assert_eq!(hex([0, 128, 255]), "#0080FF");
    }
}
