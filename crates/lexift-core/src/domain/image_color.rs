//! Non-destructive document color treatment of premultiplied RGBA pixels.
use crate::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub grayscale: bool,
    pub inverted: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Grayscale,
    Invert,
}
impl Color {
    pub fn action(mut self, action: Action) -> Self {
        match action {
            Action::Grayscale => self.grayscale = !self.grayscale,
            Action::Invert => self.inverted = !self.inverted,
        }
        self
    }
    /// Process owned display/output pixels once; UI decoration is a separate layer.
    pub fn apply(self, pixels: &mut [u8], cancel: &AtomicBool) -> Result<()> {
        if !pixels.len().is_multiple_of(4) {
            return Err(Error::new("Invalid RGBA buffer"));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(Error::new("Image processing cancelled"));
        }
        if self == Self::default() {
            return Ok(());
        }
        for (i, p) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if i.is_multiple_of(4096) && cancel.load(Ordering::Acquire) {
                return Err(Error::new("Image processing cancelled"));
            }
            let a = p[3];
            if a == 0 {
                p.fill(0);
                continue;
            }
            if self.grayscale {
                let y =
                    ((54 * u32::from(p[0]) + 183 * u32::from(p[1]) + 19 * u32::from(p[2]) + 128)
                        >> 8) as u8;
                p[..3].fill(y.min(a));
            }
            if self.inverted {
                for c in &mut p[..3] {
                    *c = a.saturating_sub(*c);
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_premultiplied_colors_and_combinations() {
        for a in [0u8, 1, 80, 128, 255] {
            for grayscale in [false, true] {
                for inverted in [false, true] {
                    let color = Color {
                        grayscale,
                        inverted,
                    };
                    let original = [a, a / 2, a / 3, a];
                    let mut pixel = original;
                    color.apply(&mut pixel, &AtomicBool::new(false)).unwrap();
                    let y =
                        ((54 * u32::from(a) + 183 * u32::from(a / 2) + 19 * u32::from(a / 3) + 128)
                            / 256) as u8;
                    let rgb = if grayscale { [y; 3] } else { [a, a / 2, a / 3] };
                    let mut expected = original;
                    for i in 0..3 {
                        expected[i] = if inverted { a - rgb[i] } else { rgb[i] };
                    }
                    assert_eq!(pixel, expected);
                }
            }
        }
        let mut transparent = [1, 2, 3, 0];
        Color {
            grayscale: true,
            inverted: true,
        }
        .apply(&mut transparent, &AtomicBool::new(false))
        .unwrap();
        assert_eq!(transparent, [0; 4]);
    }
    #[test]
    fn inverse_cancellation_and_invalid_buffer() {
        let c = Color {
            grayscale: false,
            inverted: true,
        };
        let mut p = [2, 30, 50, 80];
        let before = p;
        c.apply(&mut p, &AtomicBool::new(false)).unwrap();
        c.apply(&mut p, &AtomicBool::new(false)).unwrap();
        assert_eq!(p, before);
        assert!(c.apply(&mut p, &AtomicBool::new(true)).is_err());
        assert_eq!(p, before);
        assert!(c.apply(&mut [0; 3], &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn shared_history_restores_color_without_old_history() {
        use crate::domain::{
            annotation::Session,
            geometry::Rect,
            image_transform::{Action as Turn, Orientation},
        };
        let mut s = Session::new();
        let full = Rect {
            left: -10,
            top: 2,
            right: 90,
            bottom: 202,
        };
        s.image_source = Some(full);
        let gray = Color::default().action(Action::Grayscale);
        assert!(s.color_image(gray));
        assert!(!s.color_image(gray));
        assert!(s.orient_image(Orientation::identity().action(Turn::Right)));
        assert!(s.color_image(gray.action(Action::Invert)));
        assert!(s.undo());
        assert_eq!(s.image_color, gray);
        assert!(s.undo());
        assert_eq!(s.image_orientation, Orientation::identity());
        assert!(s.undo());
        assert_eq!(s.image_color, Color::default());
        assert!(!s.color_image(Color::default()));
        assert!(s.redo());
        let frozen = s.freeze_visible(&s);
        assert_eq!(frozen.image_color, gray);
        let restored = frozen.restored_image();
        assert_eq!(restored.image_color, gray);
        assert!(!restored.can_undo());
        assert!(!restored.can_redo());
    }
}
