//! Cached procedural texture; never consumes captured desktop pixels.
use lexift_core::domain::{
    annotation::{Extra, Object, StickerAnnotation, StickerTexture},
    geometry::Rect,
};
use std::{cell::RefCell, collections::HashMap};
use tiny_skia::{FilterQuality, Paint, Pattern, Pixmap, SpreadMode, Transform};

thread_local! {
    static TEXTURES: RefCell<HashMap<(StickerTexture, u32), Pixmap>> = RefCell::new(HashMap::new());
}

pub(crate) fn settings(values: &crate::AnnotationValues, start: (f32, f32), scale: f32) -> Extra {
    let mut sticker = StickerAnnotation::new(start, scale);
    sticker.texture = if values.mode == 1 {
        StickerTexture::Frost
    } else {
        StickerTexture::Glass
    };
    sticker.opacity = values.strength.round().clamp(0., 100.) as u8;
    sticker.diameter = values.size.clamp(1, 100) as f32 * sticker.creation_scale;
    sticker.texture_size = values.zoom.clamp(1, 64) as f32 * sticker.creation_scale;
    Extra::Sticker(sticker)
}

fn texture(kind: StickerTexture, seed: u32) -> Pixmap {
    let mut tile = Pixmap::new(64, 64).unwrap();
    for y in 0..64u32 {
        for x in 0..64u32 {
            let (u, v) = if kind == StickerTexture::Glass {
                (x / 8, y / 8)
            } else {
                (x, y)
            };
            let mut hash = seed ^ u.wrapping_mul(0x9e37_79b9) ^ v.wrapping_mul(0x85eb_ca6b);
            hash ^= hash >> 16;
            hash = hash.wrapping_mul(0x7feb_352d);
            hash ^= hash >> 15;
            let gray = match kind {
                StickerTexture::Glass => 145 + (hash % 85) as u8,
                StickerTexture::Frost => 202 + (hash % 38) as u8,
            };
            let i = (y as usize * 64 + x as usize) * 4;
            tile.data_mut()[i..i + 4].copy_from_slice(&[gray, gray, gray, 255]);
        }
    }
    tile
}

pub(crate) fn draw(target: &mut Pixmap, object: &Object, monitor: Rect) {
    let Extra::Sticker(sticker) = &object.extra else {
        return;
    };
    if sticker.opacity == 0 {
        return;
    }
    TEXTURES.with(|cache| {
        let mut cache = cache.borrow_mut();
        let key = (sticker.texture, sticker.seed);
        if !cache.contains_key(&key) {
            if cache.len() >= 8 {
                cache.clear();
            }
            cache.insert(key, texture(key.0, key.1));
        }
        let tile = &cache[&key];
        let unit = sticker.texture_size / 8.;
        let paint = Paint {
            shader: Pattern::new(
                tile.as_ref(),
                SpreadMode::Repeat,
                if sticker.texture == StickerTexture::Glass {
                    FilterQuality::Nearest
                } else {
                    FilterQuality::Bilinear
                },
                sticker.opacity as f32 / 100.,
                Transform::from_row(
                    unit * sticker.stretch.0,
                    0.,
                    0.,
                    unit * sticker.stretch.1,
                    sticker.origin.0 - monitor.left as f32,
                    sticker.origin.1 - monitor.top as f32,
                ),
            ),
            ..Paint::default()
        };
        crate::annotation_render::mosaic::paint_mask(target, object, monitor, &paint);
    });
}

pub(crate) fn clear_cache() {
    TEXTURES.with(|cache| cache.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation_scene::{SceneCompiler, SceneInput, compose_document};
    use lexift_core::domain::{
        annotation::{Bounds, Kind, Session, Style},
        annotation_erase::{EraseRegion, EraseTarget},
    };
    use lexift_core::ports::annotation_render::{RenderLayer, RenderStamp, RenderSurface};
    use lexift_core::ports::magnifier::WindowToken;

    fn sticker(kind: Kind, scale: f32, mode: i32) -> Object {
        let values = crate::AnnotationValues {
            mode,
            size: 30,
            strength: 75.,
            zoom: 12,
            ..Default::default()
        };
        let mut object = Object {
            id: Default::default(),
            erased: vec![],
            kind,
            bounds: Bounds::from_corners((-80., -60.), (60., 50.)),
            style: Style::default(),
            points: if kind == Kind::StickerBrush {
                vec![(-60., -40.), (40., 30.)]
            } else {
                vec![]
            },
            extra: settings(&values, (-80., -60.), scale),
        };
        if kind == Kind::StickerBrush {
            object.refresh_bounds();
        }
        object
    }

    #[test]
    fn sticker_pixels_are_cached_stable_clipped_and_match_native_raster_scene() {
        for scale in [1., 1.25] {
            for kind in [Kind::StickerArea, Kind::StickerBrush] {
                for mode in [0, 1] {
                    let view = Rect {
                        left: -100,
                        top: -80,
                        right: 100,
                        bottom: 80,
                    };
                    let mut session = Session::new();
                    session.add(sticker(kind, scale, mode));
                    session.selected = None;
                    let original =
                        crate::annotation_render::render(&session, view, scale, None, None)
                            .unwrap();
                    assert!(
                        original
                            .data()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|p| p[3] > 180 && p[3] < 200)
                    );
                    let again = crate::annotation_render::render(&session, view, scale, None, None)
                        .unwrap();
                    assert_eq!(original.data(), again.data());
                    let mut compiler = SceneCompiler::default();
                    let surfaces = [RenderSurface {
                        input_window: WindowToken(1),
                        bounds: view,
                        scale,
                    }];
                    let build = |compiler: &mut SceneCompiler, session: &Session, revision| {
                        compiler
                            .compile(SceneInput {
                                session,
                                edit: None,
                                surfaces: &surfaces,
                                stamp: RenderStamp {
                                    session: 1,
                                    revision,
                                },
                                draft: None,
                                hovered: None,
                                reference: None,
                                editor: None,
                                paused_regions: &[],
                                excluded_windows: &[],
                            })
                            .unwrap()
                    };
                    let scene = build(&mut compiler, &session, 1);
                    assert!(
                        scene
                            .layers
                            .iter()
                            .all(|l| matches!(l, RenderLayer::Raster(_)))
                    );
                    let native = compose_document(&scene, view).unwrap();
                    assert_eq!(original.data(), native.data());
                    let repeated = build(&mut compiler, &session, 2);
                    for (a, b) in scene.layers.iter().zip(repeated.layers.iter()) {
                        let (RenderLayer::Raster(a), RenderLayer::Raster(b)) = (a, b) else {
                            panic!()
                        };
                        assert_eq!(a.key, b.key);
                        assert!(std::sync::Arc::ptr_eq(&a.pixels, &b.pixels));
                    }
                    let id = session.objects[0].id;
                    session.erase(
                        EraseRegion::Rectangle(Bounds::from_corners((-65., -45.), (-30., -15.))),
                        &[EraseTarget::Object(id)],
                    );
                    let erased = build(&mut compiler, &session, 3);
                    let erased = compose_document(&erased, view).unwrap();
                    assert_eq!(erased.pixel(50, 50).unwrap().alpha(), 0);
                    session.selected = Some(0);
                    let before_move = session.objects[0].clone();
                    session.begin_drag();
                    session.update_selected(|o| o.move_by(20., 10.));
                    session.finish_drag();
                    if kind == Kind::StickerBrush {
                        assert_eq!(session.objects[0], before_move);
                    }
                    session.selected = None;
                    let moved = crate::annotation_render::render(&session, view, scale, None, None)
                        .unwrap();
                    let hole = if kind == Kind::StickerArea {
                        (70, 60)
                    } else {
                        (50, 50)
                    };
                    assert_eq!(moved.pixel(hole.0, hole.1).unwrap().alpha(), 0);
                    let compiled =
                        compose_document(&build(&mut compiler, &session, 4), view).unwrap();
                    assert_eq!(moved.data(), compiled.data());
                    if let Ok(directory) = std::env::var("LEXIFT_STICKER_RENDER_DIR") {
                        let mut proof = Pixmap::new(200, 160).unwrap();
                        proof.fill(tiny_skia::Color::from_rgba8(25, 80, 120, 255));
                        proof.draw_pixmap(
                            0,
                            0,
                            original.as_ref(),
                            &tiny_skia::PixmapPaint::default(),
                            Transform::identity(),
                            None,
                        );
                        let mut bytes = b"P6\n200 160\n255\n".to_vec();
                        for pixel in proof.data().as_chunks::<4>().0.iter() {
                            bytes.extend_from_slice(&pixel[..3]);
                        }
                        std::fs::write(
                            format!("{directory}/sticker-{kind:?}-{mode}-{scale}.ppm"),
                            bytes,
                        )
                        .unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn zero_opacity_and_texture_parameters_do_not_capture_or_leak_old_pixels() {
        let mut object = sticker(Kind::StickerArea, 1.25, 0);
        let view = Rect {
            left: -100,
            top: -80,
            right: 100,
            bottom: 80,
        };
        let mut first = Pixmap::new(200, 160).unwrap();
        draw(&mut first, &object, view);
        assert!(first.data().as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        let Extra::Sticker(s) = &mut object.extra else {
            panic!()
        };
        s.opacity = 0;
        let mut clear = Pixmap::new(200, 160).unwrap();
        draw(&mut clear, &object, view);
        assert!(clear.data().iter().all(|p| *p == 0));
        let Extra::Sticker(s) = &mut object.extra else {
            panic!()
        };
        s.opacity = 100;
        s.texture = StickerTexture::Frost;
        let mut other = Pixmap::new(200, 160).unwrap();
        draw(&mut other, &object, view);
        assert_ne!(first.data(), other.data());
    }

    #[test]
    fn magnifier_include_annotations_samples_sticker_in_document_order() {
        use lexift_core::domain::annotation::MagnifierConnector;
        let mut session = Session::new();
        session.add(sticker(Kind::StickerArea, 1., 0));
        let mut lens = Object {
            id: Default::default(),
            erased: vec![],
            kind: Kind::Magnifier,
            bounds: Bounds::from_corners((-40., -30.), (40., 30.)),
            points: vec![],
            style: Style {
                outline: false,
                ..Style::default()
            },
            extra: Extra::Magnifier {
                output: Bounds::from_corners((100., 100.), (200., 200.)),
                zoom: 2.,
                ellipse: false,
                connector: MagnifierConnector::Plain,
                erase_annotations: false,
                antialias: true,
                shadow: false,
            },
        };
        session.add(lens.clone());
        session.selected = None;
        let view = Rect {
            left: -100,
            top: -100,
            right: 300,
            bottom: 300,
        };
        let include = crate::annotation_render::render(&session, view, 1., None, None).unwrap();
        assert!(include.pixel(250, 250).unwrap().alpha() > 180);
        if let Extra::Magnifier {
            erase_annotations, ..
        } = &mut lens.extra
        {
            *erase_annotations = true;
        }
        session.objects[1] = lens;
        let exclude = crate::annotation_render::render(&session, view, 1., None, None).unwrap();
        assert_eq!(exclude.pixel(250, 250).unwrap().alpha(), 0);
    }
}
