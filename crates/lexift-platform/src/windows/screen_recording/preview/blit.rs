//! Scale the last accepted GPU image, never a pending Media Engine seek result.
use super::engine::Engine;
use crate::windows::d3d_shader::{bytes, compile};
use windows::{
    Win32::Graphics::{Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D11::*},
    core::*,
};
pub(super) struct Blitter {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    source: ID3D11ShaderResourceView,
    guide: ID3D11Buffer,
}
impl Blitter {
    pub fn new(engine: &Engine) -> Result<Self> {
        unsafe {
            let code = "struct V{float4 p:SV_POSITION;float2 uv:TEXCOORD0;};V vsMain(uint id:SV_VertexID){V o;float2 uv=float2((id<<1)&2,id&2);o.p=float4(uv*float2(2,-2)+float2(-1,1),0,1);o.uv=uv;return o;}Texture2D image:register(t0);SamplerState filterSampler:register(s0);cbuffer Guide:register(b0){float4 box;float4 settings;}float4 psMain(V v):SV_TARGET{float4 color=image.Sample(filterSampler,v.uv);if(settings.x==0)return color;float2 pixel=v.uv*settings.yz;float2 a=pixel-box.xy;float2 b=box.zw-pixel;float2 d=min(a,b);if(min(d.x,d.y)<-1)return float4(color.rgb*0.4,1);if(min(d.x,d.y)<1.5 || ((d.x<5 || d.y<5) && (max(d.x,d.y)<5 || abs(pixel.x-(box.x+box.z)*0.5)<5 || abs(pixel.y-(box.y+box.w)*0.5)<5)))return float4(0.1,0.85,0.45,1);return color;}";
            let v = compile(code, s!("vsMain"), s!("vs_5_0"))?;
            let p = compile(code, s!("psMain"), s!("ps_5_0"))?;
            let (mut vs, mut ps, mut sampler, mut source) = (None, None, None, None);
            engine
                .device
                .CreateVertexShader(bytes(&v), None, Some(&mut vs))?;
            engine
                .device
                .CreatePixelShader(bytes(&p), None, Some(&mut ps))?;
            engine.device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            engine.device.CreateShaderResourceView(
                engine.texture.as_ref().unwrap(),
                None,
                Some(&mut source),
            )?;
            let mut guide = None;
            engine.device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut guide),
            )?;
            Ok(Self {
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                sampler: sampler.unwrap(),
                source: source.unwrap(),
                guide: guide.unwrap(),
            })
        }
    }
    pub fn draw(
        &self,
        engine: &Engine,
        back: &ID3D11Texture2D,
        (x, y, w, h): (i32, i32, i32, i32),
        guide: Option<lexift_core::domain::recording_trim::FrameCrop>,
    ) -> Result<()> {
        unsafe {
            let mut target = None;
            engine
                .device
                .CreateRenderTargetView(back, None, Some(&mut target))?;
            let target = target.unwrap();
            let c = &engine._context;
            c.ClearRenderTargetView(&target, &[0., 0., 0., 1.]);
            c.OMSetRenderTargets(Some(&[Some(target)]), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: x as f32,
                TopLeftY: y as f32,
                Width: w as f32,
                Height: h as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
            }]));
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(&self.ps, None);
            c.PSSetShaderResources(0, Some(&[Some(self.source.clone())]));
            c.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            let mut constants = [0f32; 8];
            if let Some(guide) = guide {
                let sx = w as f32 / engine.size.0 as f32;
                let sy = h as f32 / engine.size.1 as f32;
                constants = [
                    guide.left as f32 * sx,
                    guide.top as f32 * sy,
                    guide.right as f32 * sx,
                    guide.bottom as f32 * sy,
                    1.,
                    w as f32,
                    h as f32,
                    0.,
                ];
            }
            c.UpdateSubresource(&self.guide, 0, None, constants.as_ptr().cast(), 0, 0);
            c.PSSetConstantBuffers(0, Some(&[Some(self.guide.clone())]));
            c.Draw(3, 0);
            // Release context bindings before Media Engine writes the accepted-frame
            // texture again, and before ResizeBuffers retires the swap-chain target.
            c.PSSetShaderResources(0, Some(&[None]));
            c.OMSetRenderTargets(None, None);
            Ok(())
        }
    }
}
