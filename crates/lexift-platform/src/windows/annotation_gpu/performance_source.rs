//! Owned source publication fixture. GPU and GDI publish the same identity bands;
//! Startup desktop pixel verification never runs on the production render worker.
use super::{Backend, PaintPublication, SourceFrame, band_color};
use std::{cell::RefCell, time::Instant};
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

#[derive(Clone, Copy)]
pub(super) enum SourceKind {
    Gdi,
    Dxgi,
}
impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gdi => "offscreen_gdi_single_bitblt",
            Self::Dxgi => "d3d11_upload_copy_hwnd_flip_sequential_present0",
        }
    }
}

pub(super) enum Publisher {
    Gdi(SourceFrame),
    Dxgi(GpuSource),
}
impl Publisher {
    pub fn new(hwnd: HWND, backend: &Backend, kind: SourceKind) -> Self {
        match kind {
            SourceKind::Gdi => Self::Gdi(SourceFrame::new(hwnd)),
            SourceKind::Dxgi => Self::Dxgi(GpuSource::new(hwnd, backend)),
        }
    }
    pub fn paint(&self, tick: u32) -> PaintPublication {
        match self {
            Self::Gdi(source) => source.paint(tick),
            Self::Dxgi(source) => source.paint(tick),
        }
    }
    pub fn verify(&self, tick: u32) {
        match self {
            Self::Gdi(source) => source.verify(tick),
            Self::Dxgi(source) => source.verify(tick),
        }
    }
}

pub(super) struct GpuSource {
    hwnd: HWND,
    context: ID3D11DeviceContext,
    swap: IDXGISwapChain1,
    upload: ID3D11Texture2D,
    pixels: RefCell<Vec<u32>>,
}
impl GpuSource {
    fn new(hwnd: HWND, backend: &Backend) -> Self {
        let device = backend.device().clone();
        let context = backend.context().clone();
        let dxgi: IDXGIDevice = device.cast().unwrap();
        unsafe {
            let factory: IDXGIFactory2 = dxgi.GetAdapter().unwrap().GetParent().unwrap();
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 1280,
                Height: 720,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap = factory
                .CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)
                .unwrap();
            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: 1280,
                Height: 720,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                ..Default::default()
            };
            let mut upload = None;
            device
                .CreateTexture2D(&texture_desc, None, Some(&mut upload))
                .unwrap();
            Self {
                hwnd,
                context,
                swap,
                upload: upload.unwrap(),
                pixels: RefCell::new(vec![0; 1280 * 720]),
            }
        }
    }
    fn paint(&self, tick: u32) -> PaintPublication {
        let mut pixels = self.pixels.borrow_mut();
        for band in 0..8 {
            let rgb = band_color(tick, band).0;
            let bgra = 0xff00_0000 | ((rgb & 255) << 16) | (rgb & 0xff00) | (rgb >> 16);
            for row in pixels.as_chunks_mut::<1280>().0 {
                row[(band as usize * 160)..(band as usize + 1) * 160].fill(bgra);
            }
        }
        unsafe {
            let before = Instant::now();
            self.context.UpdateSubresource(
                &self.upload,
                0,
                None,
                pixels.as_ptr().cast(),
                1280 * 4,
                0,
            );
            // Reacquire buffer 0 on each flip; never reuse a stale buffer index.
            let buffer: ID3D11Texture2D = self.swap.GetBuffer(0).unwrap();
            self.context.CopyResource(&buffer, &self.upload);
            self.swap.Present(0, DXGI_PRESENT(0)).ok().unwrap();
            PaintPublication {
                before,
                after: Instant::now(),
            }
        }
    }
    /// Verify actual desktop pixels before timing; a submitted swap chain alone is insufficient.
    fn verify(&self, tick: u32) {
        use windows::Win32::{
            Foundation::POINT,
            Graphics::Gdi::{ClientToScreen, GetDC, GetPixel, HDC, ReleaseDC},
        };
        struct DesktopDc(HDC);
        impl Drop for DesktopDc {
            fn drop(&mut self) {
                unsafe {
                    ReleaseDC(None, self.0);
                }
            }
        }
        let desktop = DesktopDc(unsafe { GetDC(None) });
        assert!(!desktop.0.is_invalid());
        let mut origin = POINT::default();
        assert!(unsafe { ClientToScreen(self.hwnd, &mut origin) }.as_bool());
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            super::super::surface::pump_messages();
            let valid = (0..8).all(|band| {
                [0, 360, 719].into_iter().all(|y| {
                    (unsafe {
                        GetPixel(desktop.0, origin.x + (band * 160 + 80) as i32, origin.y + y)
                    }) == band_color(tick, band)
                })
            });
            if valid {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "DXGI source desktop pixels did not preserve the eight identity bands"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
