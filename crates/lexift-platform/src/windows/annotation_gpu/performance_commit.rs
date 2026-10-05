//! Offline bounds for CPU work outside the three timed native submission calls.
//! This residual includes mailbox acquisition and bookkeeping, not measured lock time.
use super::{Attempt, Duration, Stages};

#[derive(Default)]
struct Costs {
    skipped: usize,
    invalid: usize,
    residuals: Vec<Duration>,
    totals: Vec<Duration>,
}

impl Costs {
    fn observe(&mut self, stages: Stages) {
        // Native call durations are copied only after an accepted submission.
        // Stale authorization, rejected Present and no-commit attempts lack them.
        if stages.submission_cpu[0].is_zero() {
            self.skipped += 1;
            return;
        }
        let calls = stages
            .submission_cpu
            .into_iter()
            .try_fold(Duration::ZERO, Duration::checked_add);
        let Some(residual) = calls.and_then(|calls| stages.commit.checked_sub(calls)) else {
            self.invalid += 1;
            return;
        };
        self.residuals.push(residual);
        self.totals.push(stages.commit);
    }
}

pub(super) fn append(report: &mut String, attempts: &[Attempt], p95: impl Fn(Vec<f64>) -> f64) {
    let mut costs = Costs::default();
    for attempt in attempts {
        costs.observe(attempt.stages);
    }
    let metric = |samples: &[Duration]| {
        if samples.is_empty() {
            "unavailable".to_owned()
        } else {
            format!(
                "{:.6}",
                p95(samples.iter().map(|d| d.as_secs_f64() * 1000.).collect())
            )
        }
    };
    let maximum = costs.residuals.iter().max().map_or_else(
        || "unavailable".to_owned(),
        |d| format!("{:.6}", d.as_secs_f64() * 1000.),
    );
    report.push_str(&format!(
        "commit_cpu_residual_scope=same_accepted_attempt_outer_commit_minus_native_call_sum_includes_lock_acquisition_and_bookkeeping_not_lock_hold_gpu_or_os_sync\ncommit_cpu_residual_samples={}\ncommit_cpu_residual_skipped={}\ncommit_cpu_residual_invalid={}\ncommit_cpu_accepted_total_p95_ms={}\ncommit_cpu_residual_p95_ms={}\ncommit_cpu_residual_max_ms={}\ncommit_cpu_residual_at_least_1ms={}\n",
        costs.residuals.len(), costs.skipped, costs.invalid, metric(&costs.totals),
        metric(&costs.residuals), maximum,
        costs.residuals.iter().filter(|&&d| d >= Duration::from_millis(1)).count(),
    ));
}

#[test]
fn commit_residual_uses_paired_calls_and_rejects_missing_or_inverted_timing() {
    let stages = |total, calls: [u64; 3]| Stages {
        commit: Duration::from_millis(total),
        submission_cpu: calls.map(Duration::from_millis),
        ..Default::default()
    };
    let mut costs = Costs::default();
    costs.observe(stages(11, [8, 1, 1]));
    costs.observe(stages(10, [1, 0, 0]));
    costs.observe(stages(3, [1, 1, 1]));
    costs.observe(stages(2, [1, 1, 1]));
    costs.observe(stages(20, [0, 0, 0]));
    assert_eq!(costs.residuals, [1, 9, 0].map(Duration::from_millis));
    assert_eq!(costs.totals, [11, 10, 3].map(Duration::from_millis));
    assert_eq!((costs.invalid, costs.skipped), (1, 1));
    costs.observe(Stages {
        commit: Duration::MAX,
        submission_cpu: [Duration::MAX, Duration::MAX, Duration::ZERO],
        ..Default::default()
    });
    assert_eq!(costs.invalid, 2);
}

#[test]
fn empty_commit_report_does_not_claim_zero_cost() {
    let mut report = String::new();
    append(&mut report, &[], |_| {
        panic!("empty samples must not be measured")
    });
    assert!(report.contains("commit_cpu_residual_samples=0\n"));
    assert!(report.contains("commit_cpu_residual_p95_ms=unavailable\n"));
    assert!(report.contains("commit_cpu_residual_max_ms=unavailable\n"));
}
