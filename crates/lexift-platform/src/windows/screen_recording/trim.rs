//! Exact, bounded transcoding of a sealed recording. All COM stays on this MTA.
use super::{
    encoder::{Encoder, Media},
    err,
};
use lexift_core::{
    Error, Result,
    domain::{recording_clock::Clock, recording_trim::Range},
    ports::screen_recording::{ArtifactInfo, ExportStage},
};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{Media::MediaFoundation::*, System::Com::*},
    core::*,
};
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::new("Recording export cancelled"))
    } else {
        Ok(())
    }
}
/// Decode pre-roll, select original frame indices and emit one new CFR timeline.
pub(super) fn export(
    source: &Path,
    output: &Path,
    info: &ArtifactInfo,
    range: Range,
    cancel: &AtomicBool,
    progress: impl Fn(ExportStage, u64, u64),
) -> Result<()> {
    if !range.valid(info.frames) {
        return Err(Error::new("Invalid recording trim range"));
    }
    cancelled(cancel)?;
    let started = Instant::now();
    let mut read_time = Duration::ZERO;
    let mut encode_time = Duration::ZERO;
    let mut audio_peak = 0usize;
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(err)?;
    let _apartment = Apartment;
    let _media = Media::new().map_err(err)?;
    let bytes = (info.width as usize)
        .checked_mul(info.height as usize)
        .and_then(|n| n.checked_mul(3))
        .map(|n| n / 2)
        .filter(|n| *n <= 16 * 1024 * 1024)
        .ok_or_else(|| Error::new("Recording export buffer budget exceeded"))?;
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        let path: Vec<_> = source.as_os_str().encode_wide().chain([0]).collect();
        let reader = MFCreateSourceReaderFromURL(PCWSTR(path.as_ptr()), None).map_err(err)?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(err)?;
        let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        reader.SetStreamSelection(video, true).map_err(err)?;
        reader
            .SetCurrentMediaType(
                video,
                None,
                &super::encoder::video_type(
                    &MFVideoFormat_NV12,
                    info.width,
                    info.height,
                    info.video,
                )
                .map_err(err)?,
            )
            .map_err(err)?;
        let has_audio = reader.GetNativeMediaType(audio, 0).is_ok();
        if has_audio {
            let kind = MFCreateMediaType().map_err(err)?;
            kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(err)?;
            kind.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)
                .map_err(err)?;
            kind.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)
                .map_err(err)?;
            kind.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(err)?;
            kind.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)
                .map_err(err)?;
            reader.SetStreamSelection(audio, true).map_err(err)?;
            reader
                .SetCurrentMediaType(audio, None, &kind)
                .map_err(err)?;
        }
        let fps = info.video.frame_rate.get();
        let begin =
            Range::ticks(range.start, fps).ok_or_else(|| Error::new("Recording time overflow"))?;
        // Source Reader seeks to preceding keyframes. Read and reject pre-roll.
        let position = windows::Win32::System::Com::StructuredStorage::PROPVARIANT::from(begin);
        reader
            .SetCurrentPosition(&GUID::zeroed(), &position)
            .map_err(err)?;
        let encoder =
            Encoder::new_av(output, info.width, info.height, has_audio, info.video).map_err(err)?;
        let clock = Clock::new(fps, Duration::ZERO)
            .ok_or_else(|| Error::new("Invalid recording frame rate"))?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(bytes)
            .map_err(|_| Error::new("Recording export buffer unavailable"))?;
        pixels.resize(bytes, 0u8);
        let (mut coded_width, mut coded_height) = (info.width as usize, info.height as usize);
        let mut decoded = Vec::new();
        let start_sample = ((u128::from(range.start) * 48000) / u128::from(fps)) as u64;
        let audio_length = ((u128::from(range.len()) * 48000) / u128::from(fps)) as u64;
        let mut audio_written = 0u64;
        let mut video_written = 0u64;
        let mut video_done = false;
        let mut audio_done = !has_audio;
        let mut notify = Instant::now();
        let zeros = vec![[0f32; 2]; 480];
        progress(ExportStage::Transcoding, 0, range.len());
        let (mut video_time, mut audio_time) = (i64::MIN, i64::MIN);
        while !video_done || !audio_done {
            cancelled(cancel)?;
            let mut stream = 0;
            let mut flags = 0;
            let mut timestamp = 0;
            let mut sample = None;
            let selected = if !video_done && (audio_done || video_time <= audio_time) {
                video
            } else {
                audio
            };
            let read_started = Instant::now();
            reader
                .ReadSample(
                    selected,
                    0,
                    Some(&mut stream),
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|e| Error::new(format!("Read recording sample: {e}")))?;
            read_time += read_started.elapsed();
            #[cfg(test)]
            if std::env::var_os("LEXIFT_TRIM_TRACE").is_some() {
                eprintln!(
                    "sample stream={stream} flags={flags} ts={timestamp} v={video_written} a={audio_written} vd={video_done} ad={audio_done}"
                );
            }
            let kind = reader
                .GetCurrentMediaType(stream)
                .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
                .map_err(err)?;
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                let t = reader.GetCurrentMediaType(stream).map_err(err)?;
                #[cfg(test)]
                eprintln!(
                    "trim negotiated stream={stream} kind={kind:?} subtype={:?} size={:?} channels={:?} rate={:?}",
                    t.GetGUID(&MF_MT_SUBTYPE),
                    t.GetUINT64(&MF_MT_FRAME_SIZE),
                    t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS),
                    t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                );
                let valid = if kind == MFMediaType_Video {
                    t.GetGUID(&MF_MT_SUBTYPE).map_err(err)? == MFVideoFormat_NV12 && {
                        let size = t.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
                        coded_width = (size >> 32) as usize;
                        coded_height = (size & 0xffff_ffff) as usize;
                        coded_width >= info.width as usize
                            && coded_width - (info.width as usize) < 32
                            && coded_height >= info.height as usize
                            && coded_height - (info.height as usize) < 32
                    }
                } else {
                    t.GetGUID(&MF_MT_SUBTYPE).map_err(err)? == MFAudioFormat_Float
                        && t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(err)? == 2
                        && t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).map_err(err)? == 48000
                };
                if !valid {
                    return Err(Error::new("Recording export media type changed"));
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if kind == MFMediaType_Video {
                    video_done = true;
                } else {
                    audio_done = true;
                }
                continue;
            }
            let Some(sample) = sample else { continue };
            let timestamp = sample.GetSampleTime().unwrap_or(timestamp);
            if kind == MFMediaType_Video {
                video_time = timestamp;
            } else {
                audio_time = timestamp;
            }
            if kind == MFMediaType_Video && !video_done {
                let index = ((i128::from(timestamp) * i128::from(fps) + 5_000_000) / 10_000_000)
                    .max(0) as u64;
                if index < range.start {
                    continue;
                }
                if index >= range.end {
                    video_done = true;
                    continue;
                }
                if index != range.start + video_written {
                    return Err(Error::new(
                        "Recording export has a missing or duplicate video frame",
                    ));
                }
                let coded_bytes = coded_width
                    .checked_mul(coded_height)
                    .and_then(|n| n.checked_mul(3))
                    .map(|n| n / 2)
                    .filter(|n| *n <= 16 * 1024 * 1024)
                    .ok_or_else(|| Error::new("Recording decode budget exceeded"))?;
                if decoded.len() != coded_bytes {
                    decoded = Vec::new();
                    decoded
                        .try_reserve_exact(coded_bytes)
                        .map_err(|_| Error::new("Recording export buffer unavailable"))?;
                    decoded.resize(coded_bytes, 0);
                }
                let buffer = sample.GetBufferByIndex(0).map_err(err)?;
                if let Ok(two) = buffer.cast::<IMF2DBuffer>() {
                    if two.GetContiguousLength().map_err(err)? as usize != coded_bytes {
                        return Err(Error::new("Unexpected recording frame geometry"));
                    }
                    two.ContiguousCopyTo(&mut decoded).map_err(err)?;
                } else {
                    let buffer = sample.ConvertToContiguousBuffer().map_err(err)?;
                    let mut data = std::ptr::null_mut();
                    let mut length = 0;
                    buffer
                        .Lock(&mut data, None, Some(&mut length))
                        .map_err(err)?;
                    if length as usize != coded_bytes {
                        let _ = buffer.Unlock();
                        return Err(Error::new("Unexpected recording frame stride"));
                    }
                    std::ptr::copy_nonoverlapping(data, decoded.as_mut_ptr(), coded_bytes);
                    buffer.Unlock().map_err(err)?;
                }
                let w = info.width as usize;
                let h = info.height as usize;
                for row in 0..h {
                    pixels[row * w..(row + 1) * w]
                        .copy_from_slice(&decoded[row * coded_width..row * coded_width + w]);
                }
                for row in 0..h / 2 {
                    let at = coded_width * coded_height + row * coded_width;
                    pixels[w * h + row * w..w * h + (row + 1) * w]
                        .copy_from_slice(&decoded[at..at + w]);
                }
                let (pts, duration) = clock
                    .sample(video_written)
                    .ok_or_else(|| Error::new("Recording time overflow"))?;
                let encode_started = Instant::now();
                encoder
                    .write(&pixels, pts, duration)
                    .map_err(|e| Error::new(format!("Encode selected frame: {e}")))?;
                encode_time += encode_started.elapsed();
                video_written += 1;
                if video_written == range.len() {
                    video_done = true;
                }
                if notify.elapsed() >= Duration::from_millis(100) {
                    progress(ExportStage::Transcoding, video_written, range.len());
                    notify = Instant::now();
                }
            } else if kind == MFMediaType_Audio && !audio_done {
                let buffer = sample.ConvertToContiguousBuffer().map_err(err)?;
                let mut data = std::ptr::null_mut();
                let mut length = 0;
                buffer
                    .Lock(&mut data, None, Some(&mut length))
                    .map_err(err)?;
                if length as usize > 192_000 || length % 8 != 0 {
                    let _ = buffer.Unlock();
                    return Err(Error::new("Recording export audio budget exceeded"));
                }
                let values = std::slice::from_raw_parts(data, length as usize);
                let count = values.len() / 8;
                let first =
                    ((i128::from(timestamp) * 48000 + 5_000_000) / 10_000_000).max(0) as u64;
                let from = first.max(start_sample + audio_written);
                let end = (first + count as u64).min(start_sample + audio_length);
                let mut pcm = Vec::new();
                if pcm.try_reserve_exact(count).is_err() {
                    let _ = buffer.Unlock();
                    return Err(Error::new("Recording export buffer unavailable"));
                }
                audio_peak = audio_peak.max(count * 12 + zeros.len() * 8);
                if end > from {
                    for index in from..end {
                        let offset = (index - first) as usize * 8;
                        let l = f32::from_le_bytes(values[offset..offset + 4].try_into().unwrap());
                        let r =
                            f32::from_le_bytes(values[offset + 4..offset + 8].try_into().unwrap());
                        pcm.push([l, r]);
                    }
                }
                buffer.Unlock().map_err(err)?;
                if end > from {
                    let gap = from.saturating_sub(start_sample + audio_written);
                    if gap > 4800 {
                        return Err(Error::new("Recording export audio discontinuity"));
                    }
                    while audio_written < from - start_sample {
                        cancelled(cancel)?;
                        let n = ((from - start_sample - audio_written) as usize).min(zeros.len());
                        encoder
                            .write_audio(&zeros[..n], audio_written)
                            .map_err(err)?;
                        audio_written += n as u64;
                    }
                    for block in pcm.chunks(24000) {
                        cancelled(cancel)?;
                        encoder.write_audio(block, audio_written).map_err(err)?;
                        audio_written += block.len() as u64;
                    }
                }
                if audio_written >= audio_length || first >= start_sample + audio_length {
                    audio_done = true;
                }
            }
        }
        if video_written != range.len() {
            return Err(Error::new(
                "Recording export ended before the last selected frame",
            ));
        }
        if has_audio {
            if audio_length.saturating_sub(audio_written) > 4800 {
                return Err(Error::new("Recording export audio ended early"));
            }
            while audio_written < audio_length {
                cancelled(cancel)?;
                let n = ((audio_length - audio_written) as usize).min(zeros.len());
                encoder
                    .write_audio(&zeros[..n], audio_written)
                    .map_err(err)?;
                audio_written += n as u64;
            }
        }
        cancelled(cancel)?;
        progress(ExportStage::Finalizing, range.len(), range.len());
        cancelled(cancel)?;
        let finalize_started = Instant::now();
        encoder
            .finish()
            .map_err(|e| Error::new(format!("Finalize selected recording: {e}")))?;
        cancelled(cancel)?;

        tracing::debug!(
            frames = video_written,
            audio_samples = audio_written,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
            read_ms = read_time.as_secs_f64() * 1000.,
            encode_ms = encode_time.as_secs_f64() * 1000.,
            finalize_ms = finalize_started.elapsed().as_secs_f64() * 1000.,
            owned_video_bytes = pixels.capacity() + decoded.capacity() + bytes,
            owned_audio_bytes = audio_peak,
            "Recording exact trim sealed"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit owned MP4 trim/decode validation"]
    fn native_exact_trim_gate() {
        let source = std::path::PathBuf::from(std::env::var_os("LEXIFT_TRIM_SOURCE").unwrap());
        let output = std::path::PathBuf::from(std::env::var_os("LEXIFT_TRIM_OUTPUT").unwrap());
        let value = |key: &str| std::env::var(key).unwrap().parse::<u64>().unwrap();
        let fps = value("LEXIFT_TRIM_FPS") as u32;
        let info = ArtifactInfo {
            video: lexift_core::domain::screen_recording::Video {
                frame_rate: if fps == 60 {
                    lexift_core::domain::screen_recording::FrameRate::Fps60
                } else {
                    lexift_core::domain::screen_recording::FrameRate::Fps30
                },
                quality: Default::default(),
            },
            artifact: lexift_core::domain::screen_recording::Artifact(1),
            width: value("LEXIFT_TRIM_WIDTH") as u32,
            height: value("LEXIFT_TRIM_HEIGHT") as u32,
            frames: value("LEXIFT_TRIM_FRAMES"),
            duration: Duration::ZERO,
            interrupted: None,
        };
        let range = Range {
            start: value("LEXIFT_TRIM_START"),
            end: value("LEXIFT_TRIM_END"),
        };
        let started = Instant::now();
        let cancel = AtomicBool::new(false);
        let outcome = export(&source, &output, &info, range, &cancel, |_, _, _| {});
        if std::env::var_os("LEXIFT_TRIM_EXPECT_ERROR").is_some() {
            assert!(outcome.is_err());
            if output.exists() {
                std::fs::remove_file(&output).unwrap();
            }
            super::super::super::recording_gate::resources::assert_retired();
            return;
        }
        outcome.unwrap();
        eprintln!(
            "Exact trim gate frames={} elapsed_ms={}",
            range.len(),
            started.elapsed().as_secs_f64() * 1000.
        );
    }
    #[test]
    #[ignore = "owned native cancellation and retirement"]
    fn native_trim_cancel_gate() {
        let source = std::path::PathBuf::from(std::env::var_os("LEXIFT_TRIM_SOURCE").unwrap());
        let before = std::fs::read(&source).unwrap();
        for round in 0..20 {
            let file = super::super::FileArtifact::new().unwrap();
            let path = file.path.clone();
            let info = ArtifactInfo {
                video: lexift_core::domain::screen_recording::Video {
                    frame_rate: lexift_core::domain::screen_recording::FrameRate::Fps60,
                    quality: Default::default(),
                },
                artifact: lexift_core::domain::screen_recording::Artifact(1),
                width: 1920,
                height: 1080,
                frames: 360,
                duration: Duration::from_secs(6),
                interrupted: None,
            };
            let cancel = AtomicBool::new(false);
            let when = std::cell::Cell::new(None);
            let percent = if round % 2 == 0 { 0 } else { 50 };
            let result = export(
                &source,
                &path,
                &info,
                Range {
                    start: 17,
                    end: 317,
                },
                &cancel,
                |_, done, total| {
                    if done * 100 >= total * percent && !cancel.swap(true, Ordering::AcqRel) {
                        when.set(Some(Instant::now()));
                    }
                },
            );
            assert!(result.is_err() && cancel.load(Ordering::Acquire));
            let elapsed = when.get().unwrap().elapsed().as_secs_f64() * 1000.;
            drop(file);
            assert!(!path.exists());
            super::super::super::recording_gate::resources::assert_retired();
            eprintln!("TRIM_CANCEL round={round} cancel_to_retired_ms={elapsed}");
        }
        assert_eq!(std::fs::read(source).unwrap(), before);
    }
}
