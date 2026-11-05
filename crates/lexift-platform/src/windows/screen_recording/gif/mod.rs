//! Two bounded decode passes and a single global palette, owned by one MTA.
mod reader;
use super::{encoder::Media, err};
use lexift_core::{
    Error, Result,
    domain::{recording_gif::Plan, recording_trim::Range},
};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use windows::{
    Win32::{
        Graphics::Imaging::*,
        System::Com::{StructuredStorage::*, *},
    },
    core::*,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Analyzing,
    Encoding,
    Finalizing,
}
pub(super) fn buffer(length: usize) -> Result<Vec<u8>> {
    if length > 64 * 1024 * 1024 {
        return Err(Error::new("GIF pixel buffer budget exceeded"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::new("GIF pixel buffer unavailable"))?;
    bytes.resize(length, 0);
    Ok(bytes)
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::new("GIF export cancelled"))
    } else {
        Ok(())
    }
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

/// The caller owns temporary-file cleanup and atomic publication.
pub(super) fn export(
    source: &Path,
    output: &Path,
    info: &super::ArtifactInfo,
    plan: &Plan,
    cancel: &AtomicBool,
    progress: impl Fn(Phase, u64, u64),
) -> Result<()> {
    cancelled(cancel)?;
    if Plan::new(
        plan.range,
        info.frames,
        info.video.frame_rate.get(),
        info.width,
        info.height,
    )
    .as_ref()
        != Ok(plan)
    {
        return Err(Error::new("Invalid GIF export plan"));
    }
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(err)?;
    let _apartment = Apartment;
    let _media = Media::new().map_err(err)?;
    let started = Instant::now();
    let rgba = plan.width as usize * plan.height as usize * 4;
    let sheet_upper = 96 * 96 * 4 * plan.palette_frames().len();
    let controlled_upper =
        16 * 1024 * 1024 + rgba * 2 + sheet_upper * 2 + 96 * 96 * 4 + rgba / 2 + 262144 + 16384;
    if controlled_upper > 64 * 1024 * 1024 {
        return Err(Error::new("GIF pixel buffer budget exceeded"));
    }
    let last = std::cell::Cell::new(None::<(Phase, Instant)>);
    let report = |phase, done, total| {
        if done == total
            || last.get().is_none_or(|(old, at)| {
                old != phase || at.elapsed() >= std::time::Duration::from_millis(100)
            })
        {
            last.set(Some((phase, Instant::now())));
            progress(phase, done, total);
        }
    };
    let (analysis_ms, encoding_ms, finalize_ms, measured_pixels) = unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        let representatives = plan.palette_frames();
        let mut sheet = buffer(96 * 96 * 4 * representatives.len())?;
        report(Phase::Analyzing, 0, representatives.len() as u64 + 1);
        let analysis_sample =
            reader::visit(source, info, plan, &representatives, cancel, |i, pixels| {
                let bitmap = factory
                    .CreateBitmapFromMemory(
                        plan.width,
                        plan.height,
                        &GUID_WICPixelFormat32bppBGRA,
                        plan.width * 4,
                        pixels,
                    )
                    .map_err(err)?;
                let scaler = factory.CreateBitmapScaler().map_err(err)?;
                let longest = plan.width.max(plan.height);
                let w = ((u64::from(plan.width) * 96 / u64::from(longest)).max(1)) as u32;
                let h = ((u64::from(plan.height) * 96 / u64::from(longest)).max(1)) as u32;
                scaler
                    .Initialize(&bitmap, w, h, WICBitmapInterpolationModeNearestNeighbor)
                    .map_err(err)?;
                let mut small = buffer((w * h * 4) as usize)?;
                scaler
                    .CopyPixels(std::ptr::null(), w * 4, &mut small)
                    .map_err(err)?;
                let begin = i * 96 * 96 * 4;
                for y in 0..h as usize {
                    cancelled(cancel)?;
                    let x = (96 - w as usize) / 2;
                    let to = begin + (y * 96 + x) * 4;
                    sheet[to..to + w as usize * 4]
                        .copy_from_slice(&small[y * w as usize * 4..(y + 1) * w as usize * 4]);
                }
                report(
                    Phase::Analyzing,
                    i as u64 + 1,
                    representatives.len() as u64 + 1,
                );
                Ok(())
            })?;
        let sample = factory
            .CreateBitmapFromMemory(
                96,
                (96 * representatives.len()) as u32,
                &GUID_WICPixelFormat32bppBGRA,
                96 * 4,
                &sheet,
            )
            .map_err(err)?;
        let palette = factory.CreatePalette().map_err(err)?;
        palette
            .InitializeFromBitmap(&sample, 254, false)
            .map_err(err)?;
        let mut colors = [0; 256];
        let mut count = 0;
        palette
            .GetColors(&mut colors[..254], &mut count)
            .map_err(err)?;
        colors[count as usize] = 0xff000000;
        colors[count as usize + 1] = 0xffffffff;
        palette
            .InitializeCustom(&colors[..count as usize + 2])
            .map_err(err)?;
        let rgb: Vec<_> = colors[..count as usize + 2]
            .iter()
            .map(|c| [(*c >> 16) as u8, (*c >> 8) as u8, *c as u8])
            .collect();
        let mapping = lexift_core::domain::recording_gif::palette::Mapping::new(&rgb, || {
            cancel.load(Ordering::Acquire)
        })?;
        report(
            Phase::Analyzing,
            representatives.len() as u64 + 1,
            representatives.len() as u64 + 1,
        );
        let measured_pixels =
            analysis_sample + rgba * 2 + sheet_upper + 96 * 96 * 4 + 262144 + 16384;
        let analysis_ms = started.elapsed().as_secs_f64() * 1000.;
        drop(sample);
        drop(sheet);
        cancelled(cancel)?;
        #[cfg(test)]
        let fault = Fault::new();
        #[cfg(test)]
        let stream: IStream = if fault.mode.is_some() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(output)
                .map_err(err)?;
            super::super::recording_gate::fault::Quota::new(
                file,
                fault.armed.clone(),
                fault.failed.clone(),
            )
            .into()
        } else {
            file_stream(&factory, output)?
        };
        #[cfg(not(test))]
        let stream = file_stream(&factory, output)?;
        let encoder = factory
            .CreateEncoder(&GUID_ContainerFormatGif, std::ptr::null())
            .map_err(err)?;
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .map_err(err)?;
        encoder.SetPalette(&palette).map_err(err)?;
        let metadata = encoder.GetMetadataQueryWriter().map_err(err)?;
        let app = InitPropVariantFromBuffer(b"NETSCAPE2.0".as_ptr().cast(), 11).map_err(err)?;
        let repeat =
            InitPropVariantFromBuffer([3u8, 1, 0, 0, 0].as_ptr().cast(), 5).map_err(err)?;
        metadata
            .SetMetadataByName(w!("/appext/Application"), &app)
            .map_err(err)?;
        metadata
            .SetMetadataByName(w!("/appext/Data"), &repeat)
            .map_err(err)?;
        let mut indexed = buffer((plan.width * plan.height) as usize)?;
        let indices: Vec<_> = (0..plan.frames).collect();
        report(Phase::Encoding, 0, plan.frames);
        let encoding_started = Instant::now();
        let encoding_sample = reader::visit(source, info, plan, &indices, cancel, |i, pixels| {
            for y in 0..plan.height as usize {
                cancelled(cancel)?;
                let width = plan.width as usize;
                mapping.row(
                    &pixels[y * width * 4..(y + 1) * width * 4],
                    y,
                    &mut indexed[y * width..(y + 1) * width],
                )?;
            }
            let (mut frame, mut options) = (None, None);
            encoder
                .CreateNewFrame(&mut frame, &mut options)
                .map_err(err)?;
            let frame = frame.ok_or_else(|| Error::new("GIF encoder did not create a frame"))?;
            frame.Initialize(options.as_ref()).map_err(err)?;
            frame.SetSize(plan.width, plan.height).map_err(err)?;
            let mut format = GUID_WICPixelFormat8bppIndexed;
            frame.SetPixelFormat(&mut format).map_err(err)?;
            if format != GUID_WICPixelFormat8bppIndexed {
                return Err(Error::new("GIF encoder rejected indexed pixels"));
            }
            frame
                .SetPalette(&palette)
                .map_err(|e| Error::new(format!("GIF frame palette: {e}")))?;
            let metadata = frame.GetMetadataQueryWriter().map_err(err)?;
            metadata
                .SetMetadataByName(
                    w!("/grctlext/Delay"),
                    &PROPVARIANT::from(plan.delay_cs(i as u64).unwrap()),
                )
                .map_err(err)?;
            metadata
                .SetMetadataByName(w!("/grctlext/Disposal"), &PROPVARIANT::from(1u8))
                .map_err(err)?;
            metadata
                .SetMetadataByName(w!("/grctlext/TransparencyFlag"), &PROPVARIANT::from(false))
                .map_err(err)?;
            #[cfg(test)]
            if fault.mode.as_deref() == Some("write") && i > 0 {
                fault.armed.store(true, Ordering::Release);
            }
            frame
                .WritePixels(plan.height, plan.width, &indexed)
                .map_err(|e| Error::new(format!("GIF indexed frame write: {e}")))?;
            frame.Commit().map_err(err)?;
            report(Phase::Encoding, i as u64 + 1, plan.frames);
            Ok(())
        })?;
        cancelled(cancel)?;
        let encoding_ms = encoding_started.elapsed().as_secs_f64() * 1000.;
        let measured_pixels =
            measured_pixels.max(encoding_sample + rgba + 2 * indexed.capacity() + 262144 + 16384);
        report(Phase::Finalizing, plan.frames, plan.frames);
        cancelled(cancel)?;
        #[cfg(test)]
        if fault.mode.as_deref() == Some("finalize") {
            fault.armed.store(true, Ordering::Release);
        }
        let finalize_started = Instant::now();
        encoder.Commit().map_err(err)?;
        let finalize_ms = finalize_started.elapsed().as_secs_f64() * 1000.;
        cancelled(cancel)?;
        (analysis_ms, encoding_ms, finalize_ms, measured_pixels)
    };
    tracing::debug!(
        frames = plan.frames,
        width = plan.width,
        height = plan.height,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.,
        analysis_ms,
        encoding_ms,
        finalize_ms,
        controlled_pixel_peak_bytes = measured_pixels,
        controlled_pixel_upper_bytes = controlled_upper,
        "GIF export sealed"
    );
    Ok(())
}

fn file_stream(factory: &IWICImagingFactory, path: &Path) -> Result<IStream> {
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let stream = factory.CreateStream().map_err(err)?;
        stream
            .InitializeFromFilename(PCWSTR(name.as_ptr()), 0x40000000)
            .map_err(err)?;
        stream.cast().map_err(err)
    }
}
#[cfg(test)]
struct Fault {
    mode: Option<String>,
    armed: std::sync::Arc<AtomicBool>,
    failed: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
#[cfg(test)]
impl Fault {
    fn new() -> Self {
        Self {
            mode: std::env::var("LEXIFT_GIF_FAULT").ok(),
            armed: Default::default(),
            failed: Default::default(),
        }
    }
}
#[cfg(test)]
impl Drop for Fault {
    fn drop(&mut self) {
        if let Some(mode) = &self.mode {
            eprintln!(
                "GIF_NATIVE_FAULT mode={mode} failed_writes={}",
                self.failed.load(Ordering::Acquire)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit owned MP4/GIF encode and independent decode gate"]
    fn native_gif_export_gate() {
        let value = |key| std::env::var(key).unwrap().parse::<u64>().unwrap();
        let fps = value("LEXIFT_GIF_FPS") as u32;
        let info = super::super::ArtifactInfo {
            video: lexift_core::domain::screen_recording::Video {
                frame_rate: if fps == 60 {
                    lexift_core::domain::screen_recording::FrameRate::Fps60
                } else {
                    lexift_core::domain::screen_recording::FrameRate::Fps30
                },
                quality: Default::default(),
            },
            artifact: lexift_core::domain::screen_recording::Artifact(1),
            width: value("LEXIFT_GIF_WIDTH") as u32,
            height: value("LEXIFT_GIF_HEIGHT") as u32,
            frames: value("LEXIFT_GIF_FRAMES"),
            duration: Default::default(),
            interrupted: None,
        };
        let plan = Plan::new(
            Range {
                start: value("LEXIFT_GIF_START"),
                end: value("LEXIFT_GIF_END"),
            },
            info.frames,
            fps,
            info.width,
            info.height,
        )
        .unwrap();
        let outcome = export(
            Path::new(&std::env::var_os("LEXIFT_GIF_SOURCE").unwrap()),
            Path::new(&std::env::var_os("LEXIFT_GIF_OUTPUT").unwrap()),
            &info,
            &plan,
            &AtomicBool::new(false),
            |stage, n, total| eprintln!("GIF_GATE {stage:?} {n}/{total}"),
        );
        if std::env::var_os("LEXIFT_GIF_FAULT").is_some() {
            assert!(outcome.is_err());
        } else {
            outcome.unwrap();
        }
        super::super::super::recording_gate::resources::assert_retired();
    }
    #[test]
    #[ignore = "explicit owned GIF cancellation and resource-retirement matrix"]
    fn native_gif_cancel_gate() {
        let source = std::path::PathBuf::from(std::env::var_os("LEXIFT_GIF_SOURCE").unwrap());
        let original = std::fs::read(&source).unwrap();
        let info = super::super::ArtifactInfo {
            video: lexift_core::domain::screen_recording::Video {
                frame_rate: lexift_core::domain::screen_recording::FrameRate::Fps60,
                quality: Default::default(),
            },
            artifact: lexift_core::domain::screen_recording::Artifact(1),
            width: 1920,
            height: 1080,
            frames: 360,
            duration: std::time::Duration::from_secs(6),
            interrupted: None,
        };
        let plan = Plan::new(
            Range {
                start: 17,
                end: 317,
            },
            360,
            60,
            1920,
            1080,
        )
        .unwrap();
        for round in 0..20 {
            let file = super::super::FileArtifact::new().unwrap();
            let path = file.path.clone();
            let cancel = AtomicBool::new(false);
            let when = std::cell::Cell::new(None);
            let result = export(
                &source,
                &path,
                &info,
                &plan,
                &cancel,
                |phase, done, total| {
                    let selected = match round % 5 {
                        0 => phase == Phase::Analyzing,
                        1 => phase == Phase::Analyzing && done >= total / 2,
                        2 => phase == Phase::Encoding,
                        3 => phase == Phase::Encoding && done >= total / 2,
                        _ => phase == Phase::Finalizing,
                    };
                    if selected && !cancel.swap(true, Ordering::AcqRel) {
                        when.set(Some(Instant::now()));
                    }
                },
            );
            assert!(result.is_err() && cancel.load(Ordering::Acquire));
            let elapsed = when.get().unwrap().elapsed().as_secs_f64() * 1000.;
            drop(file);
            assert!(!path.exists());
            super::super::super::recording_gate::resources::assert_retired();
            eprintln!("GIF_CANCEL round={round} cancel_to_retired_ms={elapsed}");
        }
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
}
