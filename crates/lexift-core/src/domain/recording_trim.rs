//! Integer video-frame boundaries shared by preview and exact export.
use std::time::Duration;
/// A half-open rectangle in original video pixels, aligned to NV12 chroma cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameCrop {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl FrameCrop {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        }
    }

    pub fn size(self) -> (u32, u32) {
        (
            self.right.saturating_sub(self.left),
            self.bottom.saturating_sub(self.top),
        )
    }

    pub fn valid(self, width: u32, height: u32) -> bool {
        let (w, h) = self.size();
        self.right <= width
            && self.bottom <= height
            && w >= 16
            && h >= 16
            && [self.left, self.top, self.right, self.bottom]
                .into_iter()
                .all(|edge| edge % 2 == 0)
    }
}

/// Supported playback and export speeds, expressed as exact ratios.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Speed {
    #[default]
    Normal,
    OneAndHalf,
    Double,
}

impl Speed {
    pub const fn ratio(self) -> (u128, u128) {
        match self {
            Self::Normal => (1, 1),
            Self::OneAndHalf => (3, 2),
            Self::Double => (2, 1),
        }
    }

    pub fn multiplier(self) -> f64 {
        let (num, den) = self.ratio();
        num as f64 / den as f64
    }

    pub fn output_frames(self, source_frames: u64) -> u64 {
        let (num, den) = self.ratio();
        (u128::from(source_frames) * den).div_ceil(num) as u64
    }

    pub fn source_frame(self, range: Range, index: u64) -> Option<u64> {
        let count = self.output_frames(range.len());
        if index >= count {
            return None;
        }
        let (num, den) = self.ratio();
        Some(if count > 1 && index + 1 == count {
            range.end - 1
        } else {
            range.start + (u128::from(index) * num / den) as u64
        })
    }
}

/// Policy for the mixed audio track of a sealed recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportAudio {
    #[default]
    Original,
    HalfVolume,
    RemoveTrack,
}

impl ExportAudio {
    pub const fn gain(self) -> f32 {
        match self {
            Self::Original => 1.0,
            Self::HalfVolume => 0.5,
            Self::RemoveTrack => 0.0,
        }
    }
}

/// The spatial and temporal boundaries and audio policy frozen for one export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub range: Range,
    pub crop: Option<FrameCrop>,
    pub speed: Speed,
    pub audio: ExportAudio,
}

impl From<Range> for Selection {
    fn from(range: Range) -> Self {
        Self {
            range,
            crop: None,
            speed: Speed::Normal,
            audio: ExportAudio::Original,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    pub end: u64,
}
impl Range {
    pub fn full(frames: u64) -> Self {
        Self {
            start: 0,
            end: frames,
        }
    }
    pub fn valid(self, frames: u64) -> bool {
        self.start < self.end && self.end <= frames
    }
    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
    pub fn time(frame: u64, fps: u32) -> Duration {
        Duration::from_nanos(
            ((u128::from(frame) * 1_000_000_000) / u128::from(fps.max(1))).min(u128::from(u64::MAX))
                as u64,
        )
    }
    pub fn ticks(frame: u64, fps: u32) -> Option<i64> {
        i64::try_from(u128::from(frame) * 10_000_000 / u128::from(fps.max(1))).ok()
    }
    pub fn clamp_time(self, time: Duration, fps: u32) -> Duration {
        let frame = ((time.as_nanos() * u128::from(fps.max(1)) + 500_000_000) / 1_000_000_000)
            .min(u128::from(u64::MAX)) as u64;
        Self::time(frame.max(self.start).min(self.end.saturating_sub(1)), fps)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_selects_monotonic_source_frames_and_retains_short_ranges() {
        for speed in [Speed::Normal, Speed::OneAndHalf, Speed::Double] {
            for len in 1..=180 {
                let range = Range {
                    start: 37,
                    end: 37 + len,
                };
                let count = speed.output_frames(len);
                let frames: Vec<_> = (0..count)
                    .map(|i| speed.source_frame(range, i).unwrap())
                    .collect();
                assert_eq!(frames[0], range.start);
                assert_eq!(speed.source_frame(range, count), None);
                assert!(frames.windows(2).all(|pair| pair[0] < pair[1]));
                if count > 1 {
                    assert_eq!(frames[count as usize - 1], range.end - 1);
                }
            }
        }
        assert_eq!(Speed::Double.output_frames(u64::MAX), 1 << 63);
    }

    #[test]
    fn exact_frame_ranges_exclude_end_and_keep_long_clock_precision() {
        for fps in [30, 60] {
            let r = Range {
                start: 17,
                end: 36001,
            };
            assert!(r.valid(36001));
            assert_eq!(r.len(), 35984);
            assert_eq!(Range::ticks(u64::from(fps) * 600, fps), Some(6_000_000_000));
            assert_eq!(r.clamp_time(Duration::ZERO, fps), Range::time(17, fps));
            assert_eq!(r.clamp_time(Duration::MAX, fps), Range::time(36000, fps));
        }
        assert!(!Range { start: 1, end: 1 }.valid(2));
        assert!(!Range { start: 0, end: 3 }.valid(2));
        assert!(Range { start: 1, end: 2 }.valid(2));
    }
}
