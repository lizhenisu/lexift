//! Test-only bounded asynchronous readback of two owned source identity pixels.
//! No Flush, blocking Map or fallback guess is allowed in the live worker.
use super::super::screen_capture::diagnostics::Published;
use super::graphics::Graphics;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use windows::Win32::Graphics::{
    Direct3D11::*,
    Dxgi::{Common::*, DXGI_ERROR_WAS_STILL_DRAWING},
};

pub(super) struct Sample {
    pub source: Published,
    pub paint: u32,
}
struct Slot {
    texture: ID3D11Texture2D,
    source: Published,
    format: DXGI_FORMAT,
}
#[derive(Default)]
pub(super) struct Probe {
    pending: VecDeque<Slot>,
    free: Vec<Slot>,
    last: Option<(u64, i64)>,
    pub samples: Vec<Sample>,
    pub dropped: usize,
    pub invalid: usize,
    pub cpu: Vec<Duration>,
}
pub(super) fn band_mask(band: u32) -> u32 {
    ((band * 31) % 256) | (((band * 19) % 256) << 8) | (((band * 13) % 256) << 16)
}
fn decode(bytes: &[u8], format: DXGI_FORMAT) -> Option<u32> {
    let rgb = |p: &[u8]| {
        if format == DXGI_FORMAT_B8G8R8A8_UNORM {
            u32::from(p[2]) | (u32::from(p[1]) << 8) | (u32::from(p[0]) << 16)
        } else {
            u32::from(p[0]) | (u32::from(p[1]) << 8) | (u32::from(p[2]) << 16)
        }
    };
    (bytes.len() == 8
        && bytes[3] == 255
        && bytes[7] == 255
        && rgb(&bytes[..4]) ^ rgb(&bytes[4..]) == band_mask(7))
    .then(|| rgb(&bytes[..4]))
}
impl Probe {
    pub fn enqueue(
        &mut self,
        graphics: &Graphics,
        texture: &ID3D11Texture2D,
        source: Published,
    ) -> windows::core::Result<()> {
        let key = (source.sequence, source.ticks);
        if self.last == Some(key) {
            return Ok(());
        }
        self.last = Some(key);
        if self.pending.len() >= 4 {
            self.dropped += 1;
            return Ok(());
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        if desc.Width < 1280
            || desc.Height < 720
            || !matches!(
                desc.Format,
                DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM
            )
        {
            self.invalid += 1;
            return Ok(());
        }
        let slot = if let Some(mut slot) = self.free.pop() {
            if slot.format != desc.Format {
                self.invalid += 1;
                return Ok(());
            }
            slot.source = source;
            slot
        } else {
            desc.Width = 2;
            desc.Height = 1;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut target = None;
            unsafe {
                graphics
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut target))?;
            }
            Slot {
                texture: target.unwrap(),
                source,
                format: desc.Format,
            }
        };
        unsafe {
            for (destination, x) in [(0, 80), (1, 1200)] {
                let region = D3D11_BOX {
                    left: x,
                    top: 360,
                    front: 0,
                    right: x + 1,
                    bottom: 361,
                    back: 1,
                };
                graphics.context.CopySubresourceRegion(
                    &slot.texture,
                    0,
                    destination,
                    0,
                    0,
                    texture,
                    0,
                    Some(&region),
                );
            }
        }
        self.pending.push_back(slot);
        Ok(())
    }
    pub fn poll(&mut self, graphics: &Graphics) -> windows::core::Result<()> {
        let start = Instant::now();
        while let Some(slot) = self.pending.front() {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            match unsafe {
                graphics.context.Map(
                    &slot.texture,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                )
            } {
                Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => break,
                result => result?,
            }
            let paint = decode(
                unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), 8) },
                slot.format,
            );
            unsafe {
                graphics.context.Unmap(&slot.texture, 0);
            }
            let slot = self.pending.pop_front().unwrap();
            if let Some(paint) = paint {
                if self.samples.len() < 16_384 {
                    self.samples.push(Sample {
                        source: slot.source,
                        paint,
                    });
                } else {
                    self.dropped += 1;
                }
            } else {
                self.invalid += 1;
            }
            self.free.push(slot);
        }
        if self.cpu.len() < 16_384 {
            self.cpu.push(start.elapsed());
        }
        Ok(())
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}
#[test]
fn identity_pixels_reject_corruption_and_do_not_wrap_at_256() {
    for id in [1, 255, 256, 65535, 65536, 0xffffff] {
        let other = id ^ band_mask(7);
        let rgba = |v: u32| [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255];
        let mut pixels = [rgba(id), rgba(other)].concat();
        assert_eq!(decode(&pixels, DXGI_FORMAT_R8G8B8A8_UNORM), Some(id));
        let bgra = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255];
        assert_eq!(
            decode(
                &[bgra(id), bgra(other)].concat(),
                DXGI_FORMAT_B8G8R8A8_UNORM
            ),
            Some(id)
        );
        pixels[4] ^= 1;
        assert!(decode(&pixels, DXGI_FORMAT_R8G8B8A8_UNORM).is_none());
    }
}
