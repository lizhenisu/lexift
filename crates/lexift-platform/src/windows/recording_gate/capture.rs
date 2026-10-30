//! An isolated monitor stream, never the per-window annotation sampling compositor.
use super::super::screen_capture::{
    activation,
    arrival::{Notice, Subscription},
    capture::Apartment,
};
use lexift_core::domain::geometry::Rect;
use std::time::Instant;
use windows::{
    Graphics::{
        Capture::*,
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::*,
            Dxgi::{Common::*, IDXGIDevice},
            Gdi::*,
        },
        System::{
            Performance::*,
            WinRT::{Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop},
        },
        UI::HiDpi::*,
    },
    core::*,
};

pub fn qpc_100ns() -> Result<i64> {
    let mut counter = 0;
    let mut frequency = 0;
    unsafe {
        QueryPerformanceCounter(&mut counter)?;
        QueryPerformanceFrequency(&mut frequency)?;
    }
    Ok((i128::from(counter) * 10_000_000 / i128::from(frequency)) as i64)
}

pub struct Capture {
    closed: bool,
    _lease: super::resources::Lease,
    subscription: Option<Subscription>,
    _notice: std::sync::Arc<Notice>,
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    staging: ID3D11Texture2D,
    context: ID3D11DeviceContext,
    device: ID3D11Device,
    monitor: HMONITOR,
    monitor_rect: windows::Win32::Foundation::RECT,
    size: windows::Graphics::SizeInt32,
    roi: Rect,
    opened: i64,
    _apartment: Apartment,
    _dpi: Dpi,
}
struct Dpi(DPI_AWARENESS_CONTEXT);
impl Drop for Dpi {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
pub struct Timing {
    pub source_100ns: i64,
    pub received: Instant,
    pub read_ms: f64,
}

impl Capture {
    pub fn new(roi: Rect, notice: std::sync::Arc<Notice>) -> Result<Self> {
        let dpi = Dpi(unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        });
        if roi.right <= roi.left
            || roi.bottom <= roi.top
            || (roi.right - roi.left) % 2 != 0
            || (roi.bottom - roi.top) % 2 != 0
        {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        let apartment = Apartment::new()?;
        let rect = windows::Win32::Foundation::RECT {
            left: roi.left,
            top: roi.top,
            right: roi.right,
            bottom: roi.bottom,
        };
        let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if monitor.is_invalid()
            || !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool()
            || roi.left < info.rcMonitor.left
            || roi.top < info.rcMonitor.top
            || roi.right > info.rcMonitor.right
            || roi.bottom > info.rcMonitor.bottom
        {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.unwrap();
        let context = context.unwrap();
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };
        let size = item.Size()?;
        let pool =
            activation::create_pool(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        session.SetIsCursorCaptureEnabled(true)?;
        session.SetIsBorderRequired(false)?;
        if session.MinUpdateInterval().is_ok() {
            session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 0 })?;
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: (roi.right - roi.left) as u32,
            Height: (roi.bottom - roi.top) as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..Default::default()
        };
        let mut staging = None;
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
        }
        let subscription = Subscription::new(&pool, &notice, 0)?;
        let opened = qpc_100ns()?;
        session.StartCapture()?;
        Ok(Self {
            closed: false,
            _lease: super::resources::Lease::new(&super::resources::CAPTURE),
            subscription: Some(subscription),
            _notice: notice,
            session,
            pool,
            staging: staging.unwrap(),
            context,
            device,
            monitor,
            monitor_rect: info.rcMonitor,
            size,
            roi,
            opened,
            _apartment: apartment,
            _dpi: dpi,
        })
    }

    pub fn texture_bytes(&self) -> u64 {
        ((self.size.Width as u64 * self.size.Height as u64) * 2
            + (self.roi.right - self.roi.left) as u64 * (self.roi.bottom - self.roi.top) as u64)
            * 4
    }

    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.subscription.take();
        let session = self.session.Close();
        let pool = self.pool.Close();
        session.and(pool)
    }

    pub fn read(&self, out: &mut [u8], fresh_after: i64) -> Result<Option<Timing>> {
        // Re-arm the coalesced FrameArrived wake before draining. Otherwise only
        // the first callback wakes the worker and timeout polling limits real FPS.
        self._notice.take();
        unsafe {
            self.device.GetDeviceRemovedReason()?;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(self.monitor, &mut info) }.as_bool()
            || info.rcMonitor != self.monitor_rect
        {
            return Err(Error::new(
                windows::Win32::Foundation::E_FAIL,
                "Recording monitor layout changed",
            ));
        }
        let mut latest = None;
        for _ in 0..2 {
            match self.pool.TryGetNextFrame() {
                Ok(f) => {
                    if let Some(old) = latest.replace(f) {
                        let _: Result<()> = old.Close();
                    }
                }
                // WinRT's nullable frame is projected as an S_OK error by windows-rs.
                Err(e)
                    if e.code() == HRESULT(0)
                        || e.code() == windows::Win32::Foundation::E_BOUNDS =>
                {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        let Some(frame) = latest else { return Ok(None) };
        let result = (|| {
            let time = frame.SystemRelativeTime()?.Duration;
            if time < self.opened.max(fresh_after) {
                return Ok(None);
            }
            if frame.ContentSize()? != self.size {
                return Err(Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "Recording dimensions changed",
                ));
            }
            let received = Instant::now();
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
            let x = (self.roi.left - self.monitor_rect.left) as u32;
            let y = (self.roi.top - self.monitor_rect.top) as u32;
            let width = (self.roi.right - self.roi.left) as usize;
            let height = (self.roi.bottom - self.roi.top) as usize;
            assert_eq!(out.len(), width * height * 4);
            let area = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + width as u32,
                bottom: y + height as u32,
                back: 1,
            };
            unsafe {
                self.context.CopySubresourceRegion(
                    &self.staging,
                    0,
                    0,
                    0,
                    0,
                    &source,
                    0,
                    Some(&area),
                );
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context
                    .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                for row in 0..height {
                    std::ptr::copy_nonoverlapping(
                        mapped
                            .pData
                            .cast::<u8>()
                            .add(row * mapped.RowPitch as usize),
                        out.as_mut_ptr().add(row * width * 4),
                        width * 4,
                    );
                }
                self.context.Unmap(&self.staging, 0);
            }
            Ok(Some(Timing {
                source_100ns: time,
                received,
                read_ms: received.elapsed().as_secs_f64() * 1000.,
            }))
        })();
        let close = frame.Close();
        result.and_then(|v| close.map(|_| v))
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
