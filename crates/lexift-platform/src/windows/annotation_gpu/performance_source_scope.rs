//! Controlled source contribution fixture; never a production occlusion heuristic.
use super::*;
use windows::Win32::Foundation::LPARAM;

fn exclusions(source: HWND) -> Vec<WindowToken> {
    struct State {
        source: HWND,
        windows: Vec<WindowToken>,
    }
    unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> windows::core::BOOL {
        let state = unsafe { &mut *(state.0 as *mut State) };
        if hwnd != state.source {
            state.windows.push(WindowToken(hwnd.0 as isize));
        }
        true.into()
    }
    let mut state = State {
        source,
        windows: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(collect), LPARAM(&mut state as *mut _ as isize)).unwrap();
    }
    state.windows
}

/// Inset the mask, preserving the full source and output dimensions. Assert against
/// the actual blur plan, including its alignment, rather than guessing the margin.
pub(super) fn document(
    bounds: Rect,
    input: HWND,
    source: HWND,
    only_owned: bool,
) -> AnnotationScene {
    let mut document = scene(bounds, input, 0);
    let inner = Rect {
        left: bounds.left + 192,
        top: bounds.top + 192,
        right: bounds.right - 192,
        bottom: bounds.bottom - 192,
    };
    let mut layers = document.layers.to_vec();
    let RenderLayer::Mosaic(layer) = &mut layers[0] else {
        unreachable!()
    };
    let mut object = (*layer.object).clone();
    object.bounds = Bounds::from_corners(
        (inner.left as f32, inner.top as f32),
        (inner.right as f32, inner.bottom as f32),
    );
    layer.object = Arc::new(object);
    let mut mask = layer.masks[0].clone();
    mask.bounds = inner;
    mask.pixels =
        vec![255; ((inner.right - inner.left) * (inner.bottom - inner.top) * 4) as usize].into();
    layer.masks = vec![mask].into();
    document.layers = layers.into();
    let regions = effects::Effects::default().regions(&document);
    assert!(!regions.is_empty());
    assert!(
        regions.iter().all(|r| bounds.left <= r.left
            && bounds.top <= r.top
            && bounds.right >= r.right
            && bounds.bottom >= r.bottom),
        "owned source must cover the aligned filter footprint"
    );
    if only_owned {
        document.excluded_windows = exclusions(source).into();
    }
    document.validate().unwrap();
    document
}

#[test]
fn covered_fixture_contains_aligned_blur_margin_at_negative_origins() {
    let bounds = Rect {
        left: -1240,
        top: -680,
        right: 40,
        bottom: 40,
    };
    let document = document(bounds, HWND(1 as _), HWND(2 as _), false);
    assert_eq!(document.surfaces[0].bounds, bounds);
    let RenderLayer::Mosaic(layer) = &document.layers[0] else {
        unreachable!()
    };
    assert_eq!(layer.masks[0].pixels.len(), 896 * 336 * 4);
}

#[test]
#[ignore = "owned all-source/single-source full-frame readback; Release WGC/D3D11 required"]
fn covered_source_exclusion_preserves_blur_and_pixelate_pixels() {
    if cfg!(debug_assertions) {
        panic!("run pixel verification with --release");
    }
    let bootstrap = Backend::new().unwrap();
    let bounds = Rect {
        left: 40,
        top: 40,
        right: 1320,
        bottom: 760,
    };
    let source = window(bounds, false);
    let input = window(bounds, true);
    surface::pump_messages();
    let painter = Publisher::new(source.0, &bootstrap, SourceKind::Dxgi);
    painter.paint(37);
    painter.verify(37);
    drop(bootstrap);
    for effect in [MosaicEffect::Blur, MosaicEffect::Pixelate] {
        let mut outputs = Vec::new();
        for only_owned in [false, true] {
            let mut renderer = Renderer::new().unwrap();
            let verification = Graphics::new(
                renderer.graphics.device.clone(),
                renderer.graphics.context.clone(),
            )
            .unwrap();
            let mut document = document(bounds, input.0, source.0, only_owned);
            let mut layers = document.layers.to_vec();
            let RenderLayer::Mosaic(layer) = &mut layers[0] else {
                unreachable!()
            };
            let mut object = (*layer.object).clone();
            let Extra::Mosaic(settings) = &mut object.extra else {
                unreachable!()
            };
            settings.effect = effect;
            layer.object = Arc::new(object);
            document.layers = layers.into();
            assert!(
                renderer
                    .effects
                    .regions(&document)
                    .iter()
                    .all(|r| bounds.left <= r.left
                        && bounds.top <= r.top
                        && bounds.right >= r.right
                        && bounds.bottom >= r.bottom)
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut output = None;
            while output.is_none() {
                surface::pump_messages();
                renderer
                    .render(&document, |surfaces| {
                        assert_eq!(surfaces.len(), 1);
                        output = Some(graphics::tests::readback(
                            &verification,
                            &surfaces[0].target,
                        ));
                        Ok(false)
                    })
                    .unwrap();
                assert!(
                    Instant::now() < deadline,
                    "no frozen output for {effect:?}, owned={only_owned}"
                );
                thread::sleep(Duration::from_millis(1));
            }
            if only_owned {
                assert_eq!(renderer.capture.capture_count(), 1);
            }
            let output = output.unwrap();
            for band in 1..7 {
                let offset = (360 * 1280 + band * 160 + 80) * 4;
                let rgb = band_color(37, band as u32).0;
                let expected = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255];
                assert!(
                    output[offset..offset + 4]
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| a.abs_diff(b) <= 1)
                );
            }
            outputs.push(output);
            renderer.capture.stop_capture();
            drop(verification);
            drop(renderer);
        }
        assert_eq!(outputs[0].len(), 1280 * 720 * 4);
        let error = outputs[0]
            .iter()
            .zip(&outputs[1])
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        println!(
            "source_scope_pixel_effect={effect:?} compared_bytes={} max_error={error}",
            outputs[0].len()
        );
        assert!(error <= 1, "excluded sources changed the covered output");
    }
}
