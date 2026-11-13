use super::{
    geometry::{Point, Rect},
    region_selection::RegionEditor,
    screenshot_presets::covers,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Saved,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Area {
    pub id: u64,
    pub bounds: Rect,
    pub status: Status,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub areas: Vec<Area>,
    pub selected: Option<u64>,
    pub request: u64,
    pub running: bool,
    pub sealed: bool,
    next_id: u64,
}
impl Batch {
    pub fn hit(&self, point: Point, scale: f32) -> Option<&Area> {
        self.areas.iter().rev().find(|area| {
            RegionEditor::new(area.bounds)
                .hit(point, scale, true)
                .is_some()
        })
    }
    pub fn put(&mut self, id: Option<u64>, bounds: Rect, layout: &[Rect]) -> bool {
        if self.running || self.sealed || !covers(bounds, layout) {
            return false;
        }
        if let Some(id) = id {
            let Some(area) = self.areas.iter_mut().find(|area| area.id == id) else {
                return false;
            };
            area.bounds = bounds;
            self.selected = Some(id);
        } else {
            self.next_id += 1;
            self.selected = Some(self.next_id);
            self.areas.push(Area {
                id: self.next_id,
                bounds,
                status: Status::Pending,
                error: None,
            });
        }
        true
    }
    pub fn delete(&mut self) {
        if !self.running
            && !self.sealed
            && let Some(id) = self.selected.take()
        {
            self.areas.retain(|area| area.id != id);
        }
    }
    pub fn begin(&mut self) -> Option<(u64, Vec<(usize, Area)>)> {
        if self.running {
            return None;
        }
        let jobs: Vec<_> = self
            .areas
            .iter()
            .enumerate()
            .filter(|(_, area)| area.status != Status::Saved)
            .map(|(index, area)| (index + 1, area.clone()))
            .collect();
        if jobs.is_empty() {
            return None;
        }
        self.request += 1;
        self.running = true;
        for area in &mut self.areas {
            if area.status == Status::Failed {
                area.status = Status::Pending;
                area.error = None;
            }
        }
        Some((self.request, jobs))
    }
    pub fn complete(&mut self, request: u64, id: u64, saved: bool) -> bool {
        if !self.running || self.request != request {
            return false;
        }
        let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.id == id && area.status == Status::Pending)
        else {
            return false;
        };
        area.status = if saved { Status::Saved } else { Status::Failed };
        self.sealed = true;
        true
    }
    pub fn finish(&mut self, request: u64) {
        if self.request == request {
            self.running = false;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Put(Option<u64>, Rect),
    Delete,
    Save(Option<crate::ports::magnifier::WindowToken>, String),
    Stop,
    DirectoryAccepted(u64),
    Item(u64, u64, Result<(), String>),
    Finished(u64, Option<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, width: i32) -> Rect {
        Rect {
            left: x,
            top: 0,
            right: x + width,
            bottom: 80,
        }
    }

    #[test]
    fn overlap_hits_latest_and_deletion_compacts_output_order() {
        let layout = [rect(-100, 100), rect(0, 100)];
        let mut batch = Batch::default();
        assert!(batch.put(None, rect(-40, 70), &layout));
        assert!(batch.put(None, rect(-20, 60), &layout));
        assert!(batch.put(None, rect(60, 20), &layout));
        let hit = batch.hit(Point { x: 5, y: 40 }, 1.0).unwrap();
        assert_eq!(hit.id, 2);
        assert!(batch.put(Some(hit.id), hit.bounds, &layout));
        batch.delete();
        let (_, jobs) = batch.begin().unwrap();
        assert_eq!(
            jobs.iter().map(|(n, a)| (*n, a.id)).collect::<Vec<_>>(),
            [(1, 1), (2, 3)]
        );
    }

    #[test]
    fn frozen_batch_preserves_physical_pixels_across_negative_monitor_origin() {
        use crate::{domain::screen_snapshot::crop_frames, ports::screen_capture::CapturedRegion};
        let layout = [rect(-4, 4), rect(0, 4)];
        let frames = layout.map(|bounds| CapturedRegion {
            bounds,
            pixels: (bounds.top..bounds.bottom)
                .flat_map(|y| {
                    (bounds.left..bounds.right).flat_map(move |x| [(x + 4) as u8, y as u8, 73, 255])
                })
                .collect(),
        });
        let mut batch = Batch::default();
        let across = Rect {
            left: -2,
            top: 5,
            right: 3,
            bottom: 8,
        };
        assert!(batch.put(None, across, &layout));
        let (_, jobs) = batch.begin().unwrap();
        let crop = crop_frames(&frames, jobs[0].1.bounds).unwrap();
        let expected: Vec<u8> = (5..8)
            .flat_map(|y| (2..7).flat_map(move |x| [x, y, 73, 255]))
            .collect();
        assert_eq!(crop.bounds, across);
        assert_eq!(crop.pixels, expected);
    }

    #[test]
    fn retry_ignores_published_items_and_rejects_stale_results() {
        let layout = [rect(-100, 90), rect(0, 100)];
        let mut batch = Batch::default();
        assert!(!batch.put(None, rect(-20, 40), &layout));
        assert!(batch.put(None, rect(-80, 30), &layout));
        assert!(batch.put(None, rect(10, 30), &layout));
        let (first, _) = batch.begin().unwrap();
        assert!(batch.complete(first, 1, true));
        assert!(!batch.complete(first, 1, true));
        assert!(batch.complete(first, 2, false));
        batch.finish(first);
        assert!(!batch.put(Some(2), rect(50, 20), &layout));
        let (retry, jobs) = batch.begin().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].0, jobs[0].1.id), (2, 2));
        assert!(!batch.complete(first, 2, true));
        assert!(batch.complete(retry, 2, true));
        batch.finish(retry);
        assert!(batch.begin().is_none());
    }
}
