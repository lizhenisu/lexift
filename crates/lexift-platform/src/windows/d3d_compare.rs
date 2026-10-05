//! Shared exact device-side pixel comparison for capture and composed output.
//! Only a sixteen-byte change extent crosses to the CPU; polling never waits.
use super::d3d_shader::{bytes, compile};
use lexift_core::domain::geometry::Rect;
use windows::{
    Win32::Graphics::{
        Direct3D11::*,
        Dxgi::{Common::*, DXGI_ERROR_WAS_STILL_DRAWING},
    },
    core::{Result, s},
};

const GROUP: u32 = 16;

const SHADER: &str = r#"
Texture2D<float4> previous : register(t0);
Texture2D<float4> candidate : register(t1);
RWByteAddressBuffer changed : register(u0);
cbuffer Region : register(b0) { uint4 region; };
groupshared uint4 extent;
[numthreads(GROUP_SIZE,GROUP_SIZE,1)]
void compare(uint3 position : SV_DispatchThreadID, uint lane : SV_GroupIndex) {
    if (lane == 0) extent = uint4(0xffffffff,0xffffffff,0,0);
    GroupMemoryBarrierWithGroupSync();
    uint width, height;
    candidate.GetDimensions(width, height);
    uint2 pixel = position.xy + region.xy;
    if (position.x < region.z && position.y < region.w && pixel.x < width && pixel.y < height &&
        any(previous.Load(int3(pixel,0)) != candidate.Load(int3(pixel,0)))) {
        InterlockedMin(extent.x, pixel.x); InterlockedMin(extent.y, pixel.y);
        InterlockedMax(extent.z, pixel.x + 1); InterlockedMax(extent.w, pixel.y + 1);
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0 && extent.z != 0) {
        uint ignored;
        changed.InterlockedMin(0, extent.x, ignored); changed.InterlockedMin(4, extent.y, ignored);
        changed.InterlockedMax(8, extent.z, ignored); changed.InterlockedMax(12, extent.w, ignored);
    }
}
"#;

/// An unfinished comparison must never be interpreted as equal pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Idle,
    Pending,
    Equal,
    Changed(Rect),
}

pub(super) struct Kernel {
    #[cfg(test)]
    group: u32,
    shader: ID3D11ComputeShader,
    region: ID3D11Buffer,
}
impl Kernel {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        Self::new_with_group(device, GROUP)
    }

    fn new_with_group(device: &ID3D11Device, group: u32) -> Result<Self> {
        // Dispatch and shader dimensions share one value. The source is compiled
        // once per device kernel, never during frame submission.
        let source = SHADER.replace("GROUP_SIZE", &group.to_string());
        let code = compile(&source, s!("compare"), s!("cs_5_0"))?;
        let mut shader = None;
        unsafe {
            device.CreateComputeShader(bytes(&code), None, Some(&mut shader))?;
        }
        let mut region = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut region),
            )?;
        }
        Ok(Self {
            #[cfg(test)]
            group,
            shader: shader.unwrap(),
            region: region.unwrap(),
        })
    }

    /// Alternative group sizes exist only for paired hardware experiments.
    #[cfg(test)]
    pub fn test_group(device: &ID3D11Device, group: u32) -> Result<Self> {
        assert!(matches!(group, 8 | 16));
        Self::new_with_group(device, group)
    }

    pub fn begin(
        &self,
        context: &ID3D11DeviceContext,
        comparison: &mut Comparison,
        regions: &[Rect],
    ) {
        self.enqueue(context, comparison, regions);
        unsafe {
            context.Flush();
        }
    }

    /// Enqueue without submitting; a composition batch adds its end timestamp
    /// before one Flush. This never waits; callers must submit the batch.
    pub fn enqueue(
        &self,
        context: &ID3D11DeviceContext,
        comparison: &mut Comparison,
        regions: &[Rect],
    ) {
        debug_assert!(
            !comparison.pending,
            "one comparison may be in flight per source"
        );
        unsafe {
            // RAW buffer clears repeat the first scalar rather than four components.
            let initial = [u32::MAX, u32::MAX, 0, 0];
            context.UpdateSubresource(&comparison.flag, 0, None, initial.as_ptr().cast(), 0, 0);
            context.CSSetShader(&self.shader, None);
            context.CSSetShaderResources(
                0,
                Some(&[
                    Some(comparison.reference_view.clone()),
                    Some(comparison.candidate_view.clone()),
                ]),
            );
            context.CSSetUnorderedAccessViews(0, 1, Some(&Some(comparison.view.clone())), None);
            context.CSSetConstantBuffers(0, Some(&[Some(self.region.clone())]));
            for region in regions {
                let data = [
                    region.left.max(0) as u32,
                    region.top.max(0) as u32,
                    region.right.saturating_sub(region.left).max(0) as u32,
                    region.bottom.saturating_sub(region.top).max(0) as u32,
                ];
                if data[2] == 0 || data[3] == 0 {
                    continue;
                }
                context.UpdateSubresource(&self.region, 0, None, data.as_ptr().cast(), 0, 0);
                #[cfg(test)]
                let group = self.group;
                #[cfg(not(test))]
                let group = GROUP;
                context.Dispatch(data[2].div_ceil(group), data[3].div_ceil(group), 1);
            }
            context.CSSetShaderResources(0, Some(&[None, None]));
            context.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
            context.CSSetShader(None, None);
            context.CopyResource(&comparison.readback, &comparison.flag);
            if let Some(query) = &comparison.completion_event {
                context.End(query);
            }
        }
        comparison.pending = true;
        comparison.candidate_fresh = true;
    }
}

pub(super) struct Comparison {
    pub candidate: ID3D11Texture2D,
    reference_view: ID3D11ShaderResourceView,
    candidate_view: ID3D11ShaderResourceView,
    flag: ID3D11Buffer,
    view: ID3D11UnorderedAccessView,
    readback: ID3D11Buffer,
    completion_event: Option<ID3D11Query>,
    pub pending: bool,
    pub candidate_fresh: bool,
    #[cfg(test)]
    pub observe_query_polls: bool,
    #[cfg(test)]
    pub query_poll: Option<(std::time::Instant, std::time::Instant, bool)>,
}

impl Comparison {
    pub fn new(
        device: &ID3D11Device,
        reference: &ID3D11Texture2D,
        desc: &D3D11_TEXTURE2D_DESC,
    ) -> Result<Self> {
        let mut candidate = None;
        unsafe {
            device.CreateTexture2D(desc, None, Some(&mut candidate))?;
        }
        let candidate = candidate.unwrap();
        let (mut reference_view, mut candidate_view) = (None, None);
        unsafe {
            device.CreateShaderResourceView(reference, None, Some(&mut reference_view))?;
            device.CreateShaderResourceView(&candidate, None, Some(&mut candidate_view))?;
        }
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: 16,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
            ..Default::default()
        };
        let mut flag = None;
        unsafe {
            device.CreateBuffer(&desc, None, Some(&mut flag))?;
        }
        let flag = flag.unwrap();
        let view_desc = D3D11_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
            Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D11_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: 4,
                    Flags: D3D11_BUFFER_UAV_FLAG_RAW.0 as u32,
                },
            },
        };
        let mut view = None;
        unsafe {
            device.CreateUnorderedAccessView(&flag, Some(&view_desc), Some(&mut view))?;
        }
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: 16,
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..Default::default()
        };
        let mut readback = None;
        unsafe {
            device.CreateBuffer(&desc, None, Some(&mut readback))?;
        }
        Ok(Self {
            candidate,
            reference_view: reference_view.unwrap(),
            candidate_view: candidate_view.unwrap(),
            flag,
            view: view.unwrap(),
            readback: readback.unwrap(),
            completion_event: None,
            pending: false,
            candidate_fresh: false,
            #[cfg(test)]
            observe_query_polls: false,
            #[cfg(test)]
            query_poll: None,
        })
    }

    /// Complete the same frozen comparison batch before polling its readback.
    /// The event never acknowledges or publishes candidate pixels itself.
    pub fn enable_completion_event(&mut self, device: &ID3D11Device) -> Result<()> {
        assert!(!self.pending);
        let mut query = None;
        unsafe {
            device.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: D3D11_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )?;
        }
        self.completion_event = Some(query.unwrap());
        Ok(())
    }

    /// Poll only metadata, without waiting or publishing the candidate baseline.
    pub fn try_complete(&mut self, context: &ID3D11DeviceContext) -> Result<Outcome> {
        if !self.pending {
            return Ok(Outcome::Idle);
        }
        if let Some(query) = &self.completion_event {
            #[cfg(test)]
            let began = self.observe_query_polls.then(std::time::Instant::now);
            let ready = super::d3d_query::data::<windows::core::BOOL>(context, query)?
                .is_some_and(|ready| ready.as_bool());
            #[cfg(test)]
            if let Some(began) = began {
                self.query_poll = Some((began, std::time::Instant::now(), ready));
            }
            if !ready {
                return Ok(Outcome::Pending);
            }
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            match context.Map(
                &self.readback,
                0,
                D3D11_MAP_READ,
                D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                Some(&mut mapped),
            ) {
                Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                    return Ok(Outcome::Pending);
                }
                result => result?,
            }
            let data = mapped.pData.cast::<u32>();
            let extent = [
                data.read_unaligned(),
                data.add(1).read_unaligned(),
                data.add(2).read_unaligned(),
                data.add(3).read_unaligned(),
            ];
            let changed = (extent[2] != 0).then(|| Rect {
                left: extent[0] as i32,
                top: extent[1] as i32,
                right: extent[2] as i32,
                bottom: extent[3] as i32,
            });
            context.Unmap(&self.readback, 0);
            self.pending = false;
            Ok(changed.map_or(Outcome::Equal, Outcome::Changed))
        }
    }

    /// Accept only a completed candidate. Display users call this after commit,
    /// so a rejected frame never becomes the reference for future equality.
    pub fn publish(&mut self, reference: &mut ID3D11Texture2D) {
        debug_assert!(!self.pending);
        self.publish_candidate(reference);
    }

    /// Capture policy changes discard this detector immediately afterwards.
    /// Adopt its newer full snapshot without waiting for the old ROI comparison;
    /// queued device copies retain their resources and remain correctly ordered.
    /// Presentation must use publish after an accepted commit instead.
    pub fn publish_candidate(&mut self, reference: &mut ID3D11Texture2D) {
        debug_assert!(self.candidate_fresh);
        std::mem::swap(reference, &mut self.candidate);
        std::mem::swap(&mut self.reference_view, &mut self.candidate_view);
        self.candidate_fresh = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_comparison_is_exact_at_dispatch_edges_and_reuses_unchanged_baseline() {
        use windows::core::Interface;
        let backend = super::super::screen_capture::capture::Backend::new().unwrap();
        let device = backend.device();
        let context = backend.context();
        for (group, completion_event) in [(8, false), (16, false), (8, true), (16, true)] {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: 13,
                Height: 7,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let texture = |pixels: &[u8]| {
                let mut texture = None;
                let data = D3D11_SUBRESOURCE_DATA {
                    pSysMem: pixels.as_ptr().cast(),
                    SysMemPitch: 13 * 4,
                    ..Default::default()
                };
                unsafe {
                    device
                        .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
                        .unwrap();
                }
                texture.unwrap()
            };
            let mut pixels = [40, 60, 80, 255].repeat(13 * 7);
            let mut baseline = texture(&pixels);
            let kernel = Kernel::test_group(device, group).unwrap();
            let mut comparison = Comparison::new(device, &baseline, &desc).unwrap();
            if completion_event {
                comparison.enable_completion_event(device).unwrap();
            }
            assert_eq!(comparison.try_complete(context).unwrap(), Outcome::Idle);
            let wait = |comparison: &mut Comparison| {
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    match comparison.try_complete(context).unwrap() {
                        Outcome::Changed(extent) => break Some(extent),
                        Outcome::Equal => break None,
                        Outcome::Pending => {}
                        Outcome::Idle => panic!("no comparison is in flight"),
                    }
                    assert!(Instant::now() < deadline, "GPU comparison did not complete");
                    std::thread::sleep(Duration::from_millis(1));
                }
            };
            for channel in 0..4 {
                let previous = baseline.as_raw();
                unsafe {
                    context.CopyResource(&comparison.candidate, &texture(&pixels));
                }
                kernel.begin(
                    context,
                    &mut comparison,
                    &[Rect {
                        left: 0,
                        top: 0,
                        right: 13,
                        bottom: 7,
                    }],
                );
                assert_eq!(wait(&mut comparison), None);
                assert_eq!(
                    baseline.as_raw(),
                    previous,
                    "equal pixels retain the published texture"
                );
                let last = pixels.len() - 4 + channel;
                pixels[last] = pixels[last].wrapping_sub(1);
                unsafe {
                    context.CopyResource(&comparison.candidate, &texture(&pixels));
                }
                kernel.begin(
                    context,
                    &mut comparison,
                    &[Rect {
                        left: 0,
                        top: 0,
                        right: 13,
                        bottom: 7,
                    }],
                );
                assert!(
                    wait(&mut comparison)
                        == Some(Rect {
                            left: 12,
                            top: 6,
                            right: 13,
                            bottom: 7
                        }),
                    "single-channel difference at the last pixel must be detected"
                );
                comparison.publish(&mut baseline);
                assert_ne!(baseline.as_raw(), previous);
                assert!(!comparison.candidate_fresh);
            }
            let previous = baseline.as_raw();
            pixels[0] = pixels[0].wrapping_add(1);
            unsafe {
                context.CopyResource(&comparison.candidate, &texture(&pixels));
            }
            kernel.begin(
                context,
                &mut comparison,
                &[Rect {
                    left: 12,
                    top: 6,
                    right: 13,
                    bottom: 7,
                }],
            );
            assert!(
                wait(&mut comparison).is_none(),
                "pixels outside the requested region must not invalidate it"
            );
            assert_eq!(baseline.as_raw(), previous);
            assert!(
                comparison.candidate_fresh,
                "unchanged ROI still retains the latest full snapshot for expansion"
            );
            kernel.begin(
                context,
                &mut comparison,
                &[Rect {
                    left: 0,
                    top: 0,
                    right: 13,
                    bottom: 7,
                }],
            );
            assert!(
                wait(&mut comparison)
                    == Some(Rect {
                        left: 0,
                        top: 0,
                        right: 1,
                        bottom: 1
                    }),
                "expanding the ROI must detect the previously excluded change"
            );
            comparison.publish(&mut baseline);
            assert!(
                !comparison.candidate_fresh,
                "the swapped-out baseline must never be republished as fresh"
            );
            pixels[(5 * 13 + 1) * 4] ^= 1;
            pixels[(13 + 10) * 4 + 1] ^= 1;
            unsafe {
                context.CopyResource(&comparison.candidate, &texture(&pixels));
            }
            kernel.begin(
                context,
                &mut comparison,
                &[
                    Rect {
                        left: 0,
                        top: 4,
                        right: 4,
                        bottom: 7,
                    },
                    Rect {
                        left: 9,
                        top: 0,
                        right: 13,
                        bottom: 3,
                    },
                ],
            );
            assert_eq!(
                wait(&mut comparison),
                Some(Rect {
                    left: 1,
                    top: 1,
                    right: 11,
                    bottom: 6
                }),
                "separate dispatches must merge their exact changed extents"
            );
        }
    }
}
