//! Fixed sharing policy and an absolute, centisecond GIF timeline.
use super::recording_trim::Range;
pub mod palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Mp4,
    Gif,
}
impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Gif => "gif",
        }
    }
    pub fn accepts_extension(self, value: &str) -> bool {
        self.extension().eq_ignore_ascii_case(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    InvalidSource,
    TooLong,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub range: Range,
    pub source_fps: u32,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    total_cs: u64,
}
impl Plan {
    pub fn new(
        range: Range,
        frames: u64,
        fps: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, Rejection> {
        if !range.valid(frames) || !matches!(fps, 30 | 60) || width == 0 || height == 0 {
            return Err(Rejection::InvalidSource);
        }
        if range.len() > u64::from(fps) * 60 {
            return Err(Rejection::TooLong);
        }
        let longest = u64::from(width.max(height));
        let dimension = |n: u32| {
            if longest <= 960 {
                n
            } else {
                ((u64::from(n) * 960 + longest / 2) / longest).max(1) as u32
            }
        };
        Ok(Self {
            range,
            source_fps: fps,
            width: dimension(width),
            height: dimension(height),
            frames: (range.len() * 15).div_ceil(u64::from(fps)),
            total_cs: ((range.len() * 100 + u64::from(fps) / 2) / u64::from(fps)).max(2),
        })
    }
    pub fn source_frame(&self, index: u64) -> Option<u64> {
        (index < self.frames).then(|| {
            if self.frames > 1 && index + 1 == self.frames {
                self.range.end - 1
            } else {
                self.range.start + index * u64::from(self.source_fps) / 15
            }
        })
    }
    fn boundary(&self, index: u64) -> u64 {
        if index == self.frames {
            self.total_cs
        } else {
            ((index * 100 + 7) / 15).min(self.total_cs.saturating_sub(2))
        }
    }
    pub fn delay_cs(&self, index: u64) -> Option<u16> {
        (index < self.frames).then(|| (self.boundary(index + 1) - self.boundary(index)) as u16)
    }
    /// Uniform representatives include both ends without retaining all pixels.
    pub fn palette_frames(&self) -> Vec<u64> {
        let count = self.frames.min(60);
        (0..count)
            .map(|i| {
                if count == 1 {
                    0
                } else {
                    i * (self.frames - 1) / (count - 1)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_short_tail_keeps_absolute_duration_and_frame_bounds() {
        for fps in [30, 60] {
            for len in 1..=fps as u64 * 60 {
                let p = Plan::new(
                    Range {
                        start: 17,
                        end: 17 + len,
                    },
                    4000,
                    fps,
                    1920,
                    1080,
                )
                .unwrap();
                assert_eq!((p.width, p.height), (960, 540));
                let delays: Vec<_> = (0..p.frames).map(|i| p.delay_cs(i).unwrap()).collect();
                assert!(delays.iter().all(|d| *d >= 2));
                let actual = delays.iter().map(|d| *d as f64 / 100.).sum::<f64>();
                assert!((actual - len as f64 / fps as f64).abs() < 0.010_001);
                assert_eq!(p.source_frame(0), Some(17));
                if p.frames > 1 {
                    assert_eq!(p.source_frame(p.frames - 1), Some(16 + len));
                }
                assert_eq!(p.source_frame(p.frames), None);
                assert!(p.palette_frames().len() <= 60);
            }
        }
    }
    #[test]
    fn dimensions_limits_and_single_frame_are_explicit() {
        assert_eq!(
            Plan::new(Range::full(3601), 3601, 60, 1920, 1080),
            Err(Rejection::TooLong)
        );
        let p = Plan::new(Range::full(1), 1, 60, 1, 9).unwrap();
        assert_eq!((p.width, p.height, p.delay_cs(0)), (1, 9, Some(2)));
        let p = Plan::new(Range::full(1800), 1800, 30, 101, 1000).unwrap();
        assert_eq!((p.width, p.height), (97, 960));
        assert_eq!(p.frames, 900);
        assert_eq!(
            Plan::new(Range::full(1), 1, 0, 1, 1),
            Err(Rejection::InvalidSource)
        );
    }
}
