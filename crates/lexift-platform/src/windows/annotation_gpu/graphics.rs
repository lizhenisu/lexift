//! Device-local premultiplied image composition. No readback in the render path.
use super::super::d3d_shader::{bytes, compile};
use lexift_core::domain::geometry::Rect;
use std::{ffi::c_void, mem::size_of};
use windows::{
    Win32::Graphics::{
        Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D11::*, Dxgi::Common::*,
    },
    core::{Result, s},
};

pub(super) struct Texture {
    pub _resource: ID3D11Texture2D,
    pub srv: Option<ID3D11ShaderResourceView>,
    pub rtv: Option<ID3D11RenderTargetView>,
    pub width: u32,
    pub height: u32,
}

impl Texture {
    pub fn matches_bounds(&self, bounds: Rect) -> bool {
        bounds.right.checked_sub(bounds.left) == i32::try_from(self.width).ok()
            && bounds.bottom.checked_sub(bounds.top) == i32::try_from(self.height).ok()
    }

    pub fn upload(&self, context: &ID3D11DeviceContext, pixels: &[u8]) {
        unsafe {
            context.UpdateSubresource(
                &self._resource,
                0,
                None,
                pixels.as_ptr().cast(),
                self.width * 4,
                0,
            );
        }
    }
    pub fn wrap(
        device: &ID3D11Device,
        resource: ID3D11Texture2D,
        sampled: bool,
        target: bool,
    ) -> Result<Self> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        let (mut srv, mut rtv) = (None, None);
        unsafe {
            resource.GetDesc(&mut desc);
            if sampled {
                device.CreateShaderResourceView(&resource, None, Some(&mut srv))?;
            }
            if target {
                device.CreateRenderTargetView(&resource, None, Some(&mut rtv))?;
            }
        }
        Ok(Self {
            _resource: resource,
            srv,
            rtv,
            width: desc.Width,
            height: desc.Height,
        })
    }

    pub fn create(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        pixels: Option<&[u8]>,
    ) -> Result<Self> {
        Self::create_format(device, width, height, pixels, DXGI_FORMAT_R8G8B8A8_UNORM)
    }

    pub fn filter_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        Self::create_format(device, width, height, None, DXGI_FORMAT_R16G16B16A16_FLOAT)
    }

    fn create_format(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        pixels: Option<&[u8]>,
        format: DXGI_FORMAT,
    ) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
            ..Default::default()
        };
        let data = pixels.map(|pixels| D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: width * 4,
            ..Default::default()
        });
        let mut resource = None;
        unsafe {
            device.CreateTexture2D(
                &desc,
                data.as_ref().map(|d| d as *const _),
                Some(&mut resource),
            )?;
        }
        Self::wrap(device, resource.unwrap(), true, true)
    }
}

#[repr(C)]
struct Parameters {
    uv: [f32; 4],
    values: [f32; 4],
}

pub(super) struct Blit {
    pub destination: Rect,
    pub source_rect: Rect,
    pub opacity: f32,
    pub blend: bool,
}

pub(super) struct Graphics {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    grid: ID3D11PixelShader,
    expand: ID3D11PixelShader,
    mask: ID3D11PixelShader,
    gaussian: ID3D11PixelShader,
    down: ID3D11PixelShader,
    up: ID3D11PixelShader,
    mix: ID3D11PixelShader,
    lens: ID3D11PixelShader,
    constants: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
}

impl Graphics {
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Result<Self> {
        let source = include_str!("blit.hlsl");
        let pixel_shader = |entry| -> Result<ID3D11PixelShader> {
            let code = compile(source, entry, s!("ps_5_0"))?;
            let mut shader = None;
            unsafe {
                device.CreatePixelShader(bytes(&code), None, Some(&mut shader))?;
            }
            Ok(shader.unwrap())
        };
        let grid = pixel_shader(s!("grid_main"))?;
        let expand = pixel_shader(s!("expand_main"))?;
        let mask = pixel_shader(s!("mask_main"))?;
        let gaussian = pixel_shader(s!("gaussian_main"))?;
        let down = pixel_shader(s!("down_main"))?;
        let up = pixel_shader(s!("up_main"))?;
        let mix = pixel_shader(s!("mix_main"))?;
        let lens = pixel_shader(s!("lens_main"))?;
        let vertex_code = compile(source, s!("vertex_main"), s!("vs_5_0"))?;
        let pixel_code = compile(source, s!("pixel_main"), s!("ps_5_0"))?;
        let (mut vertex, mut pixel, mut constants, mut sampler, mut blend, mut rasterizer) =
            (None, None, None, None, None, None);
        unsafe {
            device.CreateVertexShader(bytes(&vertex_code), None, Some(&mut vertex))?;
            device.CreatePixelShader(bytes(&pixel_code), None, Some(&mut pixel))?;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: size_of::<Parameters>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            let mut blend_desc = D3D11_BLEND_DESC::default();
            blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            device.CreateBlendState(&blend_desc, Some(&mut blend))?;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut rasterizer),
            )?;
        }
        Ok(Self {
            device,
            context,
            vertex: vertex.unwrap(),
            pixel: pixel.unwrap(),
            grid,
            expand,
            mask,
            gaussian,
            down,
            up,
            mix,
            lens,
            constants: constants.unwrap(),
            sampler: sampler.unwrap(),
            blend: blend.unwrap(),
            rasterizer: rasterizer.unwrap(),
        })
    }

    pub fn clear(&self, target: &Texture) {
        unsafe {
            self.context
                .ClearRenderTargetView(target.rtv.as_ref().unwrap(), &[0.; 4]);
        }
    }

    /// Desktop-coordinate clipping preserves texel alignment across monitor origins.
    pub fn blit(&self, source: &Texture, target: &Texture, target_bounds: Rect, blit: Blit) {
        let Blit {
            destination,
            source_rect,
            opacity,
            blend,
        } = blit;
        let left = destination.left.max(target_bounds.left);
        let top = destination.top.max(target_bounds.top);
        let right = destination.right.min(target_bounds.right);
        let bottom = destination.bottom.min(target_bounds.bottom);
        if right <= left || bottom <= top {
            return;
        }
        let dw = (destination.right - destination.left) as f32;
        let dh = (destination.bottom - destination.top) as f32;
        let sw = (source_rect.right - source_rect.left) as f32;
        let sh = (source_rect.bottom - source_rect.top) as f32;
        let parameters = Parameters {
            uv: [
                (source_rect.left as f32 + (left - destination.left) as f32 * sw / dw)
                    / source.width as f32,
                (source_rect.top as f32 + (top - destination.top) as f32 * sh / dh)
                    / source.height as f32,
                (right - left) as f32 * sw / dw / source.width as f32,
                (bottom - top) as f32 * sh / dh / source.height as f32,
            ],
            values: [opacity, 0., 0., 0.],
        };
        self.draw(
            std::slice::from_ref(&source.srv),
            target,
            D3D11_VIEWPORT {
                TopLeftX: (left - target_bounds.left) as f32,
                TopLeftY: (top - target_bounds.top) as f32,
                Width: (right - left) as f32,
                Height: (bottom - top) as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
            },
            &parameters,
            &self.pixel,
            blend,
        );
    }

    /// Average each global grid cell once, then expand its texel with integer
    /// loads. Scratch textures are owned and reused by the effect tile cache.
    pub fn pixelate(
        &self,
        source: &Texture,
        bounds: Rect,
        block: u32,
        grid: &Texture,
        output: &Texture,
    ) {
        let block = block.max(1) as i32;
        let cell = [bounds.left.div_euclid(block), bounds.top.div_euclid(block)];
        self.draw(
            std::slice::from_ref(&source.srv),
            grid,
            viewport(grid),
            &Parameters {
                uv: [cell[0] as f32, cell[1] as f32, 0., 0.],
                values: [bounds.left as f32, bounds.top as f32, block as f32, 0.],
            },
            &self.grid,
            false,
        );
        self.draw(
            std::slice::from_ref(&grid.srv),
            output,
            viewport(output),
            &Parameters {
                uv: [
                    bounds.left as f32,
                    bounds.top as f32,
                    cell[0] as f32,
                    cell[1] as f32,
                ],
                values: [0., 0., block as f32, 0.],
            },
            &self.expand,
            false,
        );
    }

    pub fn mask(
        &self,
        source: &Texture,
        bounds: Rect,
        mask: &Texture,
        mask_bounds: Rect,
        output: &Texture,
    ) {
        self.draw(
            &[source.srv.clone(), mask.srv.clone()],
            output,
            viewport(output),
            &Parameters {
                uv: [
                    (mask_bounds.left - bounds.left) as f32 / source.width as f32,
                    (mask_bounds.top - bounds.top) as f32 / source.height as f32,
                    mask.width as f32 / source.width as f32,
                    mask.height as f32 / source.height as f32,
                ],
                values: [0.; 4],
            },
            &self.mask,
            false,
        );
    }

    pub fn blur_pass(&self, source: &Texture, target: &Texture, pass: BlurPass) {
        let (shader, uv, values) = match pass {
            BlurPass::Gaussian { sigma, horizontal } => (
                &self.gaussian,
                [
                    1. / source.width as f32,
                    1. / source.height as f32,
                    if horizontal { 1. } else { 0. },
                    if horizontal { 0. } else { 1. },
                ],
                [sigma, (sigma * 3.).ceil(), 0., 0.],
            ),
            BlurPass::Down(offset) => (
                &self.down,
                [
                    offset / source.width as f32,
                    offset / source.height as f32,
                    0.,
                    0.,
                ],
                [0.; 4],
            ),
            BlurPass::Up(offset) => (
                &self.up,
                [
                    offset / source.width as f32,
                    offset / source.height as f32,
                    0.,
                    0.,
                ],
                [0.; 4],
            ),
        };
        self.draw(
            std::slice::from_ref(&source.srv),
            target,
            viewport(target),
            &Parameters { uv, values },
            shader,
            false,
        );
    }

    pub fn mix(&self, first: &Texture, second: &Texture, target: &Texture, amount: f32) {
        self.draw(
            &[first.srv.clone(), second.srv.clone()],
            target,
            viewport(target),
            &Parameters {
                uv: [0.; 4],
                values: [amount, 0., 0., 0.],
            },
            &self.mix,
            false,
        );
    }

    pub fn lens(
        &self,
        source: &Texture,
        mask: &Texture,
        target: &Texture,
        uv: [f32; 4],
        antialias: bool,
    ) {
        self.draw(
            &[source.srv.clone(), mask.srv.clone()],
            target,
            viewport(target),
            &Parameters {
                uv,
                values: [if antialias { 1. } else { 0. }, 0., 0., 0.],
            },
            &self.lens,
            false,
        );
    }

    fn draw(
        &self,
        sources: &[Option<ID3D11ShaderResourceView>],
        target: &Texture,
        viewport: D3D11_VIEWPORT,
        parameters: &Parameters,
        shader: &ID3D11PixelShader,
        blend: bool,
    ) {
        unsafe {
            self.context.PSSetShaderResources(0, Some(&[None, None]));
            self.context
                .OMSetRenderTargets(Some(std::slice::from_ref(&target.rtv)), None);
            self.context.OMSetBlendState(
                if blend { Some(&self.blend) } else { None },
                None,
                u32::MAX,
            );
            self.context.RSSetState(&self.rasterizer);
            self.context.RSSetViewports(Some(&[viewport]));
            self.context.UpdateSubresource(
                &self.constants,
                0,
                None,
                (parameters as *const Parameters).cast::<c_void>(),
                0,
                0,
            );
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.VSSetShader(&self.vertex, None);
            self.context.PSSetShader(shader, None);
            self.context
                .PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context.PSSetShaderResources(0, Some(sources));
            self.context.Draw(3, 0);
            self.context.PSSetShaderResources(0, Some(&[None, None]));
            self.context.OMSetRenderTargets(None, None);
        }
    }
}

pub(super) enum BlurPass {
    Gaussian { sigma: f32, horizontal: bool },
    Down(f32),
    Up(f32),
}

fn viewport(texture: &Texture) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT {
        Width: texture.width as f32,
        Height: texture.height as f32,
        MinDepth: 0.,
        MaxDepth: 1.,
        ..Default::default()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Readback exists only in verification, never in the live effect path.
    pub(crate) fn readback(graphics: &Graphics, texture: &Texture) -> Vec<u8> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture._resource.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            graphics
                .device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .unwrap();
            let staging = staging.unwrap();
            graphics.context.CopyResource(&staging, &texture._resource);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            graphics
                .context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            let mut pixels = Vec::new();
            let pixel_size = if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
                8
            } else {
                4
            };
            for y in 0..texture.height as usize {
                pixels.extend_from_slice(std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    texture.width as usize * pixel_size,
                ));
            }
            graphics.context.Unmap(&staging, 0);
            pixels
        }
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_grid_average_matches_reference_at_negative_origins_and_mask_preserves_alpha() {
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let bounds = Rect {
            left: -13,
            top: -7,
            right: 10,
            bottom: 10,
        };
        let width = 23usize;
        let height = 17usize;
        let pixels: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                [
                    (i * 31 % 256) as u8,
                    (i * 73 % 256) as u8,
                    (i * 11 % 256) as u8,
                    255,
                ]
            })
            .collect();
        let source =
            Texture::create(&graphics.device, width as u32, height as u32, Some(&pixels)).unwrap();
        for block in [1, 2, 3, 16, 64, 80] {
            let columns = (bounds.right - 1).div_euclid(block) - bounds.left.div_euclid(block) + 1;
            let rows = (bounds.bottom - 1).div_euclid(block) - bounds.top.div_euclid(block) + 1;
            let grid =
                Texture::create(&graphics.device, columns as u32, rows as u32, None).unwrap();
            let output =
                Texture::create(&graphics.device, width as u32, height as u32, None).unwrap();
            graphics.pixelate(&source, bounds, block as u32, &grid, &output);
            let actual = readback(&graphics, &output);
            for y in bounds.top..bounds.bottom {
                for x in bounds.left..bounds.right {
                    let left = x.div_euclid(block) * block;
                    let top = y.div_euclid(block) * block;
                    let mut sum = [0u32; 4];
                    let mut count = 0;
                    for sy in top.max(bounds.top)..(top + block).min(bounds.bottom) {
                        for sx in left.max(bounds.left)..(left + block).min(bounds.right) {
                            let index = ((sy - bounds.top) as usize * width
                                + (sx - bounds.left) as usize)
                                * 4;
                            for channel in 0..4 {
                                sum[channel] += u32::from(pixels[index + channel]);
                            }
                            count += 1;
                        }
                    }
                    let index =
                        ((y - bounds.top) as usize * width + (x - bounds.left) as usize) * 4;
                    for channel in 0..4 {
                        let expected = ((sum[channel] + count / 2) / count) as u8;
                        assert!(
                            actual[index + channel].abs_diff(expected) <= 1,
                            "block {block} ({x},{y}) channel {channel}: {} vs {expected}",
                            actual[index + channel]
                        );
                    }
                }
            }
            let mask_bounds = Rect {
                left: -8,
                top: -4,
                right: -2,
                bottom: 1,
            };
            let mask_pixels: Vec<u8> = (0..30)
                .flat_map(|i| [[0u8, 64, 128, 255][i % 4]; 4])
                .collect();
            let mask = Texture::create(&graphics.device, 6, 5, Some(&mask_pixels)).unwrap();
            let masked = Texture::create(&graphics.device, 6, 5, None).unwrap();
            graphics.mask(&output, bounds, &mask, mask_bounds, &masked);
            let result = readback(&graphics, &masked);
            for i in 0..30 {
                let source_index = ((i / 6 + 3) * width + i % 6 + 5) * 4;
                for channel in 0..4 {
                    let expected = (u32::from(actual[source_index + channel])
                        * u32::from(mask_pixels[i * 4 + 3])
                        + 127)
                        / 255;
                    assert!(result[i * 4 + channel].abs_diff(expected as u8) <= 1);
                }
            }
        }
    }
}
