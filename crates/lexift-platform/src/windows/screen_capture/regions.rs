//! Bounded invalidation metadata shared by capture and effect preparation.
use lexift_core::domain::geometry::Rect;

#[derive(Default)]
pub(crate) struct ChangedRegions(Vec<Rect>);

impl ChangedRegions {
    pub fn extend(&mut self, regions: impl IntoIterator<Item = Rect>) {
        for region in regions {
            if self.0.contains(&region) {
                continue;
            }
            if self.0.len() < 64 {
                self.0.push(region);
            } else {
                // Bound metadata during indefinite waits. A conservative union
                // can invalidate extra pixels but never discards a change.
                let mut union = region;
                for previous in self.0.drain(..) {
                    union.left = union.left.min(previous.left);
                    union.top = union.top.min(previous.top);
                    union.right = union.right.max(previous.right);
                    union.bottom = union.bottom.max(previous.bottom);
                }
                self.0.push(union);
            }
        }
    }
    pub fn as_slice(&self) -> &[Rect] {
        &self.0
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn take(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.0)
    }
}
