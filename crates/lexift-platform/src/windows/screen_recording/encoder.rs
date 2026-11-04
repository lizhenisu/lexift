//! Software-only Media Foundation encoder. COM references never leave this MTA.
use lexift_core::domain::screen_recording::Video;
use std::path::Path;
use windows::{Win32::Media::MediaFoundation::*, core::*};

pub struct Media;
impl Media {
    pub fn new() -> Result<Self> {
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        }
        Ok(Self)
    }
}
impl Drop for Media {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

pub struct Encoder {
    writer: IMFSinkWriter,
    stream: u32,
    bytes: usize,
    audio: Option<u32>,
    #[cfg(test)]
    _lease: super::super::recording_gate::resources::Lease,
}
pub(super) fn video_type(subtype: &GUID, w: u32, h: u32, video: Video) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))?;
        t.SetUINT64(
            &MF_MT_FRAME_RATE,
            (u64::from(video.frame_rate.get()) << 32) | 1,
        )?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        Ok(t)
    }
}
fn output_type(w: u32, h: u32, video: Video) -> Result<IMFMediaType> {
    let t = video_type(&MFVideoFormat_H264, w, h, video)?;
    let bitrate = video
        .bitrate(w, h)
        .map_err(|_| Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG))?;
    unsafe {
        t.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
    }
    Ok(t)
}
/// Negotiate a software MFT without capture, BeginWriting or any output file.
pub fn validate(w: u32, h: u32, video: Video) -> Result<()> {
    unsafe {
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let mut list = std::ptr::null_mut();
        let mut count = 0;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut list,
            &mut count,
        )?;
        let activations: Vec<IMFActivate> = if count == 0 {
            vec![]
        } else {
            std::slice::from_raw_parts_mut(list, count as usize)
                .iter_mut()
                .filter_map(Option::take)
                .collect()
        };
        windows::Win32::System::Com::CoTaskMemFree(Some(list.cast()));
        let mut result = Err(Error::from_hresult(
            windows::Win32::Foundation::E_INVALIDARG,
        ));
        for activation in activations {
            result = (|| {
                let transform: IMFTransform = activation.ActivateObject()?;
                transform.SetOutputType(0, &output_type(w, h, video)?, 0)?;
                transform.SetInputType(0, &video_type(&MFVideoFormat_NV12, w, h, video)?, 0)?;
                Ok(())
            })();
            let _ = activation.ShutdownObject();
            if result.is_ok() {
                break;
            }
        }
        result
    }
}
impl Encoder {
    pub fn new_av(path: &Path, w: u32, h: u32, audio: bool, video: Video) -> Result<Self> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 3)?;
            let attrs = attrs.unwrap();
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 0)?;
            attrs.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)?;
            attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
            use std::os::windows::ffi::OsStrExt;
            let path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
            let writer = MFCreateSinkWriterFromURL(PCWSTR(path.as_ptr()), None, &attrs)?;
            let output = output_type(w, h, video)?;
            let stream = writer.AddStream(&output)?;
            writer.SetInputMediaType(
                stream,
                &video_type(&MFVideoFormat_NV12, w, h, video)?,
                None,
            )?;
            let audio = if audio {
                let output = MFCreateMediaType()?;
                output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
                output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
                output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
                let index = writer.AddStream(&output)?;
                let input = MFCreateMediaType()?;
                input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
                input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000)?;
                input.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
                writer.SetInputMediaType(index, &input, None)?;
                Some(index)
            } else {
                None
            };
            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                bytes: w as usize * h as usize * 3 / 2,
                audio,
                #[cfg(test)]
                _lease: super::super::recording_gate::resources::Lease::new(
                    &super::super::recording_gate::resources::ENCODER,
                ),
            })
        }
    }
    pub fn write(&self, pixels: &[u8], pts: i64, duration: i64) -> Result<()> {
        #[cfg(test)]
        if std::env::var("LEXIFT_RECORDING_TEST_ENCODER_FAULT").is_ok_and(|s| s == "write") {
            return Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
        #[cfg(test)]
        if let Ok(delay) = std::env::var("LEXIFT_RECORDING_TEST_SUBMIT_DELAY_MS") {
            let delay: u64 = delay.parse().expect("test submission delay");
            assert!(delay <= 100);
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        unsafe {
            if pixels.len() != self.bytes {
                return Err(Error::from_hresult(
                    windows::Win32::Foundation::E_INVALIDARG,
                ));
            }
            let buffer = MFCreateMemoryBuffer(self.bytes as u32)?;
            let mut destination = std::ptr::null_mut();
            buffer.Lock(&mut destination, None, None)?;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), destination, pixels.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(pixels.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration)?;
            self.writer.WriteSample(self.stream, &sample)
        }
    }
    pub fn encoded_samples(&self) -> Result<u64> {
        let mut statistics = MF_SINK_WRITER_STATISTICS {
            cb: std::mem::size_of::<MF_SINK_WRITER_STATISTICS>() as u32,
            ..Default::default()
        };
        unsafe {
            self.writer.GetStatistics(self.stream, &mut statistics)?;
        }
        Ok(statistics.qwNumSamplesEncoded)
    }
    pub fn write_audio(&self, samples: &[[f32; 2]], first_frame: u64) -> Result<()> {
        use lexift_core::domain::recording_audio::{MAX_QUEUE_FRAMES, sample_time};
        let stream = self
            .audio
            .ok_or_else(|| Error::new(windows::Win32::Foundation::E_FAIL, "No audio stream"))?;
        if samples.is_empty() || samples.len() > MAX_QUEUE_FRAMES {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        let end = first_frame
            .checked_add(samples.len() as u64)
            .and_then(sample_time)
            .ok_or_else(|| Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG))?;
        let start = sample_time(first_frame)
            .ok_or_else(|| Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG))?;
        unsafe {
            let bytes = (samples.len() * 4) as u32;
            let buffer = MFCreateMemoryBuffer(bytes)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            for (i, value) in samples.iter().flatten().enumerate() {
                let value = if value.is_finite() {
                    (*value * 32768.).round().clamp(-32768., 32767.) as i16
                } else {
                    0
                };
                std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), data.add(i * 2), 2);
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(start)?;
            sample.SetSampleDuration(end - start)?;
            self.writer.WriteSample(stream, &sample)
        }
    }
    pub fn finish(&self) -> Result<()> {
        #[cfg(test)]
        if std::env::var("LEXIFT_RECORDING_TEST_ENCODER_FAULT").is_ok_and(|s| s == "finalize") {
            return Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
        unsafe { self.writer.Finalize() }
    }
}
