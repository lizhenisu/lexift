//! Bounded detection windows and spatial reconciliation in original image pixels.
use super::inference::{cancelled, error};
use lexift_core::Result;
use std::sync::atomic::AtomicBool;

pub(super) type Bounds = (u32, u32, u32, u32);
const HEIGHT: u32 = 960;
const OVERLAP: u32 = 192;

pub(super) fn windows(height: u32) -> Vec<(u32, u32)> {
    let mut result = Vec::new();
    let mut top: u32 = 0;
    loop {
        let bottom = top.saturating_add(HEIGHT).min(height);
        result.push((top, bottom));
        if bottom == height {
            break;
        }
        top = bottom - OVERLAP;
    }
    result
}

fn intersection(a: Bounds, b: Bounds) -> u64 {
    u64::from(a.2.min(b.2).saturating_sub(a.0.max(b.0)))
        * u64::from(a.3.min(b.3).saturating_sub(a.1.max(b.1)))
}
fn area(b: Bounds) -> u64 {
    u64::from(b.2 - b.0) * u64::from(b.3 - b.1)
}
fn duplicate(a: Bounds, b: Bounds) -> bool {
    intersection(a, b) * 5 >= area(a).min(area(b)) * 4
        && (i64::from(a.1) + i64::from(a.3) - i64::from(b.1) - i64::from(b.3)).unsigned_abs()
            <= u64::from((a.3 - a.1).max(b.3 - b.1))
}
fn union(a: Bounds, b: Bounds) -> Bounds {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}
fn truncated(b: Bounds, top: u32, bottom: u32, height: u32) -> bool {
    (top > 0 && b.1 <= top + 2) || (bottom < height && b.3.saturating_add(2) >= bottom)
}
fn covered(part: Bounds, full: Bounds) -> bool {
    intersection(part, full) * 5 >= area(part) * 4
}

/// Detect one window at a time. Unresolved seam fragments fail the complete request,
/// so callers cannot mistake partial text for a successful recognition.
pub(super) fn detect(
    height: u32,
    cancel: &AtomicBool,
    mut detector: impl FnMut(u32, u32) -> Result<Vec<Bounds>>,
) -> Result<Vec<Bounds>> {
    let mut complete = Vec::new();
    let mut fragments = Vec::new();
    for (top, bottom) in windows(height) {
        cancelled(cancel)?;
        for b in detector(top, bottom)? {
            if truncated(b, top, bottom, height) {
                fragments.push(b);
            } else {
                complete.push(b);
            }
        }
    }
    let mut retries = Vec::new();
    for &fragment in &fragments {
        if complete.iter().any(|&b| covered(fragment, b)) {
            continue;
        }
        let center = fragment.1 + (fragment.3 - fragment.1) / 2;
        let top = center
            .saturating_sub(HEIGHT / 2)
            .min(height.saturating_sub(HEIGHT));
        if !retries.contains(&top) {
            retries.push(top);
        }
    }
    for top in retries {
        cancelled(cancel)?;
        let bottom = (top + HEIGHT).min(height);
        for b in detector(top, bottom)? {
            if !truncated(b, top, bottom, height) {
                complete.push(b);
            }
        }
    }
    cancelled(cancel)?;
    if fragments
        .iter()
        .any(|&f| !complete.iter().any(|&b| covered(f, b)))
    {
        return Err(error("unresolved text at detection seam"));
    }
    let mut merged: Vec<Bounds> = Vec::new();
    for mut b in complete {
        let mut i = 0;
        while i < merged.len() {
            if duplicate(b, merged[i]) {
                b = union(b, merged.remove(i));
                i = 0;
            } else {
                i += 1;
            }
        }
        merged.push(b);
    }
    merged.sort_by_key(|b| (b.1, b.0));
    // Keep reading order independent of tile order and recognition batch widths.
    let mut start = 0;
    while start < merged.len() {
        let baseline = merged[start];
        let mut end = start + 1;
        while end < merged.len() && merged[end].1 - baseline.1 <= (baseline.3 - baseline.1) / 2 {
            end += 1;
        }
        merged[start..end].sort_by_key(|b| b.0);
        start = end;
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_cover_tail_and_bound_memory() {
        assert_eq!(windows(1800), vec![(0, 960), (768, 1728), (1536, 1800)]);
        assert_eq!(windows(960), vec![(0, 960)]);
    }
    #[test]
    fn seams_merge_spatially_but_repeated_lines_remain() {
        let lines = [
            (10, 850, 200, 880),
            (10, 950, 200, 990),
            (10, 1730, 200, 1760),
        ];
        let result = detect(1800, &AtomicBool::new(false), |t, b| {
            Ok(lines
                .iter()
                .filter_map(|&(l, y, r, z)| (z > t && y < b).then_some((l, y.max(t), r, z.min(b))))
                .collect())
        })
        .unwrap();
        assert_eq!(result, lines);
    }
    #[test]
    fn unresolved_fragments_fail_and_cancellation_skips_detection() {
        assert!(
            detect(1800, &AtomicBool::new(false), |t, b| Ok(vec![(
                1, t, 20, b
            )]))
            .is_err()
        );
        assert!(detect(1800, &AtomicBool::new(true), |_, _| panic!("cancelled")).is_err());
    }
    #[test]
    fn ambiguous_edge_gets_one_joint_context_retry() {
        let mut calls = 0;
        let result = detect(1100, &AtomicBool::new(false), |t, b| {
            calls += 1;
            Ok(if t == 0 {
                vec![(10, 940, 80, b)]
            } else if t == 768 {
                vec![]
            } else {
                vec![(10, 940, 80, 980)]
            })
        })
        .unwrap();
        assert_eq!(result, vec![(10, 940, 80, 980)]);
        assert_eq!(calls, 3);
    }
}
