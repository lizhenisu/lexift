//! Native annotation presentation. All GPU objects and HWNDs stay on one worker.
mod blur;
#[cfg(test)]
mod composition_timing;
mod content;
#[cfg(test)]
mod display_timing;
mod effects;
#[cfg(test)]
use super::gpu_timing;
mod graphics;
mod lens;
mod output;
#[cfg(test)]
mod paint_probe;
#[cfg(test)]
mod performance;
mod recovery;
mod scheduler;
mod surface;

use super::screen_capture::capture::Backend;
use graphics::{Graphics, Texture};
use lexift_core::{
    Error, Result,
    ports::{annotation_render::*, screen_capture::ScreenCaptureRequest},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Mailbox {
    signal: Option<Arc<scheduler::Signal>>,
    #[cfg(test)]
    record_presentations: bool,
    #[cfg(test)]
    composition_probe: bool,
    #[cfg(test)]
    record_native_waits: bool,
    #[cfg(test)]
    disable_precise_clock: bool,
    #[cfg(test)]
    fallback_waits: usize,
    #[cfg(test)]
    enumeration_probe: bool,
    #[cfg(test)]
    paint_probe_token: Option<(isize, bool)>,
    #[cfg(test)]
    completion_queries: super::screen_capture::capture::CompletionQueries,
    #[cfg(test)]
    output_completion_reference: bool,
    #[cfg(test)]
    paint_samples: Vec<paint_probe::Sample>,
    #[cfg(test)]
    paint_probe_summary: (usize, usize, usize, Vec<Duration>),
    #[cfg(test)]
    presentations: Vec<Instant>,
    #[cfg(test)]
    source_presentations: Vec<performance::Presentation>,
    #[cfg(test)]
    source_presentations_dropped: usize,
    #[cfg(test)]
    display_observations: Vec<display_timing::Observation>,
    #[cfg(test)]
    display_observations_dropped: usize,
    #[cfg(test)]
    display_observation_errors: usize,
    #[cfg(test)]
    attempts: Vec<performance::Attempt>,
    #[cfg(test)]
    native_waits: Vec<scheduler::WaitSample>,
    #[cfg(test)]
    native_waits_dropped: usize,
    #[cfg(test)]
    gpu_samples: Vec<gpu_timing::Sample>,
    #[cfg(test)]
    capture_events: Vec<super::screen_capture::diagnostics::Event>,
    #[cfg(test)]
    capture_events_dropped: usize,
    #[cfg(test)]
    gpu_query_dropped: usize,
    #[cfg(test)]
    gpu_query_invalid: usize,
    #[cfg(test)]
    gpu_query_pending: usize,
    #[cfg(test)]
    captures: usize,
    #[cfg(test)]
    injected_device_losses: usize,
    #[cfg(test)]
    defer_device_rebuild: bool,
    #[cfg(test)]
    devices_created: usize,
    scene: Option<Arc<AnnotationScene>>,
    completion: Option<RenderCompletion>,
    error: Option<Error>,
    generation: u64,
    stopped: bool,
}

impl Mailbox {
    fn owns(&self, generation: u64, stamp: RenderStamp) -> bool {
        !self.stopped
            && self.generation == generation
            && self
                .scene
                .as_ref()
                .is_some_and(|scene| scene.stamp == stamp)
    }
}

/// Serialize only the nonblocking native commit with submit/clear. Holding an
/// epoch guard during GPU preparation would stall input; checking without the
/// guard would leave a race between authorization and DXGI/DComp submission.
fn commit_current(
    shared: &(Mutex<Mailbox>, Condvar),
    generation: u64,
    stamp: RenderStamp,
    commit: impl FnOnce() -> Result<bool>,
) -> Result<bool> {
    #[cfg(not(test))]
    let state = shared.0.lock().unwrap();
    #[cfg(test)]
    let mut state = shared.0.lock().unwrap();
    if !state.owns(generation, stamp) {
        return Ok(false);
    }
    let presented = commit()?;
    #[cfg(test)]
    if presented && state.record_presentations {
        state.presentations.push(Instant::now());
    }
    Ok(presented)
}

fn present_surfaces(surfaces: &[surface::Surface]) -> Result<bool> {
    let mut presented = true;
    for surface in surfaces {
        presented &= surface.present().map_err(native_error)?;
    }
    Ok(presented)
}

pub(crate) struct WindowsAnnotationRenderPort {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl WindowsAnnotationRenderPort {
    pub fn new() -> Option<Self> {
        if !super::screen_capture::activation::supported().ok()? {
            return None;
        }
        let shared = Arc::new((
            Mutex::new(Mailbox {
                signal: Some(Arc::new(scheduler::Signal::new().ok()?)),
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let state = shared.clone();
        let thread = thread::Builder::new()
            .name("annotation-gpu".into())
            .spawn(move || run(state))
            .ok()?;
        Some(Self {
            shared,
            thread: Some(thread),
        })
    }
}

impl AnnotationRenderPort for WindowsAnnotationRenderPort {
    fn capabilities(&self) -> RenderCapabilities {
        RenderCapabilities {
            backend: RenderBackend::Gpu,
            max_refresh_hz: 60,
        }
    }
    fn submit(&self, scene: Arc<AnnotationScene>) -> Result<()> {
        scene.validate()?;
        let (lock, wake) = &*self.shared;
        let mut mailbox = lock.lock().unwrap();
        if mailbox.stopped {
            return Err(Error::new("Annotation renderer has stopped"));
        }
        if mailbox
            .scene
            .as_ref()
            .is_some_and(|old| old.stamp == scene.stamp)
        {
            return Ok(());
        }
        mailbox.scene = Some(scene);
        mailbox.error = None;
        if let Some(signal) = &mailbox.signal {
            signal.wake();
        }
        wake.notify_one();
        Ok(())
    }
    fn latest(&self) -> Result<Option<RenderCompletion>> {
        let state = self.shared.0.lock().unwrap();
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        Ok(state.completion.clone())
    }
    fn clear(&self) {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.scene = None;
        state.completion = None;
        #[cfg(test)]
        {
            state.captures = 0;
        }
        state.error = None;
        if let Some(signal) = &state.signal {
            signal.wake();
        }
        wake.notify_one();
    }
}

impl Drop for WindowsAnnotationRenderPort {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        {
            let mut state = lock.lock().unwrap();
            state.stopped = true;
            if let Some(signal) = &state.signal {
                signal.wake();
            }
        }
        wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Renderer {
    #[cfg(test)]
    gpu_profiler: Option<gpu_timing::Profiler>,
    #[cfg(test)]
    gpu_trace: Option<gpu_timing::Trace>,
    #[cfg(test)]
    stages: performance::Stages,
    #[cfg(test)]
    sampled: Option<super::screen_capture::capture::GpuCapture>,
    #[cfg(test)]
    last_commit_sources: Option<Vec<super::screen_capture::diagnostics::Published>>,
    #[cfg(test)]
    last_commit_counts: Vec<display_timing::Count>,
    #[cfg(test)]
    last_display_query: Option<Instant>,
    #[cfg(test)]
    paint_probe: Option<(isize, paint_probe::Probe)>,
    #[cfg(test)]
    profile_capture: bool,
    #[cfg(test)]
    composition_probe: bool,
    #[cfg(test)]
    defer_presentation: bool,
    surfaces: Vec<surface::Surface>,
    raster: HashMap<RasterKey, Texture>,
    effects: effects::Effects,
    effect_inputs: content::EffectInputs,
    pending: content::PendingFrame,
    presented_stamp: Option<RenderStamp>,
    presented_inputs: Option<content::PresentationInputs>,
    output_pixels: output::Pixels,
    graphics: Graphics,
    // Keep the COM apartment alive until graphics and presentation objects drop.
    capture: Backend,
}

impl Renderer {
    fn device_error(&self) -> Option<windows::core::Error> {
        unsafe { self.graphics.device.GetDeviceRemovedReason().err() }
    }
    fn device_removed(&self) -> bool {
        self.device_error().is_some()
    }
    fn new() -> windows::core::Result<Self> {
        let capture = Backend::new()?;
        let graphics = Graphics::new(capture.device().clone(), capture.context().clone())?;
        Ok(Self {
            #[cfg(test)]
            gpu_profiler: None,
            #[cfg(test)]
            gpu_trace: None,
            #[cfg(test)]
            stages: Default::default(),
            #[cfg(test)]
            sampled: None,
            #[cfg(test)]
            last_commit_sources: None,
            #[cfg(test)]
            last_commit_counts: Vec::new(),
            #[cfg(test)]
            last_display_query: None,
            #[cfg(test)]
            paint_probe: None,
            #[cfg(test)]
            profile_capture: false,
            #[cfg(test)]
            composition_probe: false,
            #[cfg(test)]
            defer_presentation: false,
            surfaces: Vec::new(),
            raster: HashMap::new(),
            effects: Default::default(),
            effect_inputs: Default::default(),
            pending: Default::default(),
            presented_stamp: None,
            presented_inputs: None,
            output_pixels: Default::default(),
            graphics,
            capture,
        })
    }

    /// Source changes and document changes invalidate effects independently of
    /// caret/selection revisions. Remember inputs only after successful work.
    fn update_effects<'a>(
        &mut self,
        scene: &AnnotationScene,
        captured: &[effects::CaptureLayer],
        source_changed: impl Into<content::SourceChanges<'a>>,
    ) -> Result<bool> {
        let source_changed = source_changed.into();
        if !source_changed.any() && self.effect_inputs.matches(scene) {
            return Ok(false);
        }
        let updated = self
            .effects
            .update(
                &self.graphics,
                scene,
                &self.raster,
                captured,
                source_changed,
            )
            .map_err(native_error)?;
        self.effect_inputs.retain(scene);
        Ok(updated)
    }

    fn render(
        &mut self,
        scene: &AnnotationScene,
        commit: impl FnOnce(&[surface::Surface]) -> Result<bool>,
    ) -> Result<RenderCompletion> {
        #[cfg(test)]
        {
            self.stages = Default::default();
            self.last_commit_sources = None;
            self.last_commit_counts.clear();
        }
        // Confirm the frozen output before enumerating sources. It owns the
        // back buffer until equality/commit resolves; capture resumes promptly
        // afterwards so a queued latest source cannot be acknowledged as old pixels.
        let decision = self
            .output_pixels
            .poll(&self.graphics, scene.stamp)
            .map_err(native_error)?;
        if decision != output::Decision::Compose {
            #[cfg(test)]
            {
                self.stages.output_retry = true;
            }
            return self.resolve_output(scene, decision, true, commit);
        }
        #[cfg(test)]
        {
            self.capture.comparison_timing_source = self
                .paint_probe
                .as_ref()
                .filter(|_| self.profile_capture)
                .map(|(token, _)| *token);
        }
        #[cfg(test)]
        let capture_start = Instant::now();
        let regions = self.effects.regions(scene);
        let mut captured = Vec::new();
        #[cfg(test)]
        let mut changed = false;
        let mut comparison_waiting = false;
        if regions.is_empty() {
            self.capture.stop_capture();
            self.pending.applied();
        }
        if !regions.is_empty() {
            let request = ScreenCaptureRequest {
                revision: scene.stamp.revision,
                regions,
                paused_regions: scene.paused_regions.to_vec(),
                excluded_windows: scene
                    .excluded_windows
                    .iter()
                    .copied()
                    .chain(scene.surfaces.iter().map(|surface| surface.input_window))
                    .collect(),
            };
            #[cfg(test)]
            {
                // Injected frames bypass capture and must not reuse its previous timing.
                self.capture.last_gpu_enumeration = None;
                self.capture.last_gpu_enumeration_span = None;
            }
            #[cfg(test)]
            let frame = if let Some(frame) = self.sampled.take() {
                Some(frame)
            } else {
                self.capture.sample_gpu(&request).map_err(native_error)?
            };
            #[cfg(not(test))]
            let frame = self.capture.sample_gpu(&request).map_err(native_error)?;
            #[cfg(test)]
            {
                self.stages.enumeration = self.capture.last_gpu_enumeration;
                self.stages.capture_details[0] = self.capture.last_gpu_enumeration_span;
            }
            let Some(frame) = frame else {
                return Ok(RenderCompletion {
                    stamp: scene.stamp,
                    sampled_regions: Arc::from([]),
                    presented: false,
                    wait: Some(RenderWait::Source),
                });
            };
            #[cfg(test)]
            {
                self.stages.comparison_waiting = frame.comparison_waiting;
            }
            comparison_waiting = frame.comparison_waiting;
            #[cfg(test)]
            {
                changed = !frame.changed_regions.is_empty();
            }
            self.pending.capture(frame.changed_regions);
            #[cfg(test)]
            let mut view_index = 1;
            for layer in frame.layers {
                #[cfg(test)]
                if let Some((token, probe)) = &mut self.paint_probe
                    && let Some(source) = layer.source.filter(|source| source.token == *token)
                    && probe
                        .enqueue(&self.graphics, &layer.texture, source)
                        .is_err()
                {
                    probe.invalid += 1;
                }
                #[cfg(test)]
                let view_start = self.capture.enumeration_probe.then(Instant::now);
                let texture = Texture::wrap(&self.graphics.device, layer.texture, true, false)
                    .map_err(native_error)?;
                #[cfg(test)]
                if let Some(view_start) = view_start {
                    let view_end = Instant::now();
                    if let Some(span) = self.stages.capture_details.get_mut(view_index) {
                        *span = Some((view_start, view_end));
                    } else {
                        self.stages.capture_details_overflow = true;
                    }
                    view_index += 1;
                }
                captured.push(effects::CaptureLayer {
                    #[cfg(test)]
                    source: layer.source,
                    bounds: layer.bounds,
                    opacity: layer.opacity,
                    texture,
                });
            }
        }
        #[cfg(test)]
        {
            self.stages.capture = capture_start.elapsed();
            self.stages.cpu_spans[0] = Some((capture_start, capture_start + self.stages.capture));
            self.stages.source_changed = changed;
            if let Some(trace) = &mut self.gpu_trace {
                trace.mark(&self.graphics.context, 1);
            }
        }
        if !self.pending.needs_display() && self.presented_stamp == Some(scene.stamp) {
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions: self.effects.completed_regions(scene).into(),
                presented: true,
                wait: comparison_waiting.then_some(RenderWait::Source),
            });
        }
        let rebuild = self.surfaces.len() != scene.surfaces.len()
            || self
                .surfaces
                .iter()
                .zip(&*scene.surfaces)
                .any(|(surface, spec)| {
                    surface.spec.input_window != spec.input_window
                        || surface.spec.bounds != spec.bounds
                });
        if rebuild {
            self.presented_inputs = None;
            self.output_pixels.reset();
            self.surfaces.clear();
            for &spec in &*scene.surfaces {
                self.surfaces
                    .push(surface::Surface::new(&self.graphics, spec).map_err(native_error)?);
            }
        }
        #[cfg(test)]
        let mut ready = !self.defer_presentation;
        #[cfg(not(test))]
        let mut ready = true;
        for surface in &self.surfaces {
            ready &= surface.ready().map_err(native_error)?;
        }
        #[cfg(test)]
        {
            self.stages.compose_permit = Some(ready);
        }
        if !ready {
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions: Arc::from([]),
                presented: false,
                wait: Some(RenderWait::Presentation),
            });
        }
        let layers: Vec<_> = scene
            .layers
            .iter()
            .flat_map(RenderLayer::rasters)
            .chain(scene.interaction.iter())
            .collect();
        let used: HashSet<_> = layers.iter().map(|raster| raster.key).collect();
        let obsolete: Vec<_> = self
            .raster
            .keys()
            .filter(|key| !used.contains(key))
            .copied()
            .collect();
        let mut reusable: HashMap<u64, Vec<Texture>> = HashMap::new();
        for key in obsolete {
            reusable
                .entry(key.asset)
                .or_default()
                .push(self.raster.remove(&key).unwrap());
        }
        for raster in &layers {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.raster.entry(raster.key)
            {
                let texture = reusable
                    .get_mut(&raster.key.asset)
                    .and_then(Vec::pop)
                    .filter(|texture| texture.matches_bounds(raster.bounds));
                entry.insert(if let Some(texture) = texture {
                    texture.upload(&self.graphics.context, &raster.pixels);
                    texture
                } else {
                    Texture::create(
                        &self.graphics.device,
                        (raster.bounds.right - raster.bounds.left) as u32,
                        (raster.bounds.bottom - raster.bounds.top) as u32,
                        Some(&raster.pixels),
                    )
                    .map_err(native_error)?
                });
            }
        }
        #[cfg(test)]
        let effects_start = Instant::now();
        // The source textures are always latest-only; acknowledge metadata only
        // after effect preparation succeeds, independently of display readiness.
        let mut pending = std::mem::take(&mut self.pending);
        let applied = self.update_effects(scene, &captured, pending.changes());
        if applied.is_ok() {
            pending.applied();
        }
        self.pending = pending;
        applied?;
        #[cfg(test)]
        {
            self.stages.effects = effects_start.elapsed();
            self.stages.cpu_spans[1] = Some((effects_start, effects_start + self.stages.effects));
            if let Some(trace) = &mut self.gpu_trace {
                trace.mark(&self.graphics.context, 2);
            }
        }
        // Keep the last complete display while a new/translated paused output
        // has no initialized result; never replace it with a partial scene.
        if !self.effects.ready(scene) {
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions: self.effects.completed_regions(scene).into(),
                presented: false,
                wait: Some(RenderWait::Paused),
            });
        }
        let presentation_inputs = self.effects.presentation_inputs(scene);
        if self.presented_inputs.as_ref() == Some(&presentation_inputs) {
            self.presented_stamp = Some(scene.stamp);
            self.pending.presented();
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions: self.effects.completed_regions(scene).into(),
                presented: true,
                wait: comparison_waiting.then_some(RenderWait::Source),
            });
        }
        #[cfg(test)]
        let compose_start = Instant::now();
        for surface in &self.surfaces {
            self.graphics.clear(&surface.target);
            for layer in &*scene.layers {
                self.effects.draw_layer(
                    &self.graphics,
                    layer,
                    &self.raster,
                    &surface.target,
                    surface.spec.bounds,
                );
            }
            for raster in &*scene.interaction {
                effects::blit(
                    &self.graphics,
                    &self.raster[&raster.key],
                    raster.bounds,
                    &surface.target,
                    surface.spec.bounds,
                    1.,
                );
            }
        }
        #[cfg(test)]
        {
            self.stages.compose = compose_start.elapsed();
            self.stages.cpu_spans[2] = Some((compose_start, compose_start + self.stages.compose));
            if let Some(trace) = &mut self.gpu_trace {
                trace.mark(&self.graphics.context, 3);
            }
        }
        #[cfg(test)]
        let compare_start = Instant::now();
        let decision = self
            .output_pixels
            .stage(
                &self.graphics,
                &self.surfaces,
                presentation_inputs,
                scene.stamp,
            )
            .map_err(native_error)?;
        #[cfg(test)]
        {
            self.stages.compare = compare_start.elapsed();
            self.stages.cpu_spans[3] = Some((compare_start, compare_start + self.stages.compare));
            if let Some(trace) = &mut self.gpu_trace {
                trace.mark(&self.graphics.context, 4);
            }
            // End the complete capture/effects/compose/compare attempt before
            // the production batch Flush, not during its later commit retry.
            if let Some(trace) = self.gpu_trace.take() {
                self.gpu_profiler
                    .as_mut()
                    .unwrap()
                    .finish(&self.graphics.context, trace);
            }
        }
        // The comparison metadata and opt-in timestamp end markers enter the
        // same production submission. No query-triggered flush or GPU wait.
        unsafe {
            self.graphics.context.Flush();
        }
        self.resolve_output(scene, decision, comparison_waiting, commit)
    }

    /// Resolve only the staged output; retries do not enumerate captures or
    /// reprocess effects, and only an accepted native commit advances pixels.
    fn resolve_output(
        &mut self,
        scene: &AnnotationScene,
        decision: output::Decision,
        source_waiting: bool,
        commit: impl FnOnce(&[surface::Surface]) -> Result<bool>,
    ) -> Result<RenderCompletion> {
        let sampled_regions = self.effects.completed_regions(scene).into();
        if decision == output::Decision::Pending {
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions,
                presented: false,
                wait: Some(RenderWait::Source),
            });
        }
        debug_assert!(decision != output::Decision::Compose);
        let candidate_inputs = self.output_pixels.inputs().clone();
        let latest = candidate_inputs == self.effects.presentation_inputs(scene);
        if decision == output::Decision::Equal {
            self.presented_stamp = Some(scene.stamp);
            self.presented_inputs = Some(candidate_inputs);
            self.output_pixels.acknowledge_equal();
            if latest {
                self.pending.presented();
            }
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions,
                presented: true,
                wait: (source_waiting || !latest).then_some(RenderWait::Source),
            });
        }
        #[cfg(test)]
        let mut ready = !self.defer_presentation;
        #[cfg(not(test))]
        let mut ready = true;
        for surface in &self.surfaces {
            ready &= surface.ready().map_err(native_error)?;
        }
        #[cfg(test)]
        {
            self.stages.commit_permit = Some(ready);
        }
        if !ready {
            return Ok(RenderCompletion {
                stamp: scene.stamp,
                sampled_regions,
                presented: false,
                wait: Some(if source_waiting {
                    RenderWait::Source
                } else {
                    RenderWait::Presentation
                }),
            });
        }
        #[cfg(test)]
        let commit_start = Instant::now();
        #[cfg(test)]
        for surface in &self.surfaces {
            surface.composition_probe.set(self.composition_probe);
        }
        let presented = commit(&self.surfaces)?;
        #[cfg(test)]
        {
            self.stages.commit = commit_start.elapsed();
            self.stages.cpu_spans[4] = Some((commit_start, commit_start + self.stages.commit));
            if presented {
                for surface in &self.surfaces {
                    for (total, elapsed) in self
                        .stages
                        .submission_cpu
                        .iter_mut()
                        .zip(surface.submission_cpu())
                    {
                        *total += elapsed;
                    }
                }
            }
        }
        if presented {
            #[cfg(test)]
            {
                self.last_commit_sources = Some(candidate_inputs.sources.clone());
                self.last_commit_counts = self
                    .surfaces
                    .iter()
                    .filter_map(|surface| surface.submitted_count().ok())
                    .collect();
            }
            self.output_pixels.accept();
            self.presented_stamp = Some(scene.stamp);
            self.presented_inputs = Some(candidate_inputs);
            // Captures received during comparison still owe a display. Their
            // effects are prepared already; don't acknowledge them with older pixels.
            if latest {
                self.pending.presented();
            }
        }
        let sampled_regions = self.effects.completed_regions(scene).into();
        Ok(RenderCompletion {
            stamp: scene.stamp,
            sampled_regions,
            presented,
            wait: if source_waiting {
                Some(RenderWait::Source)
            } else if !presented {
                Some(RenderWait::Presentation)
            } else {
                (source_waiting || !latest).then_some(RenderWait::Source)
            },
        })
    }
}

fn native_error(error: windows::core::Error) -> Error {
    Error::new(error.to_string())
}

fn live_effect(layer: &RenderLayer) -> bool {
    match layer {
        RenderLayer::Raster(_) => false,
        RenderLayer::Magnifier(layer) => !layer.masks.is_empty(),
        RenderLayer::Mosaic(layer) => {
            !layer.masks.is_empty()
                && matches!(&layer.object.extra,
                lexift_core::domain::annotation::Extra::Mosaic(settings) if settings.strength != 0)
        }
    }
}

fn run(shared: Arc<(Mutex<Mailbox>, Condvar)>) {
    let (lock, wake) = &*shared;
    let signal = lock.lock().unwrap().signal.as_ref().unwrap().clone();
    // Capture backends can retire before their last front buffer. Keep the
    // thread apartment initialized until all presentation resources are gone.
    let mut presentation_apartment = None;
    let mut clock = scheduler::Clock::new()
        .map_err(|error| {
            tracing::warn!(%error, "Precise annotation timer unavailable; using event wait");
        })
        .ok();
    let mut renderer: Option<Renderer> = None;
    // Presentation ownership survives a bounded device rebuild independently of
    // capture/filter resources. Only accepted replacement pixels retire it.
    let mut retained_surfaces: Vec<surface::Surface> = Vec::new();
    let mut active_generation = 0;
    let mut active_session = None;
    let mut shown = None;
    let mut recovery = recovery::Recovery::default();
    let period = Duration::from_secs_f64(1. / 60.);
    let mut cadence = scheduler::Cadence::new(Instant::now(), period);
    loop {
        surface::pump_messages();
        let state = lock.lock().unwrap();
        if state.stopped {
            break;
        }
        #[cfg(test)]
        if state.disable_precise_clock {
            clock = None;
        }
        if state.generation != active_generation {
            active_generation = state.generation;
            drop(state);
            renderer = None;
            retained_surfaces.clear();
            active_session = None;
            shown = None;
            cadence.reset(Instant::now());
            #[cfg(test)]
            if let Some(clock) = &clock {
                clock.take_waits();
            }
            continue;
        }
        let scene = state.scene.clone();
        let needs_frame = scene.as_ref().is_some_and(|s| {
            shown != Some(s.stamp) || (state.error.is_none() && s.layers.iter().any(live_effect))
        });
        // Static scenes have no capture notices. Check the device on the
        // existing idle wake instead of relying on another UI scene submission.
        let idle_device_error = if !needs_frame && scene.is_some() {
            renderer.as_ref().and_then(Renderer::device_error)
        } else {
            None
        };
        #[cfg(test)]
        let idle_injected_loss = scene.is_some() && state.injected_device_losses > 0;
        #[cfg(not(test))]
        let idle_injected_loss = false;
        if !needs_frame && idle_device_error.is_none() && !idle_injected_loss {
            let _ = wake
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap();
            continue;
        }
        // A source notice belongs to this backend; a stale callback can wake
        // the event but cannot advance its successor's source attempt.
        let arrival = renderer
            .as_ref()
            .is_some_and(|renderer| renderer.capture.take_arrival());
        let remaining = cadence.remaining(Instant::now(), scene.as_ref().unwrap().stamp, arrival);
        if !remaining.is_zero() {
            if let Some(timer) = &clock {
                #[cfg(test)]
                timer.record(state.record_presentations && state.record_native_waits);
                drop(state);
                if let Err(error) = timer.wait(remaining, &signal) {
                    tracing::warn!(%error, "Precise annotation wait failed; using event wait");
                    clock = None;
                }
            } else {
                drop(state);
                #[cfg(test)]
                {
                    lock.lock().unwrap().fallback_waits += 1;
                }
                if let Err(error) = signal.wait(remaining) {
                    // The event is owned for the entire worker lifetime. A failed
                    // wait is terminal; retrying it would spin and flood logs.
                    let mut state = lock.lock().unwrap();
                    state.error = Some(native_error(error));
                    state.stopped = true;
                    break;
                }
            }
            continue;
        }
        drop(state);
        let frame_start = Instant::now();
        let scene = scene.unwrap();
        cadence.begin(frame_start, scene.stamp);
        recovery.enter(active_generation, scene.stamp.session);
        // Raster keys are scoped to a session. Never reuse old GPU assets when
        // a caller starts a new session without an intervening clear request.
        if active_session != Some(scene.stamp.session) {
            renderer = None;
            retained_surfaces.clear();
            active_session = Some(scene.stamp.session);
        }
        #[cfg(test)]
        let injected_loss = {
            let mut state = lock.lock().unwrap();
            let loss = state.injected_device_losses > 0;
            if loss {
                state.injected_device_losses -= 1;
            }
            loss
        };
        let result = (|| {
            if let Some(error) = idle_device_error {
                return Err(native_error(error));
            }
            #[cfg(test)]
            if injected_loss {
                return Err(Error::new("Injected annotation device removal"));
            }
            #[cfg(test)]
            if renderer.is_none() && lock.lock().unwrap().defer_device_rebuild {
                return Ok(RenderCompletion {
                    stamp: scene.stamp,
                    sampled_regions: Arc::from([]),
                    presented: false,
                    wait: Some(RenderWait::Recovering),
                });
            }
            if renderer.is_none() {
                if presentation_apartment.is_none() {
                    presentation_apartment = Some(
                        super::screen_capture::capture::Apartment::new().map_err(native_error)?,
                    );
                }
                let mut created = Renderer::new().map_err(native_error)?;
                let source_signal = signal.clone();
                let mailbox = Arc::downgrade(&shared);
                created
                    .capture
                    .set_notice(super::screen_capture::arrival::Notice::new(move || {
                        source_signal.wake();
                        if let Some(mailbox) = mailbox.upgrade() {
                            mailbox.1.notify_one();
                        }
                    }));
                renderer = Some(created);
                #[cfg(test)]
                {
                    lock.lock().unwrap().devices_created += 1;
                }
            }
            #[cfg(test)]
            if lock.lock().unwrap().record_presentations {
                let renderer = renderer.as_mut().unwrap();
                let profiler = renderer.gpu_profiler.get_or_insert_with(Default::default);
                renderer.gpu_trace = profiler
                    .begin(&renderer.graphics.device, &renderer.graphics.context)
                    .map_err(native_error)?;
            }
            #[cfg(test)]
            if let Some((token, profile_capture)) = lock.lock().unwrap().paint_probe_token {
                let r = renderer.as_mut().unwrap();
                if r.paint_probe.as_ref().is_none_or(|(old, _)| *old != token) {
                    r.paint_probe = Some((token, Default::default()));
                    r.profile_capture = profile_capture;
                }
            }
            #[cfg(test)]
            {
                renderer.as_mut().unwrap().composition_probe =
                    lock.lock().unwrap().composition_probe;
                renderer.as_mut().unwrap().capture.enumeration_probe =
                    lock.lock().unwrap().enumeration_probe;
                renderer.as_mut().unwrap().capture.completion_queries =
                    lock.lock().unwrap().completion_queries;
                renderer
                    .as_mut()
                    .unwrap()
                    .output_pixels
                    .disable_completion_events = lock.lock().unwrap().output_completion_reference;
            }
            renderer.as_mut().unwrap().render(&scene, |surfaces| {
                commit_current(&shared, active_generation, scene.stamp, || {
                    present_surfaces(surfaces)
                })
            })
        })();
        #[cfg(test)]
        let gpu_samples = if let Some(renderer) = renderer.as_mut()
            && let Some(profiler) = &mut renderer.gpu_profiler
        {
            if let Some(trace) = renderer.gpu_trace.take() {
                profiler.finish(&renderer.graphics.context, trace);
            }
            profiler
                .poll(&renderer.graphics.context)
                .expect("GPU timestamp poll failed")
        } else {
            Vec::new()
        };
        #[cfg(test)]
        let (observations, observation_errors) = {
            let recording = lock.lock().unwrap().record_presentations;
            if let Some(r) = renderer.as_mut().filter(|r| {
                recording
                    && r.last_display_query
                        .is_none_or(|at| at.elapsed() >= Duration::from_millis(2))
            }) {
                r.last_display_query = Some(Instant::now());
                let mut observations = Vec::new();
                let mut errors = 0;
                for surface in &r.surfaces {
                    match surface.display_observation() {
                        Ok(observation) => observations.push(observation),
                        Err(_) => errors += 1,
                    }
                }
                (observations, errors)
            } else {
                (Vec::new(), 0)
            }
        };
        let mut state = lock.lock().unwrap();
        if !state.owns(active_generation, scene.stamp) {
            continue;
        }
        #[cfg(test)]
        {
            if let Some(clock) = &clock {
                let (waits, dropped) = clock.take_waits();
                if state.record_presentations {
                    let available = 65_536usize.saturating_sub(state.native_waits.len());
                    state.native_waits_dropped += dropped + waits.len().saturating_sub(available);
                    state.native_waits.extend(waits.into_iter().take(available));
                }
            }
            state.captures = renderer.as_ref().map_or(0, |r| r.capture.capture_count());
            if let Some(r) = renderer.as_mut()
                && let Some((_, probe)) = &mut r.paint_probe
            {
                // Nonblocking and bounded; pending work never delays presentation.
                if probe.poll(&r.graphics).is_err() {
                    probe.invalid += 1;
                }
                let available = 16_384usize.saturating_sub(state.paint_samples.len());
                state.paint_probe_summary.0 += probe.samples.len().saturating_sub(available);
                state
                    .paint_samples
                    .extend(probe.samples.drain(..).take(available));
                state.paint_probe_summary.0 += std::mem::take(&mut probe.dropped);
                state.paint_probe_summary.1 += std::mem::take(&mut probe.invalid);
                state.paint_probe_summary.2 = probe.pending();
                let available = 16_384usize.saturating_sub(state.paint_probe_summary.3.len());
                state
                    .paint_probe_summary
                    .3
                    .extend(probe.cpu.drain(..).take(available));
            }
            // Drain during warmup too: its observations must not consume the
            // measured interval's bounded capacity or count as lost samples.
            let (events, dropped) = renderer.as_mut().map_or((Vec::new(), 0), |renderer| {
                renderer.capture.take_diagnostics()
            });
            if state.record_presentations {
                let available = 16_384usize.saturating_sub(state.display_observations.len());
                state.display_observations_dropped += observations.len().saturating_sub(available);
                state.display_observation_errors += observation_errors;
                state
                    .display_observations
                    .extend(observations.into_iter().take(available));
                if let Some(sources) = renderer.as_mut().and_then(|r| r.last_commit_sources.take())
                {
                    if state.source_presentations.len() < 16_384 {
                        if let Some(at) = state.presentations.last().copied() {
                            let renderer = renderer.as_ref().unwrap();
                            let native_commit = if renderer.surfaces.len() == 1
                                && renderer.last_commit_counts.len() == 1
                            {
                                renderer
                                    .surfaces
                                    .first()
                                    .and_then(surface::Surface::native_commit)
                            } else {
                                None
                            };
                            state.source_presentations.push(performance::Presentation {
                                stamp: scene.stamp,
                                native_commit,
                                at,
                                sources,
                                counts: renderer.last_commit_counts.clone(),
                            });
                        }
                    } else {
                        state.source_presentations_dropped += 1;
                    }
                }
                let available = 131_072usize.saturating_sub(state.capture_events.len());
                state.capture_events_dropped += dropped + events.len().saturating_sub(available);
                state
                    .capture_events
                    .extend(events.into_iter().take(available));
                state.gpu_samples.extend(gpu_samples);
                if let Some(profiler) = renderer
                    .as_ref()
                    .and_then(|renderer| renderer.gpu_profiler.as_ref())
                {
                    state.gpu_query_dropped = profiler.dropped;
                    state.gpu_query_invalid = profiler.invalid;
                    state.gpu_query_pending = profiler.pending_count();
                }
                state.attempts.push(performance::Attempt {
                    stamp: scene.stamp,
                    at: frame_start,
                    work: frame_start.elapsed(),
                    stages: renderer
                        .as_ref()
                        .map_or(Default::default(), |renderer| renderer.stages),
                    wait: result.as_ref().ok().and_then(|frame| frame.wait.clone()),
                });
            }
        }
        let mut release_retained = false;
        match result {
            Ok(frame) => {
                if matches!(frame.wait, Some(RenderWait::Source)) {
                    cadence.retry_source(Instant::now());
                }
                if frame.presented {
                    shown = Some(frame.stamp);
                    release_retained = true;
                }
                state.completion = Some(frame);
                state.error = None;
            }
            Err(error) => {
                let recoverable = renderer.as_ref().is_none_or(Renderer::device_removed);
                #[cfg(test)]
                let recoverable = recoverable || injected_loss;
                if recovery.request(recoverable) {
                    tracing::warn!(%error, session = scene.stamp.session, "annotation GPU device failed; rebuilding once");
                    state.completion = Some(RenderCompletion {
                        stamp: scene.stamp,
                        sampled_regions: Arc::from([]),
                        presented: false,
                        wait: Some(RenderWait::Recovering),
                    });
                    state.error = None;
                    shown = None;
                    #[cfg(test)]
                    {
                        state.captures = 0;
                    }
                    // Keep only a display that reached an accepted presentation.
                    // New surfaces without a front buffer cannot replace it.
                    let displayed = renderer
                        .as_ref()
                        .is_some_and(|r| r.presented_stamp.is_some());
                    // Driver/context release must never hold the mailbox.
                    drop(state);
                    if displayed {
                        retained_surfaces =
                            std::mem::take(&mut renderer.as_mut().unwrap().surfaces);
                    }
                    renderer = None;
                    continue;
                }
                state.error = Some(error);
                shown = Some(scene.stamp);
            }
        }
        drop(state);
        if release_retained {
            retained_surfaces.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence_scene(revision: u64) -> Arc<AnnotationScene> {
        Arc::new(AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision,
            },
            surfaces: Arc::from([]),
            layers: Arc::from([]),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        })
    }

    #[test]
    fn worker_without_precise_timer_handles_submit_clear_and_shutdown() {
        let shared = Arc::new((
            Mutex::new(Mailbox {
                signal: Some(Arc::new(scheduler::Signal::new().unwrap())),
                disable_precise_clock: true,
                // Exercise scheduling without creating graphics resources.
                defer_device_rebuild: true,
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let worker_state = shared.clone();
        let port = WindowsAnnotationRenderPort {
            shared: shared.clone(),
            thread: Some(thread::spawn(move || run(worker_state))),
        };
        port.submit(fence_scene(1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.0.lock().unwrap().fallback_waits == 0 {
            assert!(
                Instant::now() < deadline,
                "worker did not enter event fallback"
            );
            thread::sleep(Duration::from_millis(1));
        }
        port.submit(fence_scene(2)).unwrap();
        while !port
            .latest()
            .unwrap()
            .is_some_and(|frame| frame.stamp.revision == 2)
        {
            assert!(
                Instant::now() < deadline,
                "fallback did not handle scene revision"
            );
            thread::sleep(Duration::from_millis(1));
        }
        port.clear();
        assert!(port.latest().unwrap().is_none());
        port.submit(fence_scene(3)).unwrap();
        while !port
            .latest()
            .unwrap()
            .is_some_and(|frame| frame.stamp.revision == 3)
        {
            assert!(
                Instant::now() < deadline,
                "fallback did not resume after clear"
            );
            thread::sleep(Duration::from_millis(1));
        }
        drop(port);
        assert!(shared.0.lock().unwrap().stopped);
    }

    #[test]
    fn cancel_during_preparation_prevents_native_commit() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let shared = Arc::new((
            Mutex::new(Mailbox {
                scene: Some(fence_scene(1)),
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let prepared = Arc::new(Barrier::new(2));
        let cancelled = Arc::new(Barrier::new(2));
        let commits = Arc::new(AtomicUsize::new(0));
        let worker = {
            let shared = shared.clone();
            let prepared = prepared.clone();
            let cancelled = cancelled.clone();
            let commits = commits.clone();
            thread::spawn(move || {
                prepared.wait();
                cancelled.wait();
                commit_current(
                    &shared,
                    0,
                    RenderStamp {
                        session: 1,
                        revision: 1,
                    },
                    || {
                        commits.fetch_add(1, Ordering::Relaxed);
                        Ok(true)
                    },
                )
                .unwrap()
            })
        };
        prepared.wait();
        {
            let mut state = shared.0.lock().unwrap();
            state.generation += 1;
            state.scene = None;
        }
        cancelled.wait();
        assert!(!worker.join().unwrap());
        assert_eq!(commits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn newer_scene_and_shutdown_reject_old_commits() {
        let shared = (
            Mutex::new(Mailbox {
                scene: Some(fence_scene(2)),
                ..Default::default()
            }),
            Condvar::new(),
        );
        assert!(
            !commit_current(
                &shared,
                0,
                RenderStamp {
                    session: 1,
                    revision: 1
                },
                || panic!("stale scene committed")
            )
            .unwrap()
        );
        assert!(
            commit_current(
                &shared,
                0,
                RenderStamp {
                    session: 1,
                    revision: 2
                },
                || Ok(true)
            )
            .unwrap()
        );
        shared.0.lock().unwrap().stopped = true;
        assert!(
            !commit_current(
                &shared,
                0,
                RenderStamp {
                    session: 1,
                    revision: 2
                },
                || panic!("closed renderer committed")
            )
            .unwrap()
        );
    }

    #[test]
    fn submit_and_clear_are_ordered_after_an_authorized_commit() {
        use std::sync::mpsc;
        let shared = Arc::new((
            Mutex::new(Mailbox {
                scene: Some(fence_scene(1)),
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let (start, ready) = mpsc::channel();
        let (attempt, attempted) = mpsc::channel();
        let (finish, finished) = mpsc::channel();
        let writer = {
            let shared = shared.clone();
            thread::spawn(move || {
                ready.recv().unwrap();
                attempt.send(()).unwrap();
                let mut state = shared.0.lock().unwrap();
                state.generation += 1;
                state.scene = None;
                finish.send(()).unwrap();
            })
        };
        assert!(
            commit_current(
                &shared,
                0,
                RenderStamp {
                    session: 1,
                    revision: 1
                },
                || {
                    start.send(()).unwrap();
                    attempted.recv_timeout(Duration::from_secs(1)).unwrap();
                    assert!(matches!(
                        finished.try_recv(),
                        Err(mpsc::TryRecvError::Empty)
                    ));
                    Ok(true)
                }
            )
            .unwrap()
        );
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
        writer.join().unwrap();
        assert!(
            !commit_current(
                &shared,
                0,
                RenderStamp {
                    session: 1,
                    revision: 1
                },
                || panic!("cleared scene committed")
            )
            .unwrap()
        );
    }
    #[test]
    #[ignore = "requires a Windows D3D11 hardware device"]
    fn gpu_overlay_revisions_reuse_effects_and_source_changes_refresh_pixels() {
        use lexift_core::domain::{
            annotation::{Bounds, Extra, Kind, MosaicAnnotation, MosaicEffect, Object, Style},
            geometry::Rect,
        };
        let mut renderer = Renderer::new().unwrap();
        let bounds = Rect {
            left: -4,
            top: 0,
            right: 0,
            bottom: 4,
        };
        let mask = RasterLayer {
            key: RasterKey {
                asset: 1,
                revision: 1,
            },
            bounds,
            pixels: vec![255; 64].into(),
        };
        renderer.raster.insert(
            mask.key,
            Texture::create(&renderer.graphics.device, 4, 4, Some(&mask.pixels)).unwrap(),
        );
        let mut scene = AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision: 1,
            },
            layers: vec![RenderLayer::Mosaic(MosaicLayer {
                object: Arc::new(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::MosaicArea,
                    bounds: Bounds::from_corners((-4., 0.), (0., 4.)),
                    points: Vec::new(),
                    style: Style::default(),
                    extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 12., 1.)),
                }),
                masks: vec![mask.clone()].into(),
            })]
            .into(),
            surfaces: Arc::from([]),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        let source = |color: [u8; 4]| {
            vec![effects::CaptureLayer {
                source: None,
                bounds,
                opacity: 255,
                texture: Texture::create(&renderer.graphics.device, 4, 4, Some(&color.repeat(16)))
                    .unwrap(),
            }]
        };
        let red = source([200, 0, 0, 255]);
        let green = source([0, 200, 0, 255]);
        assert!(renderer.update_effects(&scene, &red, false).unwrap());
        let target = Texture::create(&renderer.graphics.device, 4, 4, None).unwrap();
        let read = |renderer: &Renderer, scene: &AnnotationScene| {
            renderer.graphics.clear(&target);
            renderer.effects.draw_layer(
                &renderer.graphics,
                &scene.layers[0],
                &renderer.raster,
                &target,
                bounds,
            );
            graphics::tests::readback(&renderer.graphics, &target)
        };
        assert_eq!(read(&renderer, &scene), [200, 0, 0, 255].repeat(16));
        scene.stamp.revision += 1;
        scene.interaction = vec![mask].into();
        assert!(
            !renderer.update_effects(&scene, &green, false).unwrap(),
            "overlay must not recompute effects"
        );
        assert_eq!(read(&renderer, &scene), [200, 0, 0, 255].repeat(16));
        assert!(
            renderer.update_effects(&scene, &green, true).unwrap(),
            "new source must recompute without a document revision"
        );
        assert_eq!(read(&renderer, &scene), [0, 200, 0, 255].repeat(16));
    }
    use lexift_core::{domain::geometry::Rect, ports::magnifier::WindowToken};
    use windows::{
        Win32::{
            Foundation::{COLORREF, HWND},
            Graphics::Gdi::{GetDC, GetPixel, ReleaseDC},
            UI::WindowsAndMessaging::*,
        },
        core::w,
    };

    struct InputWindow(HWND);
    impl Drop for InputWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    #[test]
    #[ignore = "requires Windows D3D11 and an owned native input window"]
    fn gpu_single_source_change_survives_unready_surface_and_rejected_commit() {
        use super::super::screen_capture::capture::{GpuCapture, GpuCaptureLayer};
        use lexift_core::domain::annotation::{
            Bounds, Extra, Kind, MosaicAnnotation, MosaicEffect, Object, Style,
        };
        let mut renderer = Renderer::new().unwrap();
        let bounds = Rect {
            left: 400,
            top: 400,
            right: 404,
            bottom: 404,
        };
        let input = InputWindow(unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Lexift deferred presentation fixture"),
                WS_POPUP,
                400,
                400,
                4,
                4,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let scene = AnnotationScene {
            stamp: RenderStamp {
                session: 99,
                revision: 1,
            },
            surfaces: vec![RenderSurface {
                input_window: WindowToken(input.0.0 as isize),
                bounds,
                scale: 1.,
            }]
            .into(),
            layers: vec![RenderLayer::Mosaic(MosaicLayer {
                object: Arc::new(Object {
                    erased: Vec::new(),
                    id: Default::default(),
                    kind: Kind::MosaicArea,
                    bounds: Bounds::from_corners((400., 400.), (404., 404.)),
                    points: Vec::new(),
                    style: Style::default(),
                    extra: Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 1, 12., 1.)),
                }),
                masks: vec![RasterLayer {
                    key: RasterKey {
                        asset: 1,
                        revision: 1,
                    },
                    bounds,
                    pixels: vec![255; 64].into(),
                }]
                .into(),
            })]
            .into(),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        let frame = |renderer: &Renderer, color: [u8; 4], changed| GpuCapture {
            comparison_waiting: false,
            changed_regions: if changed { vec![bounds] } else { Vec::new() },
            layers: vec![GpuCaptureLayer {
                source: Some(crate::windows::screen_capture::diagnostics::Published {
                    token: 7,
                    sequence: u32::from_le_bytes(color) as u64,
                    ticks: u32::from_le_bytes(color) as i64,
                }),
                bounds,
                opacity: 255,
                texture: Texture::create(&renderer.graphics.device, 4, 4, Some(&color.repeat(16)))
                    .unwrap()
                    ._resource,
            }],
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            renderer.sampled = Some(frame(&renderer, [200, 0, 0, 255], true));
            if renderer.render(&scene, |_| Ok(true)).unwrap().presented {
                break;
            }
            assert!(Instant::now() < deadline);
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        // Exactly one changed sample, then a stationary source with no further
        // invalidations. Delaying either preparation or commit must not lose it.
        renderer.defer_presentation = true;
        renderer.sampled = Some(frame(&renderer, [0, 200, 0, 255], true));
        let blocked = renderer
            .render(&scene, |_| panic!("unready surface committed"))
            .unwrap();
        assert!(matches!(blocked.wait, Some(RenderWait::Presentation)));
        assert_eq!(renderer.stages.compose_permit, Some(false));
        assert_eq!(renderer.stages.commit_permit, None);
        assert!(!blocked.presented);
        assert!(renderer.pending.needs_display());
        renderer.defer_presentation = false;
        renderer.sampled = Some(frame(&renderer, [0, 200, 0, 255], false));
        let mut commits = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        let rejected = loop {
            renderer.sampled = Some(frame(&renderer, [0, 200, 0, 255], false));
            let completion = renderer
                .render(&scene, |_| {
                    commits += 1;
                    Ok(false)
                })
                .unwrap();
            if commits > 0 {
                break completion;
            }
            assert!(matches!(completion.wait, Some(RenderWait::Source)));
            assert!(
                Instant::now() < deadline,
                "output comparison did not complete"
            );
            thread::sleep(Duration::from_millis(1));
        };
        assert!(!rejected.presented);
        assert_eq!(commits, 1, "stationary source must still reach commit");
        let inputs = renderer.effects.presentation_inputs(&scene);
        renderer.sampled = Some(frame(&renderer, [0, 200, 0, 255], false));
        assert!(
            renderer
                .render(&scene, |_| {
                    commits += 1;
                    Ok(true)
                })
                .unwrap()
                .presented
        );
        assert_eq!(
            commits, 2,
            "failed commit must retry without a new source frame"
        );
        assert!(
            inputs == renderer.effects.presentation_inputs(&scene),
            "commit retry must reuse prepared effects"
        );
        let target = Texture::create(&renderer.graphics.device, 4, 4, None).unwrap();
        renderer.graphics.clear(&target);
        renderer.effects.draw_layer(
            &renderer.graphics,
            &scene.layers[0],
            &renderer.raster,
            &target,
            bounds,
        );
        assert_eq!(
            graphics::tests::readback(&renderer.graphics, &target),
            [0, 200, 0, 255].repeat(16)
        );
        renderer.sampled = Some(frame(&renderer, [0, 200, 0, 255], false));
        assert!(
            renderer
                .render(&scene, |_| panic!("settled frame resubmitted"))
                .unwrap()
                .presented
        );
        // Queued captures during output comparison must not starve the frozen
        // candidate, or acknowledge the newer source with its older pixels.
        renderer.sampled = Some(frame(&renderer, [0, 0, 200, 255], true));
        assert!(matches!(
            renderer
                .render(&scene, |_| panic!("comparison must wait"))
                .unwrap()
                .wait,
            Some(RenderWait::Source)
        ));
        let candidate_inputs = renderer.output_pixels.inputs().clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        renderer.sampled = Some(frame(&renderer, [180, 0, 180, 255], true));
        loop {
            if renderer.sampled.is_none() {
                renderer.sampled = Some(frame(&renderer, [180, 0, 180, 255], false));
            }
            let completion = renderer.render(&scene, |_| Ok(true)).unwrap();
            if completion.presented {
                assert!(
                    matches!(completion.wait, Some(RenderWait::Source)),
                    "retired output must request a prompt latest-source attempt"
                );
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            renderer.stages.output_retry,
            "staged output must use the confirmation phase"
        );
        assert_eq!(
            renderer.stages.capture,
            Duration::ZERO,
            "confirmation retry re-enumerated capture"
        );
        assert!(renderer.presented_inputs.as_ref() == Some(&candidate_inputs));
        assert_eq!(
            renderer.last_commit_sources.as_ref(),
            Some(&candidate_inputs.sources)
        );
        assert!(
            renderer.effects.presentation_inputs(&scene) == candidate_inputs,
            "output retry must not re-enumerate or prepare a newer capture"
        );
        assert!(
            renderer.sampled.is_some(),
            "new source must remain available after old candidate commits"
        );
        let mut sampled_new_source = false;
        loop {
            if renderer.sampled.is_none() {
                renderer.sampled = Some(frame(&renderer, [180, 0, 180, 255], false));
            }
            let completion = renderer.render(&scene, |_| Ok(true)).unwrap();
            if !sampled_new_source
                && renderer.effects.presentation_inputs(&scene) != candidate_inputs
            {
                sampled_new_source = true;
                assert!(
                    renderer.pending.needs_display(),
                    "sampled source must retain debt until commit"
                );
            }
            if completion.presented {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(sampled_new_source);
        assert!(!renderer.pending.needs_display());
        let mut revised = scene.clone();
        revised.stamp.revision += 1;
        let mut waiting = frame(&renderer, [180, 0, 180, 255], false);
        waiting.comparison_waiting = true;
        renderer.sampled = Some(waiting);
        let equal = renderer
            .render(&revised, |_| panic!("same inputs resubmitted"))
            .unwrap();
        assert!(equal.presented);
        assert!(
            matches!(equal.wait, Some(RenderWait::Source)),
            "metadata equality erased capture confirmation wait"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut changed = true;
        loop {
            let mut waiting = frame(&renderer, [50, 50, 50, 255], changed);
            changed = false;
            waiting.comparison_waiting = true;
            renderer.sampled = Some(waiting);
            let completion = renderer.render(&revised, |_| Ok(true)).unwrap();
            assert!(
                matches!(completion.wait, Some(RenderWait::Source)),
                "output resolution erased capture confirmation wait"
            );
            if completion.presented {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }

        assert!(
            renderer.presented_inputs.as_ref()
                == Some(&renderer.effects.presentation_inputs(&scene))
        );
        // A new scene arriving while a comparison owns the back buffer must
        // retire that candidate without presenting its old interaction layer.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            renderer.sampled = Some(frame(&renderer, [100, 100, 100, 255], true));
            let completion = renderer
                .render(&revised, |_| panic!("staging committed early"))
                .unwrap();
            if matches!(completion.wait, Some(RenderWait::Source)) {
                break;
            }
            assert!(Instant::now() < deadline);
            surface::pump_messages();
            thread::sleep(Duration::from_millis(1));
        }
        let mut newest = revised.clone();
        newest.stamp.revision += 1;
        let overlay = [200, 10, 80, 255].repeat(16);
        newest.interaction = vec![RasterLayer {
            key: RasterKey {
                asset: 999,
                revision: 1,
            },
            bounds,
            pixels: overlay.clone().into(),
        }]
        .into();
        let mut new_commits = 0;
        loop {
            renderer.sampled = Some(frame(&renderer, [100, 100, 100, 255], false));
            let completion = renderer
                .render(&newest, |_| {
                    new_commits += 1;
                    Ok(true)
                })
                .unwrap();
            if completion.presented {
                break;
            }
            assert!(Instant::now() < deadline);
            surface::pump_messages();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(new_commits, 1);
        assert_eq!(renderer.presented_stamp, Some(newest.stamp));
        assert_eq!(
            // RasterLayer uploads RGBA; swap-chain readback is BGRA.
            graphics::tests::readback(&renderer.graphics, &renderer.surfaces[0].target),
            [80, 10, 200, 255].repeat(16)
        );
    }

    fn desktop_pixel(point: (i32, i32)) -> [u8; 3] {
        unsafe {
            let dc = GetDC(None);
            let color = GetPixel(dc, point.0, point.1).0;
            ReleaseDC(None, dc);
            [color as u8, (color >> 8) as u8, (color >> 16) as u8]
        }
    }

    fn wait_pixel(port: &WindowsAnnotationRenderPort, expected: [u8; 3], point: (i32, i32)) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            surface::pump_messages();
            port.latest().expect("native rendering must succeed");
            let actual = desktop_pixel(point);
            if actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {expected:?}, actual {actual:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    #[ignore = "opens owned native windows; requires an interactive Windows desktop"]
    fn gpu_premultiplied_layers_present_and_clear_on_real_desktop() {
        let input = InputWindow(unsafe {
            let input = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                w!("STATIC"),
                w!("Lexift GPU Test Input"),
                WS_POPUP,
                400,
                400,
                100,
                80,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            SetLayeredWindowAttributes(input, COLORREF(0), 1, LWA_ALPHA).unwrap();
            let _ = ShowWindow(input, SW_SHOWNOACTIVATE);
            input
        });
        let port = WindowsAnnotationRenderPort::new().expect("WGC available");
        let bounds = Rect {
            left: 400,
            top: 400,
            right: 500,
            bottom: 480,
        };
        let make_scene = |revision, pixels: [u8; 4]| AnnotationScene {
            stamp: RenderStamp {
                session: 1,
                revision,
            },
            surfaces: vec![RenderSurface {
                input_window: WindowToken(input.0.0 as isize),
                bounds,
                scale: 1.,
            }]
            .into(),
            layers: vec![RenderLayer::Raster(RasterLayer {
                key: RasterKey { asset: 1, revision },
                bounds,
                pixels: pixels.repeat(100 * 80).into(),
            })]
            .into(),
            interaction: Arc::from([]),
            paused_regions: Arc::from([]),
            excluded_windows: Arc::from([]),
        };
        port.submit(Arc::new(make_scene(1, [20, 100, 180, 255])))
            .unwrap();
        wait_pixel(&port, [20, 100, 180], (450, 440));
        let mut scene = make_scene(2, [20, 100, 180, 255]);
        scene.interaction = vec![RasterLayer {
            key: RasterKey {
                asset: 2,
                revision: 1,
            },
            bounds,
            pixels: [64, 0, 0, 128].repeat(100 * 80).into(),
        }]
        .into();
        port.submit(Arc::new(scene)).unwrap();
        wait_pixel(&port, [74, 50, 90], (450, 440));
        let completion = port.latest().unwrap().unwrap();
        assert_eq!(completion.stamp.revision, 2);
        assert!(completion.presented);
        // Identical asset/revision numbers in a new session must upload new
        // pixels rather than showing the previous session's cached texture.
        let mut reopened = make_scene(1, [180, 60, 20, 255]);
        reopened.stamp.session = 2;
        port.submit(Arc::new(reopened)).unwrap();
        wait_pixel(&port, [180, 60, 20], (450, 440));
        for revision in 2..10 {
            let color = [20 + revision as u8 * 10, 120, 80, 255];
            let mut scene = make_scene(revision, color);
            scene.stamp.session = 2;
            port.submit(Arc::new(scene)).unwrap();
            wait_pixel(&port, [color[0], color[1], color[2]], (450, 440));
        }
        let previous_devices = {
            let mut state = port.shared.0.lock().unwrap();
            state.injected_device_losses = 1;
            state.defer_device_rebuild = true;
            state.devices_created
        };
        let mut recovered = make_scene(10, [40, 180, 60, 255]);
        recovered.stamp.session = 2;
        port.submit(Arc::new(recovered)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while port
            .latest()
            .unwrap()
            .is_none_or(|frame| !matches!(frame.wait, Some(RenderWait::Recovering)))
        {
            assert!(
                Instant::now() < deadline,
                "device loss did not enter recovery"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..20 {
            surface::pump_messages();
            let pixels = desktop_pixel((450, 440));
            assert!(
                pixels
                    .iter()
                    .zip([110, 120, 80])
                    .all(|(a, b)| a.abs_diff(b) <= 2),
                "rebuild removed the last successful display: {pixels:?}"
            );
            assert_eq!(port_window_count(), 1);
            thread::sleep(Duration::from_millis(5));
        }
        port.shared.0.lock().unwrap().defer_device_rebuild = false;
        port.shared.1.notify_one();
        wait_pixel(&port, [40, 180, 60], (450, 440));
        let deadline = Instant::now() + Duration::from_secs(3);
        while port_window_count() != 1 {
            assert!(
                Instant::now() < deadline,
                "accepted replacement retained an old window"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            port.shared.0.lock().unwrap().devices_created,
            previous_devices + 1
        );
        port.shared.0.lock().unwrap().injected_device_losses = 1;
        let mut second_loss = make_scene(11, [200, 20, 30, 255]);
        second_loss.stamp.session = 2;
        port.submit(Arc::new(second_loss)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while port.latest().is_ok() {
            assert!(
                Instant::now() < deadline,
                "repeated device loss was not surfaced for software fallback"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            port.shared.0.lock().unwrap().devices_created,
            previous_devices + 1,
            "recovery budget must not reset on a successful frame"
        );
        port.clear();
        drop(port); // Join guarantees no renderer HWND or retained GPU context remains.
        assert!(port_window_count() == 0);

        // UI content deduplication must not make static scenes dependent on
        // another submit to notice removal and rebuild the accepted scene.
        let idle = WindowsAnnotationRenderPort::new().unwrap();
        idle.submit(Arc::new(make_scene(1, [20, 100, 180, 255])))
            .unwrap();
        wait_pixel(&idle, [20, 100, 180], (450, 440));
        let idle_stamp = idle.latest().unwrap().unwrap().stamp;
        let idle_devices = {
            let mut state = idle.shared.0.lock().unwrap();
            state.injected_device_losses = 1;
            state.defer_device_rebuild = true;
            state.devices_created
        };
        // No submit or wake: the idle health check must pick this up itself.
        let deadline = Instant::now() + Duration::from_secs(3);
        while idle
            .latest()
            .unwrap()
            .is_none_or(|frame| !matches!(frame.wait, Some(RenderWait::Recovering)))
        {
            assert!(
                Instant::now() < deadline,
                "idle device loss waited for a new UI scene"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(idle.latest().unwrap().unwrap().stamp, idle_stamp);
        assert_eq!(port_window_count(), 1);
        idle.shared.0.lock().unwrap().defer_device_rebuild = false;
        idle.shared.1.notify_one();
        let deadline = Instant::now() + Duration::from_secs(3);
        while idle.latest().unwrap().is_none_or(|frame| !frame.presented)
            || port_window_count() != 1
        {
            assert!(
                Instant::now() < deadline,
                "static scene did not finish its own recovery"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(idle.latest().unwrap().unwrap().stamp, idle_stamp);
        assert_eq!(
            idle.shared.0.lock().unwrap().devices_created,
            idle_devices + 1
        );
        wait_pixel(&idle, [20, 100, 180], (450, 440));
        save_owned_snapshot(bounds, "target/annotation-idle-recovered.bmp");
        idle.clear();
        drop(idle);
        assert_eq!(port_window_count(), 0);

        // Clear while the old front buffer is specifically retained and the
        // replacement device has not been allowed to exist yet.
        let clearing = WindowsAnnotationRenderPort::new().unwrap();
        clearing
            .submit(Arc::new(make_scene(1, [20, 100, 180, 255])))
            .unwrap();
        wait_pixel(&clearing, [20, 100, 180], (450, 440));
        {
            let mut state = clearing.shared.0.lock().unwrap();
            state.injected_device_losses = 1;
            state.defer_device_rebuild = true;
        }
        clearing
            .submit(Arc::new(make_scene(2, [200, 20, 30, 255])))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while clearing
            .latest()
            .unwrap()
            .is_none_or(|frame| !matches!(frame.wait, Some(RenderWait::Recovering)))
        {
            assert!(
                Instant::now() < deadline,
                "clear fixture did not retain a display"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(port_window_count(), 1);
        clearing.clear();
        let deadline = Instant::now() + Duration::from_secs(3);
        while port_window_count() != 0 {
            assert!(
                Instant::now() < deadline,
                "clear retained an old recovery window"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(clearing.latest().unwrap().is_none());
        drop(clearing);
        assert_eq!(port_window_count(), 0);

        // Prepare an actual red back buffer, then invalidate its epoch before
        // native submission. The desktop must retain the earlier blue frame.
        let mut renderer = Renderer::new().unwrap();
        let mut original = make_scene(10, [20, 100, 180, 255]);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !renderer
            .render(&original, present_surfaces)
            .unwrap()
            .presented
        {
            assert!(Instant::now() < deadline);
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        let desktop_pixel = || {
            use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
            unsafe {
                let dc = GetDC(None);
                let pixel = GetPixel(dc, 450, 440).0;
                ReleaseDC(None, dc);
                [
                    (pixel & 255) as u8,
                    ((pixel >> 8) & 255) as u8,
                    ((pixel >> 16) & 255) as u8,
                ]
            }
        };
        while desktop_pixel()
            .iter()
            .zip([20, 100, 180])
            .any(|(a, b)| a.abs_diff(b) > 2)
        {
            assert!(
                Instant::now() < deadline,
                "initial direct scene not visible"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        let mut equivalent = original.clone();
        equivalent.stamp.revision += 1;
        equivalent.layers = original.layers.to_vec().into();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = renderer
                .render(&equivalent, |_| {
                    panic!("identical visible commands must not submit a frame")
                })
                .unwrap();
            if frame.presented {
                break;
            }
            assert!(Instant::now() < deadline);
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        // A new immutable asset key and different pixels hidden below the
        // opaque foreground change commands, but not the final composition.
        let mut pixel_equivalent = equivalent.clone();
        pixel_equivalent.stamp.revision += 1;
        let mut pixel_layers = pixel_equivalent.layers.to_vec();
        if let RenderLayer::Raster(foreground) = &mut pixel_layers[0] {
            foreground.key.revision += 100;
            let bounds = foreground.bounds;
            pixel_layers.insert(
                0,
                RenderLayer::Raster(RasterLayer {
                    key: RasterKey {
                        asset: 99,
                        revision: 1,
                    },
                    bounds,
                    pixels: [250, 10, 200, 255]
                        .repeat(
                            (bounds.right - bounds.left) as usize
                                * (bounds.bottom - bounds.top) as usize,
                        )
                        .into(),
                }),
            );
        }
        pixel_equivalent.layers = pixel_layers.into();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if renderer
                .render(&pixel_equivalent, |_| {
                    panic!("equal composed pixels must not submit")
                })
                .unwrap()
                .presented
            {
                break;
            }
            assert!(Instant::now() < deadline);
            surface::pump_messages();
            thread::sleep(Duration::from_millis(1));
        }
        equivalent = pixel_equivalent;
        let mut offscreen = equivalent.clone();
        offscreen.stamp.revision += 1;
        let mut layers = offscreen.layers.to_vec();
        layers.push(RenderLayer::Raster(RasterLayer {
            key: RasterKey {
                asset: 90,
                revision: 1,
            },
            bounds: Rect {
                left: -200,
                top: -200,
                right: -199,
                bottom: -199,
            },
            pixels: vec![200, 0, 0, 255].into(),
        }));
        offscreen.layers = layers.into();
        assert!(
            renderer
                .render(&offscreen, |_| panic!(
                    "offscreen assets must not submit a frame"
                ))
                .unwrap()
                .presented
        );
        original = offscreen;
        let rejected = make_scene(original.stamp.revision + 1, [200, 20, 30, 255]);
        let shared = (
            Mutex::new(Mailbox {
                scene: Some(Arc::new(rejected.clone())),
                ..Default::default()
            }),
            Condvar::new(),
        );
        let mut attempted = false;
        while !attempted {
            let frame = renderer
                .render(&rejected, |surfaces| {
                    attempted = true;
                    {
                        let mut state = shared.0.lock().unwrap();
                        state.generation += 1;
                        state.scene = None;
                    }
                    commit_current(&shared, 0, rejected.stamp, || present_surfaces(surfaces))
                })
                .unwrap();
            assert!(!frame.presented);
            assert!(
                Instant::now() < deadline,
                "cancelled scene did not reach the commit boundary"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(40));
        surface::pump_messages();
        assert!(
            desktop_pixel()
                .iter()
                .zip([20, 100, 180])
                .all(|(a, b)| a.abs_diff(b) <= 2),
            "rejected back buffer changed the desktop"
        );
        assert_eq!(renderer.presented_stamp, Some(original.stamp));
        save_owned_snapshot(bounds, "target/annotation-gpu-cancelled-frame-native.bmp");
        drop(renderer);
        assert_eq!(port_window_count(), 0);
    }

    #[test]
    #[ignore = "opens owned native windows and captures the interactive desktop"]
    fn gpu_mosaic_scene_presents_ordered_marks_and_retains_paused_tiles() {
        use lexift_core::domain::annotation::{
            Bounds, Extra, Kind, MosaicAnnotation, MosaicEffect, Object, Style,
        };
        let input = InputWindow(unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                w!("STATIC"),
                w!("Lexift GPU Mosaic Test"),
                WS_POPUP,
                400,
                400,
                100,
                80,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA).unwrap();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            hwnd
        });
        let port = WindowsAnnotationRenderPort::new().unwrap();
        let source_bounds = Rect {
            left: 384,
            top: 384,
            right: 512,
            bottom: 512,
        };
        let mask_bounds = Rect {
            left: 410,
            top: 410,
            right: 480,
            bottom: 460,
        };
        let make_scene = |revision, changed: bool, paused: bool| {
            let pixels: Vec<u8> = (0..128 * 128)
                .flat_map(|i| {
                    if changed {
                        [0, 0, 200, 255]
                    } else {
                        [(i % 128 % 64 * 2) as u8, (i / 128 % 64 * 2) as u8, 0, 255]
                    }
                })
                .collect();
            AnnotationScene {
                stamp: RenderStamp {
                    session: 1,
                    revision,
                },
                surfaces: vec![RenderSurface {
                    input_window: WindowToken(input.0.0 as isize),
                    scale: 1.,
                    bounds: Rect {
                        left: 400,
                        top: 400,
                        right: 500,
                        bottom: 480,
                    },
                }]
                .into(),
                layers: vec![
                    RenderLayer::Raster(RasterLayer {
                        key: RasterKey { asset: 1, revision },
                        bounds: source_bounds,
                        pixels: pixels.into(),
                    }),
                    RenderLayer::Mosaic(MosaicLayer {
                        object: Arc::new(Object {
                            erased: Vec::new(),
                            id: Default::default(),
                            bounds: Bounds::from_corners((410., 410.), (480., 460.)),
                            kind: Kind::MosaicArea,
                            style: Style::default(),
                            points: Vec::new(),
                            extra: Extra::Mosaic(MosaicAnnotation::new(
                                MosaicEffect::Pixelate,
                                100,
                                12.,
                                1.,
                            )),
                        }),
                        masks: vec![RasterLayer {
                            key: RasterKey {
                                asset: 2,
                                revision: 1,
                            },
                            bounds: mask_bounds,
                            pixels: vec![255; 70 * 50 * 4].into(),
                        }]
                        .into(),
                    }),
                ]
                .into(),
                interaction: Arc::from([]),
                excluded_windows: Arc::from([]),
                paused_regions: if paused {
                    vec![mask_bounds].into()
                } else {
                    Arc::from([])
                },
            }
        };
        port.submit(Arc::new(make_scene(0, false, true))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(frame) = port.latest().unwrap()
                && matches!(frame.wait, Some(RenderWait::Paused))
            {
                assert!(!frame.presented && frame.sampled_regions.is_empty());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "new paused effect did not report an intentional wait"
            );
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        port.submit(Arc::new(make_scene(1, false, false))).unwrap();
        wait_pixel(&port, [63, 63, 0], (450, 440));
        save_owned_snapshot(mask_bounds, "target/annotation-gpu-pixelate-native.bmp");
        port.submit(Arc::new(make_scene(2, true, true))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while port
            .latest()
            .unwrap()
            .is_none_or(|frame| frame.stamp.revision != 2 || !frame.presented)
        {
            assert!(Instant::now() < deadline, "paused scene was not presented");
            surface::pump_messages();
            thread::sleep(Duration::from_millis(10));
        }
        wait_pixel(&port, [63, 63, 0], (450, 440));
        port.submit(Arc::new(make_scene(3, true, false))).unwrap();
        wait_pixel(&port, [0, 0, 200], (450, 440));
        let mut scene = make_scene(4, true, false);
        let mut layers = scene.layers.to_vec();
        layers.push(RenderLayer::Raster(RasterLayer {
            key: RasterKey {
                asset: 3,
                revision: 1,
            },
            bounds: Rect {
                left: 445,
                top: 435,
                right: 455,
                bottom: 445,
            },
            pixels: [0, 200, 0, 255].repeat(100).into(),
        }));
        scene.layers = layers.into();
        port.submit(Arc::new(scene)).unwrap();
        wait_pixel(&port, [0, 200, 0], (450, 440));
        save_owned_snapshot(mask_bounds, "target/annotation-gpu-order-native.bmp");
        let mut scene = make_scene(5, false, false);
        let mut layers = scene.layers.to_vec();
        layers[0] = RenderLayer::Raster(RasterLayer {
            key: RasterKey {
                asset: 1,
                revision: 5,
            },
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 1024,
            },
            pixels: (0..1024 * 1024)
                .flat_map(|index| {
                    let gray = if index % 1024 < 448 { 20 } else { 220 };
                    [gray, gray, gray, 255]
                })
                .collect::<Vec<u8>>()
                .into(),
        });
        if let RenderLayer::Mosaic(layer) = &mut layers[1]
            && let Extra::Mosaic(settings) = &mut Arc::make_mut(&mut layer.object).extra
        {
            settings.effect = MosaicEffect::Blur;
        }
        scene.layers = layers.into();
        let plan = blur::Plan::new(20.);
        let reference: Vec<f64> = (0..1024)
            .map(|x| if x < 448 { 20. / 255. } else { 220. / 255. })
            .collect();
        let gray = (blur::tests::reference_signal(&reference, &plan)[450] * 255.).round() as u8;
        assert!(
            gray > 40 && gray < 200,
            "reference must distinguish blur from the unfiltered step"
        );
        port.submit(Arc::new(scene)).unwrap();
        wait_pixel(&port, [gray; 3], (450, 440));
        save_owned_snapshot(mask_bounds, "target/annotation-gpu-blur-native.bmp");
        let mut scene = make_scene(6, false, false);
        let output = Rect {
            left: 400,
            top: 400,
            right: 464,
            bottom: 464,
        };
        let mut layers = vec![RenderLayer::Raster(RasterLayer {
            key: RasterKey {
                asset: 1,
                revision: 6,
            },
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 1024,
            },
            pixels: (0..1024 * 1024)
                .flat_map(|index| {
                    [
                        20 + (index % 32 * 6) as u8,
                        30 + (index / 1024 % 32 * 5) as u8,
                        80,
                        255,
                    ]
                })
                .collect::<Vec<_>>()
                .into(),
        })];
        let coverage: Vec<u8> = (0..64 * 64)
            .flat_map(|index| {
                let x = (index % 64) as f32 - 31.5;
                let y = (index / 64) as f32 - 31.5;
                [if x * x + y * y <= 32. * 32. { 255 } else { 0 }; 4]
            })
            .collect();
        layers.push(RenderLayer::Magnifier(MagnifierLayer {
            object: Arc::new(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Magnifier,
                bounds: Bounds::from_corners((512., 384.), (544., 416.)),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::Magnifier {
                    output: Bounds::from_corners((400., 400.), (464., 464.)),
                    zoom: 2.,
                    ellipse: true,
                    connector: lexift_core::domain::annotation::MagnifierConnector::None,
                    erase_annotations: false,
                    antialias: false,
                    shadow: false,
                },
            }),
            masks: vec![RasterLayer {
                key: RasterKey {
                    asset: 20,
                    revision: 1,
                },
                bounds: output,
                pixels: coverage.into(),
            }]
            .into(),
            decorations: Arc::from([]),
        }));
        scene.layers = layers.into();
        let mut without_effects = scene.clone();
        without_effects.stamp.revision = 7;
        without_effects.layers = vec![scene.layers[0].clone()].into();
        port.submit(Arc::new(scene)).unwrap();
        wait_pixel(&port, [170, 130, 80], (450, 440));
        wait_pixel(&port, [116, 110, 80], (400, 400));
        save_owned_snapshot(output, "target/annotation-gpu-lens-native.bmp");
        let deadline = Instant::now() + Duration::from_secs(3);
        while port.shared.0.lock().unwrap().captures == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            port.shared.0.lock().unwrap().captures > 0,
            "live lens captures sources"
        );
        port.submit(Arc::new(without_effects)).unwrap();
        wait_pixel(&port, [32, 150, 80], (450, 440));
        let deadline = Instant::now() + Duration::from_secs(3);
        while port.shared.0.lock().unwrap().captures != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            port.shared.0.lock().unwrap().captures,
            0,
            "removing the last effect stops all WGC producers"
        );
        port.clear();
        drop(port);
        assert_eq!(port_window_count(), 0);
    }

    /// Capture only the test's opaque, owned annotation rectangle. This is
    /// native QA evidence, not a pixel readback in the production render path.
    fn save_owned_snapshot(bounds: Rect, path: &str) {
        use windows::Win32::Graphics::Gdi::*;
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        let bytes = width as u32 * height as u32 * 4;
        let mut bitmap = Vec::new();
        bitmap.extend_from_slice(b"BM");
        bitmap.extend_from_slice(&(54 + bytes).to_le_bytes());
        bitmap.extend_from_slice(&0u32.to_le_bytes());
        bitmap.extend_from_slice(&54u32.to_le_bytes());
        bitmap.extend_from_slice(&40u32.to_le_bytes());
        bitmap.extend_from_slice(&width.to_le_bytes());
        bitmap.extend_from_slice(&(-height).to_le_bytes());
        bitmap.extend_from_slice(&1u16.to_le_bytes());
        bitmap.extend_from_slice(&32u16.to_le_bytes());
        bitmap.extend_from_slice(&0u32.to_le_bytes());
        bitmap.extend_from_slice(&bytes.to_le_bytes());
        bitmap.extend_from_slice(&[0; 16]);
        let mut pixels = vec![0u8; bytes as usize];
        unsafe {
            let dc = GetDC(None);
            let memory = CreateCompatibleDC(Some(dc));
            let image = CreateCompatibleBitmap(dc, width, height);
            let previous = SelectObject(memory, image.into());
            let copied = BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(dc),
                bounds.left,
                bounds.top,
                SRCCOPY | CAPTUREBLT,
            );
            SelectObject(memory, previous);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let lines = GetDIBits(
                memory,
                image,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            );
            let _ = DeleteObject(image.into());
            let _ = DeleteDC(memory);
            ReleaseDC(None, dc);
            copied.unwrap();
            assert_eq!(lines, height);
        }
        bitmap.extend_from_slice(&pixels);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        std::fs::write(path, bitmap).unwrap();
    }

    fn port_window_count() -> usize {
        unsafe extern "system" fn count(
            hwnd: HWND,
            parameter: windows::Win32::Foundation::LPARAM,
        ) -> windows::core::BOOL {
            let mut class = [0u16; 128];
            let length = unsafe { GetClassNameW(hwnd, &mut class) };
            if String::from_utf16_lossy(&class[..length.max(0) as usize])
                == "Lexift.Annotation.GpuSurface"
            {
                unsafe {
                    *(parameter.0 as *mut usize) += 1;
                }
            }
            true.into()
        }
        let mut result = 0usize;
        unsafe {
            EnumWindows(
                Some(count),
                windows::Win32::Foundation::LPARAM(&mut result as *mut _ as isize),
            )
            .unwrap();
        }
        result
    }
}
