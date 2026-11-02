//! Apartment-local Media Foundation format conversion. Native device positions
//! remain in the device rate; converted output has its own 48 kHz sample count.
use std::mem::ManuallyDrop;
use windows::{
    Win32::{
        Foundation::*,
        Media::{Audio::WAVEFORMATEX, MediaFoundation::*},
        System::Com::*,
    },
    core::*,
};
pub(super) struct Converter {
    transform: IMFTransform,
    rate: u32,
}
impl Converter {
    pub unsafe fn new(format: *const WAVEFORMATEX) -> Result<Self> {
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CLSID_AudioResamplerMediaObject, None, CLSCTX_INPROC_SERVER)?;
            let input = MFCreateMediaType()?;
            MFInitMediaTypeFromWaveFormatEx(
                &input,
                format,
                std::mem::size_of::<WAVEFORMATEX>() as u32 + u32::from((*format).cbSize),
            )?;
            transform.SetInputType(0, &input, 0)?;
            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)?;
            output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)?;
            output.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 8)?;
            output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 384000)?;
            output.SetUINT32(&MF_MT_AUDIO_CHANNEL_MASK, 3)?;
            transform.SetOutputType(0, &output, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(Self {
                transform,
                rate: (*format).nSamplesPerSec,
            })
        }
    }
    pub fn reset(&self) -> Result<()> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
        }
    }
    pub fn input(&self, bytes: &[u8], frames: u32, qpc: u64) -> Result<Vec<(u64, Vec<[f32; 2]>)>> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(
                i64::try_from(qpc).map_err(|_| Error::from_hresult(E_INVALIDARG))?,
            )?;
            sample.SetSampleDuration(
                (u64::from(frames) * 10_000_000 / u64::from(self.rate)) as i64,
            )?;
            self.transform.ProcessInput(0, &sample, 0)?;
        }
        self.output()
    }
    pub fn finish(&self) -> Result<Vec<(u64, Vec<[f32; 2]>)>> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        self.output()
    }
    fn output(&self) -> Result<Vec<(u64, Vec<[f32; 2]>)>> {
        unsafe {
            let mut packets = Vec::new();
            let mut output_frames = 0usize;
            loop {
                let buffer = MFCreateMemoryBuffer(48000 * 8 / 2)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(sample.clone())),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let result = self.transform.ProcessOutput(0, &mut output, &mut 0);
                ManuallyDrop::drop(&mut output[0].pEvents);
                ManuallyDrop::drop(&mut output[0].pSample);
                if result
                    .as_ref()
                    .is_err_and(|e| e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT)
                {
                    break;
                }
                result?;
                let length = buffer.GetCurrentLength()? as usize;
                if length == 0 {
                    break;
                }
                if !length.is_multiple_of(8) {
                    return Err(Error::new(E_FAIL, "Invalid converted audio block"));
                }
                output_frames += length / 8;
                if output_frames > lexift_core::domain::recording_audio::MAX_QUEUE_FRAMES {
                    return Err(Error::new(
                        E_FAIL,
                        "Recording audio conversion exceeded 500ms",
                    ));
                }
                let time = sample.GetSampleTime()?;
                if time < 0 {
                    return Err(Error::new(E_FAIL, "Invalid converted audio timestamp"));
                }
                let mut pointer = std::ptr::null_mut();
                buffer.Lock(&mut pointer, None, None)?;
                let pcm = (0..length / 8)
                    .map(|i| {
                        std::array::from_fn(|c| {
                            pointer.add(i * 8 + c * 4).cast::<f32>().read_unaligned()
                        })
                    })
                    .collect();
                buffer.Unlock()?;
                packets.push((time as u64, pcm));
            }
            Ok(packets)
        }
    }
}
