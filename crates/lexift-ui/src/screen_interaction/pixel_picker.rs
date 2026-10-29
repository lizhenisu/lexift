//! Session-owned frozen-pixel feedback. Cached Slint assets are prepared before pointer input.
use super::*;
use crate::{PixelPickerGlyphWindow, PixelPickerTextureWindow};
use lexift_core::domain::{
    geometry::Point,
    screen_snapshot::{PixelSample, sample_pixels},
    screenshot::{ScreenshotEvent, ScreenshotPhase, ScreenshotState},
};
use slint::Model;
use std::{cell::RefCell, collections::HashMap, sync::Arc, time::Instant};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

const CHARS: &str = "0123456789ABCDEF RG#(),-";
type Key = (u32, &'static str, i32);
struct Assets {
    shells: Vec<Pixmap>,
    glyphs: Vec<Pixmap>,
    advances: Vec<f32>,
}
#[derive(Default)]
struct State {
    id: u64,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    point: Option<Point>,
    core: ScreenshotState,
    result_at: Option<(u64, Instant)>,
    timer: slint::Timer,
    refresh: Option<Rc<dyn Fn()>>,
}
thread_local! {
    static STATE:RefCell<State>=RefCell::new(State::default());
    static ASSETS:RefCell<HashMap<Key,Arc<Assets>>>=RefCell::new(HashMap::new());
}
fn key(scale: f32) -> Key {
    (
        scale.to_bits(),
        crate::i18n::language().code(),
        crate::theme::current_index(),
    )
}
pub(crate) fn begin(id: u64, frames: Arc<Vec<CapturedRegion>>, refresh: Rc<dyn Fn()>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.id = id;
        s.frames = Some(frames);
        s.point = None;
        s.refresh = Some(refresh);
    });
}
pub(crate) fn replace_frames(id: u64, frames: Arc<Vec<CapturedRegion>>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.id == id {
            s.frames = Some(frames);
            s.timer.stop();
            s.result_at = None;
        }
    });
}
pub(crate) fn clear() {
    STATE.with(|s| *s.borrow_mut() = State::default());
    ASSETS.with(|s| s.borrow_mut().clear());
}
pub(crate) fn sync(core: &ScreenshotState) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let result = core.color_result.map(|r| r.request.id);
        let changed = s.core.color_result != core.color_result || s.core.phase != core.phase;
        if result != s.core.color_result.map(|r| r.request.id) {
            s.result_at = core
                .color_result
                .filter(|r| Some(r.request.point) == s.point)
                .map(|r| (r.request.id, Instant::now()));
            if s.result_at.is_some() {
                let session = core.session;
                s.timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_secs(1),
                    move || {
                        let callback = STATE.with(|s| {
                            let mut s = s.borrow_mut();
                            if s.id != session {
                                return None;
                            }
                            s.result_at = None;
                            s.refresh.clone()
                        });
                        if let Some(callback) = callback {
                            callback();
                        }
                    },
                );
            }
        }
        s.core = core.clone();
        changed.then(|| s.refresh.clone()).flatten()
    });
    if let Some(refresh) = changed {
        let id = core.session;
        slint::Timer::single_shot(Duration::ZERO, move || {
            if STATE.with(|s| s.borrow().id == id && s.borrow().frames.is_some()) {
                refresh();
            }
        });
    }
}
pub(crate) fn point(id: u64, point: Option<Point>) -> bool {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.id != id || s.point == point {
            return false;
        }
        s.point = point;
        s.result_at = None;
        s.timer.stop();
        true
    })
}
pub(crate) fn location() -> Option<Point> {
    STATE.with(|s| s.borrow().point)
}
pub(crate) fn copy() -> bool {
    let event = STATE.with(|s| {
        let s = s.borrow();
        if s.id != s.core.session
            || !matches!(
                s.core.phase,
                ScreenshotPhase::Selecting | ScreenshotPhase::Editing
            )
            || s.core.color_pending.is_some()
        {
            return None;
        }
        let sample = sample_pixels(s.frames.as_deref()?, s.point?)?;
        Some(ScreenshotEvent::CopyColor {
            session: s.id,
            point: sample.point,
            color: sample.color,
        })
    });
    if let Some(event) = event {
        crate::annotation::screenshot::emit(event);
        true
    } else {
        false
    }
}

fn pixmap(
    image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) -> Result<Pixmap, slint::PlatformError> {
    let mut out =
        Pixmap::new(image.width(), image.height()).ok_or("pixel picker allocation failed")?;
    for (dst, p) in out
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(image.as_slice())
    {
        let a = u32::from(p.a);
        *dst = [
            ((u32::from(p.r) * a + 127) / 255) as u8,
            ((u32::from(p.g) * a + 127) / 255) as u8,
            ((u32::from(p.b) * a + 127) / 255) as u8,
            p.a,
        ];
    }
    Ok(out)
}
pub(crate) fn prepare(
    scale: f32,
    lifecycle: &WindowLifecycleCallbacks,
) -> Result<(), slint::PlatformError> {
    if ASSETS.with(|s| s.borrow().contains_key(&key(scale))) {
        return Ok(());
    }
    let w = super::create(lifecycle, PixelPickerGlyphWindow::new)?;
    crate::theme::apply(&w);
    let mut image = None;
    for _ in 0..2 {
        let ratio = scale / w.window().scale_factor().max(0.1);
        w.set_render_scale(ratio);
        w.window().set_size(slint::LogicalSize::new(
            24. * CHARS.len() as f32 * ratio,
            18. * ratio,
        ));
        image = Some(w.window().take_snapshot()?);
    }
    let advances = (0..CHARS.len())
        .map(|i| w.get_advances().row_data(i).unwrap_or(0.) * w.window().scale_factor())
        .collect();
    let source = pixmap(&image.unwrap())?;
    let glyphs = (0..CHARS.len())
        .map(|i| {
            let start = (i as f32 * 24. * scale).round() as i32;
            let end = ((i + 1) as f32 * 24. * scale).round() as i32;
            source
                .clone_rect(
                    tiny_skia::IntRect::from_xywh(start, 0, (end - start) as u32, source.height())
                        .unwrap(),
                )
                .unwrap()
        })
        .collect();
    drop(w);
    let w = super::create(lifecycle, PixelPickerTextureWindow::new)?;
    crate::theme::apply(&w);
    let mut shells = Vec::new();
    for status in 0..3 {
        w.set_status(status);
        let mut image = None;
        for _ in 0..2 {
            let ratio = scale / w.window().scale_factor().max(0.1);
            w.set_render_scale(ratio);
            w.window()
                .set_size(slint::LogicalSize::new(184. * ratio, 184. * ratio));
            w.window()
                .set_size(slint::LogicalSize::new(w.get_content_width(), 184. * ratio));
            image = Some(w.window().take_snapshot()?);
        }
        shells.push(pixmap(&image.unwrap())?);
    }
    ASSETS.with(|s| {
        s.borrow_mut().insert(
            key(scale),
            Arc::new(Assets {
                shells,
                glyphs,
                advances,
            }),
        )
    });
    Ok(())
}

#[derive(Clone)]
pub(crate) struct Feedback {
    pub host: Rect,
    pub bounds: Rect,
    pub sample: PixelSample,
    pub status: i32,
    assets: Option<Arc<Assets>>,
    scale: f32,
    pub grid: Arc<Pixmap>,
}
impl Feedback {
    pub fn image(&self) -> slint::Image {
        let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            self.grid.data(),
            self.grid.width(),
            self.grid.height(),
        );
        slint::Image::from_rgba8_premultiplied(pixels)
    }
    pub fn coordinates(&self) -> String {
        format!("({}, {})", self.sample.point.x, self.sample.point.y)
    }
    pub fn rgb(&self) -> String {
        let c = self.sample.color;
        format!("RGB({}, {}, {})", c.0, c.1, c.2)
    }
    pub fn scale(&self) -> f32 {
        self.scale
    }
    pub fn paint(&self, target: &mut Pixmap, monitor: Rect) {
        if monitor != self.host {
            return;
        }
        let Some(assets) = self.assets.as_ref() else {
            return;
        };
        let x = self.bounds.left - monitor.left;
        let y = self.bounds.top - monitor.top;
        let scale = self.scale();
        let shell = &assets.shells[self.status as usize];
        target.draw_pixmap(
            x,
            y,
            shell.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        target.draw_pixmap(
            x + (shell.width() as i32 - self.grid.width() as i32) / 2,
            y + (8. * scale).round() as i32,
            self.grid.as_ref().as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        for (i, text) in [self.coordinates(), self.sample.color.hex(), self.rgb()]
            .iter()
            .enumerate()
        {
            let mut at = (x as f32 + 8. * scale).round();
            for c in text.chars() {
                if let Some(j) = CHARS.chars().position(|g| g == c) {
                    target.draw_pixmap(
                        at.round() as i32,
                        y + ((104. + i as f32 * 18.) * scale).round() as i32,
                        assets.glyphs[j].as_ref(),
                        &PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                    at += assets.advances[j];
                }
            }
        }
    }
}
pub(crate) fn feedback(screens: &[(Rect, f32)], area: Option<Rect>) -> Option<Feedback> {
    feedback_with_size(screens, area, None)
}
pub(crate) fn live_feedback(
    screens: &[(Rect, f32)],
    area: Option<Rect>,
    size: (u32, u32),
) -> Option<Feedback> {
    feedback_with_size(screens, area, Some(size))
}
fn feedback_with_size(
    screens: &[(Rect, f32)],
    area: Option<Rect>,
    size: Option<(u32, u32)>,
) -> Option<Feedback> {
    STATE.with(|s| {
        let s = s.borrow();
        let point = s.point?;
        let (host, scale) = *screens.iter().find(|(b, _)| {
            point.x >= b.left && point.x < b.right && point.y >= b.top && point.y < b.bottom
        })?;
        let sample = sample_pixels(s.frames.as_deref()?, point)?;
        let assets = ASSETS.with(|s| s.borrow().get(&key(scale)).cloned());
        let (w, h) = match size {
            Some(size) => size,
            None => {
                let a = assets.as_ref()?;
                (a.shells[0].width(), a.shells[0].height())
            }
        };
        let mut status = 0;
        if let Some(result) = s.core.color_result
            && s.result_at.is_some_and(|(id, t)| {
                id == result.request.id && t.elapsed() < Duration::from_secs(1)
            })
            && result.request.point == point
            && result.request.color == sample.color
        {
            status = if result.success { 1 } else { 2 };
        }
        let (w, h) = (w as i32, h as i32);
        let area = area.unwrap_or(host);
        let gap = (16. * scale).round() as i32;
        let mut position = None;
        for (x, y) in [
            (point.x + gap, point.y + gap),
            (point.x - gap - w, point.y + gap),
            (point.x + gap, point.y - gap - h),
            (point.x - gap - w, point.y - gap - h),
        ] {
            if x >= area.left && y >= area.top && x + w <= area.right && y + h <= area.bottom {
                position = Some((x, y));
                break;
            }
        }
        let (x, y) = position.unwrap_or((
            point
                .x
                .saturating_add(gap)
                .clamp(area.left, (area.right - w).max(area.left)),
            point
                .y
                .saturating_add(gap)
                .clamp(area.top, (area.bottom - h).max(area.top)),
        ));
        let grid = Arc::new(grid(&sample, scale)?);
        Some(Feedback {
            host,
            bounds: Rect {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            sample,
            status,
            assets,
            scale,
            grid,
        })
    })
}
fn grid(sample: &PixelSample, scale: f32) -> Option<Pixmap> {
    let mut image = Pixmap::new((150. * scale).round() as u32, (90. * scale).round() as u32)?;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let col = ((x as f32 / (10. * scale)).floor() as usize).min(14);
            let row = ((y as f32 / (10. * scale)).floor() as usize).min(8);
            let color = sample.grid[row * 15 + col].map_or_else(
                || {
                    if (col + row).is_multiple_of(2) {
                        [90, 90, 90, 255]
                    } else {
                        [120, 120, 120, 255]
                    }
                },
                |c| [c.0, c.1, c.2, 255],
            );
            let at = ((y * image.width() + x) * 4) as usize;
            image.data_mut()[at..at + 4].copy_from_slice(&color);
        }
    }
    // Pixel-aligned grid and a black/white center outline remain visible on any sampled color.
    for row in 0..=9 {
        let y = (row as f32 * 10. * scale).round() as i32;
        line(
            &mut image,
            0,
            y,
            (150. * scale).round() as i32,
            1,
            [180, 180, 180, 255],
        );
    }
    for col in 0..=15 {
        let x = (col as f32 * 10. * scale).round() as i32;
        line(
            &mut image,
            x,
            0,
            1,
            (90. * scale).round() as i32,
            [180, 180, 180, 255],
        );
    }
    let x = (70. * scale).round() as i32;
    let y = (40. * scale).round() as i32;
    let w = (80. * scale).round() as i32 - x;
    let h = (50. * scale).round() as i32 - y;
    for (inset, color) in [(0, [0, 0, 0, 255]), (1, [255, 255, 255, 255])] {
        line(&mut image, x + inset, y + inset, w - 2 * inset, 1, color);
        line(
            &mut image,
            x + inset,
            y + h - 1 - inset,
            w - 2 * inset,
            1,
            color,
        );
        line(&mut image, x + inset, y + inset, 1, h - 2 * inset, color);
        line(
            &mut image,
            x + w - 1 - inset,
            y + inset,
            1,
            h - 2 * inset,
            color,
        );
    }
    Some(image)
}
fn line(image: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    for yy in y.max(0)..(y + h).min(image.height() as i32) {
        for xx in x.max(0)..(x + w).min(image.width() as i32) {
            let i = ((yy as u32 * image.width() + xx as u32) * 4) as usize;
            image.data_mut()[i..i + 4].copy_from_slice(&color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn setup() -> Rect {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        clear();
        let b = Rect {
            left: -50,
            top: -30,
            right: 590,
            bottom: 450,
        };
        begin(
            7,
            Arc::new(vec![CapturedRegion {
                bounds: b,
                pixels: [10, 20, 30, 255].repeat(640 * 480),
            }]),
            Rc::new(|| {}),
        );
        sync(&ScreenshotState {
            session: 7,
            phase: ScreenshotPhase::Selecting,
            ..Default::default()
        });
        b
    }
    #[test]
    fn copy_freezes_the_input_pixel_and_rejects_missing_pointer_and_late_session() {
        let _b = setup();
        let events = Rc::new(RefCell::new(vec![]));
        let target = events.clone();
        crate::annotation::set_handler(Rc::new(move |e| {
            if let lexift_core::AppEvent::Screenshot(e) = e {
                target.borrow_mut().push(e);
            }
        }));
        assert!(!copy());
        point(7, Some(Point { x: -5, y: 20 }));
        assert!(copy());
        point(7, Some(Point { x: 50, y: 100 }));
        assert!(matches!(
            events.borrow().as_slice(),
            [ScreenshotEvent::CopyColor {
                point: Point { x: -5, y: 20 },
                color: lexift_core::domain::screen_snapshot::Rgb(10, 20, 30),
                ..
            }]
        ));
        point(7, None);
        assert!(!copy());
        sync(&ScreenshotState {
            session: 8,
            phase: ScreenshotPhase::Selecting,
            ..Default::default()
        });
        assert!(!point(8, Some(Point { x: 1, y: 1 })));
        assert!(!copy());
        clear();
    }
    #[test]
    fn cached_feedback_freezes_geometry_and_status_without_creating_more_assets() {
        use lexift_core::domain::{
            screen_snapshot::Rgb,
            screenshot::{ColorRequest, ColorResult},
        };
        let b = setup();
        prepare(1.25, &lifecycle()).unwrap();
        point(7, Some(Point { x: -49, y: -29 }));
        let f = feedback(&[(b, 1.25)], Some(b)).unwrap();
        assert_eq!(f.sample.color, Rgb(10, 20, 30));
        assert!(f.sample.grid[0].is_none());
        for p in [
            Point { x: 589, y: 449 },
            Point { x: -50, y: 449 },
            Point { x: 589, y: -30 },
        ] {
            point(7, Some(p));
            let v = feedback(&[(b, 1.25)], Some(b)).unwrap();
            assert!(
                v.bounds.left >= b.left
                    && v.bounds.top >= b.top
                    && v.bounds.right <= b.right
                    && v.bounds.bottom <= b.bottom
            );
        }
        assert_eq!(f.sample.point, Point { x: -49, y: -29 });
        let p = Point { x: 589, y: -30 };
        sync(&ScreenshotState {
            session: 7,
            phase: ScreenshotPhase::Editing,
            color_result: Some(ColorResult {
                request: ColorRequest {
                    id: 1,
                    point: p,
                    color: Rgb(10, 20, 30),
                },
                success: true,
            }),
            ..Default::default()
        });
        assert_eq!(feedback(&[(b, 1.25)], Some(b)).unwrap().status, 1);
        point(7, Some(Point { x: 588, y: -30 }));
        assert_eq!(feedback(&[(b, 1.25)], Some(b)).unwrap().status, 0);
        assert_eq!(ASSETS.with(|s| s.borrow().len()), 1);
        clear();
        slint::platform::update_timers_and_animations();
        assert!(feedback(&[(b, 1.25)], Some(b)).is_none());
    }
    #[test]
    fn render_pixel_picker_all_languages_themes_and_dpi() {
        use lexift_core::domain::{settings::ThemePreference, ui_language::UiLanguage};
        let b = setup();
        point(7, Some(Point { x: 100, y: 100 }));
        for language in UiLanguage::ALL {
            crate::i18n::select(language);
            for dark in [false, true] {
                crate::theme::set_current(if dark {
                    ThemePreference::Dark
                } else {
                    ThemePreference::Light
                });
                for scale in [1., 1.25] {
                    prepare(scale, &lifecycle()).unwrap();
                    let f = feedback(&[(b, scale)], Some(b)).unwrap();
                    assert_eq!(f.grid.width(), (150. * scale).round() as u32);
                    assert_eq!(f.grid.height(), (90. * scale).round() as u32);
                    let mut image = Pixmap::new(
                        f.assets.as_ref().unwrap().shells[0].width(),
                        f.assets.as_ref().unwrap().shells[0].height(),
                    )
                    .unwrap();
                    let monitor = Rect {
                        left: f.bounds.left,
                        top: f.bounds.top,
                        right: f.bounds.right,
                        bottom: f.bounds.bottom,
                    };
                    let mut local = f.clone();
                    local.host = monitor;
                    local.paint(&mut image, monitor);
                    assert!(image.pixels().iter().any(|p| p.red() > 200));
                    if let Ok(dir) = std::env::var("LEXIFT_PIXEL_RENDER") {
                        crate::i18n::write_test_bitmap(
                            &dir,
                            &format!("{}-{dark}-{scale}", language.code()),
                            image.width(),
                            image.height(),
                            image
                                .pixels()
                                .iter()
                                .map(|p| [p.red(), p.green(), p.blue(), p.alpha()]),
                        );
                    }
                }
            }
        }
        crate::i18n::select(UiLanguage::EnglishUs);
        crate::theme::set_current(ThemePreference::Light);
        clear();
    }
}
