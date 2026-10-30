//! Gate-only MF encoder. All COM objects stay on the encoding MTA.
use super::fault::{Fault, Quota};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use windows::Win32::Foundation::E_FAIL;
use windows::{Win32::Media::MediaFoundation::*, core::*};

pub struct Encoder {
    _lease: super::resources::Lease,
    writer: IMFSinkWriter,
    stream: u32,
    pub identity: serde_json::Value,
    width: usize,
    height: usize,
    fault: Option<Fault>,
    armed: Arc<AtomicBool>,
    submitted: AtomicU64,
    failed_writes: Arc<AtomicU64>,
    // Fields drop in declaration order: release the sink writer before shutting
    // down the explicitly activated hardware transform and its factory.
    _selection: Option<super::hardware::Selection>,
}

pub struct Media;
impl Media {
    pub fn new() -> Result<Self> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        Ok(Self)
    }
}
impl Drop for Media {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

thread_local! {static STAGES:std::cell::RefCell<Vec<serde_json::Value>>=const{std::cell::RefCell::new(Vec::new())};}
pub fn take_stages() -> Vec<serde_json::Value> {
    STAGES.with(|stages| std::mem::take(&mut *stages.borrow_mut()))
}
pub(super) fn stage<T>(name: &str, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let at = Instant::now();
    let result = operation();
    let ms = at.elapsed().as_secs_f64() * 1000.;
    // Per-frame write timings already stream with the sample. Only bounded setup
    // and finalization diagnostics stay in this task-local scratch area.
    if name != "write_sample" {
        STAGES.with(|stages|{let mut stages=stages.borrow_mut();if stages.len()<16 {stages.push(serde_json::json!({"stage":name,"ms":ms,"hresult":result.as_ref().err().map(|e|e.code().0).unwrap_or(0)}));}});
    }
    result.map_err(|e| Error::new(e.code(), format!("{name}: elapsed_ms={ms:.3}; {e}")))
}

fn packed(a: u32, b: u32) -> u64 {
    (u64::from(a) << 32) | u64::from(b)
}

impl Encoder {
    pub fn new(
        path: &Path,
        w: u32,
        h: u32,
        fps: u32,
        hardware: bool,
        fault: Option<Fault>,
    ) -> Result<Self> {
        unsafe {
            let factory_before = super::hardware::factory_calls();
            let _selection = if hardware {
                super::hardware::Selection::from_environment()?
            } else {
                None
            };
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 5)?;
            let attrs = attrs.unwrap();
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware.into())?;
            attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
            // MF rejects DISABLE_CONVERTERS together with enabled hardware MFTs.
            if !hardware {
                attrs.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, 1)?;
            }
            let wide: Vec<_> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain([0])
                .collect();
            let armed = Arc::new(AtomicBool::new(false));
            let failed_writes = Arc::new(AtomicU64::new(0));
            let quota = if matches!(fault, Some(Fault::Write | Fault::Finalize)) {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|e| Error::new(E_FAIL, e.to_string()))?;
                let stream: windows::Win32::System::Com::IStream =
                    Quota::new(file, armed.clone(), failed_writes.clone()).into();
                Some(MFCreateMFByteStreamOnStream(&stream)?)
            } else {
                None
            };
            let writer = stage("create_sink_writer", || {
                MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), quota.as_ref(), &attrs)
            })?;
            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            output.SetUINT32(
                &MF_MT_AVG_BITRATE,
                (12_000_000u64 * u64::from(w) * u64::from(h) / (1920 * 1080)) as u32,
            )?;
            set_video(&output, w, h, fps)?;
            let stream = stage("set_output_type", || writer.AddStream(&output))?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            set_video(&input, w, h, fps)?;
            let mut encoding = None;
            MFCreateAttributes(&mut encoding, 8)?;
            let encoding = encoding.unwrap();
            for (key, value) in [
                (CODECAPI_AVEncVideoInputColorPrimaries, 2),
                (CODECAPI_AVEncVideoOutputColorPrimaries, 2),
                (CODECAPI_AVEncVideoInputColorTransferFunction, 5),
                (CODECAPI_AVEncVideoOutputColorTransferFunction, 5),
                (CODECAPI_AVEncVideoInputColorTransferMatrix, 1),
                (CODECAPI_AVEncVideoOutputColorTransferMatrix, 1),
                (CODECAPI_AVEncVideoInputColorNominalRange, 2),
                (CODECAPI_AVEncVideoOutputColorNominalRange, 2),
            ] {
                encoding.SetUINT32(&key, value)?;
            }
            stage("set_input_type", || {
                writer.SetInputMediaType(
                    stream,
                    &input,
                    if hardware { Some(&encoding) } else { None },
                )
            })?;
            stage("begin_writing", || writer.BeginWriting())?;
            if _selection.is_some() && super::hardware::factory_calls() == factory_before {
                return Err(Error::new(E_FAIL, "Explicit encoder factory was not used"));
            }
            let identity_probe = stage("encoder_identity", || -> Result<serde_json::Value> {
                let ex: IMFSinkWriterEx = writer.cast()?;
                let mut transform = None;
                ex.GetTransformForStream(stream, 0, None, &mut transform)?;
                let transform = transform.unwrap();
                let a = transform.GetAttributes()?;
                let mut name = [0u16; 256];
                let _ = a.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut name, None);
                let len = name.iter().position(|x| *x == 0).unwrap_or(name.len());
                let clsid = a.GetGUID(&MFT_TRANSFORM_CLSID_Attribute).ok().or_else(|| {
                    transform
                        .cast::<windows::Win32::System::Com::IPersist>()
                        .ok()
                        .and_then(|p| p.GetClassID().ok())
                });
                let hardware_url = a.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute).is_ok();
                use windows::Win32::{Foundation::HMODULE, System::LibraryLoader::*};
                let mut module = HMODULE::default();
                let mut module_name = [0u16; 512];
                let implementation_module = if GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    PCWSTR(transform.vtable().ProcessInput as *const u16),
                    &mut module,
                )
                .is_ok()
                {
                    let count = GetModuleFileNameW(Some(module), &mut module_name) as usize;
                    let full = String::from_utf16_lossy(&module_name[..count]);
                    full.rsplit('\\').next().unwrap_or("").to_string()
                } else {
                    String::new()
                };
                let identity = serde_json::json!({"name":String::from_utf16_lossy(&name[..len]),"clsid":format!("{clsid:?}"),"hardware_requested":hardware,"hardware_attribute":hardware_url,"low_latency_requested":true,"low_latency_attribute":a.GetUINT32(&MF_LOW_LATENCY).ok()});
                let mut identity = identity;
                identity["implementation_module"] = serde_json::json!(implementation_module);
                identity["selected_clsid"] =
                    serde_json::json!(std::env::var("LEXIFT_RECORDING_GATE_ENCODER").ok());
                identity["selected_device_index"] =
                    serde_json::json!(std::env::var("LEXIFT_RECORDING_GATE_DEVICE_INDEX").ok());
                identity["explicit_factory_calls"] =
                    serde_json::json!(super::hardware::factory_calls() - factory_before);
                identity["input_type"] = transform
                    .GetInputCurrentType(0)
                    .ok()
                    .map(|t| media_report(&t))
                    .unwrap_or(serde_json::Value::Null);
                identity["output_type"] = transform
                    .GetOutputCurrentType(0)
                    .ok()
                    .map(|t| media_report(&t))
                    .unwrap_or(serde_json::Value::Null);
                Ok(identity)
            });
            let mut identity=identity_probe.unwrap_or_else(|e|serde_json::json!({"hardware_requested":hardware,"identity_probe_error":e.to_string(),"identity_probe_hresult":e.code().0}));
            identity["setup_stages"] = serde_json::json!(take_stages());
            Ok(Self {
                _lease: super::resources::Lease::new(&super::resources::ENCODER),
                writer,
                stream,
                identity,
                width: w as usize,
                height: h as usize,
                fault,
                armed,
                submitted: AtomicU64::new(0),
                failed_writes,
                _selection,
            })
        }
    }

    pub fn write(&self, nv12: &[u8], pts: i64, duration: i64) -> Result<f64> {
        assert_eq!(nv12.len(), self.width * self.height * 3 / 2);
        let at = Instant::now();
        if matches!(self.fault, Some(Fault::Write))
            && self.submitted.fetch_add(1, Ordering::Relaxed) >= 10
        {
            self.armed.store(true, Ordering::Release);
        }
        unsafe {
            let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
            let mut dest = std::ptr::null_mut();
            buffer.Lock(&mut dest, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), dest, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(pts)?;
            sample.SetSampleDuration(duration)?;
            stage("write_sample", || {
                self.writer.WriteSample(self.stream, &sample)
            })?;
        }
        Ok(at.elapsed().as_secs_f64() * 1000.)
    }

    pub fn finish(self) -> Result<f64> {
        let at = Instant::now();
        unsafe {
            if matches!(self.fault, Some(Fault::Finalize)) {
                self.armed.store(true, Ordering::Release);
            }
            stage("finalize", || self.writer.Finalize()).map_err(|e| {
                Error::new(
                    e.code(),
                    format!(
                        "{e}; quota_failed_writes={}",
                        self.failed_writes.load(Ordering::Acquire)
                    ),
                )
            })?;
        }
        Ok(at.elapsed().as_secs_f64() * 1000.)
    }
}

fn media_report(t: &IMFMediaType) -> serde_json::Value {
    unsafe {
        serde_json::json!({"size":t.GetUINT64(&MF_MT_FRAME_SIZE).ok(),"frame_rate":t.GetUINT64(&MF_MT_FRAME_RATE).ok(),"bitrate":t.GetUINT32(&MF_MT_AVG_BITRATE).ok(),"profile":t.GetUINT32(&MF_MT_MPEG2_PROFILE).ok(),"range":t.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).ok(),"matrix":t.GetUINT32(&MF_MT_YUV_MATRIX).ok(),"primaries":t.GetUINT32(&MF_MT_VIDEO_PRIMARIES).ok(),"transfer":t.GetUINT32(&MF_MT_TRANSFER_FUNCTION).ok()})
    }
}

unsafe fn set_video(t: &IMFMediaType, w: u32, h: u32, fps: u32) -> Result<()> {
    unsafe {
        t.SetUINT64(&MF_MT_FRAME_SIZE, packed(w, h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, packed(fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, packed(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
    }
    Ok(())
}

/// Limited-range BT.709, averaging chroma over each 2x2 block before rounding.
/// Input is the opaque physical desktop in BGRA order, with no alpha unpremultiply.
pub fn nv12(bgra: &[u8], w: usize, h: usize, out: &mut [u8]) {
    assert!(w.is_multiple_of(2) && h.is_multiple_of(2));
    assert_eq!(bgra.len(), w * h * 4);
    assert_eq!(out.len(), w * h * 3 / 2);
    let (y, uv) = out.split_at_mut(w * h);
    for row in 0..h {
        for x in 0..w {
            let p = &bgra[(row * w + x) * 4..];
            y[row * w + x] = (16
                + ((47 * i32::from(p[2]) + 157 * i32::from(p[1]) + 16 * i32::from(p[0]) + 128)
                    >> 8))
                .clamp(16, 235) as u8;
        }
    }
    for row in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut r = 0;
            let mut g = 0;
            let mut b = 0;
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let p = &bgra[((row + dy) * w + x + dx) * 4..];
                r += i32::from(p[2]);
                g += i32::from(p[1]);
                b += i32::from(p[0]);
            }
            let offset = row / 2 * w + x;
            uv[offset] = (128 + ((-26 * r - 86 * g + 112 * b + 512) >> 10)).clamp(16, 240) as u8;
            uv[offset + 1] =
                (128 + ((112 * r - 102 * g - 10 * b + 512) >> 10)).clamp(16, 240) as u8;
        }
    }
}

#[test]
fn nv12_known_bt709_blocks_and_dimensions() {
    for (bgr, expected) in [
        ([0, 0, 0], [16, 128, 128]),
        ([255, 255, 255], [235, 128, 128]),
        ([0, 0, 255], [63, 102, 240]),
        ([255, 0, 0], [32, 240, 118]),
    ] {
        let src: Vec<_> = (0..4).flat_map(|_| [bgr[0], bgr[1], bgr[2], 255]).collect();
        let mut out = [0; 6];
        nv12(&src, 2, 2, &mut out);
        assert_eq!(&out[..4], &[expected[0]; 4]);
        assert_eq!(&out[4..], &expected[1..]);
    }
}
