//! Local lens workspaces and desktop-to-source mapping, with no native lens HWND.
use super::graphics::{Graphics, Texture};
use lexift_core::{
    domain::{
        annotation::{Extra, Object},
        geometry::Rect,
    },
    ports::annotation_render::RasterLayer,
};

pub(super) fn source_for(object: &Object, mask: Rect) -> Rect {
    let Extra::Magnifier {
        output,
        zoom,
        antialias,
        ..
    } = object.extra
    else {
        unreachable!()
    };
    let zoom = zoom.clamp(1., 8.);
    let edge = i32::from(antialias);
    let source = Rect {
        left: (object.bounds.left + (mask.left as f32 - output.left) / zoom).floor() as i32,
        top: (object.bounds.top + (mask.top as f32 - output.top) / zoom).floor() as i32,
        right: (object.bounds.left + (mask.right as f32 - output.left) / zoom).ceil() as i32,
        bottom: (object.bounds.top + (mask.bottom as f32 - output.top) / zoom).ceil() as i32,
    };
    Rect {
        left: source
            .left
            .saturating_sub(edge)
            .max(object.bounds.left.floor() as i32),
        top: source
            .top
            .saturating_sub(edge)
            .max(object.bounds.top.floor() as i32),
        right: source
            .right
            .saturating_add(edge)
            .min(object.bounds.right.ceil() as i32),
        bottom: source
            .bottom
            .saturating_add(edge)
            .min(object.bounds.bottom.ceil() as i32),
    }
}

pub(super) struct Tile {
    pub sample: Rect,
    pub bounds: Rect,
    pub source: Texture,
    pub output: Texture,
    pub valid: bool,
}
impl Tile {
    pub fn new(graphics: &Graphics, bounds: Rect, sample: Rect) -> windows::core::Result<Self> {
        let size = |a: i32, b: i32| {
            b.checked_sub(a)
                .filter(|v| *v > 0)
                .map(|v| v as u32)
                .ok_or_else(|| {
                    windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG)
                })
        };
        Ok(Self {
            sample,
            bounds,
            valid: false,
            source: Texture::create(
                &graphics.device,
                size(sample.left, sample.right)?,
                size(sample.top, sample.bottom)?,
                None,
            )?,
            output: Texture::create(
                &graphics.device,
                size(bounds.left, bounds.right)?,
                size(bounds.top, bounds.bottom)?,
                None,
            )?,
        })
    }
    pub fn reuse(&mut self, bounds: Rect, sample: Rect) -> bool {
        if !self.source.matches_bounds(sample) || !self.output.matches_bounds(bounds) {
            return false;
        }
        if self.bounds != bounds || self.sample != sample {
            self.valid = false;
        }
        self.bounds = bounds;
        self.sample = sample;
        true
    }
    pub fn render(
        &mut self,
        graphics: &Graphics,
        object: &Object,
        mask: &RasterLayer,
        texture: &Texture,
    ) {
        let Extra::Magnifier {
            output,
            zoom,
            antialias,
            ..
        } = object.extra
        else {
            unreachable!()
        };
        let zoom = zoom.clamp(1., 8.);
        let uv = [
            (object.bounds.left + (mask.bounds.left as f32 - output.left) / zoom
                - self.sample.left as f32)
                / self.source.width as f32,
            (object.bounds.top + (mask.bounds.top as f32 - output.top) / zoom
                - self.sample.top as f32)
                / self.source.height as f32,
            self.output.width as f32 / zoom / self.source.width as f32,
            self.output.height as f32 / zoom / self.source.height as f32,
        ];
        graphics.lens(&self.source, texture, &self.output, uv, antialias);
        self.valid = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::annotation::{Bounds, Kind, MagnifierConnector, Style},
        ports::annotation_render::RasterKey,
    };
    use std::sync::Arc;

    fn object(antialias: bool) -> Object {
        Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: Bounds::from_corners((-4., -4.), (0., 0.)),
            kind: Kind::Magnifier,
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Magnifier {
                output: Bounds::from_corners((10., 20.), (18., 28.)),
                zoom: 2.,
                ellipse: true,
                connector: MagnifierConnector::None,
                erase_annotations: false,
                antialias,
                shadow: false,
            },
        }
    }

    #[test]
    fn cropped_lens_requests_only_its_visible_source_with_bilinear_margin() {
        let bounds = Rect {
            left: 14,
            top: 20,
            right: 18,
            bottom: 28,
        };
        assert_eq!(
            source_for(&object(false), bounds),
            Rect {
                left: -2,
                top: -4,
                right: 0,
                bottom: 0
            }
        );
        assert_eq!(
            source_for(&object(true), bounds),
            Rect {
                left: -3,
                top: -4,
                right: 0,
                bottom: 0
            }
        );
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_lens_matches_nearest_bilinear_and_clipped_output_at_negative_source() {
        use super::super::graphics::Blit;
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let pixels: Vec<u8> = (0..16)
            .flat_map(|i| [20 + (i % 4) * 40, 40 + (i / 4) * 40, 10, 255])
            .collect();
        let source = Texture::create(&graphics.device, 4, 4, Some(&pixels)).unwrap();
        for antialias in [false, true] {
            let object = object(antialias);
            let bounds = Rect {
                left: 10,
                top: 20,
                right: 18,
                bottom: 28,
            };
            let mask_pixels: Vec<u8> = (0..64)
                .flat_map(|i| {
                    let alpha = if [0, 7, 56, 63].contains(&i) {
                        0
                    } else if i == 27 {
                        128
                    } else {
                        255
                    };
                    [alpha; 4]
                })
                .collect();
            let mask = RasterLayer {
                key: RasterKey {
                    asset: 1,
                    revision: 1,
                },
                bounds,
                pixels: mask_pixels.clone().into(),
            };
            let mask_texture = Texture::create(&graphics.device, 8, 8, Some(&mask_pixels)).unwrap();
            let mut tile = Tile::new(&graphics, bounds, source_for(&object, bounds)).unwrap();
            graphics.clear(&tile.source);
            graphics.blit(
                &source,
                &tile.source,
                tile.sample,
                Blit {
                    destination: Rect {
                        left: -4,
                        top: -4,
                        right: 0,
                        bottom: 0,
                    },
                    source_rect: Rect {
                        left: 0,
                        top: 0,
                        right: 4,
                        bottom: 4,
                    },
                    opacity: 1.,
                    blend: true,
                },
            );
            tile.render(&graphics, &object, &mask, &mask_texture);
            let result = super::super::graphics::tests::readback(&graphics, &tile.output);
            for y in 0..8 {
                for x in 0..8 {
                    let position = |value: usize| {
                        if antialias {
                            ((value as f32 + 0.5) / 2. - 0.5).clamp(0., 3.)
                        } else {
                            (value / 2) as f32
                        }
                    };
                    let color = [20. + position(x) * 40., 40. + position(y) * 40., 10., 255.];
                    let index = (y * 8 + x) * 4;
                    for channel in 0..4 {
                        let expected = (color[channel] * f32::from(mask_pixels[index + 3]) / 255.)
                            .round() as u8;
                        assert!(
                            result[index + channel].abs_diff(expected) <= 1,
                            "({x},{y}) channel {channel}"
                        );
                    }
                }
            }
            let cropped_bounds = Rect { left: 14, ..bounds };
            let cropped_pixels: Vec<u8> = mask_pixels
                .as_chunks::<32>()
                .0
                .iter()
                .flat_map(|row| row[16..].iter().copied())
                .collect();
            let cropped_mask = RasterLayer {
                key: mask.key,
                bounds: cropped_bounds,
                pixels: Arc::from(cropped_pixels),
            };
            let cropped_texture =
                Texture::create(&graphics.device, 4, 8, Some(&cropped_mask.pixels)).unwrap();
            let mut cropped = Tile::new(
                &graphics,
                cropped_bounds,
                source_for(&object, cropped_bounds),
            )
            .unwrap();
            graphics.clear(&cropped.source);
            graphics.blit(
                &source,
                &cropped.source,
                cropped.sample,
                Blit {
                    destination: Rect {
                        left: -4,
                        top: -4,
                        right: 0,
                        bottom: 0,
                    },
                    source_rect: Rect {
                        left: 0,
                        top: 0,
                        right: 4,
                        bottom: 4,
                    },
                    opacity: 1.,
                    blend: true,
                },
            );
            cropped.render(&graphics, &object, &cropped_mask, &cropped_texture);
            let actual = super::super::graphics::tests::readback(&graphics, &cropped.output);
            for y in 0..8 {
                assert_eq!(
                    &actual[y * 16..y * 16 + 16],
                    &result[y * 32 + 16..y * 32 + 32]
                );
            }
        }
    }
}
