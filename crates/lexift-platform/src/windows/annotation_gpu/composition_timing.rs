//! Opt-in, same-device DComposition estimates; never used as a scheduling deadline.
use super::display_timing::{ClockAnchor, NativeCommit, Observation};
use std::time::Instant;
use windows::Win32::Graphics::DirectComposition::{
    DCOMPOSITION_FRAME_STATISTICS, IDCompositionDevice,
};

#[derive(Clone, Copy)]
pub(super) struct Clock {
    pub stats: DCOMPOSITION_FRAME_STATISTICS,
    pub before: Instant,
    pub after: Instant,
    pub anchor: ClockAnchor,
}
#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub prediction_qpc: i64,
    pub sync_qpc: i64,
    pub frequency: i64,
    pub rate: (u32, u32),
    pub period_ms: f64,
    pub query_ms: f64,
    pub current_minus_last_ms: f64,
    pub estimated_lead_ms: (f64, f64),
    pub sync_minus_estimate_ms: f64,
}
impl Sample {
    /// Same-clock adjacent prediction and OS intervals. Predictions may stay
    /// unchanged or move backwards; retain their signed delta for diagnosis.
    pub fn adjacent_intervals_ms(self, next: Self) -> Option<(f64, f64, f64)> {
        if self.frequency <= 0
            || self.frequency != next.frequency
            || self.prediction_qpc <= 0
            || next.prediction_qpc <= 0
            || self.sync_qpc <= 0
            || next.sync_qpc <= self.sync_qpc
        {
            return None;
        }
        let ms = |delta: i128| delta as f64 * 1000. / self.frequency as f64;
        let predicted = ms(i128::from(next.prediction_qpc) - i128::from(self.prediction_qpc));
        let actual = ms(i128::from(next.sync_qpc) - i128::from(self.sync_qpc));
        Some((predicted, actual, actual - predicted))
    }

    /// Rounded signed refresh-period error, not a measured missed deadline.
    /// Negative values mean the matched OS sync preceded the prediction.
    pub fn estimate_error_cycles(self) -> Option<i64> {
        let cycles = self.sync_minus_estimate_ms / self.period_ms;
        (self.period_ms > 0. && cycles.is_finite() && cycles.abs() < i64::MAX as f64)
            .then(|| cycles.round() as i64)
    }
}
impl Clock {
    /// Compare same-frequency predictions in ordered query windows; no CPU
    /// projection or refresh-count inference is needed for this signed delta.
    pub fn estimate_advance_ms(self, later: Self) -> Option<f64> {
        if self.stats.timeFrequency <= 0
            || self.stats.timeFrequency != later.stats.timeFrequency
            || self.stats.nextEstimatedFrameTime <= 0
            || later.stats.nextEstimatedFrameTime <= 0
            || self.after > later.before
        {
            return None;
        }
        Some(
            (i128::from(later.stats.nextEstimatedFrameTime)
                - i128::from(self.stats.nextEstimatedFrameTime)) as f64
                * 1000.
                / self.stats.timeFrequency as f64,
        )
    }

    pub fn query(device: &IDCompositionDevice) -> Option<Self> {
        let before = Instant::now();
        let stats = unsafe { device.GetFrameStatistics() }.ok()?;
        let anchor = ClockAnchor::sample().ok()?;
        Some(Self {
            stats,
            before,
            after: Instant::now(),
            anchor,
        })
    }
    /// Project the reported estimate into the bounded CPU clock. A stale estimate
    /// remains negative; it is not clamped to the call or declared a missed frame.
    pub fn sample(self, native: NativeCommit, displayed: &Observation) -> Option<Sample> {
        let s = self.stats;
        if s.timeFrequency <= 0
            || s.timeFrequency != displayed.frequency
            || s.currentCompositionRate.Numerator == 0
            || s.currentCompositionRate.Denominator == 0
            || s.currentTime <= 0
            || s.lastFrameTime <= 0
            || s.nextEstimatedFrameTime <= 0
            || self.anchor.qpc < s.currentTime
            || displayed.qpc <= 0
            || displayed.present_refresh != displayed.sync_refresh
            || self.after < self.before
            || self.after > native.before
            || self.anchor.after < self.anchor.before
            || self.anchor.before < self.before
            || self.anchor.after > self.after
            || native.after < native.before
        {
            return None;
        }
        let projected_lead = |anchor: Instant| {
            let cpu_delta = native.after.checked_duration_since(anchor)?.as_secs_f64() * 1000.;
            Some(
                (i128::from(s.nextEstimatedFrameTime) - i128::from(self.anchor.qpc)) as f64 * 1000.
                    / s.timeFrequency as f64
                    - cpu_delta,
            )
        };
        Some(Sample {
            prediction_qpc: s.nextEstimatedFrameTime,
            sync_qpc: displayed.qpc,
            frequency: s.timeFrequency,
            rate: (
                s.currentCompositionRate.Numerator,
                s.currentCompositionRate.Denominator,
            ),
            period_ms: 1000. * s.currentCompositionRate.Denominator as f64
                / s.currentCompositionRate.Numerator as f64,
            query_ms: self.after.duration_since(self.before).as_secs_f64() * 1000.,
            current_minus_last_ms: (i128::from(s.currentTime) - i128::from(s.lastFrameTime)) as f64
                * 1000.
                / s.timeFrequency as f64,
            estimated_lead_ms: (
                projected_lead(self.anchor.before)?,
                projected_lead(self.anchor.after)?,
            ),
            sync_minus_estimate_ms: (i128::from(displayed.qpc)
                - i128::from(s.nextEstimatedFrameTime)) as f64
                * 1000.
                / s.timeFrequency as f64,
        })
    }
}

#[test]
fn estimate_projection_preserves_negative_values_and_rejects_incompatible_clocks() {
    use super::display_timing::Count;
    use std::time::Duration;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL;
    let base = Instant::now();
    let mut clock = Clock {
        stats: DCOMPOSITION_FRAME_STATISTICS {
            lastFrameTime: 1010,
            currentTime: 1040,
            timeFrequency: 1000,
            nextEstimatedFrameTime: 1050,
            currentCompositionRate: DXGI_RATIONAL {
                Numerator: 100,
                Denominator: 1,
            },
        },
        before: base + Duration::from_millis(39),
        after: base + Duration::from_millis(42),
        anchor: ClockAnchor {
            before: base + Duration::from_millis(40),
            after: base + Duration::from_millis(41),
            qpc: 1040,
        },
    };
    let native = NativeCommit {
        before: base + Duration::from_millis(43),
        after: base + Duration::from_millis(44),
        composition: None,
        composition_before_present: None,
    };
    let mut displayed = Observation {
        frequency: 1000,
        count: Count {
            chain: 1,
            present: 1,
        },
        at: base + Duration::from_millis(65),
        qpc: 1060,
        present_refresh: 1,
        sync_refresh: 1,
        clock: None,
    };
    let sample = clock.sample(native, &displayed).unwrap();
    assert_eq!(sample.period_ms, 10.);
    assert_eq!(sample.query_ms, 3.);
    assert_eq!(sample.estimated_lead_ms, (6., 7.));
    assert_eq!(sample.current_minus_last_ms, 30.);
    assert_eq!(sample.sync_minus_estimate_ms, 10.);
    // The native probe reports a future lastFrameTime; retain this signed
    // relationship instead of imposing an undocumented past-time invariant.
    clock.stats.lastFrameTime = 1060;
    assert_eq!(
        clock
            .sample(native, &displayed)
            .unwrap()
            .current_minus_last_ms,
        -20.
    );
    clock.stats.nextEstimatedFrameTime = 1030;
    let stale = clock.sample(native, &displayed).unwrap();
    assert_eq!(stale.estimated_lead_ms, (-14., -13.));
    clock.stats.nextEstimatedFrameTime = 1070;
    assert_eq!(
        clock
            .sample(native, &displayed)
            .unwrap()
            .sync_minus_estimate_ms,
        -10.
    );
    displayed.frequency = 1001;
    assert!(clock.sample(native, &displayed).is_none());
    displayed.frequency = 1000;
    clock.stats.currentCompositionRate.Numerator = 0;
    assert!(clock.sample(native, &displayed).is_none());
}

#[test]
fn estimate_error_cycles_preserve_early_and_late_sync() {
    let mut sample = Sample {
        prediction_qpc: 1050,
        sync_qpc: 1060,
        frequency: 1000,
        rate: (100, 1),
        period_ms: 10.,
        query_ms: 0.,
        current_minus_last_ms: 0.,
        estimated_lead_ms: (0., 0.),
        sync_minus_estimate_ms: -10.001,
    };
    assert_eq!(sample.estimate_error_cycles(), Some(-1));
    sample.sync_minus_estimate_ms = 10.001;
    assert_eq!(sample.estimate_error_cycles(), Some(1));
    sample.sync_minus_estimate_ms = -0.001;
    assert_eq!(sample.estimate_error_cycles(), Some(0));
    sample.period_ms = 0.;
    assert_eq!(sample.estimate_error_cycles(), None);
    sample.period_ms = f64::NAN;
    assert_eq!(sample.estimate_error_cycles(), None);
}

#[test]
fn prediction_advance_preserves_direction_and_requires_ordered_same_clock() {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL;
    let base = Instant::now();
    let first = Clock {
        stats: DCOMPOSITION_FRAME_STATISTICS {
            lastFrameTime: 1000,
            currentTime: 1000,
            timeFrequency: 1000,
            nextEstimatedFrameTime: 1050,
            currentCompositionRate: DXGI_RATIONAL {
                Numerator: 100,
                Denominator: 1,
            },
        },
        before: base,
        after: base,
        anchor: ClockAnchor {
            before: base,
            after: base,
            qpc: 1000,
        },
    };
    let mut later = first;
    later.stats.nextEstimatedFrameTime = 1060;
    assert_eq!(first.estimate_advance_ms(later), Some(10.));
    later.stats.nextEstimatedFrameTime = 1040;
    assert_eq!(first.estimate_advance_ms(later), Some(-10.));
    later.stats.timeFrequency = 1001;
    assert_eq!(first.estimate_advance_ms(later), None);
    later.stats.timeFrequency = 1000;
    later.before = base - std::time::Duration::from_millis(1);
    assert_eq!(first.estimate_advance_ms(later), None);
}

#[test]
fn adjacent_prediction_intervals_preserve_expansion_and_long_uptime_precision() {
    let first = Sample {
        prediction_qpc: 3_000_000_000_000_000,
        sync_qpc: 3_000_000_000_000_000,
        frequency: 3_000_000,
        rate: (100, 1),
        period_ms: 10.,
        query_ms: 0.,
        current_minus_last_ms: 0.,
        estimated_lead_ms: (0., 0.),
        sync_minus_estimate_ms: 0.,
    };
    let mut next = first;
    next.prediction_qpc += 60000;
    next.sync_qpc += 90000;
    assert_eq!(first.adjacent_intervals_ms(next), Some((20., 30., 10.)));
    next.prediction_qpc = first.prediction_qpc;
    assert_eq!(first.adjacent_intervals_ms(next), Some((0., 30., 30.)));
    next.prediction_qpc -= 30000;
    assert_eq!(first.adjacent_intervals_ms(next), Some((-10., 30., 40.)));
    next.frequency += 1;
    assert_eq!(first.adjacent_intervals_ms(next), None);
    next.frequency = first.frequency;
    next.sync_qpc = first.sync_qpc;
    assert_eq!(first.adjacent_intervals_ms(next), None);
}
