//! Full-frame regressions: feedback must belong to the pixels' frozen crop, never a live window.
use super::*;
use crate::screen_interaction::size_label;
use lexift_core::domain::settings::ThemePreference;
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
pub(super) fn prepare_font_fixture(scale: f32, dark: bool) {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    prepare(scale, dark);
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
fn prepare(scale: f32, dark: bool) {
    let service = super::super::font_service::Service::new(Arc::new(|| {
        panic!("size labels may not start a system font scan")
    }));
    super::super::font_service::TEST_SERVICE.with(|slot| *slot.borrow_mut() = Some(service));
    size_label::clear();
    crate::theme::set_current(if dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
    size_label::prepare(scale, &lifecycle()).unwrap();
    assert!(
        !super::super::font_service::ready(),
        "size label must not scan annotation fonts"
    );
}
fn fixture(scale: f32) -> Input {
    let monitor = Rect {
        left: -100,
        top: -80,
        right: 500,
        bottom: 320,
    };
    Input {
        snapshot: ExportSnapshot {
            session: Arc::new(Session::new()),
            monitor,
            scale,
            monitors: vec![(monitor, scale)],
            excluded: vec![],
            owner: None,
            name: String::new(),
            title: String::new(),
        },
        capture: Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![lexift_core::ports::screen_capture::CapturedRegion {
                bounds: monitor,
                pixels: [120, 160, 180, 255].repeat(600 * 400),
            }],
        }),
        crop: Rect {
            left: 20,
            top: 40,
            right: 180,
            bottom: 190,
        },
        label: None,
        help: None,
        picker: None,
        hovered: None,
        crop_handles: true,
        crop_border: true,
        erase: None,
    }
}

#[test]
fn frozen_pixel_feedback_reuses_document_cache_and_never_enters_output() {
    use crate::screen_interaction::pixel_picker as picker;
    prepare_font_fixture(1.25, false);
    let mut input = fixture(1.25);
    let monitor = input.snapshot.monitor;
    picker::begin(9, Arc::new(input.capture.regions.clone()), Rc::new(|| {}));
    picker::prepare(1.25, &lifecycle()).unwrap();
    picker::point(9, Some(Point { x: 50, y: 70 }));
    let first = picker::feedback(&[(monitor, 1.25)], Some(monitor)).unwrap();
    input.picker = Some(first.clone());
    let mut cache = Cache::default();
    let cancelled = AtomicBool::new(false);
    let old = paint_frames(&input, &cancelled, &mut cache).unwrap();
    picker::point(9, Some(Point { x: 90, y: 100 }));
    input.picker = picker::feedback(&[(monitor, 1.25)], Some(monitor));
    let new = paint_frames(&input, &cancelled, &mut cache).unwrap();
    assert_eq!(cache.builds, 1);
    assert_ne!(old[0].data(), new[0].data());
    assert_eq!(first.sample.point, Point { x: 50, y: 70 });
    assert_eq!(first.sample.color.hex(), "#78A0B4");
    let output = crate::annotation_export_render::compose(
        &input.snapshot,
        input.capture.clone(),
        &cancelled,
    )
    .unwrap();
    assert!(
        output
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [120, 160, 180, 255])
    );
    picker::clear();
}
fn set_label(input: &mut Input) {
    input.label = size_label::layout(
        input.crop,
        &[(input.snapshot.monitor, input.snapshot.scale)],
        Some(input.snapshot.monitor),
    );
}

#[test]
fn screenshot_help_is_frozen_with_preview_reuses_base_and_never_enters_clean_output() {
    use crate::screen_interaction::screenshot_help::{self as help, Stage};
    prepare_font_fixture(1.25, false);
    let mut input = fixture(1.25);
    let monitor = input.snapshot.monitor;
    help::prepare(1.25, monitor, &lifecycle()).unwrap();
    let mut state = help::State::default();
    state.pointer(
        monitor,
        Some(lexift_core::domain::geometry::Point { x: 480, y: -60 }),
    );
    input.help = state.feedback(&[(monitor, 1.25)], None, Stage::Mouse, None, false);
    let b = input.help.as_ref().unwrap().bounds;
    let x = (b.left - monitor.left + 20) as u32;
    let y = (b.top - monitor.top + 10) as u32;
    let mut cache = Cache::default();
    let cancelled = AtomicBool::new(false);
    let visible = paint_cached(&input, &cancelled, &mut cache, true).unwrap();
    let clean = paint_cached(&input, &cancelled, &mut cache, false).unwrap();
    assert_ne!(visible.pixel(x, y), clean.pixel(x, y));
    // A completed frame retains the old pixels; later mouse input cannot alter that frame.
    state.pointer(
        monitor,
        Some(lexift_core::domain::geometry::Point {
            x: b.left,
            y: b.top,
        }),
    );
    input.help = state.feedback(&[(monitor, 1.25)], None, Stage::Mouse, None, false);
    assert!(input.help.is_none());
    let hidden = paint_cached(&input, &cancelled, &mut cache, true).unwrap();
    assert_ne!(hidden.pixel(x, y), visible.pixel(x, y));
    assert_eq!(
        cache.builds, 1,
        "help visibility must not rebuild annotations or effects"
    );
    let clean_after = paint_cached(&input, &cancelled, &mut cache, false).unwrap();
    assert_eq!(clean.data(), clean_after.data());
    help::clear();
}
fn label_pixel(input: &Input, frame: &tiny_skia::Pixmap, dark: bool) {
    let label = input.label.as_ref().unwrap();
    assert_eq!(label.crop, input.crop);
    let x = (label.bounds.left - label.host.left + 2) as u32;
    let y = (label.bounds.top - label.host.top + (14. * input.snapshot.scale) as i32) as u32;
    assert_eq!(
        frame.pixel(x, y).unwrap().red(),
        if dark { 0xe8 } else { 0x20 }
    );
    let bx = (input.crop.left - label.host.left) as u32;
    let by = (input.crop.top - label.host.top) as u32;
    let edge = frame.pixel(bx, by).unwrap();
    assert_ne!(
        (edge.red(), edge.green(), edge.blue()),
        (120, 160, 180),
        "crop edge must be in this same frame"
    );
}
#[test]
fn whole_frames_keep_size_text_geometry_and_cache_together_through_fast_changes() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    for dark in [false, true] {
        for scale in [1., 1.25] {
            prepare(scale, dark);
            let mut input = fixture(scale);
            let mut cache = Cache::default();
            let cancelled = AtomicBool::new(false);
            let mut delayed = None;
            for n in 0..60 {
                input.crop = Rect {
                    left: 20 + n * 2,
                    top: 40 + (n % 5) * 3,
                    right: 180 + n * 2 + (n % 7),
                    bottom: 190 + (n % 5) * 3,
                };
                set_label(&mut input);
                let result = paint_frames(&input, &cancelled, &mut cache).unwrap();
                label_pixel(&input, &result[0], dark);
                assert_eq!(
                    input.label.as_ref().unwrap().text,
                    format!(
                        "{} × {} px",
                        input.crop.right - input.crop.left,
                        input.crop.bottom - input.crop.top
                    )
                );
                if n == 0 {
                    delayed = Some(result[0].clone());
                }
                if n == 59 {
                    let old = delayed.as_ref().unwrap();
                    let mut first = fixture(scale);
                    set_label(&mut first);
                    label_pixel(&first, old, dark);
                    if let Ok(dir) = std::env::var("LEXIFT_SCREENSHOT_SNAPSHOT_DIR") {
                        let _ = std::fs::create_dir_all(&dir);
                        crate::i18n::write_test_bitmap(
                            &dir,
                            &format!("same-frame-{dark}-{scale}"),
                            result[0].width(),
                            result[0].height(),
                            result[0]
                                .pixels()
                                .iter()
                                .map(|p| [p.red(), p.green(), p.blue(), p.alpha()]),
                        );
                    }
                }
            }
            assert_eq!(
                cache.builds, 1,
                "moving and resizing crop must reuse composed pixels"
            );
            Arc::make_mut(&mut input.snapshot.session).spotlight_opacity = 0.5;
            paint_frames(&input, &cancelled, &mut cache).unwrap();
            assert_eq!(cache.builds, 2);
            let output = crate::annotation_export_render::compose(
                &input.snapshot,
                input.capture.clone(),
                &cancelled,
            )
            .unwrap();
            let label = input.label.as_ref().unwrap();
            let index = (((label.bounds.top - label.host.top + 8) * 600 + label.bounds.left
                - label.host.left
                + 2)
                * 4) as usize;
            assert_eq!(
                &output.pixels[index..index + 4],
                &[120, 160, 180, 255],
                "export cannot contain the label"
            );
            let stale = input.label.take();
            input.crop.left += 1;
            input.label = stale;
            assert!(
                paint_frames(&input, &cancelled, &mut cache).is_none(),
                "mismatched feedback must never be presented"
            );
        }
    }
}
#[test]
fn atlas_is_bounded_and_layout_uses_only_one_cross_screen_host() {
    prepare_font_fixture(1., false);
    size_label::prepare(1.25, &lifecycle()).unwrap();
    let screens = [
        (
            Rect {
                left: -500,
                top: -200,
                right: 0,
                bottom: 400,
            },
            1.,
        ),
        (
            Rect {
                left: 0,
                top: 0,
                right: 600,
                bottom: 400,
            },
            1.25,
        ),
    ];
    let crop = Rect {
        left: -200,
        top: 20,
        right: 200,
        bottom: 220,
    };
    let label = size_label::layout(crop, &screens, Some(screens[0].0)).unwrap();
    assert_eq!(label.text, "400 × 200 px");
    assert_eq!(label.host, screens[0].0);
    for (host, _) in screens {
        let mut frame = tiny_skia::Pixmap::new(600, 600).unwrap();
        label.paint(&mut frame, host);
        assert_eq!(frame.data().iter().any(|p| *p != 0), host == label.host);
    }
    assert!(
        size_label::layout(
            Rect {
                right: crop.left,
                ..crop
            },
            &screens,
            None
        )
        .is_none()
    );
    size_label::clear();
    assert!(size_label::layout(crop, &screens, None).is_none());
}

#[test]
fn side_captions_share_crop_frames_in_both_themes_and_scales() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    for dark in [false, true] {
        for scale in [1., 1.25] {
            prepare(scale, dark);
            for (name, crop) in [
                (
                    "left",
                    Rect {
                        left: 60,
                        top: -78,
                        right: 260,
                        bottom: 180,
                    },
                ),
                (
                    "right",
                    Rect {
                        left: -98,
                        top: -78,
                        right: 80,
                        bottom: 180,
                    },
                ),
                (
                    "above",
                    Rect {
                        left: 60,
                        top: 20,
                        right: 260,
                        bottom: 318,
                    },
                ),
                (
                    "below",
                    Rect {
                        left: -98,
                        top: -78,
                        right: 498,
                        bottom: 180,
                    },
                ),
                (
                    "inside",
                    Rect {
                        left: -98,
                        top: -78,
                        right: 498,
                        bottom: 318,
                    },
                ),
            ] {
                let mut input = fixture(scale);
                input.crop = crop;
                set_label(&mut input);
                let b = input.label.as_ref().unwrap().bounds;
                let gap = (8. * scale) as i32;
                match name {
                    "left" => assert!(b.right + gap <= crop.left),
                    "right" => assert!(b.left >= crop.right + gap),
                    "above" => assert!(b.bottom + gap <= crop.top),
                    "below" => assert!(b.top >= crop.bottom + gap),
                    _ => assert_eq!((b.left, b.top), (crop.left + gap, crop.top + gap)),
                }
                let image = paint(&input, &AtomicBool::new(false)).unwrap();
                label_pixel(&input, &image, dark);
                let output = crate::annotation_export_render::compose(
                    &input.snapshot,
                    input.capture.clone(),
                    &AtomicBool::new(false),
                )
                .unwrap();
                let at = (((b.top - input.snapshot.monitor.top + 2) * 600 + b.left
                    - input.snapshot.monitor.left
                    + 2)
                    * 4) as usize;
                assert_eq!(
                    &output.pixels[at..at + 4],
                    &[120, 160, 180, 255],
                    "even inside labels must not enter output"
                );
                if let Ok(dir) = std::env::var("LEXIFT_SCREENSHOT_SNAPSHOT_DIR") {
                    crate::i18n::write_test_bitmap(
                        &dir,
                        &format!("side-{name}-{dark}-{scale}"),
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
}

#[test]
fn image_color_changes_content_once_but_never_edit_controls() {
    prepare_font_fixture(1., false);
    let mut input = fixture(1.);
    input.crop_border = false;
    let session = Arc::make_mut(&mut input.snapshot.session);
    session.image_source = Some(input.snapshot.monitor);
    session.add(Object {
        image_basis: Default::default(),
        id: Default::default(),
        kind: Kind::Rectangle,
        bounds: Bounds::from_corners((20., 40.), (160., 180.)),
        points: vec![],
        erased: vec![],
        style: Style::default(),
        extra: Extra::None,
    });
    let cancel = AtomicBool::new(false);
    let mut cache = Cache::default();
    let original = paint_frames(&input, &cancel, &mut cache).unwrap();
    for grayscale in [false, true] {
        for inverted in [false, true] {
            let color = lexift_core::domain::image_color::Color {
                grayscale,
                inverted,
            };
            Arc::make_mut(&mut input.snapshot.session).image_color = color;
            let frames = paint_frames(&input, &cancel, &mut cache).unwrap();
            let expected: Vec<u8> = original[0]
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let y = ((54 * u32::from(p[0])
                        + 183 * u32::from(p[1])
                        + 19 * u32::from(p[2])
                        + 128)
                        / 256) as u8;
                    let mut out = [p[0], p[1], p[2], p[3]];
                    for i in 0..3 {
                        let c = if grayscale { y } else { p[i] };
                        out[i] = if inverted { p[3] - c } else { c };
                    }
                    out
                })
                .collect();
            assert_eq!(frames[0].data(), expected);
            assert_eq!(
                frames[1].data(),
                original[1].data(),
                "controls remain unfiltered"
            );
        }
    }
}
