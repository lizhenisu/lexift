//! Offline correlation of existing worker attempts with pixel-identified intervals.
//! Work is observed elapsed time, not CPU utilization; gaps have no assigned cause.
use super::super::scheduler::{WaitSample, Wake};
use super::super::{
    composition_timing::Sample,
    display_timing::{CadenceFrame, CadenceSource, cadence_pair},
};
use super::{Attempt, PermitPolls};
use lexift_core::ports::annotation_render::RenderWait;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Eq)]
enum Group {
    Short,
    PredictedLong,
    ExpandedLong,
    EarlierLong,
}
impl Group {
    fn name(&self) -> &'static str {
        match self {
            Self::Short => "continuous_short",
            Self::PredictedLong => "continuous_long_no_rounded_expansion",
            Self::ExpandedLong => "continuous_long_positive_rounded_expansion",
            Self::EarlierLong => "continuous_long_negative_rounded_expansion",
        }
    }
}
fn group(cycles: u32, expansion_ms: f64, period_ms: f64) -> Option<Group> {
    let expansion = expansion_ms / period_ms;
    if period_ms <= 0. || !period_ms.is_finite() || !expansion.is_finite() {
        return None;
    }
    Some(if cycles <= 3 {
        Group::Short
    } else if expansion.round() > 0. {
        Group::ExpandedLong
    } else if expansion.round() < 0. {
        Group::EarlierLong
    } else {
        Group::PredictedLong
    })
}

struct Work {
    elapsed: Duration,
    observed: Duration,
    attempts: usize,
    polls: PermitPolls,
    accepting: usize,
    started_stage_totals: [Duration; 5],
    waits: WaitCoverage,
}
#[derive(Default)]
struct WaitCoverage {
    covered: Duration,
    after_outcome: [Duration; 8],
    signals: usize,
    timers: usize,
    max_timer_overrun: Duration,
}
pub(super) const OUTCOME_NAMES: [&str; 8] = [
    "output_retry_source",
    "source_comparison",
    "other_source",
    "no_wait_hint",
    "presentation",
    "paused",
    "recovering",
    "no_preceding_attempt",
];

/// Label the last completed attempt, not the OS wake cause or current GPU state.
/// A capture arrival can change readiness while this wait is in progress.
pub(super) fn preceding_outcome(before: Instant, attempts: &[Attempt]) -> usize {
    let mut latest = None;
    let mut ambiguous = false;
    for attempt in attempts {
        let Some(end) = attempt
            .at
            .checked_add(attempt.work)
            .filter(|end| *end <= before)
        else {
            continue;
        };
        match latest {
            Some((previous, _)) if previous > end => {}
            Some((previous, _)) if previous == end => ambiguous = true,
            _ => {
                latest = Some((end, attempt));
                ambiguous = false;
            }
        }
    }
    let Some((_, attempt)) = latest.filter(|_| !ambiguous) else {
        return 7;
    };
    match attempt.wait {
        Some(RenderWait::Source) if attempt.stages.output_retry => 0,
        Some(RenderWait::Source) if attempt.stages.comparison_waiting => 1,
        Some(RenderWait::Source) => 2,
        None => 3,
        Some(RenderWait::Presentation) => 4,
        Some(RenderWait::Paused) => 5,
        Some(RenderWait::Recovering) => 6,
    }
}
/// These are actual kernel wait-call bounds. Reject overlaps with worker attempts;
/// residual time retains its unknown cause, including unrecorded or fallback waits.
fn wait_coverage(
    start: Instant,
    end: Instant,
    waits: &[WaitSample],
    attempts: &[Attempt],
) -> Option<WaitCoverage> {
    let mut coverage = WaitCoverage::default();
    let mut last_end = start;
    for wait in waits {
        if wait.after < wait.before {
            return None;
        }
        if wait.after <= start || wait.before >= end {
            continue;
        }
        let before = wait.before.max(start);
        let after = wait.after.min(end);
        if before < last_end
            || attempts.iter().any(|a| {
                a.at < after
                    && a.at
                        .checked_add(a.work)
                        .is_none_or(|finish| finish > before)
            })
        {
            return None;
        }
        last_end = after;
        let elapsed = after.checked_duration_since(before)?;
        coverage.covered += elapsed;
        // Use the actual wait entry, including waits clipped at interval start.
        coverage.after_outcome[preceding_outcome(wait.before, attempts)] += elapsed;
        match wait.wake {
            Wake::Signal => coverage.signals += 1,
            Wake::Timer => {
                coverage.timers += 1;
                if wait.before >= start && wait.after <= end {
                    coverage.max_timer_overrun = coverage.max_timer_overrun.max(
                        wait.after
                            .duration_since(wait.before)
                            .saturating_sub(wait.requested),
                    );
                }
            }
        }
    }
    Some(coverage)
}

/// Clip every observed attempt to the exact accepted-commit interval. Reject
/// overlapping traces rather than double count elapsed work or invent idle time.
fn work(start: Instant, end: Instant, attempts: &[Attempt], waits: &[WaitSample]) -> Option<Work> {
    let elapsed = end.checked_duration_since(start)?;
    if elapsed.is_zero() {
        return None;
    }
    let mut observed = Duration::ZERO;
    let mut last_end = start;
    let mut polls = PermitPolls::default();
    let mut counted = 0;
    let mut started_stage_totals = [Duration::ZERO; 5];
    let mut accepting = None;
    for (index, attempt) in attempts.iter().enumerate() {
        let finish = attempt.at.checked_add(attempt.work)?;
        if attempt.at <= end && finish >= end && accepting.replace(index).is_some() {
            return None;
        }
        if finish <= start || attempt.at >= end {
            continue;
        }
        let clipped_start = attempt.at.max(start);
        let clipped_end = finish.min(end);
        if clipped_start < last_end {
            return None;
        }
        observed += clipped_end.checked_duration_since(clipped_start)?;
        last_end = clipped_end;
        // Gates are reported by attempts begun within this interval; the previous
        // accepting attempt may finish here but its earlier polls are excluded.
        if attempt.at >= start {
            counted += 1;
            polls.observe(attempt.stages, attempt.wait.as_ref());
            for (total, elapsed) in started_stage_totals.iter_mut().zip([
                attempt.stages.capture,
                attempt.stages.effects,
                attempt.stages.compose,
                attempt.stages.compare,
                attempt.stages.commit,
            ]) {
                *total += elapsed;
            }
        }
    }
    let waits = wait_coverage(start, end, waits, attempts)?;
    elapsed.checked_sub(observed + waits.covered)?;
    Some(Work {
        elapsed,
        observed,
        attempts: counted,
        polls,
        accepting: accepting?,
        started_stage_totals,
        waits,
    })
}

pub(super) fn report(
    report: &mut String,
    frames: &[CadenceFrame],
    clocks: &HashMap<u32, Sample>,
    attempts: &[Attempt],
    waits: &[WaitSample],
    publication_budget_ms: f64,
    metric: &dyn Fn(Vec<f64>) -> String,
) {
    report.push_str("interval_worker_scope=existing_attempt_windows_between_same_pixel_adjacent_accepted_commits_not_exclusive_frame_work_cpu_utilization_wait_reason_or_photon\ninterval_worker_stage_totals_scope=whole_stages_of_attempts_started_in_interval_accepting_attempt_may_finish_after_commit_marker_not_clipped_elapsed_time\ninterval_wait_outcome_scope=clipped_kernel_wait_after_last_completed_attempt_outcome_not_wake_cause_gpu_busy_duration_or_frame_ownership\n");
    for wanted in [
        Group::Short,
        Group::PredictedLong,
        Group::ExpandedLong,
        Group::EarlierLong,
    ] {
        let mut accepted = Vec::new();
        let mut missing = 0;
        let mut invalid = 0;
        for pair in frames.windows(2) {
            let Some(cadence) = cadence_pair(pair[0], pair[1]) else {
                continue;
            };
            if cadence.source_group(publication_budget_ms)
                != Some(CadenceSource::ContinuousWithinBudget)
            {
                continue;
            }
            let (Some(previous), Some(current)) =
                (clocks.get(&pair[0].paint), clocks.get(&pair[1].paint))
            else {
                missing += 1;
                continue;
            };
            let Some((_, _, expansion)) = previous.adjacent_intervals_ms(*current) else {
                invalid += 1;
                continue;
            };
            let Some(actual) = group(cadence.refresh_cycles, expansion, current.period_ms) else {
                invalid += 1;
                continue;
            };
            if actual != wanted {
                continue;
            }
            let Some(work) = work(pair[0].endpoints[2], pair[1].endpoints[2], attempts, waits)
            else {
                invalid += 1;
                continue;
            };
            accepted.push((cadence, work));
        }
        let name = wanted.name();
        let ms = |d: Duration| d.as_secs_f64() * 1000.;
        let accepting: Vec<_> = accepted
            .iter()
            .map(|(_, w)| &attempts[w.accepting])
            .collect();
        report.push_str(&format!("interval_worker_{name}_samples={}\ninterval_worker_{name}_missing_clocks={missing}\ninterval_worker_{name}_invalid={invalid}\ninterval_worker_{name}_accepted_interval_p95_ms={}\ninterval_worker_{name}_observed_attempt_elapsed_p95_ms={}\ninterval_worker_{name}_unobserved_gap_p95_ms={}\ninterval_worker_{name}_attempt_count_p95={}\ninterval_worker_{name}_compose_blocked_polls={}\ninterval_worker_{name}_commit_blocked_polls={}\ninterval_worker_{name}_accepting_output_retry={}\n",
            accepted.len(), metric(accepted.iter().map(|(_,w)|ms(w.elapsed)).collect()),
            metric(accepted.iter().map(|(_,w)|ms(w.observed)).collect()),
            metric(accepted.iter().map(|(_,w)|ms(w.elapsed-w.observed)).collect()),
            metric(accepted.iter().map(|(_,w)|w.attempts as f64).collect()),
            accepted.iter().map(|(_,w)|w.polls.compose_blocked).sum::<usize>(),
            accepted.iter().map(|(_,w)|w.polls.commit_blocked).sum::<usize>(),
            accepting.iter().filter(|a|a.stages.output_retry).count()));
        report.push_str(&format!("interval_worker_{name}_recorded_kernel_wait_p95_ms={}\ninterval_worker_{name}_remaining_unassigned_p95_ms={}\ninterval_worker_{name}_signal_wakes={}\ninterval_worker_{name}_timer_wakes={}\ninterval_worker_{name}_max_full_timer_overrun_p95_ms={}\n",
            metric(accepted.iter().map(|(_,w)|ms(w.waits.covered)).collect()),
            metric(accepted.iter().map(|(_,w)|ms(w.elapsed-w.observed-w.waits.covered)).collect()),
            accepted.iter().map(|(_,w)|w.waits.signals).sum::<usize>(),
            accepted.iter().map(|(_,w)|w.waits.timers).sum::<usize>(),
            metric(accepted.iter().map(|(_,w)|ms(w.waits.max_timer_overrun)).collect())));
        for (index, label) in OUTCOME_NAMES.iter().enumerate() {
            report.push_str(&format!(
                "interval_worker_{name}_wait_after_{label}_p95_ms={}\n",
                metric(
                    accepted
                        .iter()
                        .map(|(_, w)| ms(w.waits.after_outcome[index]))
                        .collect()
                )
            ));
        }
        for (label, field) in [
            (
                "capture",
                (|a: &Attempt| a.stages.capture) as fn(&Attempt) -> Duration,
            ),
            (
                "effects",
                (|a: &Attempt| a.stages.effects) as fn(&Attempt) -> Duration,
            ),
            ("compose", |a: &Attempt| a.stages.compose),
            ("compare", |a: &Attempt| a.stages.compare),
            ("commit", |a: &Attempt| a.stages.commit),
        ] {
            report.push_str(&format!(
                "interval_worker_{name}_accepting_attempt_{label}_p95_ms={}\n",
                metric(accepting.iter().map(|a| ms(field(a))).collect())
            ));
        }
        for (index, label) in ["capture", "effects", "compose", "compare", "commit"]
            .iter()
            .enumerate()
        {
            report.push_str(&format!(
                "interval_worker_{name}_started_attempt_stage_total_{label}_p95_ms={}\n",
                metric(
                    accepted
                        .iter()
                        .map(|(_, w)| ms(w.started_stage_totals[index]))
                        .collect()
                )
            ));
        }
        for index in 0..3 {
            report.push_str(&format!(
                "interval_worker_{name}_cadence_growth_{index}_p95_ms={}\n",
                metric(accepted.iter().map(|(p, _)| p.growth_ms[index]).collect())
            ));
        }
    }
}

#[test]
fn interval_work_clips_boundaries_and_keeps_gate_scope() {
    use super::Stages;
    use lexift_core::ports::annotation_render::RenderStamp;
    let at = Instant::now();
    let attempt = |offset: u64, duration: u64, blocked: bool| Attempt {
        stamp: RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at + Duration::from_millis(offset),
        work: Duration::from_millis(duration),
        stages: Stages {
            compose_permit: Some(!blocked),
            ..Default::default()
        },
        wait: None,
    };
    let attempts = [
        attempt(0, 5, true),
        attempt(7, 1, true),
        attempt(9, 4, false),
    ];
    let sample = work(
        at + Duration::from_millis(3),
        at + Duration::from_millis(11),
        &attempts,
        &[],
    )
    .unwrap();
    assert_eq!(sample.elapsed, Duration::from_millis(8));
    assert_eq!(sample.observed, Duration::from_millis(5));
    assert_eq!(sample.attempts, 2);
    assert_eq!(sample.polls.compose_blocked, 1);
    assert_eq!(sample.accepting, 2);
    assert!(
        work(at, at + Duration::from_millis(15), &attempts, &[]).is_none(),
        "no accepting attempt"
    );
    let overlapping = [attempt(0, 5, false), attempt(4, 10, false)];
    assert!(work(at, at + Duration::from_millis(11), &overlapping, &[]).is_none());
    assert!(work(at, at, &attempts, &[]).is_none());
    assert!(work(at + Duration::from_millis(11), at, &attempts, &[]).is_none());
    let ambiguous = [attempt(0, 11, false), attempt(11, 2, false)];
    assert!(work(at, at + Duration::from_millis(11), &ambiguous, &[]).is_none());
}

#[test]
fn interval_groups_keep_signed_expansion_and_source_cadence_separate() {
    assert_eq!(group(3, 7., 7.), Some(Group::Short));
    assert_eq!(group(4, 0.1, 7.), Some(Group::PredictedLong));
    assert_eq!(group(4, 7., 7.), Some(Group::ExpandedLong));
    assert_eq!(group(4, -7., 7.), Some(Group::EarlierLong));
    assert_eq!(group(4, f64::NAN, 7.), None);
    assert_eq!(group(4, 1., 0.), None);
    assert_eq!(group(4, 1., f64::INFINITY), None);
}

#[test]
fn reporting_joins_exact_intervals_and_excludes_skipped_source_versions() {
    use super::super::display_timing::Count;
    use super::Stages;
    use lexift_core::ports::annotation_render::RenderStamp;
    let at = Instant::now();
    let frame = |paint, present, refresh, endpoints: [u64; 4]| CadenceFrame {
        paint,
        count: Count { chain: 1, present },
        sync_refresh: refresh,
        endpoints: endpoints.map(|ms| at + Duration::from_millis(ms)),
    };
    let frames = [
        frame(1, 1, 10, [0, 2, 8, 20]),
        frame(2, 2, 14, [17, 19, 26, 48]),
    ];
    let sample = |prediction_qpc, sync_qpc| Sample {
        prediction_qpc,
        sync_qpc,
        frequency: 1000,
        rate: (1000, 7),
        period_ms: 7.,
        query_ms: 0.,
        current_minus_last_ms: 0.,
        estimated_lead_ms: (0., 0.),
        sync_minus_estimate_ms: 0.,
    };
    let clocks = HashMap::from([(1, sample(100, 110)), (2, sample(121, 138))]);
    let attempts = [Attempt {
        stamp: RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at + Duration::from_millis(16),
        work: Duration::from_millis(13),
        stages: Stages {
            output_retry: true,
            commit_permit: Some(true),
            ..Default::default()
        },
        wait: None,
    }];
    let metric = |values: Vec<f64>| format!("{}", values.len());
    let render = |frames: &[CadenceFrame], clocks: &HashMap<u32, Sample>| {
        let mut output = String::new();
        report(&mut output, frames, clocks, &attempts, &[], 21., &metric);
        output
    };
    let output = render(&frames, &clocks);
    assert!(
        output.contains("interval_worker_continuous_long_positive_rounded_expansion_samples=1\n")
    );
    assert!(output.contains(
        "interval_worker_continuous_long_positive_rounded_expansion_accepting_output_retry=1\n"
    ));
    assert!(output.contains("interval_worker_continuous_long_no_rounded_expansion_samples=0\n"));
    let mut skipped = frames;
    skipped[1].paint = 3;
    assert!(!render(&skipped, &clocks).contains("_samples=1\n"));
    assert!(render(&frames, &HashMap::new()).contains("_missing_clocks=1\n"));
    let mut other_chain = frames;
    other_chain[1].count.chain = 2;
    assert!(!render(&other_chain, &clocks).contains("_samples=1\n"));
}

#[test]
fn native_wait_coverage_clips_bounds_and_rejects_overlap_with_active_work() {
    let at = Instant::now();
    let wait = |before, after, requested, wake| WaitSample {
        before: at + Duration::from_millis(before),
        after: at + Duration::from_millis(after),
        requested: Duration::from_millis(requested),
        wake,
    };
    let waits = [
        wait(0, 4, 10, Wake::Signal),
        wait(5, 10, 3, Wake::Timer),
        wait(12, 18, 6, Wake::Timer),
    ];
    let c = wait_coverage(
        at + Duration::from_millis(2),
        at + Duration::from_millis(15),
        &waits,
        &[],
    )
    .unwrap();
    assert_eq!(c.covered, Duration::from_millis(10));
    assert_eq!(c.after_outcome[7], c.covered);
    assert_eq!((c.signals, c.timers), (1, 2));
    assert_eq!(c.max_timer_overrun, Duration::from_millis(2));
    let reversed = [wait(4, 0, 3, Wake::Timer)];
    assert!(wait_coverage(at, at + Duration::from_millis(15), &reversed, &[]).is_none());
    let overlaps = [wait(0, 4, 4, Wake::Timer), wait(3, 6, 3, Wake::Timer)];
    assert!(wait_coverage(at, at + Duration::from_millis(15), &overlaps, &[]).is_none());
    let attempts = [Attempt {
        stamp: super::RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at + Duration::from_millis(3),
        work: Duration::from_millis(2),
        stages: Default::default(),
        wait: None,
    }];
    assert!(wait_coverage(at, at + Duration::from_millis(15), &waits, &attempts).is_none());
}

#[test]
fn wait_outcomes_follow_completed_attempts_and_partition_clipped_coverage() {
    use super::Stages;
    let at = Instant::now();
    let ms = Duration::from_millis;
    let attempt = |start, output_retry, comparison_waiting, wait| Attempt {
        stamp: super::RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at + ms(start),
        work: ms(1),
        stages: Stages {
            output_retry,
            comparison_waiting,
            ..Default::default()
        },
        wait,
    };
    let attempts = [
        attempt(0, true, true, Some(RenderWait::Source)),
        attempt(4, false, true, Some(RenderWait::Source)),
        attempt(8, false, false, Some(RenderWait::Source)),
        attempt(12, true, true, None),
    ];
    assert_eq!(
        preceding_outcome(at, &attempts),
        7,
        "unfinished attempt is not preceding"
    );
    assert_eq!(preceding_outcome(at + ms(1), &attempts), 0);
    assert_eq!(preceding_outcome(at + ms(5), &attempts), 1);
    assert_eq!(preceding_outcome(at + ms(9), &attempts), 2);
    assert_eq!(preceding_outcome(at + ms(13), &attempts), 3);
    let waits: Vec<_> = [1, 5, 9, 13]
        .into_iter()
        .map(|start| WaitSample {
            before: at + ms(start),
            after: at + ms(start + 3),
            requested: ms(3),
            wake: Wake::Timer,
        })
        .collect();
    let coverage = wait_coverage(at + ms(2), at + ms(15), &waits, &attempts).unwrap();
    assert_eq!(
        coverage.after_outcome,
        [ms(2), ms(3), ms(3), ms(2), ms(0), ms(0), ms(0), ms(0)]
    );
    assert_eq!(
        coverage.after_outcome.iter().sum::<Duration>(),
        coverage.covered
    );
    let ambiguous = [
        attempt(0, true, false, Some(RenderWait::Source)),
        attempt(0, false, true, Some(RenderWait::Source)),
    ];
    assert_eq!(preceding_outcome(at + ms(1), &ambiguous), 7);
    // A non-source outcome must not inherit stale source-comparison flags.
    for (hint, expected) in [
        (RenderWait::Presentation, 4),
        (RenderWait::Paused, 5),
        (RenderWait::Recovering, 6),
    ] {
        let attempts = [attempt(0, true, true, Some(hint))];
        assert_eq!(preceding_outcome(at + ms(1), &attempts), expected);
        let c = wait_coverage(at, at + ms(4), &waits[..1], &attempts).unwrap();
        assert_eq!(c.after_outcome[expected], ms(3));
        assert_eq!(c.after_outcome.iter().sum::<Duration>(), c.covered);
    }
}
