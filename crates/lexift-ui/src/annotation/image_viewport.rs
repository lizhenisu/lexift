//! Physical-pixel image navigation, independent of documents and window backends.
// Both hosts share projection and anchor arithmetic, while retaining their own
// precision, fit padding and zoom limits. Generic arithmetic preserves the
// independent editor's existing f32 rounding and the pin viewport's f64 precision.
fn project<T>(point: [T; 2], offset: [T; 2], scale: T) -> [T; 2]
where
    T: Copy + std::ops::Mul<Output = T> + std::ops::Add<Output = T>,
{
    std::array::from_fn(|i| point[i] * scale + offset[i])
}
fn unproject<T>(point: [T; 2], offset: [T; 2], scale: T) -> [T; 2]
where
    T: Copy + std::ops::Sub<Output = T> + std::ops::Div<Output = T>,
{
    std::array::from_fn(|i| (point[i] - offset[i]) / scale)
}
fn anchored<T>(point: [T; 2], source: [T; 2], scale: T) -> [T; 2]
where
    T: Copy + std::ops::Sub<Output = T> + std::ops::Mul<Output = T>,
{
    std::array::from_fn(|i| point[i] - source[i] * scale)
}

/// A bounded host can magnify a visible interval without growing its source texture.
/// f64 view arithmetic is checked against the f32 coordinates used by annotation objects.
#[derive(Clone, Debug)]
pub(crate) struct PreciseViewport {
    pub content: [f64; 4],
    pub size: [f64; 2],
    pub scale: f64,
    pub offset: [f64; 2],
    pan: Option<([f64; 2], [f64; 2])>,
    wheel: f64,
    pub limited: bool,
    pub precision: Option<(
        lexift_core::domain::geometry::Rect,
        lexift_core::domain::image_transform::Orientation,
    )>,
}
impl PreciseViewport {
    pub fn new(content: [f64; 4], size: [f64; 2]) -> Self {
        let mut view = Self {
            content,
            size,
            scale: 1.,
            offset: [0.; 2],
            pan: None,
            wheel: 0.,
            limited: false,
            precision: None,
        };
        view.fit(1.);
        view
    }
    pub fn source(&self, p: [f64; 2]) -> [f64; 2] {
        unproject(p, self.offset, self.scale)
    }
    pub fn screen(&self, p: [f64; 2]) -> [f64; 2] {
        project(p, self.offset, self.scale)
    }
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let p = self.source(p);
        (0..2).all(|i| p[i] >= self.content[i] && p[i] < self.content[i + 2])
    }
    pub fn fit(&mut self, ceiling: f64) {
        self.scale = (0..2)
            .map(|i| self.size[i] / (self.content[i + 2] - self.content[i]).max(1.))
            .fold(ceiling.min(1.), f64::min)
            .max(f64::MIN_POSITIVE);
        self.offset = std::array::from_fn(|i| {
            (self.size[i] - (self.content[i + 2] - self.content[i]) * self.scale) / 2.
                - self.content[i] * self.scale
        });
        self.limited = false;
        self.pan = None;
        self.wheel = 0.;
    }
    pub fn resize(&mut self, size: [f64; 2]) {
        if size == self.size {
            return;
        }
        let center = self.source(self.size.map(|v| v / 2.));
        self.size = size;
        self.offset = std::array::from_fn(|i| size[i] / 2. - center[i] * self.scale);
        self.finish_pan();
    }
    pub fn zoom_at(&mut self, anchor: [f64; 2], scale: f64) -> bool {
        if self.panning() || !scale.is_finite() || scale <= 0. {
            return false;
        }
        let p = self.source(anchor);
        let offset = anchored(anchor, p, scale);
        // No fixed product zoom cap: stop only before source quantization loses
        // a screen pixel or overflows the renderer's finite coordinates.
        for q in [[0.; 2], self.size, anchor] {
            let source = unproject(q, offset, scale);
            let original = if let Some((full, direction)) = self.precision {
                let Ok(oriented) = direction.rect(full, full) else {
                    return false;
                };
                direction.inverse().point(source, oriented)
            } else {
                source
            };
            let rounded = original.map(|x| f64::from(x as f32));
            if rounded.iter().any(|x| !x.is_finite()) {
                return false;
            }
            for coordinate in original {
                let magnitude = (coordinate as f32).abs();
                let quantum =
                    f64::from(f32::from_bits(magnitude.to_bits() + 1)) - f64::from(magnitude);
                if !quantum.is_finite() || quantum * scale > 1. {
                    return false;
                }
            }
            let projected = if let Some((full, direction)) = self.precision {
                direction.point(rounded, full)
            } else {
                rounded
            };
            let back = project(projected, offset, scale);
            if (0..2).any(|i| (back[i] - q[i]).abs() > 0.5) {
                return false;
            }
        }
        self.scale = scale;
        self.offset = offset;
        true
    }
    pub fn wheel(&mut self, point: [f64; 2], delta: f64) -> bool {
        self.limited = false;
        if !delta.is_finite() || self.panning() {
            return false;
        }
        self.wheel += delta;
        let steps = (self.wheel / 60.).trunc().clamp(-128., 128.) as i32;
        self.wheel -= f64::from(steps) * 60.;
        let mut changed = false;
        for _ in 0..steps.unsigned_abs() {
            let mut scale = if steps > 0 {
                self.scale * 1.1
            } else {
                self.scale / 1.1
            };
            if (self.scale < 1. && scale > 1.) || (self.scale > 1. && scale < 1.) {
                scale = 1.;
            }
            if !self.zoom_at(point, scale) {
                self.limited = true;
                break;
            }
            changed = true;
        }
        changed
    }
    pub fn begin_pan(&mut self, p: [f64; 2]) {
        self.pan = Some((p, self.offset));
        self.wheel = 0.;
    }
    pub fn pan_to(&mut self, p: [f64; 2]) {
        if let Some((start, offset)) = self.pan {
            self.offset = std::array::from_fn(|i| offset[i] + p[i] - start[i]);
        }
    }
    pub fn panning(&self) -> bool {
        self.pan.is_some()
    }
    pub fn finish_pan(&mut self) {
        self.pan = None;
    }
    pub fn cancel_pan(&mut self) -> bool {
        if let Some((_, offset)) = self.pan.take() {
            self.offset = offset;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Viewport {
    pub image: [f32; 2],
    pub size: [f32; 2],
    pub content: [f32; 4],
    pub scale: f32,
    pub offset: [f32; 2],
    pan: Option<([f32; 2], [f32; 2])>,
    wheel: f32,
}
impl Viewport {
    pub fn new(image: [f32; 2], size: [f32; 2]) -> Self {
        let mut v = Self {
            image,
            content: [0., 0., image[0], image[1]],
            size,
            scale: 1.,
            offset: [0.; 2],
            pan: None,
            wheel: 0.,
        };
        v.fit();
        v
    }
    pub fn source(&self, point: [f32; 2]) -> [f32; 2] {
        unproject(point, self.offset, self.scale)
    }
    pub fn screen(&self, point: [f32; 2]) -> [f32; 2] {
        project(point, self.offset, self.scale)
    }
    pub fn contains(&self, point: [f32; 2]) -> bool {
        let q = self.source(point);
        (0..2).all(|i| q[i] >= self.content[i] && q[i] < self.content[i + 2])
    }
    pub fn fit(&mut self) {
        let extent = [
            self.content[2] - self.content[0],
            self.content[3] - self.content[1],
        ];
        self.scale = ((self.size[0] - 16.).max(1.) / extent[0].max(1.))
            .min((self.size[1] - 16.).max(1.) / extent[1].max(1.))
            .clamp(0.001, 1.);
        self.offset = std::array::from_fn(|i| {
            (self.size[i] - extent[i] * self.scale) / 2. - self.content[i] * self.scale
        });
        self.pan = None;
        self.wheel = 0.;
    }
    /// Resizing and DPI changes retain physical zoom and the source point at the center.
    pub fn resize(&mut self, size: [f32; 2]) {
        if size == self.size {
            return;
        }
        let center = self.source(self.size.map(|x| x / 2.));
        self.size = size;
        self.offset = std::array::from_fn(|i| size[i] / 2. - center[i] * self.scale);
        self.finish_pan();
    }
    pub fn zoom_at(&mut self, point: [f32; 2], scale: f32) {
        let source = self.source(point);
        self.scale = scale.clamp(0.001, 10.);
        self.offset = anchored(point, source, self.scale);
    }
    /// The existing native input bridge reports 60 logical pixels per wheel notch.
    pub fn wheel(&mut self, point: [f32; 2], logical_delta: f32) -> bool {
        if self.panning() || !logical_delta.is_finite() {
            return false;
        }
        self.wheel += logical_delta;
        let steps = (self.wheel / 60.).trunc() as i32;
        self.wheel -= steps as f32 * 60.;
        if steps == 0 {
            return false;
        }
        // Bound an unusually large native delta without allowing unbounded UI work.
        for _ in 0..steps.unsigned_abs().min(128) {
            let mut next = if steps > 0 {
                self.scale * 1.15
            } else {
                self.scale / 1.15
            };
            if (self.scale < 1. && next > 1.) || (self.scale > 1. && next < 1.) {
                next = 1.;
            }
            self.zoom_at(point, next);
        }
        true
    }
    pub fn begin_pan(&mut self, point: [f32; 2]) {
        self.pan = Some((point, self.offset));
        self.wheel = 0.;
    }
    pub fn pan_to(&mut self, point: [f32; 2]) {
        if let Some((start, offset)) = self.pan {
            self.offset = std::array::from_fn(|i| offset[i] + point[i] - start[i]);
        }
    }
    pub fn panning(&self) -> bool {
        self.pan.is_some()
    }
    pub fn finish_pan(&mut self) {
        self.pan = None;
    }
    pub fn cancel_pan(&mut self) -> bool {
        if let Some((_, offset)) = self.pan.take() {
            self.offset = offset;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn near(a: [f32; 2], b: [f32; 2]) {
        for i in 0..2 {
            assert!((a[i] - b[i]).abs() < 0.01, "{a:?} {b:?}");
        }
    }
    #[test]
    fn cropped_content_fits_without_changing_original_coordinates() {
        let mut v = Viewport::new([600., 2000.], [900., 700.]);
        v.content = [80., 200., 520., 1800.];
        v.fit();
        let midpoint = [300., 1000.];
        near(v.screen(midpoint), [450., 350.]);
        assert!(!v.contains(v.screen([20., 400.])));
        assert!(v.contains(v.screen([100., 400.])));
        let anchor = [200., 300.];
        let q = v.source(anchor);
        v.zoom_at(anchor, 1.25);
        near(v.source(anchor), q);
        let center = v.source([450., 350.]);
        v.resize([1125., 875.]);
        near(v.source([562.5, 437.5]), center);
        assert_eq!(v.scale, 1.25);
        assert_eq!(v.image, [600., 2000.]);
    }
    #[test]
    fn anchor_roundtrip_fine_wheel_and_hundred_percent_snap() {
        let mut v = Viewport::new([701., 2087.], [1100., 800.]);
        let p = [312., 417.];
        let q = v.source(p);
        assert!(!v.wheel(p, 30.));
        assert!(v.wheel(p, 30.));
        near(v.source(p), q);
        near(v.screen(q), p);
        v.zoom_at(p, 0.9);
        v.wheel(p, 60.);
        assert_eq!(v.scale, 1.);
        v.zoom_at(p, 1.1);
        v.wheel(p, -60.);
        assert_eq!(v.scale, 1.);
        v.zoom_at(p, 100.);
        assert_eq!(v.scale, 10.);
        v.zoom_at(p, 0.);
        assert_eq!(v.scale, 0.001);
    }
    #[test]
    fn pan_outside_resize_and_cancel_do_not_modify_image() {
        let mut v = Viewport::new([600., 32768.], [900., 500.]);
        assert!(v.scale < 0.02);
        let original = v.offset;
        v.begin_pan([100., 100.]);
        v.pan_to([-2000., 3000.]);
        assert!(!v.contains([100., 100.]));
        assert!(v.cancel_pan());
        near(v.offset, original);
        v.begin_pan([0., 0.]);
        v.pan_to([200., 300.]);
        v.finish_pan();
        let center = v.source([450., 250.]);
        let scale = v.scale;
        v.resize([1125., 625.]);
        near(v.source([562.5, 312.5]), center);
        assert_eq!(v.scale, scale);
        assert_eq!(v.image, [600., 32768.]);
        let small = Viewport::new([100., 100.], [900., 500.]);
        assert_eq!(small.scale, 1.);
    }
}

#[cfg(test)]
mod precise_tests {
    use super::PreciseViewport;
    fn close(a: [f64; 2], b: [f64; 2]) {
        for i in 0..2 {
            assert!((a[i] - b[i]).abs() < 1e-7, "{a:?} != {b:?}");
        }
    }
    #[test]
    fn negative_origin_anchor_and_physical_zoom_survive_resize() {
        let mut v = PreciseViewport::new([-400., -200., 600., 32768.], [900., 600.]);
        let anchor = [200., 300.];
        let source = v.source(anchor);
        assert!(!v.wheel(anchor, 29.));
        assert!(!v.wheel(anchor, 30.));
        assert!(v.wheel(anchor, 1.));
        close(v.source(anchor), source);
        assert!(v.zoom_at(anchor, 0.95));
        assert!(v.wheel(anchor, 60.));
        assert_eq!(v.scale, 1.);
        let center = v.source([450., 300.]);
        v.resize([1125., 750.]);
        close(v.source([562.5, 375.]), center);
        assert_eq!(v.scale, 1.);
    }
    #[test]
    fn magnification_has_no_product_cap_but_stops_before_annotation_quantization() {
        let mut v = PreciseViewport::new([0., 0., 500., 2000.], [800., 600.]);
        let p = [300., 200.];
        assert!(v.zoom_at(p, 100.));
        assert_eq!(v.scale, 100.);
        let mut bounded = v.clone();
        for _ in 0..512 {
            bounded.wheel(p, 60.);
            if bounded.limited {
                break;
            }
        }
        assert!(bounded.limited && bounded.scale.is_finite());
        bounded.fit(1.);
        assert!(!bounded.limited);
        let before = v.clone();
        assert!(!v.zoom_at(p, 1e30));
        assert_eq!(v.scale, before.scale);
        assert_eq!(v.offset, before.offset);
        assert!(!v.zoom_at(p, f64::INFINITY));
        assert!(!v.zoom_at(p, f64::NAN));
        assert!(!v.zoom_at(p, -1.));
        let mut high_origin =
            PreciseViewport::new([1e6, 1e6, 1e6 + 100., 1e6 + 100.], [800., 600.]);
        assert!(!high_origin.zoom_at([400., 300.], 100.));
    }
    #[test]
    fn precision_checks_original_coordinates_after_mirroring_a_far_crop() {
        use lexift_core::domain::{
            geometry::Rect,
            image_transform::{Action, Orientation},
        };
        let full = Rect {
            left: 0,
            top: 0,
            right: 10_000_000,
            bottom: 2,
        };
        let mut v = PreciseViewport::new([0., 0., 1000., 2.], [800., 600.]);
        assert!(v.zoom_at([400., 300.], 2.));
        let mut v = PreciseViewport::new([0., 0., 1000., 2.], [800., 600.]);
        v.precision = Some((full, Orientation::identity().action(Action::Horizontal)));
        assert!(!v.zoom_at([400., 300.], 2.));
        assert!(v.zoom_at([400., 300.], 1.));
    }
    #[test]
    fn pan_escape_loss_and_fit_are_view_only() {
        let mut v = PreciseViewport::new([-5., -8., 4., 9.], [100., 80.]);
        let old = v.offset;
        v.begin_pan([4., 5.]);
        v.pan_to([4000., 3000.]);
        assert!(!v.contains([10., 10.]));
        assert!(!v.wheel([5., 5.], 60.));
        assert!(v.cancel_pan());
        close(v.offset, old);
        v.begin_pan([0., 0.]);
        v.pan_to([30., 40.]);
        v.finish_pan();
        close(v.offset, [old[0] + 30., old[1] + 40.]);
        v.fit(1.);
        assert_eq!(v.scale, 1.);
        assert_eq!(v.content, [-5., -8., 4., 9.]);
    }
}
