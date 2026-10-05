use super::compose::{compose, intersection};
#[cfg(test)]
use super::crops::disjoint_crops;
use lexift_core::{
    domain::geometry::Rect,
    ports::screen_capture::{CapturedRegion, ScreenCaptureRequest},
};
use std::{collections::HashMap, mem::size_of};
use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, HWND, LPARAM, RECT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::*,
            Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
            Dxgi::IDXGIDevice,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
            RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                SetThreadDpiAwarenessContext,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{Interface, factory},
};

struct WindowInfo {
    token: isize,
    bounds: Rect,
    opacity: u8,
}

const CAPTURE_BUFFERS: i32 = 2;

/// Drain only the pool's capacity. A continuously producing source must not
/// keep the renderer here indefinitely; superseded frame leases drop at once.
fn latest_pending<T>(capacity: usize, mut next: impl FnMut() -> Option<T>) -> Option<T> {
    let mut latest = None;
    for _ in 0..capacity {
        let Some(frame) = next() else {
            break;
        };
        latest = Some(frame);
    }
    latest
}

/// EnumWindows is top-to-bottom. Invisible/cloaked windows must never contribute
/// retained frames; in particular, hidden popup captures otherwise leave ghosts.
fn windows(request: &ScreenCaptureRequest) -> windows::core::Result<Vec<WindowInfo>> {
    struct Enumeration<'a> {
        request: &'a ScreenCaptureRequest,
        windows: Vec<WindowInfo>,
    }
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
        let enumeration = unsafe { &mut *(data.0 as *mut Enumeration<'_>) };
        let token = hwnd.0 as isize;
        if enumeration
            .request
            .excluded_windows
            .iter()
            .any(|w| w.0 == token)
            || super::super::annotation_overlay::contains(hwnd)
            || !unsafe { IsWindowVisible(hwnd) }.as_bool()
            || unsafe { IsIconic(hwnd) }.as_bool()
        {
            return true.into();
        }
        let mut cloaked = 0u32;
        unsafe {
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut _ as _,
                size_of::<u32>() as u32,
            );
        }
        if cloaked != 0 {
            return true.into();
        }
        let mut rect = RECT::default();
        if unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut rect as *mut _ as _,
                size_of::<RECT>() as u32,
            )
        }
        .is_err()
        {
            return true.into();
        }
        let bounds = Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        if !enumeration
            .request
            .regions
            .iter()
            .any(|r| intersection(*r, bounds).is_some())
        {
            return true.into();
        }
        let mut opacity = 255;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
        if unsafe { GetLayeredWindowAttributes(hwnd, None, Some(&mut opacity), Some(&mut flags)) }
            .is_err()
            || !flags.contains(LWA_ALPHA)
        {
            opacity = 255;
        }
        enumeration.windows.push(WindowInfo {
            token,
            bounds,
            opacity,
        });
        true.into()
    }
    let mut enumeration = Enumeration {
        request,
        windows: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(collect), LPARAM(&mut enumeration as *mut _ as isize))?;
    }
    enumeration.windows.reverse();
    Ok(enumeration.windows)
}

/// Reference and isolated-source modes exist only for controlled comparisons.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum CompletionQueries {
    Disabled,
    Source(isize),
    #[default]
    All,
}
#[cfg(test)]
impl CompletionQueries {
    fn includes(self, token: isize) -> bool {
        match self {
            Self::Disabled => false,
            Self::Source(source) => source == token,
            Self::All => true,
        }
    }
}

pub(crate) struct Backend {
    notice: Option<std::sync::Arc<super::arrival::Notice>>,
    windows: HashMap<isize, WindowCapture>,
    change_kernel: Option<super::super::d3d_compare::Kernel>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
    interop: IGraphicsCaptureItemInterop,
    previous_dpi: DPI_AWARENESS_CONTEXT,
    gpu_windows: Vec<(isize, Rect, u8)>,
    crops: super::crops::Plan,
    gpu_changes: super::regions::ChangedRegions,
    #[cfg(test)]
    deferred_gpu_window: Option<isize>,
    #[cfg(test)]
    pub(crate) comparison_timing_source: Option<isize>,
    #[cfg(test)]
    pub(crate) completion_queries: CompletionQueries,
    #[cfg(test)]
    pub(crate) enumeration_probe: bool,
    #[cfg(test)]
    pub(crate) last_gpu_enumeration: Option<(std::time::Duration, usize)>,
    #[cfg(test)]
    pub(crate) last_gpu_enumeration_span: Option<(std::time::Instant, std::time::Instant)>,
    _apartment: Apartment,
}
pub(crate) struct Apartment;
impl Apartment {
    /// Balanced thread initialization, also used while a retained presentation
    /// outlives its capture backend. This guard owns no device or producer.
    pub(crate) fn new() -> windows::core::Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }

    /// Capability queries may run on an already initialized UI STA. Borrow that
    /// apartment without balancing its owner's initialization; workers remain MTA.
    pub(crate) fn for_query() -> windows::core::Result<Option<Self>> {
        match Self::new() {
            Ok(apartment) => Ok(Some(apartment)),
            Err(error) if error.code() == windows::Win32::Foundation::RPC_E_CHANGED_MODE => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}
impl Backend {
    pub(crate) fn set_notice(&mut self, notice: std::sync::Arc<super::arrival::Notice>) {
        assert!(
            self.windows.is_empty(),
            "install notifications before starting capture"
        );
        self.notice = Some(notice);
    }
    pub(crate) fn take_arrival(&self) -> bool {
        self.notice.as_ref().is_some_and(|notice| notice.take())
    }

    /// Stop source producers when the scene no longer samples the desktop.
    /// Keep the device alive for ordinary annotation presentation.
    pub(crate) fn stop_capture(&mut self) {
        self.windows.clear();
        self.gpu_windows.clear();
        self.crops.clear();
        self.gpu_changes.clear();
    }

    #[cfg(test)]
    pub(crate) fn take_diagnostics(&mut self) -> (Vec<super::diagnostics::Event>, usize) {
        let mut events = Vec::new();
        let mut dropped = 0;
        for source in self.windows.values_mut() {
            events.extend(source.diagnostics.drain());
            dropped += std::mem::take(&mut source.diagnostics.dropped);
        }
        if let Some(notice) = &self.notice {
            let (arrivals, lost) = notice.drain();
            events.extend(arrivals);
            dropped += lost;
        }
        (events, dropped)
    }

    #[cfg(test)]
    pub(crate) fn capture_count(&self) -> usize {
        self.windows.len()
    }

    pub(crate) fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub(crate) fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }
    pub(super) fn is_healthy(&self) -> bool {
        unsafe { self.device.GetDeviceRemovedReason().is_ok() }
    }
    pub(crate) fn new() -> windows::core::Result<Self> {
        super::runtime::keep_code_loaded()?;
        let apartment = Apartment::new()?;
        (|| {
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
            let dxgi: IDXGIDevice = device.cast()?;
            let winrt = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
            let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let previous_dpi =
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            Ok(Self {
                notice: None,
                windows: HashMap::new(),
                device,
                change_kernel: None,
                context: context.unwrap(),
                winrt,
                interop,
                previous_dpi,
                gpu_windows: Vec::new(),
                crops: Default::default(),
                gpu_changes: Default::default(),
                #[cfg(test)]
                deferred_gpu_window: None,
                #[cfg(test)]
                comparison_timing_source: None,
                #[cfg(test)]
                completion_queries: CompletionQueries::All,
                #[cfg(test)]
                enumeration_probe: false,
                #[cfg(test)]
                last_gpu_enumeration: None,
                #[cfg(test)]
                last_gpu_enumeration_span: None,
                _apartment: apartment,
            })
        })()
    }
    pub(super) fn sample(
        &mut self,
        request: &ScreenCaptureRequest,
    ) -> windows::core::Result<Option<Vec<CapturedRegion>>> {
        // A frame pool can simply stop delivering after device loss. Detect it
        // even when no new frame reaches readback, so cached pixels cannot hide it.
        unsafe {
            self.device.GetDeviceRemovedReason()?;
        }
        let visible = windows(request)?;
        self.crops.retain(&request.regions, |token| {
            visible.iter().any(|w| w.token == token)
        });
        self.windows
            .retain(|token, _| visible.iter().any(|w| w.token == *token));
        for info in &visible {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.windows.entry(info.token)
            {
                entry.insert(WindowCapture::new(
                    info.token,
                    &self.interop,
                    &self.winrt,
                    self.notice.as_ref(),
                )?);
            }
            let crops = self.crops.get(info.token, info.bounds);
            if let Err(error) = self.windows.get_mut(&info.token).unwrap().poll(
                info.bounds,
                crops,
                &self.device,
                &self.context,
                &self.winrt,
            ) {
                // Recreate a failed window session on the next sample; healthy
                // windows keep their textures and do not pay the restart cost.
                self.windows.remove(&info.token);
                return Err(error);
            }
        }
        if visible.is_empty()
            || visible
                .iter()
                .any(|w| self.windows[&w.token].pixels.is_none())
        {
            return Ok(None);
        }
        let layers: Vec<_> = visible
            .iter()
            .flat_map(|w| {
                self.windows[&w.token]
                    .pixels
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(move |p| (p, w.opacity))
            })
            .collect();
        Ok(Some(
            request
                .regions
                .iter()
                .map(|r| compose(*r, &layers))
                .collect(),
        ))
    }

    /// Retain clean WGC textures on this device; never allocate staging or map pixels.
    pub(crate) fn sample_gpu(
        &mut self,
        request: &ScreenCaptureRequest,
    ) -> windows::core::Result<Option<GpuCapture>> {
        #[cfg(test)]
        {
            self.last_gpu_enumeration = None;
            self.last_gpu_enumeration_span = None;
        }
        unsafe {
            self.device.GetDeviceRemovedReason()?;
        }
        if self.change_kernel.is_none() {
            self.change_kernel = Some(super::super::d3d_compare::Kernel::new(&self.device)?);
        }
        #[cfg(test)]
        let enumeration_start = self.enumeration_probe.then(std::time::Instant::now);
        let visible = windows(request)?;
        #[cfg(test)]
        {
            self.last_gpu_enumeration =
                enumeration_start.map(|start| (start.elapsed(), visible.len()));
            self.last_gpu_enumeration_span = enumeration_start
                .zip(self.last_gpu_enumeration)
                .map(|(start, (elapsed, _))| (start, start + elapsed));
        }
        self.crops.retain(&request.regions, |token| {
            visible.iter().any(|w| w.token == token)
        });
        self.windows
            .retain(|token, _| visible.iter().any(|w| w.token == *token));
        let signature: Vec<_> = visible
            .iter()
            .map(|w| (w.token, w.bounds, w.opacity))
            .collect();
        if self.gpu_windows != signature {
            self.gpu_changes
                .extend(self.gpu_windows.iter().map(|(_, bounds, _)| *bounds));
            self.gpu_changes
                .extend(visible.iter().map(|window| window.bounds));
        }
        self.gpu_windows = signature;
        let mut layers = Vec::with_capacity(visible.len());
        let mut complete = true;
        for info in &visible {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.windows.entry(info.token)
            {
                entry.insert(WindowCapture::new(
                    info.token,
                    &self.interop,
                    &self.winrt,
                    self.notice.as_ref(),
                )?);
            }
            let window = self.windows.get_mut(&info.token).unwrap();
            #[cfg(test)]
            {
                window.diagnostics.gpu_enabled = self.comparison_timing_source == Some(info.token);
                window.completion_event = self.completion_queries.includes(info.token);
            }
            let crops = self.crops.get(info.token, info.bounds);
            match window.update_texture(
                info.bounds,
                &self.device,
                &self.context,
                &self.winrt,
                self.change_kernel.as_ref(),
                crops,
            ) {
                Ok(updated) => {
                    if updated {
                        self.gpu_changes
                            .extend([window.changed_bounds.unwrap_or(info.bounds)]);
                    }
                }
                Err(error) => {
                    self.windows.remove(&info.token);
                    return Err(error);
                }
            }
            let Some(texture) = &window.texture else {
                complete = false;
                continue;
            };
            #[cfg(test)]
            if self.deferred_gpu_window == Some(info.token) {
                complete = false;
                continue;
            }
            layers.push(GpuCaptureLayer {
                bounds: Rect {
                    right: info
                        .bounds
                        .right
                        .min(info.bounds.left.saturating_add(window.size.Width)),
                    bottom: info
                        .bounds
                        .bottom
                        .min(info.bounds.top.saturating_add(window.size.Height)),
                    ..info.bounds
                },
                opacity: info.opacity,
                texture: texture.clone(),
                #[cfg(test)]
                source: window.diagnostics.published(),
            });
        }
        // Poll every source even when another lacks a frame. Texture comparison
        // acknowledges each source independently; transfer changes only with a
        // complete sample so earlier confirmations cannot disappear on retry.
        if !complete || layers.is_empty() {
            return Ok(None);
        }
        Ok(Some(GpuCapture {
            comparison_waiting: self
                .windows
                .values()
                .any(|window| window.comparison_waiting),
            changed_regions: self.gpu_changes.take(),
            layers,
        }))
    }
}

pub(crate) struct GpuCapture {
    pub comparison_waiting: bool,
    pub changed_regions: Vec<Rect>,
    pub layers: Vec<GpuCaptureLayer>,
}

pub(crate) struct GpuCaptureLayer {
    #[cfg(test)]
    pub source: Option<super::diagnostics::Published>,
    pub bounds: Rect,
    pub opacity: u8,
    pub texture: ID3D11Texture2D,
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.windows.clear();
        unsafe {
            SetThreadDpiAwarenessContext(self.previous_dpi);
        }
    }
}

struct WindowCapture {
    subscription: Option<super::arrival::Subscription>,
    #[cfg(test)]
    token: isize,
    #[cfg(test)]
    diagnostics: super::diagnostics::Source,
    #[cfg(test)]
    completion_event: bool,
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    size: windows::Graphics::SizeInt32,
    texture: Option<ID3D11Texture2D>,
    staging: Option<ID3D11Texture2D>,
    pixels: Option<Vec<CapturedRegion>>,
    last_bounds: Option<Rect>,
    last_crops: Vec<Rect>,
    comparison: Option<super::super::d3d_compare::Comparison>,
    gpu_crops: Vec<Rect>,
    changed_bounds: Option<Rect>,
    comparison_waiting: bool,
}
/// Returning a frame to the pool must happen on error paths as well. Copy the
/// surface into an owned texture before releasing it; the pool may then reuse it.
struct FrameLease(windows::Graphics::Capture::Direct3D11CaptureFrame);
impl Drop for FrameLease {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
impl WindowCapture {
    fn new(
        token: isize,
        interop: &IGraphicsCaptureItemInterop,
        device: &IDirect3DDevice,
        notice: Option<&std::sync::Arc<super::arrival::Notice>>,
    ) -> windows::core::Result<Self> {
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(HWND(token as _))? };
        let size = item.Size()?;
        let pool = super::activation::create_pool(
            device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            CAPTURE_BUFFERS,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        session.SetIsCursorCaptureEnabled(false)?;
        session.SetIsBorderRequired(false)?;
        // A 16ms WGC floor can quantize to three refreshes on a 144Hz output
        // (48fps), even with a 60Hz source. Keep latest-only sampling on our
        // worker; do not impose a second producer cadence on supported systems.
        // Older WGC interfaces retain their existing behavior.
        if session.MinUpdateInterval().is_ok()
            && let Err(error) =
                session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 0 })
        {
            tracing::debug!(%error, "WGC capture interval override unavailable");
        }
        let subscription = notice
            .map(|notice| super::arrival::Subscription::new(&pool, notice, token))
            .transpose()?;
        session.StartCapture()?;
        Ok(Self {
            subscription,
            #[cfg(test)]
            token,
            #[cfg(test)]
            diagnostics: Default::default(),
            #[cfg(test)]
            completion_event: false,
            session,
            pool,
            size,
            texture: None,
            staging: None,
            pixels: None,
            last_bounds: None,
            last_crops: Vec::new(),
            comparison: None,
            gpu_crops: Vec::new(),
            changed_bounds: None,
            comparison_waiting: false,
        })
    }
    fn poll(
        &mut self,
        bounds: Rect,
        crops: &[Rect],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        winrt: &IDirect3DDevice,
    ) -> windows::core::Result<()> {
        let changed = self.update_texture(bounds, device, context, winrt, None, crops)?;
        if !changed && self.pixels.is_some() && self.last_crops == crops {
            return Ok(());
        }
        if self.texture.is_none() {
            return Ok(());
        }
        let mut regions = self.pixels.take().unwrap_or_default();
        let mut count = 0;
        for &crop in crops {
            if count == regions.len() {
                regions.push(CapturedRegion {
                    bounds: crop,
                    pixels: Vec::new(),
                });
            }
            if self.read_crop(bounds, crop, device, context, &mut regions[count])? {
                count += 1;
            }
        }
        regions.truncate(count);
        self.last_crops = crops.to_vec();
        self.pixels = Some(regions);
        Ok(())
    }

    fn update_texture(
        &mut self,
        bounds: Rect,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        winrt: &IDirect3DDevice,
        detector: Option<&super::super::d3d_compare::Kernel>,
        crops: &[Rect],
    ) -> windows::core::Result<bool> {
        let mut changed = self.last_bounds != Some(bounds);
        self.comparison_waiting = false;
        self.changed_bounds = changed.then_some(bounds);
        self.last_bounds = Some(bounds);
        if detector.is_none() {
            self.comparison = None;
            #[cfg(test)]
            self.diagnostics.cancel();
        } else {
            let local: Vec<_> = crops
                .iter()
                .map(|crop| Rect {
                    left: crop.left.saturating_sub(bounds.left),
                    top: crop.top.saturating_sub(bounds.top),
                    right: crop.right.saturating_sub(bounds.left),
                    bottom: crop.bottom.saturating_sub(bounds.top),
                })
                .collect();
            if self.gpu_crops != local {
                // A full candidate snapshot may contain newer pixels outside the old ROI.
                // Publish it before changing policy; queued GPU copies precede later drawing.
                if let Some(comparison) = self
                    .comparison
                    .as_mut()
                    .filter(|comparison| comparison.candidate_fresh)
                {
                    comparison.publish_candidate(
                        self.texture.as_mut().expect("comparison has a baseline"),
                    );
                    #[cfg(test)]
                    self.diagnostics.publish_candidate();
                }
                self.comparison = None;
                #[cfg(test)]
                self.diagnostics.cancel();
                self.gpu_crops = local;
                self.changed_bounds = Some(bounds);
                changed = true;
            }
        }
        #[cfg(test)]
        self.diagnostics.poll_gpu(context)?;
        if let Some(comparison) = self
            .comparison
            .as_mut()
            .filter(|comparison| comparison.pending)
        {
            use super::super::d3d_compare::Outcome;
            let outcome = comparison.try_complete(context)?;
            #[cfg(test)]
            self.diagnostics.query_poll(comparison.query_poll.take());
            match outcome {
                Outcome::Pending => {
                    #[cfg(test)]
                    self.diagnostics
                        .record(self.token, super::diagnostics::Kind::Pending);
                    self.comparison_waiting = true;
                    return Ok(changed);
                }
                Outcome::Changed(extent) => {
                    #[cfg(test)]
                    self.diagnostics.confirm(self.token, true);
                    comparison.publish(
                        self.texture
                            .as_mut()
                            .expect("pending comparison has a baseline"),
                    );
                    if self.changed_bounds.is_none() {
                        self.changed_bounds = Some(Rect {
                            left: bounds.left.saturating_add(extent.left),
                            top: bounds.top.saturating_add(extent.top),
                            right: bounds.left.saturating_add(extent.right),
                            bottom: bounds.top.saturating_add(extent.bottom),
                        });
                    }
                    changed = true;
                }
                Outcome::Equal | Outcome::Idle => {
                    #[cfg(test)]
                    self.diagnostics.confirm(self.token, false);
                }
            }
        }
        if let Some(frame) = latest_pending(CAPTURE_BUFFERS as usize, || {
            self.pool.TryGetNextFrame().ok().map(FrameLease)
        }) {
            #[cfg(test)]
            self.diagnostics
                .frame(self.token, frame.0.SystemRelativeTime()?.Duration);
            let current = frame.0.ContentSize()?;
            if current.Width <= 0 || current.Height <= 0 {
                return Ok(false);
            }
            if current != self.size {
                drop(frame);
                self.texture = None;
                self.comparison = None;
                self.staging = None;
                self.pixels = None;
                self.size = current;
                self.pool.Recreate(
                    winrt,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    CAPTURE_BUFFERS,
                    current,
                )?;
                return Ok(false);
            }
            let access: IDirect3DDxgiInterfaceAccess = frame.0.Surface()?.cast()?;
            let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                source.GetDesc(&mut desc);
            }
            if let Some(texture) = &self.texture {
                let mut old = D3D11_TEXTURE2D_DESC::default();
                unsafe {
                    texture.GetDesc(&mut old);
                }
                if old.Width != desc.Width || old.Height != desc.Height {
                    self.texture = None;
                    self.comparison = None;
                }
            }
            if let Some(baseline) = self.texture.as_ref() {
                if let Some(detector) = detector {
                    if self.comparison.is_none() {
                        unsafe {
                            baseline.GetDesc(&mut desc);
                        }
                        let mut comparison =
                            super::super::d3d_compare::Comparison::new(device, baseline, &desc)?;
                        #[cfg(test)]
                        {
                            comparison.observe_query_polls = self.diagnostics.gpu_enabled;
                        }
                        #[cfg(not(test))]
                        comparison.enable_completion_event(device)?;
                        #[cfg(test)]
                        if self.completion_event {
                            comparison.enable_completion_event(device)?;
                        }
                        self.comparison = Some(comparison);
                    }
                    let comparison = self.comparison.as_mut().unwrap();
                    #[cfg(test)]
                    let mut trace = self.diagnostics.start_gpu(device, context)?;
                    unsafe {
                        context.CopyResource(&comparison.candidate, &source);
                    }
                    #[cfg(test)]
                    if let Some(trace) = &mut trace {
                        trace.mark(context, 1);
                    }
                    #[cfg(test)]
                    self.diagnostics.begin();
                    #[cfg(test)]
                    if let Some(mut trace) = trace {
                        detector.enqueue(context, comparison, &self.gpu_crops);
                        trace.mark(context, 2);
                        self.diagnostics.finish_gpu(context, trace);
                        unsafe {
                            context.Flush();
                        }
                    } else {
                        detector.begin(context, comparison, &self.gpu_crops);
                    }
                    #[cfg(not(test))]
                    detector.begin(context, comparison, &self.gpu_crops);
                    // The first poll must advertise the queued comparison too.
                    // Otherwise an unchanged baseline sleeps a full refresh period
                    // before the scheduler learns that confirmation is pending.
                    self.comparison_waiting = true;
                } else {
                    unsafe {
                        context.CopyResource(baseline, &source);
                    }
                    #[cfg(test)]
                    self.diagnostics.publish_current();
                    changed = true;
                }
            } else {
                desc.Usage = D3D11_USAGE_DEFAULT;
                desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                desc.CPUAccessFlags = 0;
                desc.MiscFlags = 0;
                unsafe {
                    device.CreateTexture2D(&desc, None, Some(&mut self.texture))?;
                    context.CopyResource(self.texture.as_ref().unwrap(), &source);
                }
                #[cfg(test)]
                self.diagnostics.publish_current();
                changed = true;
            }
        }
        Ok(changed)
    }

    fn read_crop(
        &mut self,
        bounds: Rect,
        crop: Rect,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        region: &mut CapturedRegion,
    ) -> windows::core::Result<bool> {
        let texture = self.texture.as_ref().unwrap();
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        let left = (crop.left.saturating_sub(bounds.left)).max(0) as u32;
        let top = (crop.top.saturating_sub(bounds.top)).max(0) as u32;
        let right = ((crop.right.saturating_sub(bounds.left)).max(0) as u32)
            .min(desc.Width)
            .min(self.size.Width as u32);
        let bottom = ((crop.bottom.saturating_sub(bounds.top)).max(0) as u32)
            .min(desc.Height)
            .min(self.size.Height as u32);
        if right <= left || bottom <= top {
            return Ok(false);
        }
        let (width, height) = (right - left, bottom - top);
        if let Some(staging) = &self.staging {
            let mut old = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                staging.GetDesc(&mut old);
            }
            if old.Width != width || old.Height != height {
                self.staging = None;
            }
        }
        if self.staging.is_none() {
            desc.Width = width;
            desc.Height = height;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            unsafe {
                device.CreateTexture2D(&desc, None, Some(&mut self.staging))?;
            }
        }
        let staging = self.staging.as_ref().unwrap();
        let source = D3D11_BOX {
            left,
            top,
            front: 0,
            right,
            bottom,
            back: 1,
        };
        unsafe {
            context.CopySubresourceRegion(staging, 0, 0, 0, 0, texture, 0, Some(&source));
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }
        region.bounds = Rect {
            left: bounds.left + left as i32,
            top: bounds.top + top as i32,
            right: bounds.left + right as i32,
            bottom: bounds.top + bottom as i32,
        };
        region
            .pixels
            .resize(width as usize * height as usize * 4, 0);
        for y in 0..height as usize {
            // Map's actual RowPitch is used; staging is recreated on every size change.
            let row = unsafe {
                std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    width as usize * 4,
                )
            };
            for (src, dst) in row.as_chunks::<4>().0.iter().zip(
                region.pixels[y * width as usize * 4..][..width as usize * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut(),
            ) {
                dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
            }
        }
        unsafe {
            context.Unmap(staging, 0);
        }
        Ok(true)
    }
}
impl Drop for WindowCapture {
    fn drop(&mut self) {
        self.subscription = None;
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "owned opaque toolbar and changing source; requires interactive Windows WGC"]
    fn excluded_toolbar_keeps_covered_source_pixels_live() {
        use lexift_core::ports::magnifier::WindowToken;
        use std::time::{Duration, Instant};
        use windows::Win32::{Foundation::COLORREF, Graphics::Gdi::*};
        use windows::core::w;
        struct Owned(HWND);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let mut backend = Backend::new().unwrap();
        let create = || unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Lexift owned toolbar coverage fixture"),
                WS_POPUP,
                400,
                400,
                80,
                60,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Owned(hwnd)
        };
        let paint = |hwnd, rgb: [u8; 3]| unsafe {
            let dc = GetDC(Some(hwnd));
            let brush = CreateSolidBrush(COLORREF(
                u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16),
            ));
            assert_ne!(
                FillRect(
                    dc,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: 80,
                        bottom: 60
                    },
                    brush
                ),
                0
            );
            let _ = DeleteObject(brush.into());
            ReleaseDC(Some(hwnd), dc);
            let _ = GdiFlush();
        };
        let source = create();
        let toolbar = create();
        paint(toolbar.0, [220, 0, 220]);
        let request = ScreenCaptureRequest {
            revision: 1,
            regions: vec![Rect {
                left: 400,
                top: 400,
                right: 480,
                bottom: 460,
            }],
            paused_regions: Vec::new(),
            excluded_windows: vec![WindowToken(toolbar.0.0 as isize)],
        };
        for expected in [[20, 180, 60, 255], [30, 60, 190, 255], [190, 60, 30, 255]] {
            paint(source.0, [expected[0], expected[1], expected[2]]);
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut observed = None;
            loop {
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                // STATIC can receive its initial paint after ShowWindow; keep
                // the owned source publication alive while awaiting WGC.
                paint(source.0, [expected[0], expected[1], expected[2]]);
                paint(toolbar.0, [220, 0, 220]);
                if let Some(frame) = backend.sample(&request).unwrap() {
                    let pixels = frame[0].pixels.as_chunks::<4>().0;
                    observed = Some(pixels[30 * 80 + 40]);
                    if (10..50).all(|y| (10..70).all(|x| pixels[y * 80 + x] == expected)) {
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "covered source failed to update without capturing toolbar pixels: expected {expected:?}, observed {observed:?}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    #[ignore = "opens owned native windows; requires an interactive Windows desktop"]
    fn own_normal_window_is_captured_but_annotation_visuals_and_explicit_tokens_are_not() {
        use lexift_core::ports::magnifier::WindowToken;
        use std::time::{Duration, Instant};
        use windows::core::w;
        struct Owned(HWND);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let create = |name| {
            Owned(unsafe {
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    name,
                    WS_POPUP,
                    400,
                    400,
                    80,
                    60,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                hwnd
            })
        };
        // Backend establishes physical desktop coordinates before creating test windows.
        let mut backend = Backend::new().unwrap();
        let notifications = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = notifications.clone();
        let notice = super::super::arrival::Notice::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        backend.set_notice(notice.clone());
        let source = create(w!("Lexift Owned Ordinary Capture Test"));
        let visual = create(w!("Lexift Owned Annotation Capture Test"));
        super::super::super::annotation_overlay::mark(visual.0).unwrap();
        let mut request = ScreenCaptureRequest {
            revision: 1,
            regions: vec![Rect {
                left: 400,
                top: 400,
                right: 480,
                bottom: 460,
            }],
            paused_regions: Vec::new(),
            excluded_windows: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let visible = loop {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let _ = windows::Win32::Graphics::Dwm::DwmFlush();
            }
            let visible = windows(&request).unwrap();
            if visible
                .iter()
                .any(|window| window.token == source.0.0 as isize)
            {
                break visible;
            }
            if Instant::now() >= deadline {
                let mut rect = RECT::default();
                let mut cloaked = 0u32;
                let (bounds, cloak) = unsafe {
                    (
                        DwmGetWindowAttribute(
                            source.0,
                            DWMWA_EXTENDED_FRAME_BOUNDS,
                            &mut rect as *mut _ as _,
                            size_of::<RECT>() as u32,
                        ),
                        DwmGetWindowAttribute(
                            source.0,
                            DWMWA_CLOAKED,
                            &mut cloaked as *mut _ as _,
                            size_of::<u32>() as u32,
                        ),
                    )
                };
                panic!(
                    "owned source enumeration: visible={} iconic={} marked={} bounds={bounds:?} ({},{},{},{}) cloak={cloak:?}/{cloaked}",
                    unsafe { IsWindowVisible(source.0) }.as_bool(),
                    unsafe { IsIconic(source.0) }.as_bool(),
                    super::super::super::annotation_overlay::contains(source.0),
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.bottom
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            visible
                .iter()
                .any(|window| window.token == source.0.0 as isize)
        );
        assert!(
            !visible
                .iter()
                .any(|window| window.token == visual.0.0 as isize)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let _ = backend.sample_gpu(&request).unwrap();
            if backend
                .windows
                .get(&(source.0.0 as isize))
                .is_some_and(|window| window.texture.is_some())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "same-process source did not produce a GPU texture"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        eprintln!(
            "owned_capture_min_update_interval_100ns={:?}",
            backend.windows[&(source.0.0 as isize)]
                .session
                .MinUpdateInterval()
                .map(|interval| interval.Duration)
        );
        let notification_deadline = Instant::now() + Duration::from_secs(3);
        while !backend.take_arrival() {
            assert!(
                Instant::now() < notification_deadline,
                "WGC source did not wake its backend"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!backend.windows.contains_key(&(visual.0.0 as isize)));
        // A newly visible source can lag behind already-running sources. Keep
        // polling all windows and retain both pixel and topology invalidations
        // until the delayed source permits a complete GPU sample.
        let delayed = create(w!("Lexift Owned Delayed Capture Test"));
        backend.deferred_gpu_window = Some(delayed.0.0 as isize);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(backend.sample_gpu(&request).unwrap().is_none());
            if [source.0, delayed.0].iter().all(|hwnd| {
                backend
                    .windows
                    .get(&(hwnd.0 as isize))
                    .is_some_and(|window| window.texture.is_some())
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "a delayed source stopped other source polling"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            use windows::Win32::Graphics::Gdi::*;
            let dc = GetDC(Some(source.0));
            let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00_50_c0_20));
            FillRect(
                dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: 80,
                    bottom: 60,
                },
                brush,
            );
            let _ = DeleteObject(brush.into());
            ReleaseDC(Some(source.0), dc);
            assert!(GdiFlush().as_bool());
        }
        // Let WGC publish/confirm the single paint while sample delivery stays
        // blocked. No additional source paint occurs when delivery resumes.
        let mut observed_pending = false;
        for _ in 0..20 {
            assert!(backend.sample_gpu(&request).unwrap().is_none());
            for window in backend.windows.values() {
                if window
                    .comparison
                    .as_ref()
                    .is_some_and(|comparison| comparison.pending)
                {
                    observed_pending = true;
                    assert!(
                        window.comparison_waiting,
                        "newly queued comparison did not request a short retry"
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            observed_pending,
            "fixture did not queue a source comparison"
        );
        let pending = backend.gpu_changes.as_slice().to_vec();
        assert!(
            !pending.is_empty(),
            "incomplete samples discarded invalidations"
        );
        backend.deferred_gpu_window = None;
        let delivered = loop {
            if let Some(frame) = backend.sample_gpu(&request).unwrap() {
                break frame;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            pending
                .iter()
                .all(|region| delivered.changed_regions.iter().any(|delivered| {
                    delivered.left <= region.left
                        && delivered.top <= region.top
                        && delivered.right >= region.right
                        && delivered.bottom >= region.bottom
                })),
            "complete sample did not transfer retained invalidations"
        );
        assert!(backend.gpu_changes.as_slice().is_empty());
        request
            .excluded_windows
            .push(WindowToken(source.0.0 as isize));
        let _ = backend.sample_gpu(&request).unwrap();
        assert!(!backend.windows.contains_key(&(source.0.0 as isize)));
        backend.stop_capture();
        std::thread::sleep(Duration::from_millis(30));
        let stopped = notifications.load(std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            notifications.load(std::sync::atomic::Ordering::Relaxed),
            stopped,
            "closed WGC source retained its callback"
        );
        assert_eq!(backend.capture_count(), 0);
        assert!(backend.gpu_changes.as_slice().is_empty());
    }

    #[test]
    fn capture_drops_superseded_frames_and_never_drains_an_unbounded_producer() {
        use std::{cell::RefCell, rc::Rc};
        struct Lease(usize, Rc<RefCell<Vec<usize>>>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.1.borrow_mut().push(self.0);
            }
        }
        let released = Rc::new(RefCell::new(Vec::new()));
        let mut calls = 0;
        let latest = latest_pending(2, || {
            calls += 1;
            Some(Lease(calls, released.clone()))
        })
        .unwrap();
        assert_eq!(
            calls, 2,
            "rendering must get a turn even under capture pressure"
        );
        assert_eq!(latest.0, 2);
        assert_eq!(*released.borrow(), [1]);
        drop(latest);
        assert_eq!(*released.borrow(), [1, 2]);
        let mut pending = [Some(7), None].into_iter();
        assert_eq!(latest_pending(2, || pending.next().flatten()), Some(7));
    }

    #[test]
    fn separate_crops_cover_exact_union_and_preserve_translucent_composition() {
        let bounds = Rect {
            left: -20,
            top: -10,
            right: 20,
            bottom: 10,
        };
        let regions = [
            Rect {
                left: -18,
                top: -8,
                right: -12,
                bottom: -2,
            },
            Rect {
                left: 12,
                top: 2,
                right: 18,
                bottom: 8,
            },
            Rect {
                left: -16,
                top: -5,
                right: -10,
                bottom: 0,
            },
            Rect {
                left: -18,
                top: -8,
                right: -12,
                bottom: -2,
            },
        ];
        let crops = disjoint_crops(&regions, bounds);
        let mut covered = 0;
        for y in bounds.top..bounds.bottom {
            for x in bounds.left..bounds.right {
                let contains = |r: &Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
                let expected = regions.iter().any(contains);
                assert_eq!(
                    crops.iter().filter(|r| contains(r)).count(),
                    usize::from(expected)
                );
                covered += usize::from(expected);
            }
        }
        assert!(covered < 120, "empty rectangle-union gaps were included");
        let whole = CapturedRegion {
            bounds,
            pixels: [60, 20, 10, 128].repeat(800),
        };
        let pieces: Vec<_> = crops
            .iter()
            .map(|r| CapturedRegion {
                bounds: *r,
                pixels: [60, 20, 10, 128]
                    .repeat(((r.right - r.left) * (r.bottom - r.top)) as usize),
            })
            .collect();
        let layers: Vec<_> = pieces.iter().map(|r| (r, 170)).collect();
        for region in regions {
            assert_eq!(
                compose(region, &layers).pixels,
                compose(region, &[(&whole, 170)]).pixels
            );
        }
    }
}
