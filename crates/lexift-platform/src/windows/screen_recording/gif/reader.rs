//! Source-frame selection after keyframe pre-roll; no audio or native FPS conversion.
use super::*;
use windows::Win32::Media::MediaFoundation::*;

pub(super) fn visit(
    path: &Path,
    info: &super::super::ArtifactInfo,
    plan: &Plan,
    indices: &[u64],
    cancel: &AtomicBool,
    mut consume: impl FnMut(usize, &[u8]) -> Result<()>,
) -> Result<usize> {
    cancelled(cancel)?;
    let mut sample_peak = 0;
    unsafe {
        use std::os::windows::ffi::OsStrExt;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1).map_err(err)?;
        let attrs = attrs.unwrap();
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(err)?;
        let name: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        let reader = MFCreateSourceReaderFromURL(PCWSTR(name.as_ptr()), &attrs).map_err(err)?;
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(err)?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        reader.SetStreamSelection(stream, true).map_err(err)?;
        let media = super::super::encoder::video_type(
            &MFVideoFormat_RGB32,
            plan.width,
            plan.height,
            info.video,
        )
        .map_err(err)?;
        // Encoded YUV remains BT.709 limited range; bitmap RGB must be full
        // range or blacks become 16 and whites 235 before GIF quantization.
        media
            .SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_0_255.0 as u32)
            .map_err(err)?;
        reader
            .SetCurrentMediaType(stream, None, &media)
            .map_err(err)?;
        let actual = reader.GetCurrentMediaType(stream).map_err(err)?;
        let size = actual.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
        let (width, height) = ((size >> 32) as usize, size as u32 as usize);
        if width < plan.width as usize
            || height < plan.height as usize
            || width - plan.width as usize >= 32
            || height - plan.height as usize >= 32
        {
            return Err(Error::new("GIF decoder rejected output geometry"));
        }
        let stride = actual
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .unwrap_or((width * 4) as u32) as i32;
        let pitch = stride.unsigned_abs() as usize;
        let length = pitch
            .checked_mul(height)
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or_else(|| Error::new("GIF decoder buffer budget exceeded"))?;
        if pitch < width * 4 {
            return Err(Error::new("Invalid GIF decoder stride"));
        }
        let mut pixels = buffer(plan.width as usize * plan.height as usize * 4)?;
        let start = Range::ticks(plan.range.start, plan.source_fps)
            .ok_or_else(|| Error::new("GIF timestamp overflow"))?;
        reader
            .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))
            .map_err(err)?;
        let mut wanted = 0;
        while wanted < indices.len() {
            cancelled(cancel)?;
            let (mut flags, mut time, mut sample) = (0, 0, None);
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
                .map_err(err)?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(Error::new("GIF video decode failed"));
            }
            if let Some(sample) = sample {
                let index = ((i128::from(time) * i128::from(plan.source_fps) + 5_000_000)
                    / 10_000_000)
                    .max(0) as u64;
                let target = plan
                    .source_frame(indices[wanted])
                    .ok_or_else(|| Error::new("Invalid GIF sample"))?;
                if index > target {
                    return Err(Error::new("GIF decoder skipped a selected frame"));
                }
                if index == target {
                    let buffer = sample.ConvertToContiguousBuffer().map_err(err)?;
                    let (mut data, mut bytes) = (std::ptr::null_mut(), 0);
                    buffer
                        .Lock(&mut data, None, Some(&mut bytes))
                        .map_err(err)?;
                    if (bytes as usize) < length {
                        let _ = buffer.Unlock();
                        return Err(Error::new("Truncated GIF video frame"));
                    }
                    sample_peak = sample_peak.max(bytes as usize);
                    let input = std::slice::from_raw_parts(data, bytes as usize);
                    let copied = copy_rows(
                        input,
                        (width, height),
                        stride,
                        (plan.width as usize, plan.height as usize),
                        &mut pixels,
                    );
                    let unlocked = buffer.Unlock().map_err(err);
                    copied?;
                    unlocked?;
                    cancelled(cancel)?;
                    consume(wanted, &pixels)?;
                    wanted += 1;
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 && wanted < indices.len() {
                return Err(Error::new("GIF video ended before the selected frame"));
            }
        }
    }
    Ok(sample_peak)
}

/// Contiguous MF RGB buffers can be bottom-up and include coded padding.
fn copy_rows(
    input: &[u8],
    coded: (usize, usize),
    stride: i32,
    visible: (usize, usize),
    output: &mut [u8],
) -> Result<()> {
    let pitch = stride.unsigned_abs() as usize;
    let size = pitch
        .checked_mul(coded.1)
        .ok_or_else(|| Error::new("Invalid GIF stride"))?;
    if visible.0 > coded.0
        || visible.1 > coded.1
        || pitch < coded.0 * 4
        || size > input.len()
        || output.len() != visible.0 * visible.1 * 4
    {
        return Err(Error::new("Invalid GIF RGB buffer"));
    }
    let bytes = visible.0 * 4;
    for y in 0..visible.1 {
        let row = if stride < 0 { coded.1 - 1 - y } else { y };
        output[y * bytes..(y + 1) * bytes]
            .copy_from_slice(&input[row * pitch..row * pitch + bytes]);
    }
    for p in output.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positive_negative_stride_padding_and_bounds_preserve_orientation() {
        let top = [1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 0];
        let bottom = [7, 8, 9, 0, 10, 11, 12, 0, 88, 88, 88, 0];
        let (mut a, mut b) = ([0; 16], [0; 16]);
        let positive = [top, bottom].concat();
        let negative = [bottom, top].concat();
        copy_rows(&positive, (3, 2), 12, (2, 2), &mut a).unwrap();
        copy_rows(&negative, (3, 2), -12, (2, 2), &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        assert!(copy_rows(&positive[..20], (3, 2), 12, (2, 2), &mut a).is_err());
        assert!(copy_rows(&positive, (3, 2), 8, (2, 2), &mut a).is_err());
    }
}
