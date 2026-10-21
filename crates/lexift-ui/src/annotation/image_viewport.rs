//! Physical-pixel image navigation, independent of documents and window backends.
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
        std::array::from_fn(|i| (point[i] - self.offset[i]) / self.scale)
    }
    pub fn screen(&self, point: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|i| point[i] * self.scale + self.offset[i])
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
        self.offset = std::array::from_fn(|i| point[i] - source[i] * self.scale);
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
