//! Test-only association of native commits and OS-reported presentation counts.
use super::super::screen_capture::diagnostics::Published;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Count {
    pub chain: u64,
    pub present: u32,
}
/// The native counter was read somewhere inside this CPU time interval.
#[derive(Clone, Copy)]
pub(super) struct ClockAnchor {
    pub before: Instant,
    pub after: Instant,
    pub qpc: i64,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "QueryPerformanceCounter"]
    fn query_counter(value: *mut i64) -> i32;
}

impl ClockAnchor {
    pub fn sample() -> windows::core::Result<Self> {
        let before = Instant::now();
        let mut qpc = 0;
        // Test-only ABI: LARGE_INTEGER is an eight-byte signed counter; BOOL is i32.
        if unsafe { query_counter(&mut qpc) } == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let after = Instant::now();
        Ok(Self { before, after, qpc })
    }
}

pub(super) struct Observation {
    pub frequency: i64,
    pub count: Count,
    pub at: Instant,
    pub qpc: i64,
    pub present_refresh: u32,
    pub sync_refresh: u32,
    pub clock: Option<ClockAnchor>,
}
/// A jumped counter acknowledges only the reported frame, not intermediate
/// commits that may have been dropped by composition.
pub(super) fn match_sources<'a>(
    observations: &'a [Observation],
    commits: &'a [super::performance::Presentation],
) -> Vec<(&'a Observation, &'a [Published])> {
    match_commits(observations, commits)
        .into_iter()
        .map(|(observation, commit)| (observation, commit.sources.as_slice()))
        .collect()
}

/// A repeated count cannot uniquely identify a native commit; do not guess its provenance.
pub(super) fn match_commits<'a>(
    observations: &'a [Observation],
    commits: &'a [super::performance::Presentation],
) -> Vec<(&'a Observation, &'a super::performance::Presentation)> {
    match_commits_where(observations, commits, |_, _| true)
}

/// Rejected timing snapshots must not consume a displayed count. A later
/// observation may supply a valid clock for that exact same native presentation.
pub(super) fn match_commits_where<'a>(
    observations: &'a [Observation],
    commits: &'a [super::performance::Presentation],
    mut eligible: impl FnMut(&Observation, &super::performance::Presentation) -> bool,
) -> Vec<(&'a Observation, &'a super::performance::Presentation)> {
    let mut index = HashMap::new();
    for commit in commits {
        for count in &commit.counts {
            index
                .entry(*count)
                .and_modify(|entry| *entry = None)
                .or_insert(Some(commit));
        }
    }
    let mut seen = HashSet::new();
    observations
        .iter()
        .filter_map(|observation| {
            if observation.qpc <= 0
                || observation.count.present == 0
                || seen.contains(&observation.count)
            {
                return None;
            }
            let commit = index.get(&observation.count).copied().flatten()?;
            if !eligible(observation, commit) {
                return None;
            }
            seen.insert(observation.count);
            Some((observation, commit))
        })
        .collect()
}

/// Actual DComposition Commit call window, before later window-placement and worker work.
#[derive(Clone, Copy)]
pub(super) struct NativeCommit {
    pub composition: Option<super::composition_timing::Clock>,
    pub composition_before_present: Option<super::composition_timing::Clock>,
    pub before: Instant,
    pub after: Instant,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct CommitSync {
    pub call_ms: f64,
    pub accepted_tail_ms: f64,
    pub sync_ms: (f64, f64),
}
impl CommitSync {
    /// Whole measured refresh periods between the call and OS sync. Bounds
    /// crossing an integer boundary are ambiguous, not rounded into a bucket.
    pub fn whole_refreshes(self, period_ms: f64) -> Option<u32> {
        if !period_ms.is_finite() || period_ms <= 0. {
            return None;
        }
        let lower = (self.sync_ms.0 / period_ms).floor();
        let upper = (self.sync_ms.1 / period_ms).floor();
        (lower == upper && lower >= 0. && upper <= u32::MAX as f64).then_some(lower as u32)
    }
}

/// Match the actual native call to the same displayed version. These bounds do
/// not reveal DWM's batch pickup deadline or physical display time.
pub(super) fn native_commit_sync(
    observation: &Observation,
    commit: NativeCommit,
    accepted: Instant,
) -> Option<CommitSync> {
    let (sync_before, sync_after) = presented_window(observation)?;
    let ms = |d: Duration| d.as_secs_f64() * 1000.;
    Some(CommitSync {
        call_ms: ms(commit.after.checked_duration_since(commit.before)?),
        accepted_tail_ms: ms(accepted.checked_duration_since(commit.after)?),
        sync_ms: (
            ms(sync_before.checked_duration_since(commit.after)?),
            ms(sync_after.checked_duration_since(commit.before)?),
        ),
    })
}

/// Bounds in the CPU clock for the matched OS sync timestamp, not observation or photon time.
pub(super) fn presented_window(observation: &Observation) -> Option<(Instant, Instant)> {
    let anchor = observation.clock?;
    if observation.frequency <= 0
        || observation.qpc <= 0
        || anchor.qpc < observation.qpc
        || anchor.after < anchor.before
        || observation.present_refresh != observation.sync_refresh
    {
        return None;
    }
    let ticks = anchor.qpc.checked_sub(observation.qpc)?;
    let offset = Duration::try_from_secs_f64(ticks as f64 / observation.frequency as f64).ok()?;
    let before = anchor.before.checked_sub(offset)?;
    let after = anchor.after.checked_sub(offset)?.min(observation.at);
    (before <= after).then_some((before, after))
}

/// WGC reports compositor QPC time in 100ns units; DXGI reports raw QPC ticks.
/// Subtract in integer space before conversion so long system uptimes do not
/// erase short intervals. This is reported compositor time, not source paint time.
pub(super) fn capture_to_sync_ms(observation: &Observation, capture_ticks: i64) -> Option<f64> {
    if observation.frequency <= 0
        || observation.qpc <= 0
        || capture_ticks <= 0
        || observation.present_refresh != observation.sync_refresh
    {
        return None;
    }
    let delta = i128::from(observation.qpc) * 10_000_000
        - i128::from(capture_ticks) * i128::from(observation.frequency);
    (delta >= 0).then(|| delta as f64 / observation.frequency as f64 / 10_000.)
}

pub(super) fn latency_bounds(observation: &Observation, start: Instant) -> Option<(f64, f64)> {
    let (before, after) = presented_window(observation)?;
    Some((
        before.checked_duration_since(start)?.as_secs_f64() * 1000.,
        after.checked_duration_since(start)?.as_secs_f64() * 1000.,
    ))
}

#[test]
fn native_commit_bounds_exclude_accepted_tail_and_preserve_refresh_ambiguity() {
    let base = Instant::now();
    let mut observation = Observation {
        frequency: 1000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at: base + Duration::from_millis(50),
        qpc: 1020,
        present_refresh: 1,
        sync_refresh: 1,
        clock: Some(ClockAnchor {
            before: base + Duration::from_millis(40),
            after: base + Duration::from_millis(41),
            qpc: 1040,
        }),
    };
    let commit = NativeCommit {
        composition: None,
        composition_before_present: None,
        before: base + Duration::from_millis(5),
        after: base + Duration::from_millis(6),
    };
    let accepted = base + Duration::from_millis(8);
    let sample = native_commit_sync(&observation, commit, accepted).unwrap();
    assert_eq!(sample.call_ms, 1.);
    assert_eq!(sample.accepted_tail_ms, 2.);
    assert_eq!(sample.sync_ms, (14., 16.));
    assert_eq!(sample.whole_refreshes(7.), Some(2));
    assert_eq!(sample.whole_refreshes(5.), None);
    for invalid in [0., -1., f64::NAN, f64::INFINITY] {
        assert_eq!(sample.whole_refreshes(invalid), None);
    }
    assert!(native_commit_sync(&observation, commit, commit.before).is_none());
    assert!(
        native_commit_sync(
            &observation,
            NativeCommit {
                composition: None,
                composition_before_present: None,
                before: commit.after,
                after: commit.before,
            },
            accepted
        )
        .is_none()
    );
    assert!(
        native_commit_sync(
            &observation,
            NativeCommit {
                composition: None,
                composition_before_present: None,
                before: base + Duration::from_millis(19),
                after: base + Duration::from_millis(22),
            },
            base + Duration::from_millis(23)
        )
        .is_none()
    );
    observation.clock = None;
    assert!(native_commit_sync(&observation, commit, accepted).is_none());
}

#[test]
fn displayed_counter_does_not_acknowledge_skipped_or_other_chain_commits() {
    use super::performance::Presentation;
    let at = Instant::now();
    let count = |chain, present| Count { chain, present };
    let source = |sequence| Published {
        token: 7,
        sequence,
        ticks: sequence as i64,
    };
    let commits = [
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            at,
            sources: vec![source(1)],
            counts: vec![count(1, 1)],
        },
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            at,
            sources: vec![source(2)],
            counts: vec![count(1, 2)],
        },
        Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            at,
            sources: vec![source(3)],
            counts: vec![count(1, 3)],
        },
    ];
    let observation = |count| Observation {
        frequency: 10_000_000,
        count,
        at,
        qpc: 100,
        present_refresh: 1,
        sync_refresh: 1,
        clock: None,
    };
    let observations = [
        observation(count(1, 1)),
        observation(count(1, 3)),
        observation(count(1, 3)),
        observation(count(2, 2)),
    ];
    let matched = match_sources(&observations, &commits);
    assert_eq!(matched.len(), 2);
    assert_eq!(matched[0].1[0].sequence, 1);
    assert_eq!(matched[1].1[0].sequence, 3);
}

/// SyncQPCTime describes this presentation only when the refresh counters agree.
/// CPU observation timestamps cannot repair an ambiguous native sample.
pub(super) fn interval_ms(previous: &Observation, current: &Observation) -> Option<f64> {
    if previous.count.chain != current.count.chain
        || previous.frequency <= 0
        || previous.frequency != current.frequency
        || previous.qpc <= 0
        || current.qpc <= previous.qpc
        || previous.present_refresh != previous.sync_refresh
        || current.present_refresh != current.sync_refresh
    {
        return None;
    }
    Some(current.qpc.checked_sub(previous.qpc)? as f64 * 1000. / current.frequency as f64)
}

/// Infer cadence from matched OS counters, not the source's requested frame rate.
/// Counter resets/wraps are ambiguous and must not become enormous gap samples.
pub(super) fn refresh_interval(
    previous: &Observation,
    current: &Observation,
) -> Option<(u32, f64)> {
    let elapsed = interval_ms(previous, current)?;
    let cycles = current
        .present_refresh
        .checked_sub(previous.present_refresh)?;
    (cycles > 0).then_some((cycles, elapsed / f64::from(cycles)))
}

/// A reference model, not a hardware lower bound: periodic source frames are
/// shown at their first eligible refresh, with no capture/processing delay.
/// Actual source jitter, VRR and dropped versions are deliberately not modeled.
pub(super) fn ideal_periodic_intervals(
    source_period_ms: f64,
    refresh_period_ms: f64,
) -> Option<Vec<f64>> {
    if !source_period_ms.is_finite()
        || !refresh_period_ms.is_finite()
        || source_period_ms <= 0.
        || refresh_period_ms <= 0.
    {
        return None;
    }
    let mut intervals = Vec::new();
    let mut previous = 0.;
    for frame in 1..=4096 {
        let cycle = (frame as f64 * source_period_ms / refresh_period_ms - 1e-9).ceil();
        if cycle > previous {
            intervals.push((cycle - previous) * refresh_period_ms);
            previous = cycle;
        }
    }
    Some(intervals)
}

#[test]
fn periodic_reference_exposes_refresh_quantization_without_relaxing_targets() {
    let percentile = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[((values.len() as f64 * 0.95).ceil() as usize) - 1]
    };
    let samples = ideal_periodic_intervals(1000. / 60., 1000. / 144.).unwrap();
    assert!((percentile(samples.clone()) - 1000. / 48.).abs() < 1e-9);
    let over_20 = samples.iter().filter(|ms| **ms > 20.).count() as f64 / samples.len() as f64;
    assert!((over_20 - 0.4).abs() < 0.001);
    assert!(
        (percentile(ideal_periodic_intervals(1000. / 60., 1000. / 120.).unwrap()) - 1000. / 60.)
            .abs()
            < 1e-9
    );
    // Faster sources coalesce into refreshes; no zero-duration displayed frames.
    let faster = ideal_periodic_intervals(1000. / 240., 1000. / 60.).unwrap();
    assert!(faster.iter().all(|ms| (*ms - 1000. / 60.).abs() < 1e-9));
    for invalid in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(ideal_periodic_intervals(invalid, 7.).is_none());
        assert!(ideal_periodic_intervals(17., invalid).is_none());
    }
}

#[test]
fn refresh_cadence_distinguishes_display_cycles_and_rejects_counter_resets() {
    let sample = |qpc, refresh| Observation {
        frequency: 1_000_000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at: Instant::now(),
        qpc,
        present_refresh: refresh,
        sync_refresh: refresh,
        clock: None,
    };
    let previous = sample(1_000_000, 100);
    assert_eq!(
        refresh_interval(&previous, &sample(1_028_000, 104)),
        Some((4, 7.))
    );
    assert_eq!(refresh_interval(&previous, &sample(1_028_000, 100)), None);
    assert_eq!(refresh_interval(&previous, &sample(1_028_000, 99)), None);
    assert_eq!(
        refresh_interval(&sample(1_000_000, u32::MAX), &sample(1_028_000, 2)),
        None
    );
    let mut ambiguous = sample(1_028_000, 104);
    ambiguous.sync_refresh = 103;
    assert_eq!(refresh_interval(&previous, &ambiguous), None);
}
#[test]
fn system_intervals_use_qpc_and_reject_incompatible_samples() {
    let at = Instant::now();
    let previous = Observation {
        frequency: 10_000_000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at,
        qpc: 10_000_000,
        present_refresh: 1,
        sync_refresh: 1,
        clock: None,
    };
    let mut current = Observation {
        frequency: 10_000_000,
        count: Count {
            chain: 1,
            present: 2,
        },
        at: at + std::time::Duration::from_millis(90),
        qpc: 10_200_000,
        present_refresh: 2,
        sync_refresh: 2,
        clock: None,
    };
    assert_eq!(interval_ms(&previous, &current), Some(20.));
    current.sync_refresh = 3;
    assert_eq!(interval_ms(&previous, &current), None);
    current.sync_refresh = 2;
    current.frequency = 1000;
    assert_eq!(interval_ms(&previous, &current), None);
    current.frequency = 10_000_000;
    current.count.chain = 2;
    assert_eq!(interval_ms(&previous, &current), None);
    current.count.chain = 1;
    current.qpc = previous.qpc;
    assert_eq!(interval_ms(&previous, &current), None);
}

#[test]
fn sync_projection_is_bounded_and_rejects_invalid_calibration() {
    let at = Instant::now();
    let mut sample = Observation {
        frequency: 1000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at: at + Duration::from_millis(25),
        qpc: 995,
        present_refresh: 10,
        sync_refresh: 10,
        clock: Some(ClockAnchor {
            before: at + Duration::from_millis(20),
            after: at + Duration::from_millis(21),
            qpc: 1000,
        }),
    };
    assert_eq!(
        latency_bounds(&sample, at + Duration::from_millis(7)),
        Some((8., 9.))
    );
    assert!(latency_bounds(&sample, at + Duration::from_millis(17)).is_none());
    sample.at = at + Duration::from_millis(15);
    assert_eq!(
        latency_bounds(&sample, at + Duration::from_millis(7)),
        Some((8., 8.))
    );
    sample.at = at + Duration::from_millis(14);
    assert!(presented_window(&sample).is_none());
    sample.at = at + Duration::from_millis(25);
    sample.qpc = 1001;
    assert!(presented_window(&sample).is_none());
    sample.qpc = 995;
    sample.frequency = 0;
    assert!(presented_window(&sample).is_none());
    sample.frequency = 1000;
    sample.sync_refresh = 11;
    assert!(presented_window(&sample).is_none());
    sample.sync_refresh = 10;
    sample.clock.as_mut().unwrap().after = at + Duration::from_millis(19);
    assert!(presented_window(&sample).is_none());
    sample.clock = None;
    assert!(presented_window(&sample).is_none());
}

#[test]
fn reused_native_count_has_no_unique_commit_provenance() {
    let at = Instant::now();
    let count = Count {
        chain: 1,
        present: 1,
    };
    let source = |sequence| Published {
        token: 7,
        sequence,
        ticks: sequence as i64,
    };
    let commits = [
        super::performance::Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            at,
            counts: vec![count],
            sources: vec![source(1)],
        },
        super::performance::Presentation {
            stamp: lexift_core::ports::annotation_render::RenderStamp {
                session: 1,
                revision: 1,
            },
            native_commit: None,
            at,
            counts: vec![count],
            sources: vec![source(2)],
        },
    ];
    let observations = [Observation {
        frequency: 1000,
        count,
        at,
        qpc: 1,
        present_refresh: 1,
        sync_refresh: 1,
        clock: None,
    }];
    assert!(match_sources(&observations, &commits).is_empty());
    assert!(match_commits(&observations, &commits).is_empty());
}

#[test]
fn capture_clock_conversion_preserves_short_intervals_at_long_uptimes() {
    let mut observation = Observation {
        frequency: 3_000_000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at: Instant::now(),
        qpc: 3_000_000_000_042_000,
        present_refresh: 1,
        sync_refresh: 1,
        clock: None,
    };
    assert_eq!(
        capture_to_sync_ms(&observation, 10_000_000_000_000_000),
        Some(14.)
    );
    assert!(capture_to_sync_ms(&observation, 10_000_000_000_140_001).is_none());
    assert!(capture_to_sync_ms(&observation, 0).is_none());
    observation.frequency = 0;
    assert!(capture_to_sync_ms(&observation, 1).is_none());
    observation.frequency = 3_000_000;
    observation.sync_refresh = 2;
    assert!(capture_to_sync_ms(&observation, 1).is_none());
}

/// Four ordered endpoints for one pixel-identified paint: publication completed,
/// dequeue, accepted commit, and the upper CPU-clock bound of its OS sync.
#[derive(Clone, Copy)]
pub(super) struct CadenceFrame {
    pub count: Count,
    pub paint: u32,
    pub endpoints: [Instant; 4],
    pub sync_refresh: u32,
}
pub(super) struct CadencePair {
    pub paint: u32,
    pub refresh_cycles: u32,
    pub intervals_ms: [f64; 4],
    pub growth_ms: [f64; 3],
    pub skipped_paints: u32,
}
/// Separate displayed source identifiers before attributing an expanded interval.
/// A publication already beyond the refresh budget is not proof of a pipeline wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CadenceSource {
    ContinuousWithinBudget,
    ContinuousOverBudget,
    SkippedIdentifiers,
}
impl CadencePair {
    pub fn source_group(&self, publication_budget_ms: f64) -> Option<CadenceSource> {
        if !publication_budget_ms.is_finite() || publication_budget_ms <= 0. {
            return None;
        }
        Some(if self.skipped_paints != 0 {
            CadenceSource::SkippedIdentifiers
        } else if self.intervals_ms[0] > publication_budget_ms {
            CadenceSource::ContinuousOverBudget
        } else {
            CadenceSource::ContinuousWithinBudget
        })
    }
    pub fn largest_growth(&self) -> Option<usize> {
        self.growth_ms
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .filter(|(_, growth)| **growth > 0.)
            .map(|(index, _)| index)
    }
}

/// Compare the same two versions across stages. Signed interval growth identifies
/// where cadence widened; it is not GPU active time or an added percentile.
pub(super) fn cadence_pair(previous: CadenceFrame, current: CadenceFrame) -> Option<CadencePair> {
    if previous.count.chain != current.count.chain
        || current.count.present <= previous.count.present
        || previous.endpoints.windows(2).any(|pair| pair[1] < pair[0])
        || current.endpoints.windows(2).any(|pair| pair[1] < pair[0])
    {
        return None;
    }
    let refresh_cycles = current.sync_refresh.checked_sub(previous.sync_refresh)?;
    if refresh_cycles == 0 {
        return None;
    }
    let skipped_paints = current.paint.checked_sub(previous.paint)?.checked_sub(1)?;
    let mut intervals_ms = [0.; 4];
    for (index, interval) in intervals_ms.iter_mut().enumerate() {
        *interval = current.endpoints[index]
            .checked_duration_since(previous.endpoints[index])?
            .as_secs_f64()
            * 1000.;
        if *interval == 0. {
            return None;
        }
    }
    Some(CadencePair {
        paint: current.paint,
        refresh_cycles,
        growth_ms: std::array::from_fn(|index| intervals_ms[index + 1] - intervals_ms[index]),
        intervals_ms,
        skipped_paints,
    })
}

#[test]
fn cadence_source_groups_separate_publication_budget_and_skipped_identifiers() {
    let base = Instant::now();
    let frame = |paint, present, milliseconds: [u64; 4]| CadenceFrame {
        count: Count { chain: 1, present },
        sync_refresh: present * 4,
        paint,
        endpoints: milliseconds.map(|ms| base + Duration::from_millis(ms)),
    };
    let previous = frame(1, 1, [0, 3, 5, 10]);
    let continuous = cadence_pair(previous, frame(2, 2, [16, 20, 22, 38])).unwrap();
    assert_eq!(
        continuous.source_group(20.),
        Some(CadenceSource::ContinuousWithinBudget)
    );
    assert_eq!(
        continuous.source_group(16.),
        Some(CadenceSource::ContinuousWithinBudget)
    );
    assert_eq!(
        continuous.source_group(15.),
        Some(CadenceSource::ContinuousOverBudget)
    );
    assert_eq!(continuous.largest_growth(), Some(2));
    let late = cadence_pair(previous, frame(2, 2, [24, 27, 29, 38])).unwrap();
    assert_eq!(
        late.source_group(20.),
        Some(CadenceSource::ContinuousOverBudget)
    );
    let skipped = cadence_pair(previous, frame(3, 2, [32, 35, 37, 38])).unwrap();
    assert_eq!(
        skipped.source_group(20.),
        Some(CadenceSource::SkippedIdentifiers)
    );
    assert_eq!(skipped.largest_growth(), None);
    for invalid in [0., -1., f64::NAN, f64::INFINITY] {
        assert_eq!(continuous.source_group(invalid), None);
        assert_eq!(skipped.source_group(invalid), None);
    }
}

#[test]
fn same_version_cadence_identifies_widening_and_rejects_reordering() {
    let base = Instant::now();
    let frame = |paint, present, milliseconds: [u64; 4]| CadenceFrame {
        count: Count { chain: 1, present },
        sync_refresh: present * 4,
        paint,
        endpoints: milliseconds.map(|ms| base + Duration::from_millis(ms)),
    };
    let previous = frame(1, 1, [0, 3, 5, 10]);
    let current = frame(2, 2, [16, 20, 22, 38]);
    let pair = cadence_pair(previous, current).unwrap();
    assert_eq!(pair.intervals_ms, [16., 17., 17., 28.]);
    assert_eq!(pair.growth_ms, [1., 0., 11.]);
    assert_eq!(pair.skipped_paints, 0);
    assert_eq!(pair.refresh_cycles, 4);
    assert_eq!(
        pair.intervals_ms[0] + pair.growth_ms.iter().sum::<f64>(),
        pair.intervals_ms[3]
    );
    let early_gap = cadence_pair(previous, frame(2, 2, [16, 31, 33, 38])).unwrap();
    assert_eq!(early_gap.growth_ms, [12., 0., 0.]);
    let skipped = cadence_pair(previous, frame(4, 4, [48, 51, 53, 58])).unwrap();
    assert_eq!(skipped.skipped_paints, 2);
    assert!(cadence_pair(previous, frame(1, 2, [16, 20, 22, 38])).is_none());
    assert!(cadence_pair(previous, frame(2, 1, [16, 20, 22, 38])).is_none());
    assert!(cadence_pair(previous, frame(2, 2, [16, 22, 20, 38])).is_none());
    let mut other = current;
    other.sync_refresh = previous.sync_refresh;
    assert!(cadence_pair(previous, other).is_none());
    other.sync_refresh = previous.sync_refresh - 1;
    assert!(cadence_pair(previous, other).is_none());
    other.sync_refresh = previous.sync_refresh + 3;
    assert_eq!(cadence_pair(previous, other).unwrap().refresh_cycles, 3);
    other.count.chain = 2;
    assert!(cadence_pair(previous, other).is_none());
}
