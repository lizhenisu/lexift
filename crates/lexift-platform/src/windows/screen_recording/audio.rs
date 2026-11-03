//! Endpoint ownership and asynchronous preparation. Configuration acknowledges
//! only initialized devices, and never starts a capture stream.
use super::audio_source::Source;
use lexift_core::{
    Error, Result,
    domain::recording_audio::{Mode, SAMPLE_RATE, Stream, mix},
};

#[derive(Default)]
pub(super) struct Sources {
    pub mode: Mode,
    pub system: Option<Source>,
    pub microphone: Option<Source>,
    streams: [Stream; 2],
    origin: f64,
    segment_sample: u64,
    pub written: u64,
    system_silence_committed: Option<f64>,
}

impl Sources {
    /// Microphone Start acknowledges a running client before its clock has
    /// necessarily stabilized. Media may begin only after validated PCM exists.
    pub fn media_ready(&self) -> bool {
        !self.mode.microphone() || self.streams[1].covered_until().is_some()
    }
    pub fn devices(&self) -> lexift_core::domain::recording_audio::Devices {
        lexift_core::domain::recording_audio::Devices {
            system: self.system.as_ref().map(|s| s.token),
            microphone: self.microphone.as_ref().map(|s| s.token),
        }
    }
    pub fn prepare(&mut self, mode: Mode) -> Result<()> {
        if self.system.as_ref().is_some_and(|s| s.status().is_err()) {
            self.system.take();
        }
        if self
            .microphone
            .as_ref()
            .is_some_and(|s| s.status().is_err())
        {
            self.microphone.take();
        }
        if mode.system() && self.system.is_none() {
            self.system = Some(Source::prepare(true)?);
        }
        if mode.microphone() && self.microphone.is_none() {
            self.microphone = Some(Source::prepare(false)?);
        }
        Ok(())
    }
    pub fn prepared(&self, mode: Mode) -> Result<bool> {
        if (mode.system() && self.system.is_none())
            || (mode.microphone() && self.microphone.is_none())
        {
            return Err(Error::new("Recording audio unavailable"));
        }
        let mut ready = true;
        for source in [
            mode.system().then_some(self.system.as_ref()).flatten(),
            mode.microphone()
                .then_some(self.microphone.as_ref())
                .flatten(),
        ]
        .into_iter()
        .flatten()
        {
            ready &= source.status()?.0;
        }
        Ok(ready)
    }
    pub fn retain(&mut self, mode: Mode) {
        if !mode.system() {
            self.system.take();
        }
        if !mode.microphone() {
            self.microphone.take();
        }
    }
    pub fn start(&self) {
        for source in [&self.system, &self.microphone].into_iter().flatten() {
            source.start();
        }
    }
    pub fn clear_segment(&mut self) {
        for stream in &mut self.streams {
            stream.clear();
        }
        self.system_silence_committed = None;
    }
    pub fn retire_sources(&mut self) {
        self.system.take();
        self.microphone.take();
    }
    pub fn stop(&self) {
        for source in [&self.system, &self.microphone].into_iter().flatten() {
            source.stop();
        }
    }
    pub fn running(&self) -> Result<bool> {
        if (self.mode.system() && self.system.is_none())
            || (self.mode.microphone() && self.microphone.is_none())
        {
            return Err(Error::new("Recording audio unavailable"));
        }
        let mut running = true;
        for source in [&self.system, &self.microphone].into_iter().flatten() {
            running &= source.status()?.1;
        }
        Ok(running)
    }
    pub fn stopped(&self) -> Result<bool> {
        let mut stopped = true;
        for source in [&self.system, &self.microphone].into_iter().flatten() {
            stopped &= !source.status()?.1;
        }
        Ok(stopped)
    }
    pub fn segment(&mut self, qpc: i64, sample: u64) {
        self.origin = qpc as f64;
        self.segment_sample = sample;
    }
    pub fn collect(&mut self) -> Result<()> {
        for (index, (source, stream)) in [&self.system, &self.microphone]
            .into_iter()
            .zip(&mut self.streams)
            .enumerate()
        {
            if let Some(source) = source {
                for packet in source.drain()? {
                    if index == 0
                        && let Some(committed) = self.system_silence_committed
                    {
                        let late = ((committed - packet.qpc as f64) * f64::from(SAMPLE_RATE)
                            / 10_000_000.)
                            .ceil()
                            .max(0.) as usize;
                        if packet
                            .frames
                            .iter()
                            .take(late)
                            .any(|s| s.iter().any(|v| v.abs() > f32::EPSILON))
                        {
                            return Err(Error::new(
                                "Recording audio arrived after its committed silence interval",
                            ));
                        }
                    }
                    if packet.silence_gap {
                        stream.push_after_silence(packet.position, packet.qpc, packet.frames)?;
                        continue;
                    }
                    stream.push(
                        packet.position,
                        packet.qpc,
                        packet.frames,
                        packet.discontinuity,
                    )?;
                }
            }
        }
        Ok(())
    }
    /// Admit received PCM immediately. A valid idle loopback has no packets;
    /// keep a bounded 100ms reorder horizon before committing that silence.
    /// Microphone data must arrive, or the native watchdog interrupts capture.
    pub fn ready_count(&mut self, requested: usize, now_qpc: i64) -> usize {
        let mut target = self.written.saturating_add(requested as u64);
        for index in 0..2 {
            if !(if index == 0 {
                self.mode.system()
            } else {
                self.mode.microphone()
            }) {
                continue;
            }
            let coverage = self.streams[index]
                .covered_until()
                .unwrap_or(f64::NEG_INFINITY);
            let through = if index == 0 {
                coverage.max(now_qpc as f64 - 1_000_000.)
            } else {
                coverage
            };
            let samples = ((through - self.origin) * f64::from(SAMPLE_RATE) / 10_000_000.
                + self.segment_sample as f64
                - 1e-6)
                .ceil()
                .max(0.) as u64;
            target = target.min(samples);
        }
        let count = target.saturating_sub(self.written).min(requested as u64) as usize;
        if self.mode.system() && count > 0 {
            let last = self.origin
                + (self.written as f64 + count as f64 - 1. - self.segment_sample as f64)
                    * 10_000_000.
                    / f64::from(SAMPLE_RATE);
            if self.streams[0]
                .covered_until()
                .is_none_or(|end| last >= end)
            {
                self.system_silence_committed = Some(last);
            }
        }
        count
    }
    pub fn render(&mut self, count: usize) -> Result<Vec<[f32; 2]>> {
        let mut output = Vec::with_capacity(count);
        for sample in self.written..self.written + count as u64 {
            let qpc = self.origin
                + (sample as f64 - self.segment_sample as f64) * 10_000_000.
                    / f64::from(SAMPLE_RATE);
            output.push(mix(
                self.mode,
                self.streams[0].sample(qpc)?,
                self.streams[1].sample(qpc)?,
            ));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;
    #[test]
    fn rejected_start_retires_old_pcm_before_retry_and_never_reports_started() {
        use lexift_core::{
            domain::{geometry::Rect, screen_recording::Identity},
            ports::screen_recording::{Notice, PrepareRequest},
        };
        let identity = Identity {
            session: 31,
            operation: 2,
            request: 4,
        };
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 2,
            bottom: 2,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let mut job = super::super::Job::prepare(
            PrepareRequest {
                identity,
                bounds,
                monitor_bounds: bounds,
                excluded_windows: vec![],
            },
            std::sync::Arc::new(move |n| tx.send(n).unwrap()),
        );
        job.phase = super::super::Phase::Starting;
        job.audio.mode = Mode::Microphone;
        job.audio.streams[1]
            .push(0, 0, vec![[0.5; 2]; 480], false)
            .unwrap();
        assert!(job.audio.media_ready());
        job.audio_interrupted(Error::new("Injected startup clock failure"))
            .unwrap();
        assert!(matches!(job.phase, super::super::Phase::Ready));
        assert!(!job.audio.media_ready());
        assert!(job.encoder.is_none() && job.file.is_none());
        assert!(
            matches!(rx.recv().unwrap(), Notice::StartRejected { identity: got, .. } if got == identity)
        );
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn delayed_pcm_is_not_committed_as_silence() {
        let mut sources = Sources {
            mode: Mode::Microphone,
            ..Default::default()
        };
        let origin = 5_000_000_000_000;
        sources.segment(origin, 0);
        assert!(!sources.media_ready());
        assert_eq!(sources.ready_count(480, origin + 200_000), 0);
        sources.streams[1]
            .push(0, origin as u64, vec![[0.25; 2]; 480], false)
            .unwrap();
        assert!(sources.media_ready());
        assert_eq!(sources.ready_count(481, origin + 200_000), 480);
        assert_eq!(sources.render(480).unwrap(), vec![[0.25; 2]; 480]);
        sources.written = 480;
        assert_eq!(sources.ready_count(480, origin + 300_000), 0);
        sources.streams[1]
            .push(480, origin as u64 + 100_000, vec![[0.75; 2]; 480], false)
            .unwrap();
        assert_eq!(sources.ready_count(480, origin + 300_000), 480);
        assert_eq!(sources.render(480).unwrap(), vec![[0.75; 2]; 480]);
        sources.clear_segment();
        assert!(!sources.media_ready());
    }
    #[test]
    fn idle_loopback_has_bounded_reorder_horizon_and_microphone_does_not_fall_back() {
        let mut sources = Sources {
            mode: Mode::System,
            ..Default::default()
        };
        sources.segment(0, 0);
        assert_eq!(sources.ready_count(480, 999_999), 0);
        assert_eq!(sources.ready_count(480, 1_100_000), 480);
        assert!(sources.system_silence_committed.is_some());
        sources.mode = Mode::Mixed;
        assert_eq!(sources.ready_count(480, 100_000_000), 0);
        sources.clear_segment();
        assert!(sources.system_silence_committed.is_none());
    }
}
