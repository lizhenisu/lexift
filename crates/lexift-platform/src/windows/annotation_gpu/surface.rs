//! A non-activating GPU visual below the existing Slint input canvas.
use super::graphics::{Graphics, Texture};
use lexift_core::ports::annotation_render::RenderSurface;
use std::cell::Cell;
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, E_INVALIDARG, HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WAIT_TIMEOUT,
            WPARAM,
        },
        Graphics::{
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
                IDCompositionVisual,
            },
            Dxgi::{Common::*, *},
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{GetCurrentProcessId, WaitForSingleObject},
        },
        UI::WindowsAndMessaging::*,
    },
    core::{Interface, Result, w},
};

pub(super) struct Surface {
    #[cfg(test)]
    instance: u64,
    #[cfg(test)]
    qpc_frequency: Cell<i64>,
    pub spec: RenderSurface,
    pub target: Texture,
    swap: IDXGISwapChain1,
    composition: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    window: HWND,
    latency: LatencyHandle,
    frame_ready: Cell<bool>,
    shown: Cell<bool>,
    #[cfg(test)]
    submission_cpu: Cell<[std::time::Duration; 3]>,
    #[cfg(test)]
    native_commit: Cell<Option<super::display_timing::NativeCommit>>,
    #[cfg(test)]
    pub composition_probe: Cell<bool>,
}

/// The swap chain transfers ownership of its waitable handle to the caller.
struct LatencyHandle(HANDLE);
impl Drop for LatencyHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

impl Surface {
    pub fn new(graphics: &Graphics, spec: RenderSurface) -> Result<Self> {
        unsafe {
            let input = HWND(spec.input_window.0 as _);
            let mut process = 0;
            GetWindowThreadProcessId(input, Some(&mut process));
            if process != GetCurrentProcessId() || !IsWindow(Some(input)).as_bool() {
                return Err(windows::core::Error::from_hresult(E_INVALIDARG));
            }
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                lpszClassName: w!("Lexift.Annotation.GpuSurface"),
                ..Default::default()
            };
            // A worker/session can be recreated while the process owns the class.
            RegisterClassW(&class);
            let bounds = spec.bounds;
            let window = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                class.lpszClassName,
                w!("Lexift Annotation GPU"),
                WS_POPUP,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            // HTTRANSPARENT only searches windows on this thread. Layered
            // transparency lets User32 skip this visual across application
            // boundaries; constant alpha leaves composition pixels unchanged.
            let result = SetLayeredWindowAttributes(
                window,
                windows::Win32::Foundation::COLORREF(0),
                255,
                LWA_ALPHA,
            )
            .and_then(|()| super::super::annotation_overlay::mark(window))
            .and_then(|()| super::super::annotation_window_order::visual(window))
            .and_then(|()| Self::attach(graphics, spec, window));
            if result.is_err() {
                let _ = DestroyWindow(window);
            }
            result
        }
    }

    unsafe fn attach(graphics: &Graphics, spec: RenderSurface, window: HWND) -> Result<Self> {
        let device: IDXGIDevice = graphics.device.cast()?;
        let adapter = unsafe { device.GetAdapter()? };
        let factory: IDXGIFactory2 = unsafe { adapter.GetParent()? };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: (spec.bounds.right - spec.bounds.left) as u32,
            Height: (spec.bounds.bottom - spec.bounds.top) as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
        };
        let swap = unsafe { factory.CreateSwapChainForComposition(&graphics.device, &desc, None)? };
        let queue: IDXGISwapChain2 = swap.cast()?;
        unsafe {
            queue.SetMaximumFrameLatency(1)?;
        }
        let handle = unsafe { queue.GetFrameLatencyWaitableObject() };
        if handle.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        let latency = LatencyHandle(handle);
        let composition: IDCompositionDevice = unsafe { DCompositionCreateDevice(&device)? };
        let target = unsafe { composition.CreateTargetForHwnd(window, true)? };
        let visual = unsafe { composition.CreateVisual()? };
        unsafe {
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;
        }
        let texture = unsafe { swap.GetBuffer(0)? };
        let texture = Texture::wrap(&graphics.device, texture, false, true)?;
        Ok(Self {
            #[cfg(test)]
            instance: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            #[cfg(test)]
            qpc_frequency: Cell::new(0),
            spec,
            target: texture,
            swap,
            composition,
            _target: target,
            _visual: visual,
            window,
            latency,
            frame_ready: Cell::new(false),
            shown: Cell::new(false),
            #[cfg(test)]
            submission_cpu: Cell::new([std::time::Duration::ZERO; 3]),
            #[cfg(test)]
            native_commit: Cell::new(None),
            #[cfg(test)]
            composition_probe: Cell::new(false),
        })
    }

    /// Poll before touching the back buffer; retain the permit while another
    /// monitor is busy so a multi-monitor frame cannot consume it twice.
    pub fn ready(&self) -> Result<bool> {
        if self.frame_ready.get() {
            return Ok(true);
        }
        match unsafe { WaitForSingleObject(self.latency.0, 0) } {
            WAIT_OBJECT_0 => {
                self.frame_ready.set(true);
                Ok(true)
            }
            WAIT_TIMEOUT => Ok(false),
            _ => Err(windows::core::Error::from_thread()),
        }
    }

    #[cfg(test)]
    pub fn submitted_count(&self) -> Result<super::display_timing::Count> {
        Ok(super::display_timing::Count {
            chain: self.instance,
            present: unsafe { self.swap.GetLastPresentCount()? },
        })
    }
    #[cfg(test)]
    pub fn display_observation(&self) -> Result<super::display_timing::Observation> {
        let mut stats = DXGI_FRAME_STATISTICS::default();
        unsafe {
            self.swap.GetFrameStatistics(&mut stats)?;
        }
        let frequency = if self.qpc_frequency.get() > 0 {
            self.qpc_frequency.get()
        } else {
            let frequency = unsafe { self.composition.GetFrameStatistics()? }.timeFrequency;
            if frequency <= 0 {
                return Err(windows::core::Error::from_hresult(E_INVALIDARG));
            }
            self.qpc_frequency.set(frequency);
            frequency
        };
        Ok(super::display_timing::Observation {
            frequency,
            count: super::display_timing::Count {
                chain: self.instance,
                present: stats.PresentCount,
            },
            at: std::time::Instant::now(),
            qpc: stats.SyncQPCTime,
            present_refresh: stats.PresentRefreshCount,
            sync_refresh: stats.SyncRefreshCount,
            clock: Some(super::display_timing::ClockAnchor::sample()?),
        })
    }

    /// Probe native statistics separately from production submission. Success
    /// does not imply usable counters or photon timing on every desktop.
    #[cfg(test)]
    pub fn statistics_probe(&self) -> String {
        unsafe {
            let last = self.swap.GetLastPresentCount();
            let mut stats = DXGI_FRAME_STATISTICS::default();
            let regular = self.swap.GetFrameStatistics(&mut stats).map(|()| {
                (
                    stats.PresentCount,
                    stats.PresentRefreshCount,
                    stats.SyncRefreshCount,
                    stats.SyncQPCTime,
                )
            });
            let media = self.swap.cast::<IDXGISwapChainMedia>().and_then(|swap| {
                let mut stats = DXGI_FRAME_STATISTICS_MEDIA::default();
                swap.GetFrameStatisticsMedia(&mut stats).map(|()| {
                    (
                        stats.PresentCount,
                        stats.PresentRefreshCount,
                        stats.SyncRefreshCount,
                        stats.SyncQPCTime,
                    )
                })
            });
            format!("last={last:?};regular={regular:?};media={media:?}")
        }
    }

    #[cfg(test)]
    pub fn submission_cpu(&self) -> [std::time::Duration; 3] {
        self.submission_cpu.get()
    }

    /// Query actual native resources outside throughput measurement. A latency
    /// limit of one constrains DXGI queuing, not DWM or physical display latency.
    #[cfg(test)]
    pub fn verify_device_and_queue(&self, graphics: &Graphics) -> Result<()> {
        use windows::Win32::Graphics::Direct3D11::ID3D11Device;
        let same_identity = |device: ID3D11Device| -> Result<bool> {
            let actual: windows::core::IUnknown = device.cast()?;
            let expected: windows::core::IUnknown = graphics.device.cast()?;
            Ok(actual.as_raw() == expected.as_raw())
        };
        unsafe {
            assert!(
                same_identity(self.swap.GetDevice::<ID3D11Device>()?)?,
                "swap chain uses another D3D device"
            );
            assert!(
                same_identity(self.target._resource.GetDevice()?)?,
                "back buffer uses another D3D device"
            );
            assert!(
                same_identity(graphics.context.GetDevice()?)?,
                "immediate context uses another D3D device"
            );
            let desc = self.swap.GetDesc1()?;
            let queue: IDXGISwapChain2 = self.swap.cast()?;
            assert_eq!(queue.GetMaximumFrameLatency()?, 1);
            assert_eq!(desc.BufferCount, 2);
            assert_eq!(desc.SwapEffect, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL);
            assert_eq!(desc.AlphaMode, DXGI_ALPHA_MODE_PREMULTIPLIED);
            assert_ne!(
                desc.Flags & DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
                0
            );
            let thread = windows::Win32::System::Threading::GetCurrentThreadId();
            assert_eq!(
                GetWindowThreadProcessId(self.window, None),
                thread,
                "surface message pump belongs to another thread"
            );
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn native_commit(&self) -> Option<super::display_timing::NativeCommit> {
        self.native_commit.get()
    }

    pub fn present(&self) -> Result<bool> {
        #[cfg(test)]
        self.native_commit.set(None);
        #[cfg(test)]
        self.submission_cpu.set([std::time::Duration::ZERO; 3]);
        unsafe {
            #[cfg(test)]
            let composition_before_present = self
                .composition_probe
                .get()
                .then(|| super::composition_timing::Clock::query(&self.composition))
                .flatten();
            #[cfg(test)]
            let start = std::time::Instant::now();
            let result = self.swap.Present(0, DXGI_PRESENT_DO_NOT_WAIT);
            #[cfg(test)]
            self.submission_cpu.set([
                start.elapsed(),
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            ]);
            if result == DXGI_ERROR_WAS_STILL_DRAWING {
                return Ok(false);
            }
            result.ok()?;
            self.frame_ready.set(false);
            #[cfg(test)]
            let composition = self
                .composition_probe
                .get()
                .then(|| super::composition_timing::Clock::query(&self.composition))
                .flatten();
            #[cfg(test)]
            let start = std::time::Instant::now();
            self.composition.Commit()?;
            #[cfg(test)]
            self.native_commit
                .set(Some(super::display_timing::NativeCommit {
                    composition,
                    composition_before_present,
                    before: start,
                    after: std::time::Instant::now(),
                }));
            #[cfg(test)]
            self.submission_cpu.set([
                self.submission_cpu.get()[0],
                start.elapsed(),
                std::time::Duration::ZERO,
            ]);
            #[cfg(test)]
            let start = std::time::Instant::now();
            if !self.shown.get() {
                SetWindowPos(
                    self.window,
                    Some(HWND(self.spec.input_window.0 as _)),
                    self.spec.bounds.left,
                    self.spec.bounds.top,
                    self.spec.bounds.right - self.spec.bounds.left,
                    self.spec.bounds.bottom - self.spec.bounds.top,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                )?;
                self.shown.set(true);
            }
            #[cfg(test)]
            self.submission_cpu.set([
                self.submission_cpu.get()[0],
                self.submission_cpu.get()[1],
                start.elapsed(),
            ]);
        }
        Ok(true)
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}

pub(super) fn pump_messages() {
    let mut message = MSG::default();
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
