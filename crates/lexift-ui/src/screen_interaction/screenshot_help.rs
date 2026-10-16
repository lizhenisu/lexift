//! Read-only screenshot help: bounded Slint textures and physical-pixel avoidance.
use super::*;
use lexift_core::domain::geometry::Point;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    sync::Arc,
};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Stage {
    Select,
    Mouse,
    Draw,
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    scale: u32,
    width: u32,
    language: &'static str,
    theme: i32,
    stage: Stage,
}
thread_local! {static TEXTURES:RefCell<HashMap<Key,Arc<Pixmap>>>=RefCell::new(HashMap::new());}
#[derive(Default)]
pub(crate) struct State {
    host: Option<Rect>,
    pointer: Option<Point>,
    hidden: Cell<bool>,
}
#[derive(Clone)]
pub(crate) struct Feedback {
    pub host: Rect,
    pub bounds: Rect,
    image: Arc<Pixmap>,
}
fn key(scale: f32, area: Rect, stage: Stage) -> Key {
    let width = ((area.right - area.left) as f32 / scale - 16.).clamp(1., 400.);
    Key {
        scale: scale.to_bits(),
        width: width.to_bits(),
        language: crate::i18n::language().code(),
        theme: crate::theme::current_index(),
        stage,
    }
}
pub(crate) fn clear() {
    TEXTURES.with(|s| s.borrow_mut().clear());
}
/// Prepare all mode variants before showing a host; no work is repeated during pointer motion.
pub(crate) fn prepare(
    scale: f32,
    area: Rect,
    lifecycle: &WindowLifecycleCallbacks,
) -> Result<(), slint::PlatformError> {
    for stage in [Stage::Select, Stage::Mouse, Stage::Draw] {
        let key = key(scale, area, stage);
        if TEXTURES.with(|s| s.borrow().contains_key(&key)) {
            continue;
        }
        let window = super::create(lifecycle, crate::ScreenshotHelpTextureWindow::new)?;
        crate::theme::apply(&window);
        window.set_stage(stage as i32);
        let width = f32::from_bits(key.width);
        let mut pixels = None;
        for _ in 0..2 {
            let ratio = scale / window.window().scale_factor().max(0.1);
            window.set_render_scale(ratio);
            window
                .window()
                .set_size(slint::LogicalSize::new(width * ratio, 300. * ratio));
            let content_width = window.get_content_width().ceil().min(width * ratio).max(1.);
            window
                .window()
                .set_size(slint::LogicalSize::new(content_width, 300. * ratio));
            let height = window.get_content_height().ceil().max(1.);
            window
                .window()
                .set_size(slint::LogicalSize::new(content_width, height));
            pixels = Some(window.window().take_snapshot()?);
        }
        let pixels = pixels.unwrap();
        let mut image = Pixmap::new(pixels.width(), pixels.height())
            .ok_or_else(|| slint::PlatformError::from("Screenshot help allocation failed"))?;
        for (out, p) in image
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(pixels.as_slice())
        {
            let a = u32::from(p.a);
            out.copy_from_slice(&[
                ((u32::from(p.r) * a + 127) / 255) as u8,
                ((u32::from(p.g) * a + 127) / 255) as u8,
                ((u32::from(p.b) * a + 127) / 255) as u8,
                p.a,
            ]);
        }
        TEXTURES.with(|s| s.borrow_mut().insert(key, Arc::new(image)));
    }
    Ok(())
}
impl State {
    pub(crate) fn pointer(&mut self, host: Rect, point: Option<Point>) {
        if let Some(point) = point {
            if self.host != Some(host) {
                self.hidden.set(false);
            }
            self.host = Some(host);
            self.pointer = Some(point);
        } else if self.host == Some(host) {
            self.pointer = None;
        }
    }
    pub(crate) fn host(&self) -> Option<Rect> {
        self.host
    }
    pub(crate) fn feedback(
        &self,
        screens: &[(Rect, f32)],
        area: Option<Rect>,
        stage: Stage,
        active: Option<Rect>,
        blocked: bool,
    ) -> Option<Feedback> {
        let (host, scale) = screens
            .iter()
            .find(|(b, _)| Some(*b) == self.host)
            .or_else(|| screens.first())
            .copied()?;
        let area = area.unwrap_or(host);
        let work = Rect {
            left: area.left.max(host.left),
            top: area.top.max(host.top),
            right: area.right.min(host.right),
            bottom: area.bottom.min(host.bottom),
        };
        let image = TEXTURES.with(|s| s.borrow().get(&key(scale, work, stage)).cloned())?;
        let bounds = position(
            work,
            (image.width(), image.height()),
            (8. * scale).round() as i32,
        )?;
        let hidden = avoid(bounds, self.pointer, active, scale, self.hidden.get());
        self.hidden.set(hidden);
        (!blocked && !hidden).then_some(Feedback {
            host,
            bounds,
            image,
        })
    }
}
fn position(area: Rect, size: (u32, u32), gap: i32) -> Option<Rect> {
    let b = Rect {
        left: area.left + gap,
        top: area.bottom - gap - size.1 as i32,
        right: area.left + gap + size.0 as i32,
        bottom: area.bottom - gap,
    };
    (b.right <= area.right - gap && b.top >= area.top + gap).then_some(b)
}
/// Hysteresis is based on unexpanded visual bounds; static document objects are never inputs.
fn avoid(
    panel: Rect,
    pointer: Option<Point>,
    active: Option<Rect>,
    scale: f32,
    hidden: bool,
) -> bool {
    let gap = ((if hidden { 24. } else { 16. }) * scale).round() as i32;
    let b = Rect {
        left: panel.left - gap,
        top: panel.top - gap,
        right: panel.right + gap,
        bottom: panel.bottom + gap,
    };
    pointer.is_some_and(|p| p.x >= b.left && p.x <= b.right && p.y >= b.top && p.y <= b.bottom)
        || active.is_some_and(|a| {
            a.left <= b.right && a.right >= b.left && a.top <= b.bottom && a.bottom >= b.top
        })
}
impl Feedback {
    pub(crate) fn paint(&self, frame: &mut Pixmap, host: Rect) {
        if self.host == host {
            frame.draw_pixmap(
                self.bounds.left - host.left,
                self.bounds.top - host.top,
                self.image.as_ref().as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::{settings::ThemePreference, ui_language::UiLanguage};
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    fn lifecycle() -> WindowLifecycleCallbacks {
        WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        )
    }
    #[test]
    fn proximity_hysteresis_uses_current_operation_and_scaled_physical_bounds() {
        let panel = Rect {
            left: -400,
            top: 200,
            right: 0,
            bottom: 400,
        };
        for scale in [1., 1.25] {
            let p = |logical: f32| {
                Some(Point {
                    x: (logical * scale) as i32,
                    y: 250,
                })
            };
            assert!(avoid(panel, p(15.), None, scale, false));
            assert!(avoid(panel, p(20.), None, scale, true));
            assert!(!avoid(panel, p(25.), None, scale, true));
            assert!(!avoid(panel, p(20.), None, scale, false));
            let geometry = Rect {
                left: -450,
                top: 150,
                right: -399,
                bottom: 210,
            };
            assert!(avoid(panel, None, Some(geometry), scale, false));
            // Finishing the gesture removes its bounds from the predicate.
            assert!(!avoid(panel, None, None, scale, true));
        }
        assert_eq!(
            position(panel, (100, 50), 8),
            Some(Rect {
                left: -392,
                top: 342,
                right: -292,
                bottom: 392
            })
        );
        assert!(position(panel, (500, 50), 8).is_none());
    }
    #[test]
    fn cached_help_renders_all_locales_modes_themes_and_scales_without_hot_windows() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let screen = Rect {
            left: -800,
            top: -100,
            right: 0,
            bottom: 600,
        };
        for language in UiLanguage::ALL {
            crate::i18n::select(language);
            for (dark, theme) in [
                (false, ThemePreference::Light),
                (true, ThemePreference::Dark),
            ] {
                crate::theme::set_current(theme);
                for scale in [1., 1.25] {
                    clear();
                    prepare(scale, screen, &lifecycle()).unwrap();
                    assert_eq!(TEXTURES.with(|s| s.borrow().len()), 3);
                    let mut state = State::default();
                    state.pointer(screen, Some(Point { x: -100, y: 0 }));
                    for stage in [Stage::Select, Stage::Mouse, Stage::Draw] {
                        let f = state
                            .feedback(&[(screen, scale)], None, stage, None, false)
                            .unwrap();
                        assert!(f.bounds.top >= screen.top && f.bounds.left >= screen.left);
                        assert_eq!(screen.bottom - f.bounds.bottom, (8. * scale) as i32);
                        assert!(f.image.height() > (40. * scale) as u32);
                        let frozen = f.image.clone();
                        for x in [-100, -120, -140] {
                            state.pointer(screen, Some(Point { x, y: 0 }));
                            let next = state
                                .feedback(&[(screen, scale)], None, stage, None, false)
                                .unwrap();
                            assert!(Arc::ptr_eq(&frozen, &next.image));
                        }
                        let mut frame = Pixmap::new(800, 700).unwrap();
                        f.paint(&mut frame, screen);
                        crate::i18n::write_test_bitmap(
                            concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/../../target/screenshot-help/render"
                            ),
                            &format!("{}-{}-{}-{}", language.code(), dark, scale, stage as i32),
                            f.image.width(),
                            f.image.height(),
                            f.image.data().as_chunks::<4>().0.iter().copied(),
                        );
                        state.pointer(
                            screen,
                            Some(Point {
                                x: f.bounds.left,
                                y: f.bounds.top,
                            }),
                        );
                        assert!(
                            state
                                .feedback(&[(screen, scale)], None, stage, None, false)
                                .is_none()
                        );
                        state.pointer(screen, None);
                        assert!(
                            state
                                .feedback(&[(screen, scale)], None, stage, None, false)
                                .is_some()
                        );
                        assert!(
                            state
                                .feedback(&[(screen, scale)], None, stage, None, true)
                                .is_none()
                        );
                    }
                }
            }
        }
        clear();
        assert_eq!(TEXTURES.with(|s| s.borrow().len()), 0);
    }
    #[test]
    fn crossing_hosts_keeps_one_caption_and_exit_keeps_the_last_screen() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let a = Rect {
            left: -800,
            top: 0,
            right: 0,
            bottom: 600,
        };
        let b = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        prepare(1., a, &lifecycle()).unwrap();
        prepare(1.25, b, &lifecycle()).unwrap();
        let mut state = State::default();
        state.pointer(a, Some(Point { x: -100, y: 100 }));
        let screens = [(a, 1.), (b, 1.25)];
        assert_eq!(
            state
                .feedback(&screens, None, Stage::Select, None, false)
                .unwrap()
                .host,
            a
        );
        state.pointer(b, Some(Point { x: 900, y: 100 }));
        state.pointer(a, None);
        assert_eq!(
            state
                .feedback(&screens, None, Stage::Mouse, None, false)
                .unwrap()
                .host,
            b
        );
        state.pointer(b, None);
        assert_eq!(
            state
                .feedback(&screens, None, Stage::Draw, None, false)
                .unwrap()
                .host,
            b
        );
    }
}
