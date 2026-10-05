//! Distinguish observed consecutive presents from counter gaps of unknown display history.
use super::super::display_timing::{
    CadenceFrame, CadencePair, CadenceSource, Observation, cadence_pair, interval_ms,
    refresh_interval,
};

/// Keep original adjacency: filtering frames first would bridge unknown histories.
pub(super) fn consecutive_pairs(frames: &[CadenceFrame]) -> Vec<CadencePair> {
    frames
        .windows(2)
        .filter_map(|pair| {
            (pair[0].count.chain == pair[1].count.chain
                && pair[1].count.present.checked_sub(pair[0].count.present) == Some(1))
            .then(|| cadence_pair(pair[0], pair[1]))
            .flatten()
        })
        .collect()
}

pub(super) fn report_cadence(
    report: &mut String,
    frames: &[CadenceFrame],
    publication_budget_ms: f64,
    metric: &dyn Fn(Vec<f64>) -> String,
) {
    let pairs = consecutive_pairs(frames);
    report.push_str(&format!("consecutive_cadence_scope=same_pixel_adjacent_native_present_counts_stage_interval_growth_not_stage_duration_or_photon\nconsecutive_cadence_pairs={}\n", pairs.len()));
    for (name, long) in [("short", false), ("long", true)] {
        let selected: Vec<_> = pairs
            .iter()
            .filter(|pair| {
                (pair.refresh_cycles > 3) == long
                    && pair.source_group(publication_budget_ms)
                        == Some(CadenceSource::ContinuousWithinBudget)
            })
            .collect();
        report.push_str(&format!(
            "consecutive_cadence_{name}_continuous_within_budget_pairs={}\n",
            selected.len()
        ));
        let mut largest = [0usize; 3];
        for pair in &selected {
            if let Some(index) = pair.largest_growth() {
                largest[index] += 1;
            }
        }
        for (index, stage) in ["publication", "dequeue", "commit", "sync"]
            .iter()
            .enumerate()
        {
            report.push_str(&format!(
                "consecutive_cadence_{name}_{stage}_interval_p95_ms={}\n",
                metric(
                    selected
                        .iter()
                        .map(|pair| pair.intervals_ms[index])
                        .collect()
                )
            ));
        }
        for (index, stage) in ["publish_to_dequeue", "dequeue_to_commit", "commit_to_sync"]
            .iter()
            .enumerate()
        {
            report.push_str(&format!("consecutive_cadence_{name}_{stage}_growth_p95_ms={}\nconsecutive_cadence_{name}_{stage}_largest_growth_pairs={}\n", metric(selected.iter().map(|pair| pair.growth_ms[index]).collect()), largest[index]));
        }
    }
}

#[test]
fn cadence_classification_does_not_bridge_removed_gaps_or_skipped_publications() {
    use super::super::display_timing::Count;
    let base = std::time::Instant::now();
    let frame = |paint, present, refresh, ms: [u64; 4]| CadenceFrame {
        paint,
        count: Count { chain: 1, present },
        sync_refresh: refresh,
        endpoints: ms.map(|ms| base + std::time::Duration::from_millis(ms)),
    };
    let frames = [
        frame(1, 1, 1, [0, 2, 4, 10]),
        frame(2, 2, 5, [16, 20, 24, 50]),
        frame(3, 4, 9, [32, 36, 40, 90]),
        frame(5, 5, 13, [64, 68, 72, 130]),
    ];
    let pairs = consecutive_pairs(&frames);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].paint, 2);
    assert_eq!(pairs[0].growth_ms, [2., 2., 20.]);
    assert_eq!(
        pairs[1].source_group(30.),
        Some(CadenceSource::SkippedIdentifiers)
    );
    let mut report = String::new();
    report_cadence(&mut report, &frames, 30., &|values| format!("{values:?}"));
    assert!(report.contains("consecutive_cadence_long_continuous_within_budget_pairs=1\n"));
    assert!(report.contains("consecutive_cadence_long_commit_to_sync_largest_growth_pairs=1\n"));
    assert!(report.contains("consecutive_cadence_short_continuous_within_budget_pairs=0\n"));
}

#[derive(Default)]
pub(super) struct Coverage {
    pub consecutive_ms: Vec<f64>,
    pub consecutive_cycles: Vec<u32>,
    pub gap_ms: Vec<f64>,
    pub skipped_counts: u64,
    pub invalid: usize,
}
impl Coverage {
    pub fn from_observations(observations: &[&Observation]) -> Self {
        let mut result = Self::default();
        for pair in observations.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let delta = b.count.present.wrapping_sub(a.count.present);
            // A chain replacement, backwards counter or half-range ambiguity
            // cannot establish how many submissions lie between observations.
            if a.count.chain != b.count.chain || delta == 0 || delta >= (1 << 31) {
                result.invalid += 1;
                continue;
            }
            let Some(ms) = interval_ms(a, b) else {
                result.invalid += 1;
                continue;
            };
            if delta == 1 {
                result.consecutive_ms.push(ms);
                if let Some((cycles, _)) = refresh_interval(a, b) {
                    result.consecutive_cycles.push(cycles);
                }
            } else {
                result.gap_ms.push(ms);
                result.skipped_counts += u64::from(delta - 1);
            }
        }
        result
    }
}

#[test]
fn counter_gaps_are_unknown_history_and_wrap_is_still_consecutive() {
    use super::super::display_timing::Count;
    let frame = |present, qpc| Observation {
        frequency: 1000,
        count: Count { chain: 1, present },
        at: std::time::Instant::now(),
        qpc,
        present_refresh: qpc as u32 / 10,
        sync_refresh: qpc as u32 / 10,
        clock: None,
    };
    let frames = [
        frame(u32::MAX - 1, 1000),
        frame(u32::MAX, 1020),
        frame(0, 1040),
        frame(3, 1080),
        frame(2, 1100),
    ];
    let refs: Vec<_> = frames.iter().collect();
    let result = Coverage::from_observations(&refs);
    assert_eq!(result.consecutive_ms, [20., 20.]);
    assert_eq!(result.consecutive_cycles, [2, 2]);
    assert_eq!(result.gap_ms, [40.]);
    assert_eq!(result.skipped_counts, 2);
    assert_eq!(result.invalid, 1);
    let mut replacement = frame(4, 1120);
    replacement.count.chain = 2;
    assert_eq!(
        Coverage::from_observations(&[&frames[3], &replacement]).invalid,
        1
    );
    let mut invalid_clock = frame(4, 1080);
    invalid_clock.frequency = 0;
    assert_eq!(
        Coverage::from_observations(&[&frames[3], &invalid_clock]).invalid,
        1
    );
}
