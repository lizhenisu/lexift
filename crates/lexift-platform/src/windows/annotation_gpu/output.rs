//! Exact output equality against accepted pixels. Candidate comparisons are
//! latest-only and nonblocking; rejected commits never advance the baseline.
use super::super::d3d_compare::{Comparison, Kernel, Outcome};
use super::{content::PresentationInputs, graphics::Graphics, surface::Surface};
use lexift_core::{domain::geometry::Rect, ports::annotation_render::RenderStamp};
use windows::{Win32::Graphics::Direct3D11::*, core::Result};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Decision {
    Compose,
    Pending,
    Equal,
    Changed,
}
struct Output {
    reference: ID3D11Texture2D,
    comparison: Comparison,
    initialized: bool,
    changed: Option<bool>,
}
#[derive(Default)]
pub(super) struct Pixels {
    #[cfg(test)]
    pub disable_completion_events: bool,
    kernel: Option<Kernel>,
    outputs: Vec<Output>,
    candidate: Option<PresentationInputs>,
    candidate_stamp: Option<RenderStamp>,
}
impl Pixels {
    pub fn reset(&mut self) {
        self.outputs.clear();
        self.candidate = None;
        self.candidate_stamp = None;
    }
    pub fn poll(&mut self, graphics: &Graphics, stamp: RenderStamp) -> Result<Decision> {
        let Some(_) = &self.candidate else {
            return Ok(Decision::Compose);
        };
        let mut pending = false;
        for output in &mut self.outputs {
            if output.changed.is_none() {
                match output.comparison.try_complete(&graphics.context)? {
                    Outcome::Pending => pending = true,
                    Outcome::Equal => output.changed = Some(false),
                    Outcome::Changed(_) => output.changed = Some(true),
                    Outcome::Idle => unreachable!("a staged output must have a pending result"),
                }
            }
        }
        // An old comparison must finish before reusing its tiny readback and
        // candidate resources. Source changes may arrive meanwhile, but only
        // the same immutable scene stamp may authorize this frozen candidate.
        if pending {
            return Ok(Decision::Pending);
        }
        if self.candidate_stamp != Some(stamp) {
            self.candidate = None;
            return Ok(Decision::Compose);
        }
        Ok(
            if self
                .outputs
                .iter()
                .any(|output| output.changed == Some(true))
            {
                Decision::Changed
            } else {
                Decision::Equal
            },
        )
    }
    pub fn stage(
        &mut self,
        graphics: &Graphics,
        surfaces: &[Surface],
        inputs: PresentationInputs,
        stamp: RenderStamp,
    ) -> Result<Decision> {
        if self.kernel.is_none() {
            self.kernel = Some(Kernel::new(&graphics.device)?);
        }
        if self.outputs.is_empty() {
            for surface in surfaces {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe {
                    surface.target._resource.GetDesc(&mut desc);
                }
                desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                let mut reference = None;
                unsafe {
                    graphics
                        .device
                        .CreateTexture2D(&desc, None, Some(&mut reference))?;
                }
                let reference = reference.unwrap();
                let mut comparison = Comparison::new(&graphics.device, &reference, &desc)?;
                #[cfg(not(test))]
                comparison.enable_completion_event(&graphics.device)?;
                #[cfg(test)]
                if !self.disable_completion_events {
                    comparison.enable_completion_event(&graphics.device)?;
                }
                self.outputs.push(Output {
                    reference,
                    comparison,
                    initialized: false,
                    changed: None,
                });
            }
        }
        debug_assert_eq!(self.outputs.len(), surfaces.len());
        for (output, surface) in self.outputs.iter_mut().zip(surfaces) {
            unsafe {
                graphics
                    .context
                    .CopyResource(&output.comparison.candidate, &surface.target._resource);
            }
            output.comparison.candidate_fresh = true;
            if output.initialized {
                output.changed = None;
                self.kernel.as_ref().unwrap().enqueue(
                    &graphics.context,
                    &mut output.comparison,
                    &[Rect {
                        left: 0,
                        top: 0,
                        right: surface.target.width as i32,
                        bottom: surface.target.height as i32,
                    }],
                );
            } else {
                output.changed = Some(true);
            }
        }
        self.candidate = Some(inputs);
        self.candidate_stamp = Some(stamp);
        Ok(
            if self.outputs.iter().any(|output| output.changed.is_none()) {
                Decision::Pending
            } else {
                Decision::Changed
            },
        )
    }
    pub fn inputs(&self) -> &PresentationInputs {
        self.candidate
            .as_ref()
            .expect("a resolved decision owns candidate metadata")
    }
    /// Equal pixels acknowledge this candidate without advancing the accepted
    /// baseline. New source debt must be composed, not compared to this result.
    pub fn acknowledge_equal(&mut self) {
        self.candidate = None;
        self.candidate_stamp = None;
    }

    /// Called only after the scene version fence and native commit succeed.
    pub fn accept(&mut self) {
        for output in &mut self.outputs {
            output.comparison.publish(&mut output.reference);
            output.initialized = true;
            output.changed = None;
        }
        self.candidate = None;
        self.candidate_stamp = None;
    }
}
