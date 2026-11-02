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
}
impl Sources {
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
        for (source, stream) in [&self.system, &self.microphone]
            .into_iter()
            .zip(&mut self.streams)
        {
            if let Some(source) = source {
                for packet in source.drain()? {
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
