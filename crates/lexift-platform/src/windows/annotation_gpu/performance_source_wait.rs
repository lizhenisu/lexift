//! Offline evidence of independent source progress. A bracket records CPU
//! observations of Pending and Confirm, not the GPU's actual completion time.
use super::Presentation;
use crate::windows::screen_capture::diagnostics::{Event, Kind};
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

struct Bracket {
    token: isize,
    pending: Instant,
    confirmed: Instant,
}

/// Require a later confirmation of the same candidate and a Pending observation
/// inside its own comparison lifetime. Retired/unconfirmed candidates cannot
/// establish an interval; warmup and incomplete tail evidence stay excluded.
fn brackets(events: &[Event], owned: isize, start: Instant) -> Vec<Bracket> {
    let mut pending_events: HashMap<(isize, u64), Vec<Instant>> = HashMap::new();
    for event in events
        .iter()
        .filter(|event| event.at >= start && matches!(event.kind, Kind::Pending))
    {
        pending_events
            .entry((event.token, event.sequence))
            .or_default()
            .push(event.at);
    }
    events
        .iter()
        .filter_map(|confirmation| {
            let Kind::Confirm {
                started,
                candidate: Some(candidate),
                ..
            } = confirmation.kind
            else {
                return None;
            };
            if candidate.token == owned
                || candidate.token != confirmation.token
                || candidate.sequence != confirmation.sequence
                || confirmation.at <= started
            {
                return None;
            }
            let pending = pending_events
                .get(&(candidate.token, candidate.sequence))?
                .iter()
                .copied()
                .filter(|at| *at >= started && *at < confirmation.at)
                .min()?;
            Some(Bracket {
                token: candidate.token,
                pending,
                confirmed: confirmation.at,
            })
        })
        .collect()
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Evidence {
    confirmed_extra_brackets: usize,
    owned_versions: usize,
    owned_versions_during_extra_pending: usize,
    extra_tokens_overlapping_commits: usize,
}

fn analyze(events: &[Event], commits: &[Presentation], owned: isize, start: Instant) -> Evidence {
    let brackets = brackets(events, owned, start);
    let mut result = Evidence {
        confirmed_extra_brackets: brackets.len(),
        ..Default::default()
    };
    let mut seen = HashSet::new();
    let mut tokens = HashSet::new();
    for commit in commits.iter().filter(|commit| commit.at >= start) {
        let mut sources = commit.sources.iter().filter(|source| source.token == owned);
        let Some(source) = sources.next() else {
            continue;
        };
        if sources.any(|other| other != source) || !seen.insert(*source) {
            continue;
        }
        result.owned_versions += 1;
        let mut overlapping = false;
        for bracket in &brackets {
            if commit.at > bracket.pending && commit.at < bracket.confirmed {
                overlapping = true;
                tokens.insert(bracket.token);
            }
        }
        result.owned_versions_during_extra_pending += usize::from(overlapping);
    }
    result.extra_tokens_overlapping_commits = tokens.len();
    result
}

pub(super) fn report(
    events: &[Event],
    commits: &[Presentation],
    owned: isize,
    start: Instant,
) -> String {
    let evidence = analyze(events, commits, owned, start);
    let mut report = format!(
        "source_wait_scope=accepted_owned_version_inside_other_candidate_cpu_pending_confirm_bracket_not_gpu_queue_or_os_sync\nsource_wait_confirmed_extra_brackets={}\nsource_wait_owned_versions={}\nsource_wait_owned_versions_during_extra_pending={}\nsource_wait_extra_tokens_overlapping_commits={}\n",
        evidence.confirmed_extra_brackets,
        evidence.owned_versions,
        evidence.owned_versions_during_extra_pending,
        evidence.extra_tokens_overlapping_commits
    );
    let gaps = observed_confirmation_gaps(events, owned, start);
    let mut values: Vec<_> = gaps.iter().map(|gap| gap.as_secs_f64() * 1000.).collect();
    values.sort_by(f64::total_cmp);
    let p95 = values
        .get((values.len() * 95).div_ceil(100).saturating_sub(1))
        .map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.6}"));
    report.push_str(&format!("source_confirmation_gap_scope=same_owned_candidate_gpu_query_cpu_observation_to_changed_confirmation_not_actual_gpu_completion_or_os_sync\nsource_confirmation_gap_samples={}\nsource_confirmation_gap_p95_ms={p95}\nsource_confirmation_gap_at_least_1ms={}\n", gaps.len(), values.iter().filter(|value| **value >= 1.).count()));
    let brackets = query_poll_brackets(events, owned, start);
    for (name, values) in [
        (
            "readiness_bracket",
            brackets.iter().map(|pair| pair.0).collect::<Vec<_>>(),
        ),
        (
            "between_calls",
            brackets.iter().map(|pair| pair.1).collect::<Vec<_>>(),
        ),
    ] {
        let mut values: Vec<_> = values
            .into_iter()
            .map(|v| v.as_secs_f64() * 1000.)
            .collect();
        values.sort_by(f64::total_cmp);
        let p95 = values
            .get((values.len() * 95).div_ceil(100).saturating_sub(1))
            .map_or_else(|| "unavailable".to_owned(), |v| format!("{v:.6}"));
        let max = values
            .last()
            .map_or_else(|| "unavailable".to_owned(), |v| format!("{v:.6}"));
        report.push_str(&format!("source_query_{name}_samples={}\nsource_query_{name}_p95_ms={p95}\nsource_query_{name}_max_ms={max}\nsource_query_{name}_at_least_1ms={}\n", values.len(), values.iter().filter(|v| **v >= 1.).count()));
    }
    report.push_str("source_query_poll_scope=last_not_ready_call_start_to_first_ready_call_end_and_intercall_gap_same_confirmed_candidate_not_absolute_gpu_completion\n");
    report
}

/// The bracket bounds CPU readiness observations; unavailable metadata does not
/// establish the hardware's absolute completion time. Never match by proximity.
fn query_poll_brackets(
    events: &[Event],
    owned: isize,
    start: Instant,
) -> Vec<(std::time::Duration, std::time::Duration)> {
    query_poll_intervals(events, owned, start)
        .into_iter()
        .map(|(readiness, (from, to))| (readiness, to.duration_since(from)))
        .collect()
}

fn query_poll_intervals(
    events: &[Event],
    owned: isize,
    start: Instant,
) -> Vec<(std::time::Duration, (Instant, Instant))> {
    let mut confirmations = HashMap::new();
    let mut polls = HashMap::<_, Vec<_>>::new();
    for event in events
        .iter()
        .filter(|event| event.token == owned && event.at >= start)
    {
        match event.kind {
            Kind::QueryPoll {
                candidate,
                began,
                finished,
                ready,
            } if candidate.token == owned && candidate.sequence == event.sequence => {
                polls
                    .entry(candidate)
                    .or_default()
                    .push((began, finished, event.at, ready));
            }
            Kind::Confirm {
                candidate: Some(candidate),
                started,
                changed: true,
                ..
            } if candidate.token == owned && candidate.sequence == event.sequence => {
                confirmations
                    .entry(candidate)
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some((started, event.at)));
            }
            _ => {}
        }
    }
    confirmations
        .into_iter()
        .filter_map(|(candidate, confirmation)| {
            let (started, confirmed) = confirmation?;
            let polls = polls.get(&candidate)?;
            if polls.iter().any(|(began, finished, at, _)| {
                *began < started || *finished < *began || *at < *finished || *at > confirmed
            }) || polls.windows(2).any(|pair| pair[0].1 > pair[1].0)
            {
                return None;
            }
            let ready = polls.iter().position(|poll| poll.3)?;
            let previous = polls.get(ready.checked_sub(1)?)?;
            Some((
                polls[ready].1.checked_duration_since(previous.0)?,
                (previous.1, polls[ready].0),
            ))
        })
        .collect()
}

/// Clip recorded work/wait spans to the exact inter-call gap. Reject overlap
/// rather than double-count; the remainder includes uninstrumented worker work.
fn partition_gap(
    from: Instant,
    to: Instant,
    work: &[(Instant, Instant)],
    waits: &[super::scheduler::WaitSample],
) -> Option<[std::time::Duration; 5]> {
    use std::time::Duration;
    let gap = to.checked_duration_since(from)?;
    let mut spans = Vec::new();
    let mut requested = Duration::ZERO;
    let mut overrun = Duration::ZERO;
    for (begin, end, category) in work
        .iter()
        .map(|&(begin, end)| (begin, end, 0))
        .chain(waits.iter().map(|wait| (wait.before, wait.after, 1)))
    {
        if end < begin {
            return None;
        }
        let (begin, end) = (begin.max(from), end.min(to));
        if begin < end {
            spans.push((begin, end, category));
        }
    }
    spans.sort_by_key(|span| span.0);
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return None;
    }
    let mut totals = [Duration::ZERO; 2];
    for (begin, end, category) in spans {
        totals[category] += end.duration_since(begin);
    }
    for wait in waits
        .iter()
        .filter(|wait| wait.before >= from && wait.after <= to)
    {
        if wait.wake == super::scheduler::Wake::Timer {
            requested += wait.requested;
            overrun += wait
                .after
                .duration_since(wait.before)
                .saturating_sub(wait.requested);
        }
    }
    Some([
        totals[0],
        totals[1],
        gap.checked_sub(totals[0] + totals[1])?,
        requested,
        overrun,
    ])
}

pub(super) fn query_gap_work_report(
    events: &[Event],
    owned: isize,
    start: Instant,
    attempts: &[super::Attempt],
    waits: &[super::scheduler::WaitSample],
) -> String {
    let work: Vec<_> = attempts
        .iter()
        .map(|attempt| (attempt.at, attempt.at + attempt.work))
        .collect();
    let intervals = query_poll_intervals(events, owned, start);
    let mut rows = Vec::new();
    let mut invalid = 0;
    for (_, (from, to)) in &intervals {
        if let Some(row) = partition_gap(*from, *to, &work, waits) {
            rows.push(row);
        } else {
            invalid += 1;
        }
    }
    let mut report = format!(
        "source_query_partition_scope=same_intercall_gap_clipped_worker_attempt_and_native_wait_spans_not_gpu_busy_or_photon\nsource_query_partition_brackets={}\nsource_query_partition_valid={}\nsource_query_partition_invalid={invalid}\n",
        intervals.len(),
        rows.len()
    );
    for (column, name) in [
        "attempt_work",
        "native_wait",
        "unassigned",
        "full_timer_requested",
        "full_timer_overrun",
    ]
    .iter()
    .enumerate()
    {
        let mut values: Vec<_> = rows
            .iter()
            .map(|row| row[column].as_secs_f64() * 1000.)
            .collect();
        values.sort_by(f64::total_cmp);
        let p95 = values
            .get((values.len() * 95).div_ceil(100).saturating_sub(1))
            .map_or_else(|| "unavailable".to_owned(), |v| format!("{v:.6}"));
        report.push_str(&format!("source_query_partition_{name}_p95_ms={p95}\n"));
    }
    report.push_str("source_query_partition_row_columns=attempt_work,native_wait,unassigned,full_timer_requested,full_timer_overrun_ms\n");
    for row in rows {
        report.push_str(&format!(
            "source_query_partition_row={:.6},{:.6},{:.6},{:.6},{:.6}\n",
            row[0].as_secs_f64() * 1000.,
            row[1].as_secs_f64() * 1000.,
            row[2].as_secs_f64() * 1000.,
            row[3].as_secs_f64() * 1000.,
            row[4].as_secs_f64() * 1000.
        ));
    }
    report
}

#[test]
fn query_gap_partition_clips_boundaries_and_rejects_overlapping_spans() {
    use std::time::Duration;
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let wait = super::scheduler::WaitSample {
        before: at(4),
        after: at(8),
        requested: Duration::from_millis(1),
        wake: super::scheduler::Wake::Timer,
    };
    let work = [(at(0), at(3)), (at(9), at(12))];
    assert_eq!(
        partition_gap(at(2), at(10), &work, &[wait]),
        Some([2, 4, 2, 1, 3].map(Duration::from_millis))
    );
    assert!(partition_gap(at(2), at(10), &[(at(3), at(5))], &[wait]).is_none());
    assert!(partition_gap(at(2), at(10), &work, &[wait, wait]).is_none());
    assert_eq!(
        partition_gap(at(5), at(7), &[], &[wait]),
        Some([0, 2, 0, 0, 0].map(Duration::from_millis))
    );
}

fn gap_origins(
    from: Instant,
    to: Instant,
    attempts: &[super::Attempt],
    waits: &[super::scheduler::WaitSample],
) -> Option<([std::time::Duration; 14], [usize; 8])> {
    use std::time::Duration;
    let work: Vec<_> = attempts
        .iter()
        .map(|attempt| (attempt.at, attempt.at + attempt.work))
        .collect();
    let partition = partition_gap(from, to, &work, waits)?;
    let clip = |begin: Instant, end: Instant| {
        end.min(to)
            .checked_duration_since(begin.max(from))
            .unwrap_or_default()
    };
    let mut values = [Duration::ZERO; 14];
    let mut counts = [0; 8];
    for wait in waits {
        let duration = clip(wait.before, wait.after);
        if !duration.is_zero() {
            let origin = super::interval_work::preceding_outcome(wait.before, attempts);
            values[origin] += duration;
            counts[origin] += 1;
        }
    }
    let mut stage_spans = Vec::new();
    for attempt in attempts {
        for (index, span) in attempt.stages.cpu_spans.iter().enumerate() {
            if let Some((begin, end)) = span {
                if *begin < attempt.at || *end < *begin || *end > attempt.at + attempt.work {
                    return None;
                }
                values[8 + index] += clip(*begin, *end);
                if !clip(*begin, *end).is_zero() {
                    stage_spans.push(((*begin).max(from), (*end).min(to)));
                }
            }
        }
    }
    stage_spans.sort_by_key(|span| span.0);
    if stage_spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return None;
    }
    values[13] = partition[0].checked_sub(values[8..13].iter().copied().sum())?;
    Some((values, counts))
}

pub(super) fn query_gap_origin_report(
    events: &[Event],
    owned: isize,
    start: Instant,
    attempts: &[super::Attempt],
    waits: &[super::scheduler::WaitSample],
) -> String {
    let intervals = query_poll_intervals(events, owned, start);
    let rows: Vec<_> = intervals
        .iter()
        .filter_map(|(_, (from, to))| gap_origins(*from, *to, attempts, waits))
        .collect();
    let mut names: Vec<_> = super::interval_work::OUTCOME_NAMES
        .iter()
        .map(|name| format!("wait_after_{name}"))
        .collect();
    names.extend(
        [
            "capture",
            "effects",
            "compose",
            "compare",
            "commit",
            "work_outside_stages",
        ]
        .map(str::to_owned),
    );
    let mut report = format!(
        "source_query_origin_scope=clipped_cpu_stage_spans_and_wait_after_last_completed_attempt_not_wake_cause_or_gpu_busy\nsource_query_origin_valid={}\nsource_query_origin_invalid={}\n",
        rows.len(),
        intervals.len() - rows.len()
    );
    for (index, name) in names.iter().enumerate() {
        let mut values: Vec<_> = rows
            .iter()
            .map(|row| row.0[index].as_secs_f64() * 1000.)
            .collect();
        values.sort_by(f64::total_cmp);
        let p95 = values
            .get((values.len() * 95).div_ceil(100).saturating_sub(1))
            .map_or_else(|| "unavailable".to_owned(), |v| format!("{v:.6}"));
        report.push_str(&format!("source_query_origin_{name}_p95_ms={p95}\n"));
        if index < 8 {
            report.push_str(&format!(
                "source_query_origin_{name}_calls={}\n",
                rows.iter().map(|row| row.1[index]).sum::<usize>()
            ));
        }
    }
    report.push_str(&format!(
        "source_query_origin_row_columns={}\n",
        names.join(",")
    ));
    for row in rows {
        report.push_str(&format!(
            "source_query_origin_row={}\n",
            row.0
                .iter()
                .map(|v| format!("{:.6}", v.as_secs_f64() * 1000.))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    report
}

/// Nested CPU intervals must belong to their capture stage; do not prorate
/// whole-attempt durations or count diagnostic paint readback as view creation.
fn capture_details(
    from: Instant,
    to: Instant,
    attempts: &[super::Attempt],
) -> Option<[std::time::Duration; 3]> {
    use std::time::Duration;
    let clip = |begin: Instant, end: Instant| {
        end.min(to)
            .checked_duration_since(begin.max(from))
            .unwrap_or_default()
    };
    let mut total = Duration::ZERO;
    let mut values = [Duration::ZERO; 3];
    for attempt in attempts
        .iter()
        .filter(|attempt| attempt.at < to && attempt.at + attempt.work > from)
    {
        if attempt.stages.capture_details_overflow {
            return None;
        }
        let Some((capture_start, capture_end)) = attempt.stages.cpu_spans[0] else {
            if attempt
                .stages
                .capture_details
                .iter()
                .flatten()
                .any(|&(begin, end)| !clip(begin, end).is_zero())
            {
                return None;
            }
            continue;
        };
        if capture_start < attempt.at
            || capture_end < capture_start
            || capture_end > attempt.at + attempt.work
        {
            return None;
        }
        total += clip(capture_start, capture_end);
        let mut spans = Vec::new();
        for (index, span) in attempt.stages.capture_details.iter().enumerate() {
            if let Some((begin, end)) = span {
                if *begin < capture_start || *end < *begin || *end > capture_end {
                    return None;
                }
                spans.push((*begin, *end));
                values[usize::from(index != 0)] += clip(*begin, *end);
            }
        }
        spans.sort_by_key(|span| span.0);
        if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return None;
        }
    }
    values[2] = total.checked_sub(values[0] + values[1])?;
    Some(values)
}

pub(super) fn query_capture_detail_report(
    events: &[Event],
    owned: isize,
    start: Instant,
    attempts: &[super::Attempt],
) -> String {
    let intervals = query_poll_intervals(events, owned, start);
    let rows: Vec<_> = intervals
        .iter()
        .filter_map(|(_, (from, to))| capture_details(*from, *to, attempts))
        .collect();
    let mut report = format!(
        "source_query_capture_detail_scope=clipped_nested_cpu_intervals_enumeration_and_texture_wrap_not_gpu_execution\nsource_query_capture_detail_valid={}\nsource_query_capture_detail_invalid={}\n",
        rows.len(),
        intervals.len() - rows.len()
    );
    for (index, name) in ["enumeration", "texture_wrap", "remaining"]
        .iter()
        .enumerate()
    {
        let mut values: Vec<_> = rows
            .iter()
            .map(|row| row[index].as_secs_f64() * 1000.)
            .collect();
        values.sort_by(f64::total_cmp);
        let p95 = values
            .get((values.len() * 95).div_ceil(100).saturating_sub(1))
            .map_or_else(|| "unavailable".to_owned(), |v| format!("{v:.6}"));
        report.push_str(&format!(
            "source_query_capture_detail_{name}_p95_ms={p95}\n"
        ));
    }
    report.push_str(
        "source_query_capture_detail_row_columns=enumeration,texture_wrap,remaining_ms\n",
    );
    for row in rows {
        report.push_str(&format!(
            "source_query_capture_detail_row={:.6},{:.6},{:.6}\n",
            row[0].as_secs_f64() * 1000.,
            row[1].as_secs_f64() * 1000.,
            row[2].as_secs_f64() * 1000.
        ));
    }
    report
}

#[test]
fn capture_detail_intervals_clip_nested_spans_and_reject_overflow_or_overlap() {
    use std::time::Duration;
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut attempt = super::Attempt {
        stamp: super::RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at(0),
        work: Duration::from_millis(10),
        stages: Default::default(),
        wait: None,
    };
    attempt.stages.cpu_spans[0] = Some((at(1), at(9)));
    attempt.stages.capture_details[0] = Some((at(1), at(3)));
    attempt.stages.capture_details[1] = Some((at(5), at(7)));
    assert_eq!(
        capture_details(at(2), at(8), std::slice::from_ref(&attempt)),
        Some([1, 2, 3].map(Duration::from_millis))
    );
    attempt.stages.capture_details_overflow = true;
    assert!(capture_details(at(2), at(8), std::slice::from_ref(&attempt)).is_none());
    attempt.stages.capture_details_overflow = false;
    attempt.stages.capture_details[2] = Some((at(6), at(8)));
    assert!(capture_details(at(2), at(8), std::slice::from_ref(&attempt)).is_none());
    attempt.stages.capture_details[2] = Some((at(9), at(10)));
    assert!(capture_details(at(2), at(8), std::slice::from_ref(&attempt)).is_none());
}

#[test]
fn query_gap_origins_use_completed_attempt_and_clip_cpu_stages() {
    use std::time::Duration;
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut attempts = vec![super::Attempt {
        stamp: super::RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at(0),
        work: Duration::from_millis(3),
        stages: super::Stages {
            output_retry: true,
            cpu_spans: [Some((at(0), at(2))), None, None, None, None],
            ..Default::default()
        },
        wait: Some(super::RenderWait::Source),
    }];
    let wait = super::scheduler::WaitSample {
        before: at(4),
        after: at(8),
        requested: Duration::from_millis(1),
        wake: super::scheduler::Wake::Timer,
    };
    let (values, counts) = gap_origins(at(1), at(7), &attempts, &[wait]).unwrap();
    assert_eq!(values[0], Duration::from_millis(3));
    assert_eq!(values[8], Duration::from_millis(1));
    assert_eq!(values[13], Duration::from_millis(1));
    assert_eq!(counts[0], 1);
    attempts[0].stages.output_retry = false;
    attempts[0].stages.comparison_waiting = true;
    assert_eq!(
        gap_origins(at(1), at(7), &attempts, &[wait]).unwrap().1[1],
        1
    );
    attempts[0].stages.cpu_spans[1] = Some((at(1), at(3)));
    assert!(gap_origins(at(1), at(7), &attempts, &[wait]).is_none());
}

#[test]
fn query_poll_brackets_require_ordered_same_candidate_readiness_transition() {
    use crate::windows::screen_capture::diagnostics::Published;
    use std::time::Duration;
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let candidate = Published {
        token: 1,
        sequence: 2,
        ticks: 3,
    };
    let poll = |began, finished, ready| Event {
        token: 1,
        sequence: 2,
        at: at(finished),
        kind: Kind::QueryPoll {
            candidate,
            began: at(began),
            finished: at(finished),
            ready,
        },
    };
    let confirm = Event {
        token: 1,
        sequence: 2,
        at: at(9),
        kind: Kind::Confirm {
            candidate: Some(candidate),
            started: at(1),
            elapsed: Duration::from_millis(8),
            changed: true,
        },
    };
    let pending = poll(2, 3, false);
    let ready = poll(5, 6, true);
    assert_eq!(
        query_poll_brackets(&[pending, ready, confirm], 1, start),
        vec![(Duration::from_millis(4), Duration::from_millis(2))]
    );
    for events in [
        vec![ready, confirm],
        vec![pending, confirm],
        vec![pending, ready, confirm, confirm],
        vec![ready, pending, confirm],
        vec![pending, poll(2, 4, true), confirm],
        vec![poll(0, 1, false), ready, confirm],
        vec![pending, poll(8, 10, true), confirm],
        vec![
            Event {
                token: 99,
                ..pending
            },
            ready,
            confirm,
        ],
    ] {
        assert!(query_poll_brackets(&events, 1, start).is_empty());
    }
}

/// Query results may be polled before source publication. Only unique, same-
/// candidate observations inside the comparison lifetime establish this gap.
fn observed_confirmation_gaps(
    events: &[Event],
    owned: isize,
    start: Instant,
) -> Vec<std::time::Duration> {
    let mut gpu = HashMap::new();
    let mut confirmations = HashMap::new();
    for event in events
        .iter()
        .filter(|event| event.token == owned && event.at >= start)
    {
        match event.kind {
            Kind::GpuComparison { candidate, .. }
                if candidate.token == owned && candidate.sequence == event.sequence =>
            {
                gpu.entry(candidate)
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some(event.at));
            }
            Kind::Confirm {
                candidate: Some(candidate),
                started,
                changed: true,
                ..
            } if candidate.token == owned && candidate.sequence == event.sequence => {
                confirmations
                    .entry(candidate)
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some((started, event.at)));
            }
            _ => {}
        }
    }
    confirmations
        .into_iter()
        .filter_map(|(candidate, confirmed)| {
            let (began, confirmed) = confirmed?;
            let observed = gpu.get(&candidate).copied().flatten()?;
            (observed >= began)
                .then(|| confirmed.checked_duration_since(observed))
                .flatten()
        })
        .collect()
}

#[test]
fn confirmation_gap_requires_unique_candidate_and_comparison_lifetime() {
    use crate::windows::screen_capture::diagnostics::Published;
    use std::time::Duration;
    let start = Instant::now();
    let candidate = Published {
        token: 1,
        sequence: 2,
        ticks: 3,
    };
    let gpu = Event {
        token: 1,
        sequence: 2,
        at: start + Duration::from_millis(5),
        kind: Kind::GpuComparison {
            candidate,
            copy_ms: 0.,
            compare_ms: 0.,
            observed_ms: 0.,
        },
    };
    let confirm = Event {
        token: 1,
        sequence: 2,
        at: start + Duration::from_millis(7),
        kind: Kind::Confirm {
            candidate: Some(candidate),
            started: start + Duration::from_millis(1),
            elapsed: Duration::from_millis(6),
            changed: true,
        },
    };
    assert_eq!(
        observed_confirmation_gaps(&[gpu, confirm], 1, start),
        vec![Duration::from_millis(2)]
    );
    assert!(observed_confirmation_gaps(&[gpu, gpu, confirm], 1, start).is_empty());
    assert!(observed_confirmation_gaps(&[gpu, confirm, confirm], 1, start).is_empty());
    assert!(observed_confirmation_gaps(&[Event { token: 2, ..gpu }, confirm], 1, start).is_empty());
    assert!(
        observed_confirmation_gaps(&[Event { at: start, ..gpu }, confirm], 1, start).is_empty()
    );
    assert!(
        observed_confirmation_gaps(
            &[
                Event {
                    at: start + Duration::from_millis(8),
                    ..gpu
                },
                confirm
            ],
            1,
            start
        )
        .is_empty()
    );
}

#[test]
fn independent_commit_evidence_requires_same_candidate_bracket_and_distinct_owned_version() {
    use crate::windows::screen_capture::diagnostics::Published;
    use std::time::Duration;
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let version = |token, sequence| Published {
        token,
        sequence,
        ticks: 0,
    };
    let pending = |token, sequence, ms| Event {
        token,
        sequence,
        at: at(ms),
        kind: Kind::Pending,
    };
    let confirm = |token, sequence, began, ms| Event {
        token,
        sequence,
        at: at(ms),
        kind: Kind::Confirm {
            started: at(began),
            elapsed: Duration::from_millis(ms - began),
            candidate: Some(version(token, sequence)),
            changed: false,
        },
    };
    let events = [
        pending(2, 1, 2),
        confirm(2, 1, 1, 8),
        pending(3, 1, 1),
        confirm(3, 2, 0, 9), // wrong sequence
        pending(4, 1, 1),    // no confirmation
        pending(5, 1, 1),
        confirm(5, 1, 3, 9), // old comparison lifetime
        pending(1, 1, 1),
        confirm(1, 1, 0, 9),
    ]; // owned source
    let commit = |sequence, ms| Presentation {
        stamp: super::RenderStamp {
            session: 1,
            revision: 1,
        },
        native_commit: None,
        counts: vec![],
        at: at(ms),
        sources: vec![version(1, sequence)],
    };
    let mut commits = vec![
        commit(1, 2),
        commit(2, 4),
        commit(2, 5),
        commit(3, 8),
        commit(4, 6),
    ];
    commits[4].sources.push(version(1, 99)); // mixed source version
    assert_eq!(
        analyze(&events, &commits, 1, start),
        Evidence {
            confirmed_extra_brackets: 1,
            owned_versions: 3,
            owned_versions_during_extra_pending: 1,
            extra_tokens_overlapping_commits: 1,
        }
    );
    assert_eq!(brackets(&events, 1, at(7)).len(), 0);
}
