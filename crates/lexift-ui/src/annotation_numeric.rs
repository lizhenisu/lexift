//! Numeric menus edit one field against the current target, never a panel snapshot.
use crate::AnnotationValues;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Parameter {
    Size,
    Rounding,
    Strength,
    FontSize,
    Zoom,
    Outline,
    Opacity,
    BackgroundRounding,
    Padding,
    Caption,
    TextureSize,
}

impl Parameter {
    pub fn step(self, value: i32, tool: i32, delta: f32) -> Option<i32> {
        if !delta.is_finite() || delta == 0. {
            return None;
        }
        let (min, max) = self.bounds(tool);
        Some(
            value
                .saturating_add(if delta > 0. { 1 } else { -1 })
                .clamp(min, max),
        )
    }
    /// The same permission gate is used by buttons, menus and retained callbacks.
    pub fn enabled(self, tool: i32, v: &AnnotationValues) -> bool {
        match self {
            Self::Size => match tool {
                0 | 1 => !v.fill,
                2 => v.erase,
                3 | 5 | 6 | 7 => true,
                4 => v.shape == 0,
                11..=13 => v.shape == 1,
                _ => false,
            },
            Self::Rounding => matches!(tool, 0..=2) && v.shape == 0,
            Self::Strength => matches!(tool, 2 | 9 | 11 | 12),
            Self::FontSize => matches!(tool, 8..=10),
            Self::Zoom => tool == 7,
            Self::Outline => matches!(tool, 8 | 10) && v.text_outline,
            Self::Opacity | Self::BackgroundRounding | Self::Padding => {
                tool == 8 && v.text_background
            }
            Self::Caption => tool == 10 && v.sequence_expanded,
            Self::TextureSize => tool == 12,
        }
    }
    pub fn from_field(field: i32) -> Option<Self> {
        Some(match field {
            -9 => Self::Size,
            -100 => Self::Rounding,
            -101 => Self::Strength,
            -102 => Self::FontSize,
            -103 => Self::Zoom,
            -104 => Self::Outline,
            -105 => Self::Opacity,
            -106 => Self::BackgroundRounding,
            -107 => Self::Padding,
            -108 => Self::Caption,
            -109 => Self::TextureSize,
            _ => return None,
        })
    }
    pub fn bounds(self, tool: i32) -> (i32, i32) {
        match self {
            Self::Size => (1, 100),
            Self::TextureSize => (1, 64),
            Self::FontSize => match tool {
                9 => (5, 144),
                _ => (5, i32::MAX),
            },
            Self::Zoom => (100, 800),
            Self::Outline => (1, 20),
            Self::Caption => (0, 4),
            _ => (0, 100),
        }
    }
    pub fn read(self, v: &AnnotationValues) -> i32 {
        match self {
            Self::Size => v.size,
            Self::Rounding => v.rounding,
            Self::Strength => v.strength.round() as i32,
            Self::FontSize => v.text_size,
            Self::Zoom => v.zoom,
            Self::Outline => v.text_outline_width,
            Self::Opacity => v.text_bg_opacity,
            Self::BackgroundRounding => v.text_bg_rounding,
            Self::Padding => v.text_bg_padding,
            Self::Caption => v.sequence_ratio,
            Self::TextureSize => v.zoom,
        }
    }
    pub fn write(self, v: &mut AnnotationValues, value: i32) {
        match self {
            Self::Size => v.size = value,
            Self::Rounding => v.rounding = value,
            Self::Strength => v.strength = value as f32,
            Self::FontSize => v.text_size = value,
            Self::Zoom => v.zoom = value,
            Self::Outline => v.text_outline_width = value,
            Self::Opacity => v.text_bg_opacity = value,
            Self::BackgroundRounding => v.text_bg_rounding = value,
            Self::Padding => v.text_bg_padding = value,
            Self::Caption => v.sequence_ratio = value,
            Self::TextureSize => v.zoom = value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_are_field_local_and_keep_existing_ranges() {
        for field in [-9, -100, -101, -102, -103, -104, -105, -106, -107, -108] {
            let parameter = Parameter::from_field(field).unwrap();
            let mut values = super::super::parameters::defaults(8);
            values.text = "草稿".into();
            values.custom_color = "#123456".into();
            let before = values.clone();
            let (min, max) = parameter.bounds(8);
            let value = parameter.read(&values).saturating_add(1).clamp(min, max);
            parameter.write(&mut values, value);
            assert_eq!(parameter.read(&values), value);
            parameter.write(&mut values, parameter.read(&before));
            assert_eq!(
                values, before,
                "{parameter:?} must not change another field"
            );
        }
        assert_eq!(Parameter::FontSize.bounds(9), (5, 144));
        assert_eq!(Parameter::FontSize.bounds(8), (5, i32::MAX));
        assert_eq!(Parameter::FontSize.bounds(10), (5, i32::MAX));
        assert_eq!(Parameter::Zoom.bounds(7), (100, 800));
        assert_eq!(Parameter::Caption.bounds(10), (0, 4));
        assert_eq!(Parameter::Outline.bounds(8), (1, 20));
        assert!(
            Parameter::from_field(15).is_none(),
            "next number retains its input"
        );
    }

    #[test]
    fn sequence_color_frame_and_toggle_states_render_at_both_dpis() {
        use slint::{
            ComponentHandle,
            platform::{
                Platform, WindowAdapter, WindowEvent,
                software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
            },
        };
        use std::{cell::RefCell, rc::Rc};
        struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
                *self.0.borrow_mut() = Some(window.clone());
                Ok(window)
            }
        }
        let adapter = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        for dpi in [1., 1.25] {
            let panel = crate::AnnotationPanel::new().unwrap();
            panel.set_tool(10);
            let mut values = super::super::parameters::defaults(10);
            values.sequence_expanded = true;
            panel.set_values(values);
            panel
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
            panel.window().set_size(slint::LogicalSize::new(1500., 60.));
            panel.show().unwrap();
            let size = panel.window().size();
            let width = size.width as usize;
            let mut pixels = vec![slint::Rgb8Pixel::default(); width * size.height as usize];
            let render = |pixels: &mut [slint::Rgb8Pixel]| {
                slint::platform::update_timers_and_animations();
                adapter
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .draw_if_needed(|renderer| {
                        renderer.render(pixels, width);
                    });
            };
            for expanded in [true, false, true, false, true] {
                let mut values = panel.get_values();
                values.sequence_expanded = expanded;
                panel.set_values(values);
                render(&mut pixels);
                let points: Vec<_> = ((636. * dpi) as usize..(666. * dpi) as usize)
                    .flat_map(|x| {
                        ((16. * dpi) as usize..(42. * dpi) as usize).filter_map({
                            let pixels = &pixels;
                            move |y| {
                                let p = pixels[y * width + x];
                                (p.r < 100 && p.g < 100 && p.b < 100).then_some((x, y))
                            }
                        })
                    })
                    .collect();
                assert!(!points.is_empty());
                let all = points.iter().map(|p| p.0 as f32).sum::<f32>() / points.len() as f32;
                let middle: Vec<_> = points
                    .iter()
                    .filter(|p| ((p.1 as f32 + 0.5) - 29. * dpi).abs() <= 1.)
                    .collect();
                let tip = middle.iter().map(|p| p.0 as f32).sum::<f32>() / middle.len() as f32;
                assert!(
                    if expanded { tip < all } else { tip > all },
                    "distinct left/right icon at dpi {dpi}, expanded={expanded}: tip={tip} body={all}"
                );
            }
            let requested = Rc::new(RefCell::new(None));
            let capture = requested.clone();
            panel.on_choice_requested(move |q| {
                *capture.borrow_mut() = Some(q);
            });
            for active in [-1, -8, -1] {
                panel.set_active_choice(active);
                for y in [29., 55.] {
                    panel.window().dispatch_event(WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(693., y),
                    });
                    render(&mut pixels);
                    for row in (19. * dpi) as usize..(39. * dpi) as usize {
                        assert!(
                            ((678. * dpi) as usize..(682. * dpi).ceil() as usize).any(|x| {
                                let p = pixels[row * width + x];
                                p.g < 245
                            }),
                            "color frame missing at dpi {dpi}, active={active}, hover y={y}"
                        );
                    }
                }
            }
            for event in [
                WindowEvent::PointerPressed {
                    position: slint::LogicalPosition::new(677., 29.),
                    button: slint::platform::PointerEventButton::Left,
                },
                WindowEvent::PointerReleased {
                    position: slint::LogicalPosition::new(677., 29.),
                    button: slint::platform::PointerEventButton::Left,
                },
            ] {
                panel.window().dispatch_event(event);
            }
            let request = requested.borrow();
            let q = request.as_ref().unwrap();
            assert_eq!(q.field, -8);
            assert_eq!((q.width, q.height), (38., 38.));
            panel.hide().unwrap();
        }
    }

    #[test]
    fn every_toolbar_numeric_entry_is_reachable_when_wrapped() {
        use slint::{
            ComponentHandle,
            platform::{
                Platform, WindowAdapter, WindowEvent,
                software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
            },
        };
        use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        for dpi in [1., 1.25] {
            for width in [320., 1400.] {
                for (tool, expected) in [
                    (0, vec![-9, -100]),
                    (1, vec![-9, -100]),
                    (2, vec![-9, -100, -101]),
                    (3, vec![-9]),
                    (4, vec![-9]),
                    (5, vec![-9]),
                    (6, vec![-9]),
                    (7, vec![-9, -103]),
                    (8, vec![-102]),
                    (9, vec![-101, -102]),
                    (10, vec![-102, -108]),
                    (11, vec![-9, -101]),
                    (12, vec![-9, -101, -109]),
                    (13, vec![-9]),
                ] {
                    let panel = crate::AnnotationPanel::new().unwrap();
                    panel
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    panel.set_tool(tool);
                    let mut values = super::super::parameters::defaults(tool);
                    values.shape = 0;
                    values.fill = false;
                    values.erase = true;
                    values.sequence_expanded = true;
                    if matches!(tool, 11..=13) {
                        values.shape = 1;
                    }
                    panel.set_values(values.clone());
                    panel
                        .window()
                        .set_size(slint::LogicalSize::new(width, 700.));
                    panel.show().unwrap();
                    panel.set_numeric_measuring(true);
                    let natural = panel.get_numeric_content_width();
                    assert!(
                        natural > if matches!(tool, 0..=10) { 450. } else { 250. },
                        "single-line tool {tool} natural width: {natural}"
                    );
                    panel.set_numeric_measuring(false);
                    let height = panel.get_numeric_content_height();
                    assert!((58. ..700.).contains(&height), "tool {tool}: {height}");
                    panel
                        .window()
                        .set_size(slint::LogicalSize::new(width, height));
                    let found = Rc::new(RefCell::new(BTreeMap::new()));
                    let capture = found.clone();
                    let cursor = Rc::new(std::cell::Cell::new((0., 0.)));
                    let at = cursor.clone();
                    panel.on_numeric_scrolled(move |field, _| {
                        capture.borrow_mut().entry(field).or_insert(at.get());
                    });
                    for y in (10..height as usize).step_by(8) {
                        for x in (12..width as usize).step_by(8) {
                            cursor.set((x as f32, y as f32));
                            panel.window().dispatch_event(WindowEvent::PointerScrolled {
                                position: slint::LogicalPosition::new(x as f32, y as f32),
                                delta_x: 0.,
                                delta_y: 120.,
                            });
                        }
                    }
                    let mut actual: Vec<_> = found.borrow().keys().copied().collect();
                    actual.sort();
                    let mut expected = expected;
                    expected.sort();
                    assert_eq!(actual, expected, "tool {tool} width {width}");
                    for (&field, &(x, y)) in found.borrow().iter() {
                        assert!(Parameter::from_field(field).unwrap().enabled(tool, &values));
                        let requested = Rc::new(std::cell::Cell::new(None));
                        let capture = requested.clone();
                        panel.on_choice_requested(move |q| {
                            capture.set(Some((q.field, q.x, q.y, q.width, q.height)));
                        });
                        for event in [
                            WindowEvent::PointerPressed {
                                position: slint::LogicalPosition::new(x, y),
                                button: slint::platform::PointerEventButton::Left,
                            },
                            WindowEvent::PointerReleased {
                                position: slint::LogicalPosition::new(x, y),
                                button: slint::platform::PointerEventButton::Left,
                            },
                        ] {
                            panel.window().dispatch_event(event);
                        }
                        let (actual, x, y, w, h) =
                            requested.get().expect("numeric button opens a menu");
                        assert_eq!(actual, field);
                        assert!(
                            x >= 0. && x + w <= width + 1. && y >= 0. && y + h <= height + 1.,
                            "tool {tool} anchor {x},{y},{w},{h} in {width},{height}"
                        );
                    }
                    panel.hide().unwrap();
                }
            }
        }
    }
}
