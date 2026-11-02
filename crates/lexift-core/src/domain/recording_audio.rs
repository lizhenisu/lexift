//! Bounded PCM primitives shared by native capture and deterministic audio tests.
use crate::{Error, Result};
use std::{collections::VecDeque, time::Duration};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const MAX_QUEUE_FRAMES: usize = SAMPLE_RATE as usize / 2;
pub const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
/// Session-scoped opaque handles, never OS endpoint strings or COM references.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub system: Option<u64>,
    pub microphone: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Silent,
    System,
    Microphone,
    Mixed,
}
impl Mode {
    pub fn system(self) -> bool {
        matches!(self, Self::System | Self::Mixed)
    }
    pub fn microphone(self) -> bool {
        matches!(self, Self::Microphone | Self::Mixed)
    }
}

/// Absolute sample positions avoid accumulating rounding errors per packet.
pub fn sample_time(frame: u64) -> Option<i64> {
    i64::try_from(u128::from(frame) * 10_000_000 / u128::from(SAMPLE_RATE)).ok()
}
pub fn frames_for(duration: Duration) -> Result<u64> {
    u64::try_from((duration.as_nanos() * u128::from(SAMPLE_RATE)).div_ceil(1_000_000_000))
        .map_err(|_| Error::new("Audio duration overflow"))
}
fn clean(value: f32) -> f32 {
    if value.is_finite() { value } else { 0. }
}
pub fn mix(mode: Mode, system: [f32; 2], microphone: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|i| match mode {
        Mode::Silent => 0.,
        Mode::System => clean(system[i]).clamp(-1., 1.),
        Mode::Microphone => clean(microphone[i]).clamp(-1., 1.),
        Mode::Mixed => (clean(system[i]) * 0.5 + clean(microphone[i]) * 0.5).clamp(-1., 1.),
    })
}

/// Channel-mask-aware downmix. Unknown layouts fail instead of guessing speaker order.
pub fn stereo(input: &[f32], channel_mask: u32) -> Result<[f32; 2]> {
    if input.len() == 1 {
        return Ok([clean(input[0]); 2]);
    }
    if input.len() == 2 {
        return Ok([clean(input[0]), clean(input[1])]);
    }
    if input.is_empty()
        || channel_mask.count_ones() as usize != input.len()
        || channel_mask & !0x7ff != 0
    {
        return Err(Error::new("Unsupported audio channel layout"));
    }
    let mut out = [0.; 2];
    let mut index = 0;
    for bit in 0..11 {
        if channel_mask & (1 << bit) == 0 {
            continue;
        }
        let value = clean(input[index]);
        index += 1;
        let weights = match bit {
            0 => [1., 0.],
            1 => [0., 1.],
            2 => [std::f32::consts::FRAC_1_SQRT_2; 2],
            3 => [0.; 2], // LFE is deliberately excluded from the stereo fold-down.
            4 | 6 | 9 => [std::f32::consts::FRAC_1_SQRT_2, 0.],
            5 | 7 | 10 => [0., std::f32::consts::FRAC_1_SQRT_2],
            _ => [0.5; 2],
        };
        for i in 0..2 {
            out[i] += value * weights[i];
        }
    }
    Ok(out.map(clean))
}

#[derive(Default)]
pub struct Queue {
    start: u64,
    frames: VecDeque<[f32; 2]>,
}
impl Queue {
    pub fn clear(&mut self) {
        self.frames.clear();
        self.start = 0;
    }
    pub fn retained_frames(&self) -> usize {
        self.frames.len()
    }
    /// Keep continuity and explicit timestamps; overruns never discard older PCM.
    pub fn push(&mut self, start: u64, frames: &[[f32; 2]]) -> Result<()> {
        if frames.len() > MAX_QUEUE_FRAMES
            || self.frames.len().saturating_add(frames.len()) > MAX_QUEUE_FRAMES
        {
            return Err(Error::new("Audio queue exceeded 500ms"));
        }
        if self.frames.is_empty() {
            self.start = start;
        }
        if self.start.checked_add(self.frames.len() as u64) != Some(start)
            || start.checked_add(frames.len() as u64).is_none()
        {
            return Err(Error::new("Audio packet discontinuity"));
        }
        self.frames.extend(frames.iter().map(|v| v.map(clean)));
        Ok(())
    }
    pub fn take(&mut self, frame: u64) -> Result<Option<[f32; 2]>> {
        if frame < self.start {
            return Ok(None);
        }
        if self.frames.is_empty() {
            return Ok(None);
        }
        if frame != self.start {
            return Err(Error::new("Audio consumer skipped queued samples"));
        }
        self.start = self
            .start
            .checked_add(1)
            .ok_or_else(|| Error::new("Audio sample position overflow"))?;
        Ok(self.frames.pop_front())
    }
}

/// Interpolate a fractional source position. Streaming callers retain the last
/// frame until the next packet supplies the interpolation lookahead.
pub fn resample_at(input: &[[f32; 2]], source_position: f64) -> Result<[f32; 2]> {
    if !source_position.is_finite() || source_position < 0. || input.is_empty() {
        return Err(Error::new("Invalid audio resampling position"));
    }
    let left = source_position.floor() as usize;
    if left >= input.len() {
        return Err(Error::new("Audio resampling left its packet"));
    }
    let right = (left + 1).min(input.len() - 1);
    let fraction = (source_position - left as f64) as f32;
    Ok(std::array::from_fn(|i| {
        clean(input[left][i]) * (1. - fraction) + clean(input[right][i]) * fraction
    }))
}

struct Block {
    start: f64,
    period: f64,
    frames: Vec<[f32; 2]>,
}
/// Timestamped, bounded source PCM. A slow phase correction reconciles device
/// samples with QPC without dropping or repeating whole audio packets.
#[derive(Default)]
pub struct Stream {
    blocks: VecDeque<Block>,
    retained: usize,
    next_position: Option<u64>,
    end: f64,
    period: f64,
}
impl Stream {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn push(
        &mut self,
        position: u64,
        qpc: u64,
        frames: Vec<[f32; 2]>,
        discontinuity: bool,
    ) -> Result<()> {
        self.append(position, qpc, frames, discontinuity, false)
    }
    /// A valid loopback endpoint may stop delivering packets while the render
    /// engine is idle. Its next QPC locates the silent interval explicitly.
    pub fn push_after_silence(
        &mut self,
        position: u64,
        qpc: u64,
        frames: Vec<[f32; 2]>,
    ) -> Result<()> {
        self.append(position, qpc, frames, false, true)
    }
    fn append(
        &mut self,
        position: u64,
        qpc: u64,
        frames: Vec<[f32; 2]>,
        discontinuity: bool,
        silence_gap: bool,
    ) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        if self.retained.saturating_add(frames.len()) > MAX_QUEUE_FRAMES {
            return Err(Error::new("Audio queue exceeded 500ms"));
        }
        let nominal = 10_000_000. / f64::from(SAMPLE_RATE);
        let start = if let Some(expected) = self.next_position {
            if expected != position || discontinuity {
                return Err(Error::new(format!(
                    "Audio device sample discontinuity: expected={expected}, position={position}, flagged={discontinuity}"
                )));
            }
            let error = qpc as f64 - self.end;
            if silence_gap {
                if error < 0. {
                    return Err(Error::new("Audio silence interval moved backwards"));
                }
                self.period = nominal;
                qpc as f64
            } else {
                if error.abs() > 500_000. {
                    return Err(Error::new("Audio clock diverged by more than 50ms"));
                }
                // Correct at most 1000 ppm. Error is absorbed over ten seconds,
                // preserving continuous positions at every packet boundary.
                self.period = (nominal + error / (10. * f64::from(SAMPLE_RATE)))
                    .clamp(nominal * 0.999, nominal * 1.001);
                self.end
            }
        } else {
            self.period = nominal;
            qpc as f64
        };
        self.next_position = Some(
            position
                .checked_add(frames.len() as u64)
                .ok_or_else(|| Error::new("Audio position overflow"))?,
        );
        self.end = start + self.period * frames.len() as f64;
        self.retained += frames.len();
        self.blocks.push_back(Block {
            start,
            period: self.period,
            frames,
        });
        Ok(())
    }
    /// Missing source data represents silence only while the native endpoint
    /// remains valid. The adapter separately treats endpoint errors as fatal.
    pub fn sample(&mut self, qpc: f64) -> Result<[f32; 2]> {
        if !qpc.is_finite() {
            return Err(Error::new("Invalid audio clock"));
        }
        while self
            .blocks
            .front()
            .is_some_and(|b| qpc >= b.start + b.period * b.frames.len() as f64)
        {
            self.retained -= self.blocks.pop_front().unwrap().frames.len();
        }
        let Some(block) = self.blocks.front() else {
            return Ok([0.; 2]);
        };
        if qpc < block.start {
            return Ok([0.; 2]);
        }
        let position = (qpc - block.start) / block.period;
        let left = position.floor() as usize;
        let fraction = (position - left as f64) as f32;
        let next = block
            .frames
            .get(left + 1)
            .copied()
            .or_else(|| self.blocks.get(1).and_then(|b| b.frames.first()).copied())
            .unwrap_or(block.frames[left]);
        Ok(std::array::from_fn(|i| {
            clean(block.frames[left][i]) * (1. - fraction) + clean(next[i]) * fraction
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamped_stream_is_continuous_bounded_and_rejects_lost_packets() {
        let mut stream = Stream::default();
        stream
            .push(0, 1_000_000, vec![[0.25; 2]; 480], true)
            .unwrap();
        stream
            .push(480, 1_100_000, vec![[0.75; 2]; 480], false)
            .unwrap();
        assert_eq!(stream.sample(999_999.).unwrap(), [0.; 2]);
        assert_eq!(stream.sample(1_000_000.).unwrap(), [0.25; 2]);
        let boundary = stream.sample(1_099_896.).unwrap()[0];
        assert!(boundary > 0.49 && boundary < 0.51);
        assert_eq!(stream.sample(1_100_000.).unwrap(), [0.75; 2]);
        assert!(stream.push(961, 1_200_000, vec![[0.; 2]], false).is_err());
        assert!(stream.push(960, 1_200_000, vec![[0.; 2]], true).is_err());
        stream.clear();
        stream
            .push(0, 0, vec![[0.; 2]; MAX_QUEUE_FRAMES], false)
            .unwrap();
        assert!(
            stream
                .push(MAX_QUEUE_FRAMES as u64, 5_000_000, vec![[0.; 2]], false)
                .is_err()
        );
    }
    #[test]
    fn ten_minute_device_drift_is_corrected_without_packet_deletion() {
        for ppm in [-500., -100., 100., 500.] {
            let mut stream = Stream::default();
            let period = 10_000_000. / 48000. * (1. + ppm / 1_000_000.);
            for packet in 0..60_000u64 {
                let position = packet * 480;
                let qpc = 1_000_000. + position as f64 * period;
                stream
                    .push(position, qpc.round() as u64, vec![[0.25; 2]; 480], false)
                    .unwrap();
                // Render at the current wall-clock boundary, independently of
                // the device's faster/slower absolute sample count.
                assert_eq!(stream.sample(qpc).unwrap(), [0.25; 2]);
                let expected_end = qpc + 480. * period;
                assert!(
                    (stream.end - expected_end).abs() <= 100_000.,
                    "ppm={ppm}, packet={packet}"
                );
            }
        }
    }
    #[test]
    fn located_loopback_silence_keeps_real_time_without_allocating_zero_packets() {
        let mut stream = Stream::default();
        stream
            .push(0, 1_000_000, vec![[0.25; 2]; 480], false)
            .unwrap();
        stream
            .push_after_silence(480, 601_000_000, vec![[0.75; 2]; 480])
            .unwrap();
        assert_eq!(stream.sample(1_050_000.).unwrap(), [0.25; 2]);
        assert_eq!(stream.sample(300_000_000.).unwrap(), [0.; 2]);
        assert_eq!(stream.sample(601_000_000.).unwrap(), [0.75; 2]);
        assert!(
            stream
                .push_after_silence(960, 600_000_000, vec![[0.; 2]])
                .is_err()
        );
    }
    #[test]
    fn absolute_pts_has_no_ten_hour_rounding_drift() {
        assert_eq!(sample_time(48_000 * 36_000), Some(360_000_000_000));
        assert_eq!(frames_for(Duration::from_millis(25)).unwrap(), 1200);
        assert_eq!(sample_time(u64::MAX), None);
    }
    #[test]
    fn independent_modes_and_fixed_half_gain_do_not_clip() {
        assert_eq!(mix(Mode::Mixed, [1., -1.], [1., 1.]), [1., 0.]);
        assert_eq!(mix(Mode::Mixed, [1.5; 2], [-0.5; 2]), [0.5; 2]);
        assert_eq!(mix(Mode::System, [0.75, -0.5], [1.; 2]), [0.75, -0.5]);
        assert_eq!(
            mix(Mode::Microphone, [1.; 2], [f32::NAN, f32::INFINITY]),
            [0.; 2]
        );
        assert_eq!(mix(Mode::Silent, [1.; 2], [1.; 2]), [0.; 2]);
    }
    #[test]
    fn mono_stereo_and_surround_have_explicit_mapping() {
        assert_eq!(stereo(&[0.25], 0).unwrap(), [0.25; 2]);
        assert_eq!(stereo(&[0.2, -0.3], 3).unwrap(), [0.2, -0.3]);
        assert_eq!(stereo(&[0., 0., 0., 1., 0., 0.], 0x3f).unwrap(), [0.; 2]);
        assert!(stereo(&[0.; 6], 0).is_err());
    }
    #[test]
    fn bounded_queue_preserves_samples_and_rejects_gaps() {
        let mut q = Queue::default();
        q.push(10, &[[0.25; 2]; 2]).unwrap();
        assert!(q.push(13, &[[0.; 2]]).is_err());
        assert_eq!(q.take(9).unwrap(), None);
        assert_eq!(q.take(10).unwrap(), Some([0.25; 2]));
        assert!(q.take(12).is_err());
        assert_eq!(q.take(11).unwrap(), Some([0.25; 2]));
        q.clear();
        q.push(0, &vec![[0.; 2]; MAX_QUEUE_FRAMES]).unwrap();
        assert!(q.push(MAX_QUEUE_FRAMES as u64, &[[1.; 2]]).is_err());
        assert_eq!(q.retained_frames(), MAX_QUEUE_FRAMES);
        q.clear();
        assert!(q.push(u64::MAX, &[[0.; 2]]).is_err());
    }
    #[test]
    fn interpolation_preserves_constant_signal_and_stereo_polarity() {
        assert_eq!(resample_at(&[[0., 1.], [1., -1.]], 0.5).unwrap(), [0.5, 0.]);
        assert_eq!(resample_at(&[[0.25; 2]; 4], 2.75).unwrap(), [0.25; 2]);
        assert!(resample_at(&[[0.; 2]], f64::NAN).is_err());
    }
}
