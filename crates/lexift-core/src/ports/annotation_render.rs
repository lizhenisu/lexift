//! Immutable annotation scenes exchanged with a native render adapter.
//!
//! Desktop textures and graphics devices never cross this boundary. Ordinary
//! annotation pixels are uploaded only when their asset revision changes.
use crate::{
    Error, Result,
    domain::{annotation::Object, geometry::Rect},
    ports::magnifier::WindowToken,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderStamp {
    pub session: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Session-scoped immutable raster identity. Pixel or dimension changes require
/// a new revision; translating unchanged pixels may retain the same key.
pub struct RasterKey {
    pub asset: u64,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct RasterLayer {
    pub key: RasterKey,
    pub bounds: Rect,
    /// Tightly packed premultiplied RGBA in physical desktop coordinates.
    pub pixels: Arc<[u8]>,
}

impl RasterLayer {
    /// Validate before native upload, including coordinate and buffer-size overflow.
    pub fn validate(&self) -> Result<()> {
        let (width, height) = dimensions(self.bounds)?;
        let bytes = width
            .checked_mul(height)
            .and_then(|size| size.checked_mul(4));
        if bytes != Some(self.pixels.len()) {
            return Err(Error::new(
                "Annotation raster size does not match its bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct MosaicLayer {
    pub object: Arc<Object>,
    /// Cached geometric coverage, independent of desktop pixels. Each tile is
    /// premultiplied white RGBA; only alpha is used by the native effect shader.
    pub masks: Arc<[RasterLayer]>,
}

#[derive(Clone, Debug)]
pub struct MagnifierLayer {
    pub object: Arc<Object>,
    /// Output coverage, clipped to visible viewports and independent of pixels.
    pub masks: Arc<[RasterLayer]>,
    /// Shared UI rasterization of source/output borders, connector and shadow.
    pub decorations: Arc<[RasterLayer]>,
}

#[derive(Clone, Debug)]
pub enum RenderLayer {
    Raster(RasterLayer),
    /// Metadata only: source pixels come from clean native desktop capture.
    Mosaic(MosaicLayer),
    Magnifier(MagnifierLayer),
}

impl RenderLayer {
    pub fn rasters(&self) -> impl Iterator<Item = &RasterLayer> {
        let (first, second): (&[RasterLayer], &[RasterLayer]) = match self {
            Self::Raster(layer) => (std::slice::from_ref(layer), &[]),
            Self::Mosaic(layer) => (&layer.masks, &[]),
            Self::Magnifier(layer) => (&layer.masks, &layer.decorations),
        };
        first.iter().chain(second.iter())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderSurface {
    /// The existing input canvas stays above the adapter's non-activating surface.
    pub input_window: WindowToken,
    pub bounds: Rect,
    pub scale: f32,
}

#[derive(Clone, Debug)]
pub struct AnnotationScene {
    pub stamp: RenderStamp,
    pub surfaces: Arc<[RenderSurface]>,
    /// Document order, including lower ordinary marks seen by later effects.
    pub layers: Arc<[RenderLayer]>,
    /// Caret, selection and handles; never contribute to effect sampling.
    pub interaction: Arc<[RasterLayer]>,
    pub paused_regions: Arc<[Rect]>,
    pub excluded_windows: Arc<[WindowToken]>,
}

impl AnnotationScene {
    pub fn validate(&self) -> Result<()> {
        use crate::domain::annotation::{Extra, Kind};
        for surface in &*self.surfaces {
            dimensions(surface.bounds)?;
            if surface.input_window.0 == 0 || !surface.scale.is_finite() || surface.scale <= 0. {
                return Err(Error::new("Invalid annotation render surface"));
            }
        }
        for layer in &*self.layers {
            match layer {
                RenderLayer::Raster(raster) => raster.validate()?,
                RenderLayer::Mosaic(layer)
                    if matches!(layer.object.kind, Kind::MosaicArea | Kind::MosaicBrush)
                        && matches!(layer.object.extra, Extra::Mosaic(_)) =>
                {
                    if let Extra::Mosaic(settings) = &layer.object.extra
                        && (!settings.creation_scale.is_finite()
                            || settings.creation_scale <= 0.
                            || !settings.diameter.is_finite()
                            || settings.diameter <= 0.
                            || !settings.blur_sigma().is_finite())
                    {
                        return Err(Error::new("Invalid annotation effect dimensions"));
                    }
                    if layer.masks.is_empty()
                        && matches!(&layer.object.extra, Extra::Mosaic(settings) if settings.strength != 0)
                    {
                        return Err(Error::new(
                            "Visible annotation effect needs geometric coverage",
                        ));
                    }
                    for mask in &*layer.masks {
                        mask.validate()?;
                    }
                }
                RenderLayer::Magnifier(layer)
                    if layer.object.kind == Kind::Magnifier
                        && matches!(layer.object.extra, Extra::Magnifier { .. }) =>
                {
                    if let Extra::Magnifier { output, zoom, .. } = layer.object.extra {
                        let valid = |bounds: crate::domain::annotation::Bounds| {
                            [bounds.left, bounds.top, bounds.right, bounds.bottom]
                                .iter()
                                .all(|v| v.is_finite())
                                && bounds.right > bounds.left
                                && bounds.bottom > bounds.top
                        };
                        if !valid(layer.object.bounds)
                            || !valid(output)
                            || !zoom.is_finite()
                            || zoom <= 0.
                        {
                            return Err(Error::new("Invalid magnifier geometry"));
                        }
                    }
                    for raster in layer.masks.iter().chain(layer.decorations.iter()) {
                        raster.validate()?;
                    }
                }
                _ => {
                    return Err(Error::new(
                        "Annotation effect does not match its object kind",
                    ));
                }
            }
        }
        for raster in &*self.interaction {
            raster.validate()?;
        }
        Ok(())
    }
}

fn dimensions(bounds: Rect) -> Result<(usize, usize)> {
    let width = bounds.right as i64 - bounds.left as i64;
    let height = bounds.bottom as i64 - bounds.top as i64;
    if width <= 0 || height <= 0 || width > i64::from(i32::MAX) || height > i64::from(i32::MAX) {
        return Err(Error::new("Annotation render bounds must be nonempty"));
    }
    Ok((width as usize, height as usize))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBackend {
    Gpu,
    Software,
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderCapabilities {
    pub backend: RenderBackend,
    pub max_refresh_hz: u32,
}

#[derive(Clone, Debug)]
pub enum RenderWait {
    Source,
    Paused,
    Presentation,
    Recovering,
}

#[derive(Clone, Debug)]
pub struct RenderCompletion {
    pub stamp: RenderStamp,
    /// Effect output coverage backed by a completed sample/filter result.
    /// Never includes requested but uninitialized or newly paused outputs.
    pub sampled_regions: Arc<[Rect]>,
    pub presented: bool,
    /// Intentional pause is distinct from missing source or stalled display.
    pub wait: Option<RenderWait>,
}

/// Native adapter accepts latest-only scenes and owns all capture/display resources.
pub trait AnnotationRenderPort: Send + Sync {
    fn capabilities(&self) -> RenderCapabilities;
    /// Enqueue without waiting for graphics processing or GPU readback on the caller.
    fn submit(&self, scene: Arc<AnnotationScene>) -> Result<()>;
    fn latest(&self) -> Result<Option<RenderCompletion>>;
    /// Invalidate pending work and release surfaces, captures and retained textures.
    fn clear(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_assets_include_decorations_and_invalid_geometry_is_rejected() {
        use crate::domain::annotation::{Bounds, Extra, Kind, MagnifierConnector, Style};
        let mask = RasterLayer {
            key: RasterKey {
                asset: 1,
                revision: 1,
            },
            bounds: Rect {
                left: 1,
                top: 1,
                right: 3,
                bottom: 3,
            },
            pixels: vec![255; 16].into(),
        };
        let decoration = RasterLayer {
            key: RasterKey {
                asset: 2,
                revision: 1,
            },
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            pixels: vec![255; 4].into(),
        };
        let mut object = Object {
            id: Default::default(),
            kind: Kind::Magnifier,
            bounds: Bounds::from_corners((-4., -4.), (-3., -3.)),
            style: Style::default(),
            points: Vec::new(),
            extra: Extra::Magnifier {
                output: Bounds::from_corners((1., 1.), (3., 3.)),
                zoom: 2.,
                ellipse: false,
                connector: MagnifierConnector::None,
                erase_annotations: false,
                antialias: true,
                shadow: false,
            },
        };
        let make = |object| AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: Arc::from([]),
            layers: vec![RenderLayer::Magnifier(MagnifierLayer {
                object: Arc::new(object),
                masks: vec![mask.clone()].into(),
                decorations: vec![decoration.clone()].into(),
            })]
            .into(),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        let scene = make(object.clone());
        assert!(scene.validate().is_ok());
        assert_eq!(
            scene.layers[0]
                .rasters()
                .map(|layer| layer.key.asset)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        if let Extra::Magnifier { zoom, .. } = &mut object.extra {
            *zoom = f32::NAN;
        }
        assert!(make(object).validate().is_err());
    }

    #[test]
    fn raster_validation_handles_negative_coordinates_and_rejects_bad_sizes() {
        let mut raster = RasterLayer {
            key: RasterKey {
                asset: 1,
                revision: 1,
            },
            bounds: Rect {
                left: -3,
                top: -2,
                right: -1,
                bottom: 0,
            },
            pixels: vec![0; 16].into(),
        };
        assert!(raster.validate().is_ok());
        raster.pixels = vec![0; 15].into();
        assert!(raster.validate().is_err());
        raster.bounds = Rect {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        };
        assert!(raster.validate().is_err());
        raster.bounds.right = raster.bounds.left;
        assert!(raster.validate().is_err());
    }

    #[test]
    fn surface_extent_cannot_overflow_native_coordinate_subtraction() {
        let scene = AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            surfaces: vec![RenderSurface {
                input_window: WindowToken(1),
                scale: 1.,
                bounds: Rect {
                    left: i32::MIN,
                    top: 0,
                    right: i32::MAX,
                    bottom: 10,
                },
            }]
            .into(),
            layers: Arc::from([]),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        assert!(scene.validate().is_err());
    }
}
