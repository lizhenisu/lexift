//! A worker-owned child surface; the Slint parent is retained until retirement.
use lexift_core::domain::recording_preview::Target;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D11::*,
            Dwm::DwmFlush,
            Dxgi::{Common::*, *},
        },
        UI::WindowsAndMessaging::*,
    },
    core::*,
};
pub(super) struct Surface {
    #[cfg(test)]
    _lease: super::super::super::recording_gate::resources::Lease,
    window: HWND,
    parent: HWND,
    restore_clip: bool,
    swap: Option<IDXGISwapChain1>,
    size: (u32, u32),
    blitter: Option<super::blit::Blitter>,
    #[cfg(test)]
    pub test_probe: Option<(u32, u32)>,
    #[cfg(test)]
    pub test_pixel: Option<[u8; 4]>,
}
unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut paint = Default::default();
                windows::Win32::Graphics::Gdi::BeginPaint(window, &mut paint);
                let _ = windows::Win32::Graphics::Gdi::EndPaint(window, &paint);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => DefWindowProcW(window, message, w, l),
        }
    }
}
impl Surface {
    pub fn new(target: Target) -> Result<Self> {
        unsafe {
            let parent = HWND(target.window.0 as *mut _);
            let mut pid = 0;
            GetWindowThreadProcessId(parent, Some(&mut pid));
            if !IsWindow(Some(parent)).as_bool() || pid != std::process::id() {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            let b = target.bounds;
            let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
            static CLASS: std::sync::Once = std::sync::Once::new();
            CLASS.call_once(|| {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: module.into(),
                    lpszClassName: w!("LexiftRecordingPreviewSurface"),
                    ..Default::default()
                });
            });
            let window = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("LexiftRecordingPreviewSurface"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                b.left,
                b.top,
                (b.right - b.left).max(1),
                (b.bottom - b.top).max(1),
                Some(parent),
                None,
                Some(module.into()),
                None,
            )?;
            let style = GetWindowLongPtrW(parent, GWL_STYLE);
            let restore_clip = style & WS_CLIPCHILDREN.0 as isize == 0;
            if restore_clip {
                SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN.0 as isize);
            }
            Ok(Self {
                #[cfg(test)]
                _lease: super::super::super::recording_gate::resources::Lease::new(
                    &super::super::super::recording_gate::resources::PREVIEW_SURFACE,
                ),
                window,
                parent,
                restore_clip,
                swap: None,
                blitter: None,
                #[cfg(test)]
                test_probe: None,
                #[cfg(test)]
                test_pixel: None,
                size: (
                    (b.right - b.left).max(1) as u32,
                    (b.bottom - b.top).max(1) as u32,
                ),
            })
        }
    }
    pub fn resize(&mut self, b: lexift_core::domain::geometry::Rect) -> Result<()> {
        unsafe {
            let size = (
                (b.right - b.left).max(1) as u32,
                (b.bottom - b.top).max(1) as u32,
            );
            SetWindowPos(
                self.window,
                None,
                b.left,
                b.top,
                size.0 as i32,
                size.1 as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )?;
            if size != self.size {
                if let Some(s) = &self.swap {
                    s.ResizeBuffers(
                        0,
                        size.0,
                        size.1,
                        DXGI_FORMAT_UNKNOWN,
                        DXGI_SWAP_CHAIN_FLAG(0),
                    )?;
                }
                self.size = size;
            }
            Ok(())
        }
    }
    pub fn present(&mut self, engine: &super::engine::Engine) -> Result<()> {
        unsafe {
            if self.swap.is_none() {
                let dxgi: IDXGIDevice = engine.device.cast()?;
                let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
                let desc = DXGI_SWAP_CHAIN_DESC1 {
                    Width: self.size.0,
                    Height: self.size.1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                    Scaling: DXGI_SCALING_STRETCH,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    ..Default::default()
                };
                self.swap = Some(factory.CreateSwapChainForHwnd(
                    &engine.device,
                    self.window,
                    &desc,
                    None,
                    None,
                )?);
            }
            let swap = self.swap.as_ref().unwrap();
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let (w, h) = self.size;
            let (vw, vh) = engine.size;
            let scale = (w as f64 / vw as f64).min(h as f64 / vh as f64);
            let rw = (vw as f64 * scale).round() as i32;
            let rh = (vh as f64 * scale).round() as i32;
            let x = (w as i32 - rw) / 2;
            let y = (h as i32 - rh) / 2;
            if self.blitter.is_none() {
                self.blitter = Some(super::blit::Blitter::new(engine)?);
            }
            self.blitter
                .as_ref()
                .unwrap()
                .draw(engine, &texture, x, y, rw, rh)?;
            #[cfg(test)]
            if let Some(point) = self.test_probe {
                self.test_pixel = Some(engine.read_pixels_from(&texture, &[point])?[0]);
            }
            swap.Present(1, DXGI_PRESENT(0)).ok()?;
            Ok(())
        }
    }
    pub fn flush(&self) -> Result<()> {
        unsafe { DwmFlush() }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        self.swap.take();
        let _ = unsafe { DestroyWindow(self.window) };
        if self.restore_clip && unsafe { IsWindow(Some(self.parent)).as_bool() } {
            unsafe {
                SetWindowLongPtrW(
                    self.parent,
                    GWL_STYLE,
                    GetWindowLongPtrW(self.parent, GWL_STYLE) & !(WS_CLIPCHILDREN.0 as isize),
                );
            }
        }
    }
}
