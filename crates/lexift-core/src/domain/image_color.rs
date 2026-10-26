//! Non-destructive document color treatment of premultiplied RGBA pixels.
use crate::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// A document adjustment, restricted to the nine supported menu levels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Brightness(i16);
impl Brightness {
    pub const ALL: [Self; 9] = [
        Self(-100),
        Self(-75),
        Self(-50),
        Self(-25),
        Self(0),
        Self(25),
        Self(50),
        Self(75),
        Self(100),
    ];
    pub fn new(value: i32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|level| i32::from(level.0) == value)
    }
    pub fn value(self) -> i16 {
        self.0
    }
}

/// Contrast uses the same supported levels, with a distinct document meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Contrast(Brightness);
impl Contrast {
    pub fn new(value: i32) -> Option<Self> {
        Brightness::new(value).map(Self)
    }
    pub fn levels() -> [Self; 9] {
        Brightness::ALL.map(Self)
    }
    pub fn value(self) -> i16 {
        self.0.value()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub grayscale: bool,
    pub inverted: bool,
    pub brightness: Brightness,
    pub contrast: Contrast,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Grayscale,
    Invert,
    Brightness(Brightness),
    Contrast(Contrast),
}
impl Color {
    pub fn action(mut self, action: Action) -> Self {
        match action {
            Action::Grayscale => self.grayscale = !self.grayscale,
            Action::Invert => self.inverted = !self.inverted,
            Action::Brightness(level) => self.brightness = level,
            Action::Contrast(level) => self.contrast = level,
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
            let contrast = i32::from(self.contrast.value());
            if contrast != 0 {
                for c in &mut p[..3] {
                    // Work around the premultiplied midpoint without unpremultiplying.
                    let n = 200 * i32::from(*c) + contrast * (2 * i32::from(*c) - i32::from(a));
                    let rounded = n.signum() * ((n.abs() + 100) / 200);
                    *c = rounded.clamp(0, i32::from(a)) as u8;
                }
            }
            let level = self.brightness.value();
            if level != 0 {
                let offset = level.signum() * ((i16::from(a) * level.abs() + 50) / 100);
                for c in &mut p[..3] {
                    *c = (i16::from(*c) + offset).clamp(0, i16::from(a)) as u8;
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
    fn contrast_and_brightness_match_independent_reference_for_every_alpha() {
        for value in [i32::MIN, -101, -99, 1, 24, 101, i32::MAX] {
            assert!(Contrast::new(value).is_none());
        }
        for contrast in Contrast::levels() {
            assert_eq!(Contrast::new(i32::from(contrast.value())), Some(contrast));
            for brightness in Brightness::ALL {
                for a in 0..=255u8 {
                    for grayscale in [false, true] {
                        for inverted in [false, true] {
                            let original = [a / 7, a / 2, a, a];
                            let mut actual = original;
                            Color {
                                grayscale,
                                inverted,
                                brightness,
                                contrast,
                            }
                            .apply(&mut actual, &AtomicBool::new(false))
                            .unwrap();
                            let gray = (54 * u32::from(original[0])
                                + 183 * u32::from(original[1])
                                + 19 * u32::from(original[2])
                                + 128)
                                / 256;
                            for i in 0..3 {
                                let mut c = if grayscale {
                                    f64::from(gray)
                                } else {
                                    f64::from(original[i])
                                };
                                if inverted {
                                    c = f64::from(a) - c;
                                }
                                let mid = f64::from(a) / 2.;
                                c = ((c - mid) * (1. + f64::from(contrast.value()) / 100.) + mid)
                                    .round()
                                    .clamp(0., f64::from(a));
                                let delta =
                                    (f64::from(a) * f64::from(brightness.value()) / 100.).round();
                                assert_eq!(
                                    actual[i],
                                    (c + delta).clamp(0., f64::from(a)) as u8,
                                    "{a} {contrast:?} {brightness:?}"
                                );
                            }
                            assert_eq!(actual[3], a);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn contrast_midpoint_endpoints_reset_and_mixed_history() {
        use crate::domain::{
            annotation::{Bounds, Extra, Kind, Object, Session, Style},
            geometry::Rect,
            image_transform::{Action as Turn, Orientation},
        };
        let mut session = Session::new();
        session.image_source = Some(Rect {
            left: -10,
            top: 2,
            right: 90,
            bottom: 202,
        });
        session.add(Object {
            id: Default::default(),
            image_basis: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((0., 10.), (20., 30.)),
            points: vec![],
            erased: vec![],
            style: Style::default(),
            extra: Extra::None,
        });
        let baseline = Color::default()
            .action(Action::Grayscale)
            .action(Action::Invert)
            .action(Action::Brightness(Brightness::new(25).unwrap()));
        session.color_image(baseline);
        assert!(session.set_image_crop(
            Rect {
                left: 0,
                top: 10,
                right: 50,
                bottom: 100
            },
            session.image_source.unwrap()
        ));
        session.orient_image(Orientation::identity().action(Turn::Right));
        let changed = baseline.action(Action::Contrast(Contrast::new(100).unwrap()));
        assert!(session.color_image(changed));
        assert!(session.undo());
        assert!(!session.color_image(baseline));
        assert!(session.can_redo());
        assert!(session.redo());
        let frozen = session.freeze_visible(&session).restored_image();
        assert_eq!(frozen.image_color, changed);
        assert_eq!(frozen.objects.len(), 1);
        assert_eq!(frozen.image_crop, session.image_crop);
        assert_eq!(frozen.image_orientation, session.image_orientation);
        assert!(!frozen.can_undo());
        let original = [25, 90, 210, 255];
        let mut reset = original;
        changed
            .action(Action::Contrast(Contrast::default()))
            .apply(&mut reset, &AtomicBool::new(false))
            .unwrap();
        let mut expected = original;
        baseline
            .apply(&mut expected, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(reset, expected);
        let mut gray = [0, 1, 255, 255];
        Color::default()
            .action(Action::Contrast(Contrast::new(-100).unwrap()))
            .apply(&mut gray, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(gray, [128, 128, 128, 255]);
        let mut doubled = [96, 160, 128, 255];
        Color::default()
            .action(Action::Contrast(Contrast::new(100).unwrap()))
            .apply(&mut doubled, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(doubled, [65, 193, 129, 255]);
    }
    #[test]
    fn nine_levels_match_independent_reference_for_every_alpha() {
        for value in [i32::MIN, -101, -99, 1, 24, 101, i32::MAX] {
            assert!(Brightness::new(value).is_none());
        }
        for level in Brightness::ALL {
            assert_eq!(Brightness::new(i32::from(level.value())), Some(level));
            for a in 0..=255u8 {
                for grayscale in [false, true] {
                    for inverted in [false, true] {
                        let original = [a / 7, a / 2, a, a];
                        let mut actual = original;
                        Color {
                            grayscale,
                            inverted,
                            brightness: level,
                            contrast: Default::default(),
                        }
                        .apply(&mut actual, &AtomicBool::new(false))
                        .unwrap();
                        let gray = (54 * u32::from(original[0])
                            + 183 * u32::from(original[1])
                            + 19 * u32::from(original[2])
                            + 128)
                            / 256;
                        for i in 0..3 {
                            let mut c = if grayscale {
                                f64::from(gray)
                            } else {
                                f64::from(original[i])
                            };
                            if inverted {
                                c = f64::from(a) - c;
                            }
                            let shift = (f64::from(a) * f64::from(level.value()) / 100.).round();
                            assert_eq!(actual[i], (c + shift).clamp(0., f64::from(a)) as u8);
                        }
                        assert_eq!(actual[3], a);
                    }
                }
            }
        }
    }
    #[test]
    fn brightness_history_restores_original_resources_and_redo_branch() {
        use crate::domain::annotation::Session;
        let mut session = Session::new();
        session.image_source = Some(crate::domain::geometry::Rect {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        });
        let gray = Color::default()
            .action(Action::Grayscale)
            .action(Action::Invert);
        assert!(session.color_image(gray));
        let brighter = gray.action(Action::Brightness(Brightness::new(50).unwrap()));
        assert!(session.color_image(brighter));
        assert!(session.undo());
        assert!(!session.color_image(gray));
        assert!(session.can_redo());
        assert!(session.redo());
        let snapshot = session.freeze_visible(&session);
        let restored = snapshot.restored_image();
        assert_eq!(restored.image_color, brighter);
        assert!(!restored.can_undo());
        let mut pixels = [20, 80, 140, 255];
        gray.apply(&mut pixels, &AtomicBool::new(false)).unwrap();
        let mut reset_pixels = [20, 80, 140, 255];
        brighter
            .action(Action::Brightness(Brightness::default()))
            .apply(&mut reset_pixels, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(pixels, reset_pixels);
    }
    #[test]
    fn exact_premultiplied_colors_and_combinations() {
        for a in [0u8, 1, 80, 128, 255] {
            for grayscale in [false, true] {
                for inverted in [false, true] {
                    let color = Color {
                        grayscale,
                        inverted,
                        ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
