//! Apartment-owned shared-mode WASAPI endpoint. Initialization never records;
//! Start/Stop and every buffer operation remain on the creating source thread.
use lexift_core::domain::recording_audio::MAX_QUEUE_FRAMES;
use std::collections::VecDeque;
use windows::{
    Win32::{
        Foundation::*,
        Media::Audio::*,
        System::{Com::*, Threading::*},
    },
    core::*,
};

pub(super) struct Packet {
    pub silence_gap: bool,
    pub qpc: u64,
    pub position: u64,
    pub discontinuity: bool,
    pub frames: Vec<[f32; 2]>,
}
const MAX_NATIVE_PACKET_BYTES: u64 = 1024 * 1024;
// Two owned native-format copies plus bounded conversion/queue/transfer PCM
// for each endpoint; one encoder input block. MF internals are reported apart.
const _: () = assert!(
    2 * (2 * MAX_NATIVE_PACKET_BYTES as usize + 5 * MAX_QUEUE_FRAMES * 8) + MAX_QUEUE_FRAMES * 4
        <= lexift_core::domain::recording_audio::MAX_BUFFER_BYTES
);
struct NativePacket {
    bytes: Vec<u8>,
    count: u32,
    position: u64,
    qpc: u64,
    discontinuity: bool,
}
pub(super) struct Endpoint {
    pending_native: Option<(NativePacket, std::time::Instant)>,
    enumerator: IMMDeviceEnumerator,
    device: IMMDevice,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
    flow: EDataFlow,
    identity: String,
    started: bool,
    watch: Option<WatchRegistration>,
    converter: super::audio_convert::Converter,
    block_align: u16,
    native_previous: Option<(u64, u64)>,
    converted_position: u64,
    packets: VecDeque<Packet>,
    native_rate: u32,
    input_frames: u64,
    first_qpc: Option<u64>,
    clock_correction: f64,
    system: bool,
    pending_gap: bool,
    priming_packets: u8,
    previous_count: u32,
    priming_at: std::time::Instant,
    priming_trace: VecDeque<(u32, u64, u64, u32)>,
}
fn device_id(device: &IMMDevice) -> Result<String> {
    unsafe {
        let id = device.GetId()?;
        let value = id.to_string();
        CoTaskMemFree(Some(id.0.cast()));
        Ok(value?)
    }
}
impl Endpoint {
    pub fn prepare(system: bool) -> Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let flow = if system { eRender } else { eCapture };
            let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
            let identity = device_id(&device)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let block_align = (*format).nBlockAlign;
            let native_rate = (*format).nSamplesPerSec;
            if native_rate == 0 || block_align == 0 || (*format).cbSize > 256 {
                CoTaskMemFree(Some(format.cast()));
                return Err(Error::new(E_FAIL, "Unsupported recording audio format"));
            }
            let converter = super::audio_convert::Converter::new(format);
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | if system {
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                } else {
                    0
                };
            let initialized =
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, format, None);
            CoTaskMemFree(Some(format.cast()));
            let converter = converter?;
            initialized?;
            #[cfg(test)]
            {
                let clock: IAudioClock = client.GetService()?;
                eprintln!(
                    "native endpoint system={system} client_rate={native_rate} clock_frequency={} block_align={block_align}",
                    clock.GetFrequency()?
                );
            }
            let capture: IAudioCaptureClient = client.GetService()?;
            let event = CreateEventW(None, false, false, None)?;
            if let Err(error) = client.SetEventHandle(event) {
                let _ = CloseHandle(event);
                return Err(error);
            }
            Ok(Self {
                enumerator,
                device,
                client,
                capture,
                event,
                flow,
                identity,
                started: false,
                watch: None,
                converter,
                block_align,
                pending_native: None,
                native_previous: None,
                converted_position: 0,
                packets: VecDeque::new(),
                native_rate,
                input_frames: 0,
                first_qpc: None,
                clock_correction: 0.,
                system,
                pending_gap: false,
                priming_packets: 0,
                previous_count: 0,
                priming_at: std::time::Instant::now(),
                priming_trace: VecDeque::new(),
            })
        }
    }
    pub fn event(&self) -> HANDLE {
        self.event
    }
    /// A bounded idle watchdog validates the native stream without polling for
    /// data. Healthy loopback silence is allowed to have no packets at all.
    pub fn check_health(&self) -> Result<()> {
        if self
            .pending_native
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() > std::time::Duration::from_millis(50))
        {
            return Err(Error::new(E_FAIL, "Unconfirmed recording audio timestamp"));
        }
        unsafe {
            self.client.GetCurrentPadding()?;
        }
        Ok(())
    }
    pub fn watch(&mut self, changed: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<()> {
        let callback: IMMNotificationClient = Watcher {
            identity: self.identity.clone(),
            flow: self.flow,
            changed,
        }
        .into();
        unsafe {
            self.enumerator
                .RegisterEndpointNotificationCallback(&callback)?;
        }
        self.watch = Some(WatchRegistration {
            enumerator: self.enumerator.clone(),
            callback,
        });
        Ok(())
    }
    pub fn validate_device(&self) -> Result<()> {
        unsafe {
            if self.device.GetState()? != DEVICE_STATE_ACTIVE
                || device_id(
                    &self
                        .enumerator
                        .GetDefaultAudioEndpoint(self.flow, eConsole)?,
                )? != self.identity
            {
                return Err(Error::new(E_FAIL, "Recording audio endpoint changed"));
            }
        }
        Ok(())
    }
    pub fn start(&mut self) -> Result<()> {
        self.validate_device()?;
        self.priming_at = std::time::Instant::now();
        unsafe {
            self.client.Start()?;
        }
        self.started = true;
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.started {
            unsafe {
                self.client.Stop()?;
                self.client.Reset()?;
            }
            self.started = false;
            self.converter.reset()?;
            self.native_previous = None;
            self.pending_native = None;
            self.converted_position = 0;
            self.packets.clear();
            self.input_frames = 0;
            self.first_qpc = None;
            self.clock_correction = 0.;
            self.pending_gap = false;
            self.priming_packets = 0;
            self.priming_trace.clear();
            self.previous_count = 0;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<Vec<Packet>> {
        if !self.started {
            return Ok(vec![]);
        }
        unsafe {
            self.client.Stop()?;
        }
        let mut packets = Vec::new();
        loop {
            if let Some(packet) = self.read()? {
                packets.push(packet);
                continue;
            }
            if unsafe { self.capture.GetNextPacketSize()? } == 0 {
                break;
            }
        }
        if self.pending_native.is_some() {
            return Err(Error::new(
                E_FAIL,
                "Unconfirmed recording audio timestamp at stop",
            ));
        }
        for (_, frames) in self.converter.finish()? {
            if frames.is_empty() {
                continue;
            }
            let position = self.converted_position;
            let qpc = self.first_qpc.unwrap_or(0) as f64
                + position as f64 * 10_000_000. / 48_000.
                + self.clock_correction;
            self.converted_position += frames.len() as u64;
            packets.push(Packet {
                silence_gap: false,
                qpc: qpc.round() as u64,
                position,
                frames,
                discontinuity: false,
            });
        }
        self.stop()?;
        Ok(packets)
    }
    pub fn read(&mut self) -> Result<Option<Packet>> {
        unsafe {
            if let Some(packet) = self.packets.pop_front() {
                return Ok(Some(packet));
            }
            if self.capture.GetNextPacketSize()? == 0 {
                self.check_health()?;
                return Ok(None);
            }
            let mut data = std::ptr::null_mut();
            let (mut count, mut flags, mut position, mut qpc) = (0, 0, 0, 0);
            self.capture.GetBuffer(
                &mut data,
                &mut count,
                &mut flags,
                Some(&mut position),
                Some(&mut qpc),
            )?;
            let result = (|| {
                if u64::from(count) * u64::from(self.block_align) > MAX_NATIVE_PACKET_BYTES
                    || count > self.native_rate / 2
                    || flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0
                {
                    return Err(Error::new(
                        E_FAIL,
                        "Invalid recording audio packet timing or size",
                    ));
                }
                let bytes = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    vec![0; count as usize * self.block_align as usize]
                } else {
                    if data.is_null() {
                        return Err(Error::from_hresult(E_POINTER));
                    }
                    std::slice::from_raw_parts(data, count as usize * self.block_align as usize)
                        .to_vec()
                };
                Ok(bytes)
            })();
            let released = self.capture.ReleaseBuffer(count);
            let bytes = result.and_then(|bytes| released.map(|_| bytes))?;
            let discontinuity = flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
            // Shared-mode device/SRC startup may emit overlapping preroll.
            // None of it is admitted to the media stream. Require three stable
            // packet boundaries within the traversal budget, then keep the
            // ordinary discontinuity checks for the rest of this epoch.
            if self.priming_packets < 3 {
                if self.priming_trace.len() == 12 {
                    self.priming_trace.pop_front();
                }
                self.priming_trace.push_back((count, position, qpc, flags));
                if self.native_previous.is_none() {
                    self.priming_at = std::time::Instant::now();
                }
                if let Some((_, previous_qpc)) = self.native_previous {
                    let gap = qpc as f64
                        - previous_qpc as f64
                        - f64::from(self.previous_count) * 10_000_000.
                            / f64::from(self.native_rate);
                    self.priming_packets = if !discontinuity && gap.abs() <= 50_000. {
                        self.priming_packets + 1
                    } else {
                        0
                    };
                }
                self.native_previous = Some((position, qpc));
                self.previous_count = count;
                if self.priming_packets < 3 {
                    if self.priming_at.elapsed() > std::time::Duration::from_millis(80) {
                        return Err(Error::new(
                            E_FAIL,
                            format!(
                                "Recording audio startup clock did not stabilize: rate={}, elapsed_ms={}, packets={:?}",
                                self.native_rate,
                                self.priming_at.elapsed().as_millis(),
                                self.priming_trace
                            ),
                        ));
                    }
                    return Ok(None);
                }
                self.native_previous = None;
            }
            self.process_native(NativePacket {
                bytes,
                count,
                position,
                qpc,
                discontinuity,
            })?;
            Ok(self.packets.pop_front())
        }
    }
    fn accept_native(&mut self, packet: NativePacket) -> Result<()> {
        let NativePacket {
            bytes,
            count,
            position,
            qpc,
            discontinuity,
        } = packet;
        let had_previous = self.native_previous.is_some();
        if let Some((previous_position, previous_qpc)) = self.native_previous
            && (position <= previous_position
                || qpc <= previous_qpc
                || (discontinuity && !self.system))
        {
            return Err(Error::new(
                E_FAIL,
                format!(
                    "Native audio discontinuity: previous={previous_position}, actual={position}, flagged={discontinuity}"
                ),
            ));
        }
        self.native_previous = Some((position, qpc));
        self.previous_count = count;
        let first = *self.first_qpc.get_or_insert(qpc);
        let correction = qpc as f64
            - first as f64
            - self.input_frames as f64 * 10_000_000. / f64::from(self.native_rate);
        let jump = correction - self.clock_correction;
        if had_previous && discontinuity && jump <= 50_000. {
            return Err(Error::new(
                E_FAIL,
                "Unlocated recording audio discontinuity",
            ));
        }
        if self.system && jump > 50_000. {
            self.pending_gap = true;
        } else if jump.abs() > 50_000. {
            return Err(Error::new(
                E_FAIL,
                "Recording audio timestamp jumped by more than 5ms",
            ));
        }
        self.clock_correction = correction;
        self.input_frames += u64::from(count);
        for (_, frames) in self.converter.input(&bytes, count, qpc)? {
            if frames.len() > MAX_QUEUE_FRAMES {
                return Err(Error::new(E_FAIL, "Converted audio backlog exceeded 500ms"));
            }
            let position = self.converted_position;
            // Preserve the device's measured QPC drift: an MFT is allowed
            // to synthesize nominal-rate output timestamps from its first
            // input, which alone would hide a drifting hardware clock.
            let qpc = (first as f64 + position as f64 * 10_000_000. / 48_000. + correction).round();
            if qpc < 0. || qpc > u64::MAX as f64 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            self.converted_position += frames.len() as u64;
            self.packets.push_back(Packet {
                silence_gap: std::mem::take(&mut self.pending_gap),
                qpc: qpc as u64,
                position,
                discontinuity: false,
                frames,
            });
        }

        Ok(())
    }
    fn process_native(&mut self, packet: NativePacket) -> Result<()> {
        use lexift_core::domain::recording_audio::{PacketClock, isolated_timestamp};
        if let Some((mut pending, since)) = self.pending_native.take() {
            if since.elapsed() > std::time::Duration::from_millis(50) {
                return Err(Error::new(E_FAIL, "Unconfirmed recording audio timestamp"));
            }
            let (position, qpc) = self
                .native_previous
                .ok_or_else(|| Error::from_hresult(E_FAIL))?;
            let before = PacketClock {
                position,
                frames: self.previous_count,
                timestamp: qpc,
            };
            let suspect = PacketClock {
                position: pending.position,
                frames: pending.count,
                timestamp: pending.qpc,
            };
            let after = PacketClock {
                position: packet.position,
                frames: packet.count,
                timestamp: packet.qpc,
            };
            if !packet.discontinuity
                && let Some(corrected) =
                    isolated_timestamp(before, suspect, after, self.native_rate)
            {
                tracing::warn!(
                    system = self.system,
                    position = pending.position,
                    original_qpc = pending.qpc,
                    corrected_qpc = corrected,
                    "Recording isolated timestamp recovered from adjacent sample positions"
                );
                pending.qpc = corrected;
            } else {
                // A confirmed positive loopback idle interval retains the
                // existing silence-gap path. Negative/persistent errors fail.
                let expected_next = pending.qpc as f64
                    + f64::from(pending.count) * 10_000_000. / f64::from(self.native_rate);
                let original_jump = pending.qpc as f64
                    - qpc as f64
                    - f64::from(self.previous_count) * 10_000_000. / f64::from(self.native_rate);
                if !self.system
                    || original_jump <= 50_000.
                    || packet.discontinuity
                    || pending.position.checked_add(u64::from(pending.count))
                        != Some(packet.position)
                    || (packet.qpc as f64 - expected_next).abs() > 10_000.
                {
                    return Err(Error::new(
                        E_FAIL,
                        "Unlocated recording audio timestamp discontinuity",
                    ));
                }
            }
            self.accept_native(pending)?;
            return self.accept_native(packet);
        }
        if let Some((position, qpc)) = self.native_previous {
            let jump = packet.qpc as f64
                - qpc as f64
                - f64::from(self.previous_count) * 10_000_000. / f64::from(self.native_rate);
            if !packet.discontinuity
                && packet.qpc > qpc
                && position.checked_add(u64::from(self.previous_count)) == Some(packet.position)
                && packet.count <= self.native_rate / 50
                && self.previous_count <= self.native_rate / 50
                && jump.abs() > 50_000.
            {
                self.pending_native = Some((packet, std::time::Instant::now()));
                return Ok(());
            }
        }
        self.accept_native(packet)
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.watch.take();
        let _ = self.stop();
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

struct WatchRegistration {
    enumerator: IMMDeviceEnumerator,
    callback: IMMNotificationClient,
}
impl Drop for WatchRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.callback);
        }
    }
}
#[implement(IMMNotificationClient)]
struct Watcher {
    identity: String,
    flow: EDataFlow,
    changed: std::sync::Arc<dyn Fn() + Send + Sync>,
}
impl Watcher {
    fn owns(&self, id: &PCWSTR) -> bool {
        unsafe { id.to_string().is_ok_and(|id| id == self.identity) }
    }
}
impl IMMNotificationClient_Impl for Watcher_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, state: DEVICE_STATE) -> Result<()> {
        if self.owns(id) && state != DEVICE_STATE_ACTIVE {
            (self.changed)();
        }
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, id: &PCWSTR) -> Result<()> {
        if self.owns(id) {
            (self.changed)();
        }
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, id: &PCWSTR) -> Result<()> {
        if flow == self.flow && role == eConsole && !self.owns(id) {
            (self.changed)();
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, id: &PCWSTR, key: &PROPERTYKEY) -> Result<()> {
        if self.owns(id)
            && key.fmtid == PKEY_AudioEngine_DeviceFormat.fmtid
            && key.pid == PKEY_AudioEngine_DeviceFormat.pid
        {
            (self.changed)();
        }
        Ok(())
    }
}

#[cfg(test)]
mod timestamp_replay_tests {
    use super::*;
    #[test]
    #[ignore = "real endpoint format / synthetic PCM metadata replay; never starts recording"]
    fn isolated_qpc_observation_preserves_all_pcm_and_rejects_sustained_negative_offset() {
        let _apartment = super::super::super::screen_capture::capture::Apartment::new().unwrap();
        let _media = super::super::encoder::Media::new().unwrap();
        for recovered in [true, false] {
            let mut endpoint = Endpoint::prepare(true).unwrap();
            assert_eq!(
                endpoint.native_rate, 48000,
                "fixture requires this endpoint's 48kHz mix format"
            );
            let mut frames = 0;
            for (position, qpc) in [
                (0, 1_000_000),
                (480, 1_045_889),
                (960, if recovered { 1_200_233 } else { 1_145_889 }),
            ] {
                let result = endpoint.process_native(NativePacket {
                    bytes: vec![0; 480 * endpoint.block_align as usize],
                    count: 480,
                    position,
                    qpc,
                    discontinuity: false,
                });
                if !recovered && position == 960 {
                    assert!(result.is_err());
                    break;
                }
                result.unwrap();
                for packet in endpoint.packets.drain(..) {
                    assert!(
                        packet
                            .frames
                            .iter()
                            .all(|s| s[0].abs() < 0.000001 && s[1].abs() < 0.000001)
                    );
                    frames += packet.frames.len();
                }
                if position == 480 {
                    assert!(endpoint.pending_native.is_some());
                }
            }
            if recovered {
                assert!(endpoint.pending_native.is_none());
                for (_, tail) in endpoint.converter.finish().unwrap() {
                    frames += tail.len();
                }
                assert_eq!(
                    frames, 1440,
                    "metadata repair must not delete or duplicate PCM samples"
                );
            }
        }
    }
}
