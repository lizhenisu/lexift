//! Frame-server decoder. Pixels remain on the GPU outside explicit test readback.
use std::{
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Direct3D::*, Direct3D11::*, Dxgi::Common::*},
        Media::MediaFoundation::*,
        System::Com::*,
    },
    core::*,
};

#[implement(IMFMediaEngineNotify)]
struct Callback(SyncSender<(u32, usize, u32)>);
impl IMFMediaEngineNotify_Impl for Callback_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> Result<()> {
        // Engine notifications are not invoked on the owner thread. Never call
        // back into the engine or UI here; repetitive progress is polled boundedly.
        if event != MF_MEDIA_ENGINE_EVENT_TIMEUPDATE.0 as u32 {
            let _ = self.0.try_send((event, param1, param2));
        }
        Ok(())
    }
}
pub(super) struct Engine {
    #[cfg(test)]
    _lease: super::super::super::recording_gate::resources::Lease,
    pub media: IMFMediaEngine,
    pub marker_reached: bool,
    pub seeked: bool,
    pub frame_step_done: bool,
    reloading: bool,
    pub device: ID3D11Device,
    pub _context: ID3D11DeviceContext,
    pub texture: Option<ID3D11Texture2D>,
    pub size: (u32, u32),
    events: Receiver<(u32, usize, u32)>,
    _manager: IMFDXGIDeviceManager,
    _startup: super::super::encoder::Media,
}
impl Engine {
    pub fn open(path: &Path) -> Result<Self> {
        unsafe {
            let startup = super::super::encoder::Media::new()?;
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.unwrap();
            let context = context.unwrap();
            let multithread: ID3D11Multithread = context.cast()?;
            let _ = multithread.SetMultithreadProtected(true);
            let (mut token, mut manager) = (0, None);
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(&device, token)?;
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 4)?;
            let attributes = attributes.unwrap();
            let (tx, events) = mpsc::sync_channel(64);
            let callback: IMFMediaEngineNotify = Callback(tx).into();
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &callback)?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &manager)?;
            attributes.SetUINT32(
                &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            )?;
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let media = factory.CreateInstance(0, &attributes)?;
            // Establish Shutdown ownership before fallible configuration: releasing
            // a partially configured COM interface alone does not retire its work.
            let engine = Self {
                #[cfg(test)]
                _lease: super::super::super::recording_gate::resources::Lease::new(
                    &super::super::super::recording_gate::resources::PREVIEW_ENGINE,
                ),
                media,
                marker_reached: false,
                seeked: true,
                frame_step_done: true,
                reloading: false,
                device,
                _context: context,
                texture: None,
                size: (0, 0),
                events,
                _manager: manager,
                _startup: startup,
            };
            engine.media.SetAutoPlay(false)?;
            engine.media.SetLoop(false)?;
            engine.media.SetVolume(1.)?;
            engine
                .media
                .SetSource(&BSTR::from(path.to_string_lossy().as_ref()))?;
            engine.media.Load()?;
            Ok(engine)
        }
    }
    pub fn check_events(&mut self) -> Result<()> {
        while let Ok((event, _, code)) = self.events.try_recv() {
            if self.reloading && event == MF_MEDIA_ENGINE_EVENT_LOADEDDATA.0 as u32 {
                self.seeked = true;
                self.reloading = false;
            }
            if event == MF_MEDIA_ENGINE_EVENT_FRAMESTEPCOMPLETED.0 as u32 {
                self.frame_step_done = true;
            }
            if event == MF_MEDIA_ENGINE_EVENT_SEEKED.0 as u32 {
                self.seeked = true;
            }
            if event == MF_MEDIA_ENGINE_EVENT_TIMELINE_MARKER.0 as u32 {
                self.marker_reached = true;
            }
            if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
                return Err(Error::from_hresult(if code == 0 {
                    E_FAIL
                } else {
                    HRESULT(code as i32)
                }));
            }
        }
        Ok(())
    }
    pub fn seek(&mut self, position: f64) -> Result<()> {
        self.check_events()?;
        self.seeked = false;
        self.frame_step_done = true;
        unsafe {
            if position == 0. {
                // MF can settle one frame past zero after EOF and cannot
                // reliably step backward from there. Re-prime this same source
                // and wait for loaded data, preserving the immutable artifact.
                self.reloading = true;
                self.media.Load()
            } else {
                self.reloading = false;
                self.media
                    .cast::<IMFMediaEngineEx>()?
                    .SetCurrentTimeEx(position, MF_MEDIA_ENGINE_SEEK_MODE_NORMAL)
            }
        }
    }
    pub fn tick(&self) -> Result<Option<f64>> {
        unsafe {
            // windows-rs's generated Result<i64> erases S_FALSE. Inspect the raw
            // HRESULT so an absent frame cannot be mistaken for timestamp zero.
            let mut pts = 0;
            let status = (self.media.vtable().OnVideoStreamTick)(self.media.as_raw(), &mut pts);
            status.ok()?;
            if status != S_OK {
                return Ok(None);
            }
            Ok(Some(pts as f64 / 10_000_000.))
        }
    }
    #[cfg(test)]
    pub fn frame(&mut self) -> Result<Option<f64>> {
        let frame = self.tick()?;
        if frame.is_some() {
            self.accept_frame()?;
        }
        Ok(frame)
    }
    pub fn accept_frame(&mut self) -> Result<()> {
        unsafe {
            if self.texture.is_none() {
                let (mut w, mut h) = (0, 0);
                self.media.GetNativeVideoSize(Some(&mut w), Some(&mut h))?;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                    ..Default::default()
                };
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut self.texture))?;
                self.size = (w, h);
            }
            let texture = self.texture.as_ref().unwrap();
            self.media.TransferVideoFrame(
                texture,
                None,
                &RECT {
                    left: 0,
                    top: 0,
                    right: self.size.0 as i32,
                    bottom: self.size.1 as i32,
                },
                None,
            )?;
            Ok(())
        }
    }
    #[cfg(test)]
    pub fn read_pixel(&self, x: u32, y: u32) -> Result<[u8; 4]> {
        Ok(self.read_pixels(&[(x, y)])?[0])
    }
    #[cfg(test)]
    pub fn read_pixels(&self, points: &[(u32, u32)]) -> Result<Vec<[u8; 4]>> {
        self.read_pixels_from(self.texture.as_ref().unwrap(), points)
    }
    #[cfg(test)]
    pub fn read_pixels_from(
        &self,
        texture: &ID3D11Texture2D,
        points: &[(u32, u32)],
    ) -> Result<Vec<[u8; 4]>> {
        unsafe {
            let mut desc = Default::default();
            texture.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.unwrap();
            self._context.CopyResource(&staging, texture);
            let mut map = Default::default();
            self._context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let pixel = points
                .iter()
                .map(|&(x, y)| {
                    std::slice::from_raw_parts(
                        (map.pData as *const u8).add((y * map.RowPitch + x * 4) as usize),
                        4,
                    )
                    .try_into()
                    .unwrap()
                })
                .collect();
            self._context.Unmap(&staging, 0);
            Ok(pixel)
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = unsafe { self.media.Shutdown() };
    }
}
