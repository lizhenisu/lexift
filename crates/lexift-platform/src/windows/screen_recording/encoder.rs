//! Software-only Media Foundation encoder. COM references never leave this MTA.
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
    #[cfg(test)]
    _lease: super::super::recording_gate::resources::Lease,
}
fn video_type(subtype: &GUID, w: u32, h: u32) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        Ok(t)
    }
}
impl Encoder {
    pub fn new(path: &Path, w: u32, h: u32) -> Result<Self> {
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
            let output = video_type(&MFVideoFormat_H264, w, h)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            output.SetUINT32(
                &MF_MT_AVG_BITRATE,
                (12_000_000u64 * u64::from(w) * u64::from(h) / (1920 * 1080)) as u32,
            )?;
            let stream = writer.AddStream(&output)?;
            writer.SetInputMediaType(stream, &video_type(&MFVideoFormat_NV12, w, h)?, None)?;
            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                bytes: w as usize * h as usize * 3 / 2,
                #[cfg(test)]
                _lease: super::super::recording_gate::resources::Lease::new(
                    &super::super::recording_gate::resources::ENCODER,
                ),
            })
        }
    }
    pub fn write(&self, pixels: &[u8], pts: i64, duration: i64) -> Result<()> {
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
    pub fn finish(&self) -> Result<()> {
        unsafe { self.writer.Finalize() }
    }
}
