//! Calibrated finite-support blur. Calibration is metadata work, not per-frame
//! image processing; effect caches retain plans and device-local workspaces.
use super::graphics::{BlurPass, Graphics, Texture};

#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub sigma: f32,
    pub pyramids: Vec<Kernel>,
    pub pyramid_mix: f32,
}

#[derive(Clone, Debug)]
pub(super) struct Kernel {
    pub levels: usize,
    pub offset: f32,
}

impl Plan {
    pub fn new(sigma: f32) -> Self {
        let mut plan = Self {
            sigma,
            pyramids: Vec::new(),
            pyramid_mix: 0.,
        };
        if sigma <= 3. {
            return plan;
        }
        let (mut levels, mut second) = (2, None);
        for (next, start, end) in [(3, 10., 12.), (4, 20., 24.), (5, 40., 48.)] {
            if sigma >= end {
                levels = next;
            } else if sigma > start {
                second = Some(next);
                plan.pyramid_mix = (sigma - start) / (end - start);
                break;
            } else {
                break;
            }
        }
        plan.pyramids.push(calibrate(levels, sigma));
        if let Some(levels) = second {
            plan.pyramids.push(calibrate(levels, sigma));
        }
        plan
    }

    pub fn alignment(&self) -> i32 {
        1 << self
            .pyramids
            .iter()
            .map(|kernel| kernel.levels)
            .max()
            .unwrap_or(0)
    }

    /// Sum the support of every down/up tap and bilinear interpolation, in
    /// full-resolution pixels. A three-sigma margin alone is insufficient.
    pub fn padding(&self) -> i32 {
        let pyramid = self
            .pyramids
            .iter()
            .map(|kernel| {
                ((5. * kernel.offset + 3.) * ((1 << kernel.levels) - 1) as f32).ceil() as i32
            })
            .max()
            .unwrap_or(0);
        pyramid
            .max((self.sigma * 3.).ceil() as i32)
            .saturating_add(2)
    }
}

fn calibrate(levels: usize, sigma: f32) -> Kernel {
    let (mut low, mut high) = (0., 4.);
    while high < 8. && measured_sigma(levels, high) < f64::from(sigma) {
        high *= 2.;
    }
    for _ in 0..20 {
        let middle = (low + high) / 2.;
        if measured_sigma(levels, middle) < f64::from(sigma) {
            low = middle;
        } else {
            high = middle;
        }
    }
    Kernel {
        levels,
        offset: ((low + high) / 2.) as f32,
    }
}

/// The two-dimensional filters' one-dimensional marginal includes bilinear
/// resampling. Average quarter phases rather than assuming an integer kernel.
fn measured_sigma(levels: usize, offset: f64) -> f64 {
    let mut variance = 0.;
    let period = 1usize << levels;
    for phase in [0, period / 4, period / 2, period * 3 / 4] {
        let mut samples = vec![0.; 1024];
        let center = 512 + phase;
        samples[center] = 1.;
        for _ in 0..levels {
            samples = resample(
                &samples,
                samples.len() / 2,
                &[(-offset, 0.25), (0., 0.5), (offset, 0.25)],
            );
        }
        for _ in 0..levels {
            samples = resample(
                &samples,
                samples.len() * 2,
                &[
                    (-2. * offset, 1. / 12.),
                    (-offset, 1. / 3.),
                    (0., 1. / 6.),
                    (offset, 1. / 3.),
                    (2. * offset, 1. / 12.),
                ],
            );
        }
        let mass: f64 = samples.iter().sum();
        let mean = samples
            .iter()
            .enumerate()
            .map(|(i, weight)| i as f64 * weight)
            .sum::<f64>()
            / mass;
        variance += samples
            .iter()
            .enumerate()
            .map(|(i, weight)| (i as f64 - mean).powi(2) * weight)
            .sum::<f64>()
            / mass;
    }
    (variance / 4.).sqrt()
}

fn resample(source: &[f64], count: usize, taps: &[(f64, f64)]) -> Vec<f64> {
    let mut result = vec![0.; count];
    let Some(first) = source.iter().position(|value| *value != 0.) else {
        return result;
    };
    let last = source.iter().rposition(|value| *value != 0.).unwrap();
    let min_offset = taps.iter().map(|tap| tap.0).fold(f64::INFINITY, f64::min);
    let max_offset = taps
        .iter()
        .map(|tap| tap.0)
        .fold(f64::NEG_INFINITY, f64::max);
    if taps.is_empty() {
        return result;
    }
    // Bilinear sampling can touch one neighbour outside nonzero support.
    // Keep an extra two-index guard for floating-point boundary rounding;
    // a nonzero clamp endpoint requires the full corresponding output edge.
    let ratio = count as f64 / source.len() as f64;
    let begin = if first == 0 {
        0
    } else {
        (((first as f64 - 0.5 - max_offset) * ratio - 0.5)
            .floor()
            .max(0.) as usize)
            .saturating_sub(2)
            .min(count)
    };
    let end = if last + 1 == source.len() {
        count
    } else {
        (((last as f64 + 1.5 - min_offset) * ratio - 0.5)
            .ceil()
            .max(0.) as usize)
            .saturating_add(3)
            .min(count)
    };
    for (index, value) in result.iter_mut().enumerate().take(end).skip(begin) {
        let center = (index as f64 + 0.5) * source.len() as f64 / count as f64 - 0.5;
        *value = taps
            .iter()
            .map(|(offset, weight)| {
                let position = (center + offset).clamp(0., (source.len() - 1) as f64);
                let first = position.floor() as usize;
                let fraction = position - first as f64;
                (source[first] * (1. - fraction)
                    + source[(first + 1).min(source.len() - 1)] * fraction)
                    * weight
            })
            .sum();
    }
    result
}

#[cfg(test)]
fn resample_reference(source: &[f64], count: usize, taps: &[(f64, f64)]) -> Vec<f64> {
    (0..count)
        .map(|index| {
            let center = (index as f64 + 0.5) * source.len() as f64 / count as f64 - 0.5;
            taps.iter()
                .map(|(offset, weight)| {
                    let position = (center + offset).clamp(0., (source.len() - 1) as f64);
                    let first = position.floor() as usize;
                    let fraction = position - first as f64;
                    (source[first] * (1. - fraction)
                        + source[(first + 1).min(source.len() - 1)] * fraction)
                        * weight
                })
                .sum()
        })
        .collect()
}

struct Pyramid {
    down: Vec<Texture>,
    up: Vec<Texture>,
    offset: f32,
}
impl Pyramid {
    fn new(
        graphics: &Graphics,
        width: u32,
        height: u32,
        kernel: &Kernel,
    ) -> windows::core::Result<Self> {
        let mut down = Vec::new();
        let mut up = Vec::new();
        for i in 1..=kernel.levels {
            down.push(Texture::filter_texture(
                &graphics.device,
                (width >> i).max(1),
                (height >> i).max(1),
            )?);
        }
        for i in (0..kernel.levels).rev() {
            up.push(Texture::filter_texture(
                &graphics.device,
                (width >> i).max(1),
                (height >> i).max(1),
            )?);
        }
        Ok(Self {
            down,
            up,
            offset: kernel.offset,
        })
    }
    fn render(&self, graphics: &Graphics, source: &Texture) -> &Texture {
        let mut previous = source;
        for target in &self.down {
            graphics.blur_pass(previous, target, BlurPass::Down(self.offset));
            previous = target;
        }
        for target in &self.up {
            graphics.blur_pass(previous, target, BlurPass::Up(self.offset));
            previous = target;
        }
        self.up.last().unwrap()
    }
}

pub(super) struct Workspace {
    pub plan: Plan,
    gaussian: Option<Texture>,
    gaussian_result: Option<Texture>,
    pyramids: Vec<Pyramid>,
}
impl Workspace {
    pub fn reuse_plan(&mut self, plan: &Plan) -> bool {
        if self.gaussian.is_some() != (plan.sigma < 4.)
            || self.gaussian_result.is_some() != (plan.sigma > 3. && plan.sigma < 4.)
            || self.pyramids.len() != plan.pyramids.len()
            || self
                .pyramids
                .iter()
                .zip(&plan.pyramids)
                .any(|(pyramid, kernel)| pyramid.down.len() != kernel.levels)
        {
            return false;
        }
        for (pyramid, kernel) in self.pyramids.iter_mut().zip(&plan.pyramids) {
            pyramid.offset = kernel.offset;
        }
        self.plan = plan.clone();
        true
    }
    pub fn new(
        graphics: &Graphics,
        width: u32,
        height: u32,
        plan: &Plan,
    ) -> windows::core::Result<Self> {
        let gaussian = (plan.sigma < 4.)
            .then(|| Texture::filter_texture(&graphics.device, width, height))
            .transpose()?;
        let gaussian_result = (plan.sigma > 3. && plan.sigma < 4.)
            .then(|| Texture::filter_texture(&graphics.device, width, height))
            .transpose()?;
        let pyramids = plan
            .pyramids
            .iter()
            .map(|kernel| Pyramid::new(graphics, width, height, kernel))
            .collect::<windows::core::Result<_>>()?;
        Ok(Self {
            plan: plan.clone(),
            gaussian,
            gaussian_result,
            pyramids,
        })
    }
    pub fn render(&self, graphics: &Graphics, source: &Texture, output: &Texture) {
        if let Some(temporary) = &self.gaussian {
            graphics.blur_pass(
                source,
                temporary,
                BlurPass::Gaussian {
                    sigma: self.plan.sigma,
                    horizontal: true,
                },
            );
            graphics.blur_pass(
                temporary,
                self.gaussian_result.as_ref().unwrap_or(output),
                BlurPass::Gaussian {
                    sigma: self.plan.sigma,
                    horizontal: false,
                },
            );
        }
        let Some(first) = self.pyramids.first() else {
            return;
        };
        let first = first.render(graphics, source);
        if let Some(gaussian) = &self.gaussian_result {
            graphics.mix(gaussian, first, output, self.plan.sigma - 3.);
        } else if let Some(second) = self.pyramids.get(1) {
            graphics.mix(
                first,
                second.render(graphics, source),
                output,
                self.plan.pyramid_mix,
            );
        } else {
            graphics.mix(first, first, output, 0.);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn sparse_calibration_resampling_matches_full_reference_bitwise() {
        for length in [1, 2, 3, 16, 32, 33, 1024] {
            for support in [0, length / 2, length - 1] {
                for dense in [false, true] {
                    let mut source = vec![0.; length];
                    source[support] = 1.;
                    if dense {
                        for (index, value) in source.iter_mut().enumerate() {
                            *value = (index as f64 + 1.).sin();
                        }
                    }
                    for count in [1, length, length * 2, (length / 2).max(1)] {
                        for offset in [0., 0.25, 1.375, 3., 7.999] {
                            for taps in [
                                vec![(-offset, 0.25), (0., 0.5), (offset, 0.25)],
                                vec![
                                    (-2. * offset, 1. / 12.),
                                    (-offset, 1. / 3.),
                                    (0., 1. / 6.),
                                    (offset, 1. / 3.),
                                    (2. * offset, 1. / 12.),
                                ],
                            ] {
                                let expected = resample_reference(&source, count, &taps);
                                let actual = resample(&source, count, &taps);
                                assert!(
                                    actual
                                        .iter()
                                        .zip(&expected)
                                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                                    "length={length} support={support} dense={dense} count={count} offset={offset}"
                                );
                            }
                        }
                    }
                }
            }
            let source = vec![0.; length];
            assert_eq!(
                resample(&source, length * 2, &[(0., 1.)]),
                resample_reference(&source, length * 2, &[(0., 1.)])
            );
        }
    }

    #[test]
    fn calibration_pyramids_preserve_full_reference_samples() {
        for levels in 2..=5 {
            let period = 1 << levels;
            for phase in [0, period / 4, period / 2, period * 3 / 4] {
                for offset in [0., 0.000001, 0.25, 1.375, 3., 7.999, 8.] {
                    let mut actual = vec![0.; 1024];
                    actual[512 + phase] = 1.;
                    let mut expected = actual.clone();
                    for up in [false, true] {
                        let taps = if up {
                            vec![
                                (-2. * offset, 1. / 12.),
                                (-offset, 1. / 3.),
                                (0., 1. / 6.),
                                (offset, 1. / 3.),
                                (2. * offset, 1. / 12.),
                            ]
                        } else {
                            vec![(-offset, 0.25), (0., 0.5), (offset, 0.25)]
                        };
                        for _ in 0..levels {
                            let count = if up {
                                actual.len() * 2
                            } else {
                                actual.len() / 2
                            };
                            actual = resample(&actual, count, &taps);
                            expected = resample_reference(&expected, count, &taps);
                            assert!(
                                actual
                                    .iter()
                                    .zip(&expected)
                                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                                "levels={levels} phase={phase} offset={offset} up={up}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "CPU calibration benchmark; run Release without parallel builds"]
    fn blur_new_plan_calibration_cpu_cost() {
        if cfg!(debug_assertions) {
            panic!("run calibration benchmark in Release");
        }
        for sigma in [5., 11., 22., 25., 60.] {
            for _ in 0..3 {
                std::hint::black_box(Plan::new(sigma));
            }
            let mut times = Vec::new();
            for _ in 0..40 {
                let start = std::time::Instant::now();
                std::hint::black_box(Plan::new(sigma));
                times.push(start.elapsed().as_secs_f64() * 1000.);
            }
            times.sort_by(f64::total_cmp);
            println!(
                "blur_plan_sigma={sigma} samples={} cpu_p50_ms={:.6} cpu_p95_ms={:.6}",
                times.len(),
                times[19],
                times[37]
            );
        }
    }

    pub(crate) fn reference_signal(source: &[f64], plan: &Plan) -> Vec<f64> {
        let run = |kernel: &Kernel| {
            let offset = f64::from(kernel.offset);
            let mut output = source.to_vec();
            for _ in 0..kernel.levels {
                output = resample_reference(
                    &output,
                    output.len() / 2,
                    &[(-offset, 0.25), (0., 0.5), (offset, 0.25)],
                );
            }
            for _ in 0..kernel.levels {
                output = resample_reference(
                    &output,
                    output.len() * 2,
                    &[
                        (-2. * offset, 1. / 12.),
                        (-offset, 1. / 3.),
                        (0., 1. / 6.),
                        (offset, 1. / 3.),
                        (2. * offset, 1. / 12.),
                    ],
                );
            }
            output
        };
        let mut output = run(&plan.pyramids[0]);
        if let Some(second) = plan.pyramids.get(1) {
            let second = run(second);
            for (first, second) in output.iter_mut().zip(second) {
                *first = *first * (1. - f64::from(plan.pyramid_mix))
                    + second * f64::from(plan.pyramid_mix);
            }
        }
        output
    }
    #[test]
    fn calibrated_plans_have_bounded_levels_and_cover_all_filter_taps() {
        for sigma in [3.01, 4., 8., 10.5, 12., 20., 22., 25., 40., 44., 60.] {
            let plan = Plan::new(sigma);
            for kernel in &plan.pyramids {
                assert!(kernel.levels <= 5);
                assert!(
                    (measured_sigma(kernel.levels, f64::from(kernel.offset)) / f64::from(sigma)
                        - 1.)
                        .abs()
                        < 0.005
                );
                assert!(
                    plan.padding() as f32 >= 5. * kernel.offset * ((1 << kernel.levels) - 1) as f32
                );
            }
        }
        assert_eq!(
            Plan::new(f32::MAX).padding(),
            i32::MAX,
            "unsupported capture extents must fail at allocation instead of overflowing padding"
        );
    }

    fn half(value: u16) -> f64 {
        let sign = if value & 0x8000 != 0 { -1. } else { 1. };
        let exponent = (value >> 10) & 31;
        let fraction = value & 1023;
        if exponent == 0 {
            sign * f64::from(fraction) / 1024. * 2f64.powi(-14)
        } else {
            sign * f64::from(f32::from_bits(
                (u32::from(exponent) + 112) << 23 | u32::from(fraction) << 13,
            ))
        }
    }

    fn moments(graphics: &Graphics, output: &Texture) -> (f64, f64, f64) {
        let result = super::super::graphics::tests::readback(graphics, output);
        let (mut mass, mut first, mut second) = (0., 0., 0.);
        for (index, bytes) in result.as_chunks::<8>().0.iter().enumerate() {
            let value = half(u16::from_le_bytes([bytes[0], bytes[1]]));
            let x = (index % output.width as usize) as f64;
            mass += value;
            first += value * x;
            second += value * x * x;
        }
        let mean = first / mass;
        ((second / mass - mean * mean).max(0.).sqrt(), mean, mass)
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_blur_impulse_sigma_is_calibrated_across_transition_levels_and_dpi() {
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let mut report = String::from("requested_sigma,phase,measured_sigma,centroid_shift,mass\n");
        for sigma in [
            0.5, 0.625, 1., 2., 3., 3.25, 3.75, 4., 8., 10.5, 11.9, 12., 20., 22., 24., 25., 40.,
            44., 60.,
        ] {
            let plan = Plan::new(sigma);
            let size = if sigma > 25. { 1024usize } else { 512usize };
            let workspace = Workspace::new(&graphics, size as u32, size as u32, &plan).unwrap();
            let output =
                Texture::filter_texture(&graphics.device, size as u32, size as u32).unwrap();
            for phase in [0, plan.alignment() as usize / 2] {
                let center = size / 2 + phase;
                let mut pixels = vec![0u8; size * size * 4];
                pixels[(center * size + center) * 4..(center * size + center) * 4 + 4].fill(255);
                let source =
                    Texture::create(&graphics.device, size as u32, size as u32, Some(&pixels))
                        .unwrap();
                workspace.render(&graphics, &source, &output);
                let (actual, mean, mass) = moments(&graphics, &output);
                report.push_str(&format!(
                    "{sigma},{phase},{actual},{},{mass}\n",
                    mean - center as f64
                ));
                assert!(
                    (actual / f64::from(sigma) - 1.).abs() <= 0.1,
                    "sigma {sigma}, phase {phase}: measured {actual}, mass {mass}"
                );
                assert!(
                    (mass - 1.).abs() < 0.03,
                    "filter lost impulse energy: {mass}"
                );
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/annotation-gpu-blur-kernels.csv");
        std::fs::write(path, report).unwrap();
    }

    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_blur_strength_is_monotonic_and_split_tiles_match_full_filter() {
        use lexift_core::domain::geometry::Rect;
        let capture = crate::windows::screen_capture::capture::Backend::new().unwrap();
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone()).unwrap();
        let mut pixels = vec![0u8; 512 * 512 * 4];
        pixels[(256 * 512 + 256) * 4..(256 * 512 + 256) * 4 + 4].fill(255);
        let source = Texture::create(&graphics.device, 512, 512, Some(&pixels)).unwrap();
        let output = Texture::filter_texture(&graphics.device, 512, 512).unwrap();
        let mut report = String::from("scale,strength,requested_sigma,measured_sigma\n");
        for scale in [1., 1.25] {
            let mut previous = 0.;
            for strength in 1..=100 {
                let sigma = (0.5 + (strength - 1) as f32 * 19.5 / 99.) * scale;
                let plan = Plan::new(sigma);
                Workspace::new(&graphics, 512, 512, &plan)
                    .unwrap()
                    .render(&graphics, &source, &output);
                let (actual, _, mass) = moments(&graphics, &output);
                assert!(
                    actual >= previous,
                    "scale {scale}, strength {strength}: {actual} < {previous}"
                );
                assert!((actual / f64::from(sigma) - 1.).abs() <= 0.1);
                assert!((mass - 1.).abs() < 0.03);
                previous = actual;
                report.push_str(&format!("{scale},{strength},{sigma},{actual}\n"));
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/annotation-gpu-blur-strengths.csv");
        std::fs::write(path, report).unwrap();

        let full_bounds = Rect {
            left: -256,
            top: -256,
            right: 256,
            bottom: 256,
        };
        let pattern = |bounds: Rect| -> Vec<u8> {
            (bounds.top..bounds.bottom)
                .flat_map(|y| {
                    (bounds.left..bounds.right).flat_map(move |x| {
                        [
                            if x < 0 { 20 } else { 220 },
                            if y < 0 { 40 } else { 200 },
                            ((x.div_euclid(13) + y.div_euclid(17)).rem_euclid(2) * 160 + 40) as u8,
                            255,
                        ]
                    })
                })
                .collect()
        };
        let full_source =
            Texture::create(&graphics.device, 512, 512, Some(&pattern(full_bounds))).unwrap();
        for sigma in [2., 3.5, 4., 10.5, 22., 25.] {
            let plan = Plan::new(sigma);
            Workspace::new(&graphics, 512, 512, &plan).unwrap().render(
                &graphics,
                &full_source,
                &output,
            );
            let expected = super::super::graphics::tests::readback(&graphics, &output);
            for body in [
                Rect {
                    left: -48,
                    top: -32,
                    right: 0,
                    bottom: 32,
                },
                Rect {
                    left: 0,
                    top: -32,
                    right: 48,
                    bottom: 32,
                },
            ] {
                let padding = plan.padding();
                let sample = super::super::effects::sample(
                    Rect {
                        left: body.left - padding,
                        top: body.top - padding,
                        right: body.right + padding,
                        bottom: body.bottom + padding,
                    },
                    plan.alignment() as u32,
                );
                let (width, height) = (
                    (sample.right - sample.left) as u32,
                    (sample.bottom - sample.top) as u32,
                );
                let cropped =
                    Texture::create(&graphics.device, width, height, Some(&pattern(sample)))
                        .unwrap();
                let actual_texture =
                    Texture::filter_texture(&graphics.device, width, height).unwrap();
                Workspace::new(&graphics, width, height, &plan)
                    .unwrap()
                    .render(&graphics, &cropped, &actual_texture);
                let actual = super::super::graphics::tests::readback(&graphics, &actual_texture);
                for y in body.top..body.bottom {
                    for x in body.left..body.right {
                        let full_index = ((y - full_bounds.top) as usize * 512
                            + (x - full_bounds.left) as usize)
                            * 8;
                        let tile_index = ((y - sample.top) as usize * width as usize
                            + (x - sample.left) as usize)
                            * 8;
                        for channel in 0..4 {
                            let value = |bytes: &[u8], index| {
                                half(u16::from_le_bytes([bytes[index], bytes[index + 1]]))
                            };
                            let difference = (value(&expected, full_index + channel * 2)
                                - value(&actual, tile_index + channel * 2))
                            .abs();
                            assert!(
                                difference <= 1. / 255.,
                                "sigma {sigma}, ({x},{y}), channel {channel}: split differs by {difference}"
                            );
                        }
                    }
                }
            }
        }
    }
}
