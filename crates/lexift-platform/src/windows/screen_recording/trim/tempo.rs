//! Streaming waveform-similarity overlap-add at 48 kHz stereo.
use lexift_core::domain::recording_trim::Speed;
use std::collections::VecDeque;

const HOP: usize = 960; // 20 ms of output
const SEARCH: usize = 480; // search within 10 ms of the absolute input cursor

type Stereo = [f32; 2];

pub(super) struct Tempo {
    speed: Speed,
    input: VecDeque<Stereo>,
    base: u64,
    written: u64,
    tail: [Stereo; HOP],
}

impl Tempo {
    pub fn new(speed: Speed) -> Self {
        Self {
            speed,
            input: VecDeque::new(),
            base: 0,
            written: 0,
            tail: [[0.; 2]; HOP],
        }
    }

    pub fn push(&mut self, block: &[Stereo]) {
        self.input.extend(
            block
                .iter()
                .map(|frame| frame.map(|x| if x.is_finite() { x.clamp(-1., 1.) } else { 0. })),
        );
    }

    /// Emit one block, using an absolute analysis cursor to prevent drift.
    pub fn next(&mut self, limit: u64, finishing: bool, output: &mut [Stereo; HOP]) -> usize {
        let count = limit.saturating_sub(self.written).min(HOP as u64) as usize;
        if count == 0 {
            return 0;
        }

        let (num, den) = self.speed.ratio();
        let nominal = (u128::from(self.written) * num / den) as u64;
        let low = nominal.saturating_sub(SEARCH as u64).max(self.base);
        let high = nominal + SEARCH as u64;
        let end = self.base + self.input.len() as u64;
        if !finishing && (count != HOP || end < high + (2 * HOP) as u64) {
            return 0;
        }
        let high = high.min(end.saturating_sub(HOP as u64).max(self.base));
        let low = low.min(high);
        let samples = self.input.make_contiguous();
        let at = |absolute: u64| {
            absolute
                .checked_sub(self.base)
                .and_then(|i| samples.get(i as usize))
                .copied()
                .unwrap_or([0.; 2])
        };
        let mut best = nominal.clamp(low, high);
        if self.written != 0 {
            // Correlate both channels, but apply one offset to preserve stereo.
            let mut best_score = f64::NEG_INFINITY;
            for candidate in (low..=high).step_by(4).chain(std::iter::once(best)) {
                let mut dot = 0.;
                let mut energy = 0.;
                for i in (0..HOP).step_by(8) {
                    let a = self.tail[i];
                    let b = at(candidate + i as u64);
                    dot += f64::from(a[0] * b[0] + a[1] * b[1]);
                    energy += f64::from(b[0] * b[0] + b[1] * b[1]);
                }
                let score = dot / energy.max(1e-12).sqrt();
                if score > best_score
                    || score == best_score && candidate.abs_diff(nominal) < best.abs_diff(nominal)
                {
                    best_score = score;
                    best = candidate;
                }
            }
        }
        for (i, value) in output[..count].iter_mut().enumerate() {
            let sample = at(best + i as u64);
            let mix = if self.written == 0 {
                1.
            } else {
                i as f32 / HOP as f32
            };
            *value = std::array::from_fn(|ch| self.tail[i][ch] * (1. - mix) + sample[ch] * mix);
        }
        self.tail = std::array::from_fn(|i| at(best + (HOP + i) as u64));
        self.written += count as u64;
        let next = (u128::from(self.written) * num / den) as u64;
        let keep = next.saturating_sub(SEARCH as u64).clamp(self.base, end);
        self.input.drain(..(keep - self.base) as usize);
        self.base = keep;
        count
    }
}

pub(super) const BLOCK: usize = HOP;
