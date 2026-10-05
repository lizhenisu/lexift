//! Offline notification/wait correlation; temporal overlap is not frame identity
//! or proof that a particular source caused a kernel wake.
use super::super::super::screen_capture::diagnostics::{Event, Kind};
use super::super::scheduler::{WaitSample, Wake};
use super::Attempt;
use std::time::Duration;

struct Response {
    wake: Wake,
    returned: Duration,
    attempt: Option<Duration>,
    rewaited: bool,
}
#[derive(Default)]
struct Responses {
    samples: Vec<Response>,
    invalid: usize,
    ambiguous: usize,
    late_completion: usize,
}
fn responses(
    token: isize,
    waits: &[WaitSample],
    attempts: &[Attempt],
    events: &[Event],
) -> Responses {
    let mut result = Responses::default();
    let mut ordered: Vec<_> = waits.iter().collect();
    ordered.sort_by_key(|wait| wait.before);
    result.invalid = ordered.iter().filter(|w| w.after < w.before).count()
        + ordered
            .windows(2)
            .filter(|pair| pair[0].after > pair[1].before)
            .count();
    if result.invalid != 0 {
        return result;
    }
    let mut starts: Vec<_> = attempts.iter().map(|attempt| attempt.at).collect();
    starts.sort_unstable();
    for (index, wait) in ordered.iter().enumerate() {
        let notifications: Vec<_> = events
            .iter()
            .filter_map(|event| {
                let Kind::Arrived {
                    notified,
                    woke: true,
                } = event.kind
                else {
                    return None;
                };
                if event.token != token || event.at < wait.before || event.at >= wait.after {
                    return None;
                }
                if notified < event.at {
                    result.invalid += 1;
                    return None;
                }
                if notified > wait.after {
                    result.late_completion += 1;
                    return None;
                }
                Some(notified)
            })
            .collect();
        let [notified] = notifications.as_slice() else {
            result.ambiguous += usize::from(notifications.len() > 1);
            continue;
        };
        let next = starts
            .get(starts.partition_point(|start| *start < wait.after))
            .copied();
        let rewaited = ordered
            .get(index + 1)
            .is_some_and(|later| next.is_some_and(|attempt| later.before < attempt));
        result.samples.push(Response {
            wake: wait.wake,
            returned: wait.after.duration_since(*notified),
            attempt: next.map(|at| at.duration_since(*notified)),
            rewaited,
        });
    }
    result
}
fn metric(values: impl Iterator<Item = Duration>) -> String {
    let mut values: Vec<_> = values.map(|value| value.as_secs_f64() * 1000.).collect();
    if values.is_empty() {
        return "unavailable".into();
    }
    values.sort_by(f64::total_cmp);
    format!(
        "{:.6}",
        values[((values.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)]
    )
}
pub(super) fn append(
    report: &mut String,
    token: isize,
    waits: &[WaitSample],
    attempts: &[Attempt],
    events: &[Event],
) {
    let response = responses(token, waits, attempts, events);
    report.push_str(&format!(
        "source_wake_response_scope=unique_owned_source_noncoalesced_notification_completed_inside_wait_to_return_and_first_subsequent_attempt_not_wake_cause_frame_identity_or_photon\nsource_wake_response_invalid={}\nsource_wake_response_ambiguous={}\nsource_wake_response_completion_after_return={}\n",
        response.invalid, response.ambiguous, response.late_completion,
    ));
    for (name, wake) in [("signal", Wake::Signal), ("timer", Wake::Timer)] {
        let samples: Vec<_> = response
            .samples
            .iter()
            .filter(|sample| sample.wake == wake)
            .collect();
        report.push_str(&format!(
            "source_wake_response_{name}_samples={}\nsource_wake_response_{name}_return_p95_ms={}\nsource_wake_response_{name}_attempt_p95_ms={}\nsource_wake_response_{name}_missing_attempt={}\nsource_wake_response_{name}_rewait_before_attempt={}\n",
            samples.len(), metric(samples.iter().map(|s| s.returned)),
            metric(samples.iter().filter_map(|s| s.attempt)),
            samples.iter().filter(|s| s.attempt.is_none()).count(),
            samples.iter().filter(|s| s.rewaited).count(),
        ));
    }
}

#[test]
fn response_correlation_rejects_ambiguous_late_and_unrelated_notices() {
    use super::Stages;
    use lexift_core::ports::annotation_render::RenderStamp;
    use std::time::Instant;
    let origin = Instant::now();
    let at = |ms| origin + Duration::from_millis(ms);
    let wait = |before, after, wake| WaitSample {
        before: at(before),
        after: at(after),
        requested: Duration::from_millis(after - before),
        wake,
    };
    let event = |token, entered, complete, woke| Event {
        token,
        sequence: 0,
        at: at(entered),
        kind: Kind::Arrived {
            notified: at(complete),
            woke,
        },
    };
    let attempt = |ms| Attempt {
        stamp: RenderStamp {
            session: 1,
            revision: 1,
        },
        at: at(ms),
        work: Duration::ZERO,
        stages: Stages::default(),
        wait: None,
    };
    let waits = [
        wait(0, 10, Wake::Signal),
        wait(11, 20, Wake::Timer),
        wait(30, 40, Wake::Signal),
    ];
    let attempts = [attempt(25), attempt(41)];
    let events = [
        event(7, 8, 9, true),
        event(8, 12, 13, true),
        event(7, 12, 13, false),
        event(7, 19, 21, true),
        event(7, 31, 32, true),
        event(7, 34, 35, true),
    ];
    let result = responses(7, &waits, &attempts, &events);
    assert_eq!(result.samples.len(), 1);
    assert_eq!(result.samples[0].returned, Duration::from_millis(1));
    assert_eq!(result.samples[0].attempt, Some(Duration::from_millis(16)));
    assert!(result.samples[0].rewaited);
    assert_eq!(
        (result.invalid, result.ambiguous, result.late_completion),
        (0, 1, 1)
    );
    let invalid = [wait(0, 10, Wake::Signal), wait(9, 12, Wake::Timer)];
    assert_eq!(responses(7, &invalid, &attempts, &events).invalid, 1);
    let mut report = String::new();
    append(&mut report, 7, &[], &attempts, &events);
    assert!(report.contains("source_wake_response_signal_return_p95_ms=unavailable\n"));
}
