//! Session-owned ROI plans. Visibility and Z order remain freshly enumerated.
use super::compose::intersection;
use lexift_core::domain::geometry::Rect;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Plan {
    regions: Vec<Rect>,
    windows: HashMap<isize, (Rect, Vec<Rect>)>,
}
impl Plan {
    pub fn clear(&mut self) {
        self.regions.clear();
        self.windows.clear();
    }
    /// Request regions affect every window; visibility retires only its own plan.
    pub fn retain(&mut self, regions: &[Rect], visible: impl Fn(isize) -> bool) {
        if self.regions != regions {
            self.regions.clear();
            self.regions.extend_from_slice(regions);
            self.windows.clear();
        } else {
            self.windows.retain(|token, _| visible(*token));
        }
    }
    pub fn get(&mut self, token: isize, bounds: Rect) -> &[Rect] {
        let entry = self
            .windows
            .entry(token)
            .or_insert_with(|| (bounds, disjoint_crops(&self.regions, bounds)));
        if entry.0 != bounds {
            *entry = (bounds, disjoint_crops(&self.regions, bounds));
        }
        &entry.1
    }
}

/// Union by horizontal slabs. Each slab merges X intervals, then extends an
/// identical interval from the preceding slab. All coordinates stay integral;
/// gaps are never filled and translucent windows contribute once per pixel.
pub(super) fn disjoint_crops(regions: &[Rect], bounds: Rect) -> Vec<Rect> {
    let clipped: Vec<_> = regions
        .iter()
        .filter_map(|region| intersection(*region, bounds))
        .collect();
    let mut edges: Vec<_> = clipped.iter().flat_map(|r| [r.top, r.bottom]).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut result: Vec<Rect> = Vec::new();
    let mut previous = HashMap::<(i32, i32), usize>::new();
    let mut intervals = Vec::with_capacity(clipped.len());
    for slab in edges.windows(2) {
        let (top, bottom) = (slab[0], slab[1]);
        intervals.clear();
        intervals.extend(
            clipped
                .iter()
                .filter(|r| r.top <= top && r.bottom >= bottom)
                .map(|r| (r.left, r.right)),
        );
        intervals.sort_unstable();
        let mut merged: Vec<(i32, i32)> = Vec::new();
        for &(left, right) in &intervals {
            if let Some(last) = merged.last_mut().filter(|last| left <= last.1) {
                last.1 = last.1.max(right);
            } else {
                merged.push((left, right));
            }
        }
        let mut current = HashMap::with_capacity(merged.len());
        for (left, right) in merged {
            let index = if let Some(index) = previous.remove(&(left, right)) {
                result[index].bottom = bottom;
                index
            } else {
                result.push(Rect {
                    left,
                    top,
                    right,
                    bottom,
                });
                result.len() - 1
            };
            current.insert((left, right), index);
        }
        previous = current;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Exact rectangle union without reading empty gaps or compositing overlapping
    /// pieces of the same translucent window twice.
    fn reference_crops(regions: &[Rect], bounds: Rect) -> Vec<Rect> {
        let mut result: Vec<Rect> = Vec::new();
        for region in regions {
            let Some(region) = intersection(*region, bounds) else {
                continue;
            };
            let mut pending = vec![region];
            for &existing in &result {
                pending = pending
                    .into_iter()
                    .flat_map(|piece| {
                        let Some(overlap) = intersection(piece, existing) else {
                            return vec![piece];
                        };
                        [
                            Rect {
                                bottom: overlap.top,
                                ..piece
                            },
                            Rect {
                                top: overlap.bottom,
                                ..piece
                            },
                            Rect {
                                top: overlap.top,
                                bottom: overlap.bottom,
                                right: overlap.left,
                                ..piece
                            },
                            Rect {
                                top: overlap.top,
                                bottom: overlap.bottom,
                                left: overlap.right,
                                ..piece
                            },
                        ]
                        .into_iter()
                        .filter(|r| r.right > r.left && r.bottom > r.top)
                        .collect()
                    })
                    .collect();
            }
            result.extend(pending);
        }
        result
    }

    #[test]
    #[ignore = "isolated ROI planning benchmark; run Release"]
    fn repeated_roi_planning_cost() {
        if cfg!(debug_assertions) {
            panic!("run with --release");
        }
        use std::{hint::black_box, time::Instant};
        let bounds = Rect {
            left: -200,
            top: -100,
            right: 1200,
            bottom: 800,
        };
        for count in [2, 16, 64] {
            let regions: Vec<_> = (0..count)
                .map(|i| Rect {
                    left: -180 + i * 15,
                    top: -90 + i * 8,
                    right: 100 + i * 15,
                    bottom: 150 + i * 8,
                })
                .collect();
            let mut plan = Plan::default();
            plan.retain(&regions, |_| true);
            assert_eq!(plan.get(1, bounds), disjoint_crops(&regions, bounds));
            let batch = if count == 64 { 1 } else { 100 };
            let mut old = Vec::new();
            let mut cold = Vec::new();
            let mut reused = Vec::new();
            for _ in 0..240 {
                let start = Instant::now();
                for _ in 0..batch {
                    black_box(reference_crops(black_box(&regions), black_box(bounds)));
                }
                old.push(start.elapsed().as_secs_f64() * 1000. / batch as f64);
                let start = Instant::now();
                for _ in 0..batch {
                    black_box(disjoint_crops(black_box(&regions), black_box(bounds)));
                }
                cold.push(start.elapsed().as_secs_f64() * 1000. / batch as f64);
                let start = Instant::now();
                for _ in 0..batch {
                    plan.retain(black_box(&regions), |_| true);
                    black_box(plan.get(1, black_box(bounds)));
                }
                reused.push(start.elapsed().as_secs_f64() * 1000. / batch as f64);
            }
            old.sort_by(f64::total_cmp);
            reused.sort_by(f64::total_cmp);
            cold.sort_by(f64::total_cmp);
            println!(
                "roi_count={count} samples=240 legacy_p95_ms={:.6} cold_p95_ms={:.6} reused_p95_ms={:.6} scope=isolated_repeated_geometry_not_GDI_GPU_or_display",
                old[227], cold[227], reused[227]
            );
        }
    }

    #[test]
    fn slab_union_matches_reference_and_pixel_coverage_for_overlaps_and_gaps() {
        let bounds = Rect {
            left: -12,
            top: -12,
            right: 12,
            bottom: 12,
        };
        let mut seed = 12345u32;
        for count in 0..40 {
            let mut regions = Vec::new();
            for _ in 0..count {
                let mut next = || {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed % 33) as i32 - 16
                };
                let (x1, y1, x2, y2) = (next(), next(), next(), next());
                regions.push(Rect {
                    left: x1.min(x2),
                    top: y1.min(y2),
                    right: x1.max(x2),
                    bottom: y1.max(y2),
                });
            }
            let crops = disjoint_crops(&regions, bounds);
            let reference = reference_crops(&regions, bounds);
            for y in bounds.top..bounds.bottom {
                for x in bounds.left..bounds.right {
                    let contains =
                        |r: &&Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
                    let expected = usize::from(regions.iter().any(|r| contains(&r)));
                    assert_eq!(crops.iter().filter(contains).count(), expected);
                    assert_eq!(reference.iter().filter(contains).count(), expected);
                }
            }
            regions.reverse();
            assert_eq!(
                disjoint_crops(&regions, bounds),
                crops,
                "ordering changes must not churn the plan"
            );
            if let Some(duplicate) = regions.first().copied() {
                regions.push(duplicate);
                assert_eq!(disjoint_crops(&regions, bounds), crops);
            }
        }
    }

    #[test]
    fn slab_union_handles_extreme_coordinates_and_merges_touching_edges() {
        let bounds = Rect {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        };
        let regions = [Rect { right: 0, ..bounds }, Rect { left: 0, ..bounds }];
        assert_eq!(disjoint_crops(&regions, bounds), [bounds]);
        assert!(disjoint_crops(&[], bounds).is_empty());
        assert!(
            disjoint_crops(
                &regions,
                Rect {
                    right: i32::MIN,
                    ..bounds
                }
            )
            .is_empty()
        );
        let bands = [
            Rect {
                bottom: 0,
                ..bounds
            },
            Rect { top: 0, ..bounds },
        ];
        assert_eq!(disjoint_crops(&bands, bounds), [bounds]);
    }

    #[test]
    fn plans_reuse_exact_regions_and_retire_hidden_or_changed_windows() {
        let rect = |left, top, right, bottom| Rect {
            left,
            top,
            right,
            bottom,
        };
        let regions = [rect(-100, -50, 100, 100), rect(0, 0, 200, 200)];
        let first = rect(-150, -60, 150, 150);
        let second = rect(50, 40, 250, 240);
        let mut plan = Plan::default();
        plan.retain(&regions, |_| true);
        let expected = disjoint_crops(&regions, first);
        let pointer = plan.get(1, first).as_ptr();
        assert_eq!(plan.get(1, first), expected);
        assert_eq!(plan.get(2, second), disjoint_crops(&regions, second));
        for _ in 0..100 {
            plan.retain(&regions, |_| true);
            assert_eq!(plan.get(1, first).as_ptr(), pointer);
        }
        // Moving/resizing a window updates its plan without discarding others.
        let moved = rect(-200, -100, 100, 100);
        assert_eq!(plan.get(1, moved), disjoint_crops(&regions, moved));
        plan.retain(&regions, |token| token == 2);
        assert!(!plan.windows.contains_key(&1));
        let changed = [rect(-20, -10, 70, 80)];
        plan.retain(&changed, |_| true);
        assert!(plan.windows.is_empty());
        assert_eq!(plan.get(1, first), disjoint_crops(&changed, first));
        // Returning to earlier geometry/ROI uses that exact plan again.
        plan.retain(&regions, |_| true);
        assert_eq!(plan.get(1, first), expected);
        plan.clear();
        assert!(plan.regions.is_empty() && plan.windows.is_empty());
    }
}
