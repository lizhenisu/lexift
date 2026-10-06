//! Bounded capture polling and draft completion for the current annotation session.
#[cfg(test)]
use super::document_signature;
#[cfg(test)]
use super::gestures::Active;
use super::gestures::{GeometryDrag, Gesture};
use super::{REGISTRY, Registry, capture_exclusions, schedule_render};
use crate::annotation_signature::{Content, Frame, Replacement};
use crate::{AnnotationToolbar, AnnotationValues};
use lexift_core::domain::{
    annotation::{Bounds, Extra, Kind, Object, Session, Style, TextAnnotation},
    geometry::Rect,
};
use lexift_core::ports::screen_capture::{ScreenCaptureFrame, ScreenCaptureRequest};
use slint::ComponentHandle;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    instance: Instance,
    poll_epoch: u64,
    regions: Vec<Rect>,
    revision: u64,
    capture_floor: u64,
    excluded_windows: Vec<lexift_core::ports::magnifier::WindowToken>,
    waiting_since: Option<Instant>,
    pub notice: Option<Notice>,
    notice_revision: u64,
    pub frame: Option<Arc<ScreenCaptureFrame>>,
    pub pending: Vec<u64>,
    polling: bool,
    tasks: Tasks,
    worker: Option<crate::annotation_render::mosaic_worker::Worker>,
    snapshot: crate::annotation_render::mosaic_worker::Snapshot,
    context: u64,
    pub gesture_epoch: u64,
    pub preview: Option<Preview>,
    pub effects: Option<Arc<crate::annotation_render::mosaic_worker::Frame>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Instance(u64);
impl Default for Instance {
    fn default() -> Self {
        Self(super::window::next_revision())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PollToken {
    instance: Instance,
    epoch: u64,
}

#[derive(Default)]
struct Tasks {
    poll_timer: slint::Timer,
    notice_timer: slint::Timer,
}
impl Drop for Tasks {
    fn drop(&mut self) {
        self.poll_timer.stop();
        self.notice_timer.stop();
    }
}

#[derive(Clone)]
pub(super) struct Preview {
    pub index: usize,
    pub original: Object,
    pub remask: bool,
    draft_epoch: Option<u64>,
    start: Object,
    current: Object,
}

impl Preview {
    fn matches_current(&self, object: &Object) -> bool {
        same_geometry(object, &self.current)
            && matches!((&object.extra, &self.current.extra), (Extra::Mosaic(a), Extra::Mosaic(b)) if a.id == b.id)
    }
}

impl State {
    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        self.worker.is_none() && !self.tasks.poll_timer.running() && self.pending.is_empty()
    }
    /// Only complete effects for the current immutable document can replace GPU pixels.
    pub fn handoff_ready(&self) -> bool {
        self.effects.as_ref().is_some_and(|frame| {
            frame.complete
                && frame.context == self.context
                && self.snapshot.is_current(&frame.snapshot)
        })
    }

    fn poll_token(&self) -> PollToken {
        PollToken {
            instance: self.instance,
            epoch: self.poll_epoch,
        }
    }
    fn stop_polling(&mut self) {
        self.tasks.poll_timer.stop();
        self.polling = false;
        self.poll_epoch = self
            .poll_epoch
            .checked_add(1)
            .expect("mosaic poll epoch exhausted");
    }
    /// Stop the software capture/filter pipeline without discarding provisional
    /// objects. Native completion confirms only identities in its scene.
    pub fn stop_software(
        &mut self,
        capture: Option<&Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>>,
    ) {
        self.stop_polling();
        if !self.regions.is_empty()
            && let Some(capture) = capture
        {
            capture.clear();
        }
        self.regions.clear();
        self.excluded_windows.clear();
        self.frame = None;
        self.effects = None;
        self.preview = None;
        self.worker = None;
        self.snapshot.clear();
        self.waiting_since = None;
    }

    pub fn confirm_native(&mut self, ids: &[lexift_core::domain::annotation::ObjectId]) {
        self.pending
            .retain(|id| !ids.iter().any(|confirmed| confirmed.value() == *id));
    }
    fn update_capture(&mut self, frame: Arc<ScreenCaptureFrame>) -> bool {
        let changed = self
            .frame
            .as_ref()
            .is_none_or(|f| f.serial != frame.serial || f.revision != frame.revision);
        if frame.revision == self.revision {
            self.pending.clear();
            self.waiting_since = None;
        } else if changed {
            // Re-reading a stale sample must not disable first-frame timeout.
            self.waiting_since = Some(Instant::now());
        }
        self.frame = Some(frame);
        changed
    }
    /// Insert on release so capture latency cannot reorder the document or undo.
    pub fn stage(&mut self, session: &mut Session, object: Object) {
        let adopt_preview = self.preview.as_ref().is_some_and(|p| {
            p.draft_epoch == Some(self.gesture_epoch)
                && p.remask
                && p.current.kind == object.kind
                && p.current.style == object.style
                && p.current.extra == object.extra
        });
        if session.add(object)
            && let Some(added) = session.objects.last()
        {
            if let Extra::Mosaic(m) = &added.extra
                && m.strength > 0
            {
                self.pending.push(m.id);
            }
            if adopt_preview && let Some(preview) = &mut self.preview {
                // Core assigns the permanent identity on release. This exact draft
                // owns the preview; no other zero-id draft can inherit its texture.
                preview.current = added.clone();
                preview.index = session.objects.len() - 1;
            }
        }
    }
    pub fn cancel_pending(&mut self, session: &mut Session) {
        session.discard_mosaics(&std::mem::take(&mut self.pending));
    }
}

fn capture_regions(objects: &[Object], draft: Option<&Object>, monitors: &[Rect]) -> Vec<Rect> {
    let mut regions = Vec::new();
    for (index, object) in objects.iter().chain(draft).enumerate() {
        for (_, roi) in crate::annotation_render::mosaic::work_regions(object, monitors) {
            if !regions.contains(&roi) {
                regions.push(roi);
            }
            for source in crate::annotation_render::mosaic_lens::sources(
                &objects[..index.min(objects.len())],
                roi,
            ) {
                if !regions.contains(&source) {
                    regions.push(source);
                }
            }
        }
    }
    regions
}

pub(super) fn sync(r: &mut Registry) {
    let Some(port) = &r.lifecycle.annotation_screen_capture else {
        return;
    };
    let monitors: Vec<_> = r.canvases.iter().map(|c| c.bounds).collect();
    let regions = capture_regions(
        &r.session.objects,
        super::gestures::display_draft(&r.gesture, &r.pending_drawing),
        &monitors,
    );
    let excluded_windows = capture_exclusions(r);
    if let Err(error) = r.mosaic.sync(
        port,
        r.generation,
        CapturePolicy {
            regions,
            excluded_windows,
        },
    ) {
        fail(r, &error.to_string());
    }
}

struct CapturePolicy {
    regions: Vec<Rect>,
    excluded_windows: Vec<lexift_core::ports::magnifier::WindowToken>,
}

impl State {
    fn sync(
        &mut self,
        port: &Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>,
        generation: u64,
        policy: CapturePolicy,
    ) -> lexift_core::Result<()> {
        let CapturePolicy {
            regions,
            excluded_windows,
        } = policy;
        if regions.is_empty() {
            if !self.regions.is_empty() {
                port.clear();
            }
            self.regions.clear();
            self.stop_polling();
            self.frame = None;
            self.capture_floor = self.revision.wrapping_add(1);
            self.worker = None;
            self.snapshot.clear();
            self.effects = None;
            self.preview = None;
            self.waiting_since = None;
            return Ok(());
        }
        let mut result = Ok(());
        if regions != self.regions || excluded_windows != self.excluded_windows {
            self.regions = regions.clone();
            self.excluded_windows = excluded_windows.clone();
            self.revision = self.revision.wrapping_add(1);
            self.waiting_since.get_or_insert_with(Instant::now);
            result = port.request(ScreenCaptureRequest {
                revision: self.revision,
                regions,
                // WGC composes the underlying windows after excluding our UI.
                // Freezing its ROI here would also freeze moving/resized effects.
                paused_regions: Vec::new(),
                excluded_windows,
            });
        }
        if !self.polling {
            self.polling = true;

            let token = self.poll_token();
            self.tasks.poll_timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(34),
                move || poll(generation, token),
            );
        }
        result
    }
}

fn poll(generation: u64, token: PollToken) {
    let changed = REGISTRY
        .with(|registry| {
            let mut slot = registry.borrow_mut();
            let r = slot
                .as_mut()
                .filter(|r| r.generation == generation && r.mosaic.poll_token() == token)?;
            r.mosaic.polling = false;
            sync(r);
            let port = r.lifecycle.annotation_screen_capture.as_ref()?;
            if r.mosaic.regions.is_empty() {
                return None;
            }
            let result = r.mosaic.poll_capture(port);
            if let Some(failure) = result.failure {
                fail(r, &failure);
            }
            Some(result.changed)
        })
        .unwrap_or(false);
    if changed {
        schedule_render(generation);
    }
}

struct PollResult {
    changed: bool,
    failure: Option<String>,
}
impl State {
    fn poll_capture(
        &mut self,
        port: &Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>,
    ) -> PollResult {
        let mut failure = None;
        let mut changed = false;
        match port.latest() {
            Ok(Some(frame)) if accepts_capture(self, &frame) => {
                changed = self.update_capture(frame);
            }
            Err(error) if self.frame.is_none() => {
                failure = Some(error.to_string());
                changed = true;
            }
            _ => {}
        }
        if failure.is_none()
            && self
                .frame
                .as_ref()
                .is_none_or(|f| f.revision != self.revision)
            && self
                .waiting_since
                .is_some_and(|start| start.elapsed() >= Duration::from_secs(5))
        {
            failure = Some("First screen capture timed out".into());
            changed = true;
        }
        if let Some(effects) = self.worker.as_ref().and_then(|w| w.latest(self.context))
            && self.effects.as_ref().is_none_or(|f| {
                f.key != effects.key
                    || f.serial != effects.serial
                    || !Arc::ptr_eq(&f.snapshot, &effects.snapshot)
            })
        {
            self.effects = Some(effects);
            changed = true;
        }
        PollResult { changed, failure }
    }
}

/// Geometry requests may outpace capture. Older samples remain usable within
/// this capture epoch; their bounds still gate processing and cannot confirm a
/// provisional object until the latest requested region has been captured.
fn accepts_capture(state: &State, frame: &ScreenCaptureFrame) -> bool {
    frame.revision > 0 && frame.revision >= state.capture_floor && frame.revision <= state.revision
}

pub(super) fn settings(values: &AnnotationValues, scale: f32) -> Extra {
    Extra::Mosaic(lexift_core::domain::annotation::MosaicAnnotation::new(
        if values.mode == 1 {
            lexift_core::domain::annotation::MosaicEffect::Blur
        } else {
            lexift_core::domain::annotation::MosaicEffect::Pixelate
        },
        values.strength.round().clamp(0., 100.) as u8,
        values.size as f32,
        scale,
    ))
}

fn same_geometry(a: &Object, b: &Object) -> bool {
    a.kind == b.kind && a.bounds == b.bounds && a.points == b.points
}

/// Snapshot visible geometry without edit handles or caret state.
impl State {
    pub fn submit(
        &mut self,
        signatures: &Frame<'_>,
        gesture: Option<&Gesture>,
        geometry_drag: Option<&GeometryDrag>,
        drawing_press: Option<&super::drawing::Press>,
        monitors: impl Iterator<Item = (Rect, f32)>,
    ) {
        let document = signatures.document();
        let draft = signatures.draft();
        if let Some(drag) = geometry_drag
            && let Some(index) = document.object_index(drag.target())
            && let Some(current) = document.get(index)
            && matches!(current.extra, Extra::Mosaic(_))
        {
            let start = drag.original();
            let tile = self.effects.as_ref().and_then(|f| {
                f.tiles.iter().find(|t| {
                    t.index == index
                        && t.object.kind == start.kind
                        && matches!((&t.object.extra, &start.extra),
                        (Extra::Mosaic(a), Extra::Mosaic(b)) if a.id == b.id)
                })
            });
            let related = self
                .effects
                .as_ref()
                .is_some_and(|f| f.context == self.context)
                || tile.is_some_and(|t| {
                    same_geometry(&t.object, start) || same_geometry(&t.object, current)
                })
                || self.preview.as_ref().is_some_and(|p| {
                    p.index == index
                        && (same_geometry(&p.start, start) || same_geometry(&p.current, start))
                });
            if related && let Some(tile) = tile {
                self.preview = Some(Preview {
                    index,
                    original: tile.object.clone(),
                    remask: false,
                    draft_epoch: None,
                    start: start.clone(),
                    current: current.clone(),
                });
            }
        }
        if self.preview.as_ref().is_some_and(|preview| {
            document
                .objects()
                .chain(draft)
                .nth(preview.index)
                .is_none_or(|object| !preview.matches_current(object))
        }) {
            self.preview = None;
        }
        let Some(capture) = self.frame.clone() else {
            return;
        };
        let monitors: Vec<_> = monitors.collect();
        let key = monitors.iter().fold(0u64, |key, &(bounds, scale)| {
            key.rotate_left(7) ^ signatures.document_signature(bounds, scale, None)
        });
        // Geometry changes during the same move/resize may finish processing instead
        // of cancelling every frame. Any other document or settings change invalidates
        // that context; a new pointer press also starts a distinct gesture generation.
        let context = if let Some(drag) = geometry_drag
            && let Some(index) = document.object_index(drag.target())
            && let Some(current) = document.get(index)
            && matches!(current.extra, Extra::Mosaic(_))
        {
            gesture_context_with(
                signatures,
                &monitors,
                self.gesture_epoch,
                index,
                drag.original().bounds,
                &drag.original().points,
                if drag.is_resize() {
                    if let Extra::Mosaic(initial) = &drag.original().extra {
                        Some(initial.diameter)
                    } else {
                        None
                    }
                } else {
                    None
                },
            )
        } else if let Some(info) = drawing_press.map(super::drawing::Press::info)
            && matches!(info.kind, Kind::MosaicArea | Kind::MosaicBrush)
            && let Some(draft) = draft
        {
            let start = info.start;
            let points = [start];
            gesture_context_with(
                signatures,
                &monitors,
                self.gesture_epoch,
                document.len(),
                Bounds::from_corners(start, (start.0 + 1., start.1 + 1.)),
                if draft.kind == Kind::MosaicBrush {
                    &points
                } else {
                    &[]
                },
                None,
            )
        } else {
            key
        };
        self.context = context;
        if matches!(
            gesture,
            Some(Gesture::Draw {
                kind: Kind::MosaicArea | Kind::MosaicBrush,
                ..
            })
        ) && let Some(current) = draft
        {
            let index = document.len();
            self.preview = self
                .effects
                .as_ref()
                .filter(|f| f.context == context)
                .and_then(|f| {
                    f.tiles
                        .iter()
                        .find(|t| t.index == index && t.object.kind == current.kind)
                })
                .map(|tile| Preview {
                    index,
                    original: tile.object.clone(),
                    start: current.clone(),
                    current: current.clone(),
                    remask: true,
                    draft_epoch: Some(self.gesture_epoch),
                });
        }
        let snapshot = self.snapshot.get(document, draft);
        let worker = self.worker.get_or_insert_with(Default::default);
        if !worker.needs_submission(key, context, capture.serial, &snapshot) {
            return;
        }
        worker.submit(crate::annotation_render::mosaic_worker::Job {
            key,
            context,
            provisional_index: draft
                .filter(|o| matches!(o.extra, Extra::Mosaic(_)))
                .map(|_| document.len()),
            session: snapshot,
            monitors,
            capture,
        });
    }
}

/// Ignore only geometry produced by this gesture, keeping parameter and document
/// changes in the cancellation signature. The document remains borrowed and intact.
#[cfg(test)]
fn gesture_context<'a>(
    snapshot: impl Into<crate::annotation_scene::Document<'a>>,
    draft: Option<&'a Object>,
    monitors: &[(Rect, f32)],
    epoch: u64,
    index: usize,
    start: &Object,
    resize: bool,
) -> u64 {
    let frame = Frame::new(snapshot, draft);
    let diameter = if resize {
        if let Extra::Mosaic(initial) = &start.extra {
            Some(initial.diameter)
        } else {
            None
        }
    } else {
        None
    };
    gesture_context_with(
        &frame,
        monitors,
        epoch,
        index,
        start.bounds,
        &start.points,
        diameter,
    )
}

fn gesture_context_with(
    signatures: &Frame<'_>,
    monitors: &[(Rect, f32)],
    epoch: u64,
    index: usize,
    bounds: Bounds,
    points: &[(f32, f32)],
    diameter: Option<f32>,
) -> u64 {
    let current = signatures.get(index).expect("gesture object exists");
    // Only the small mosaic settings are copied; the current and starting trajectories stay borrowed.
    let mut extra = current.extra.clone();
    if let Some(diameter) = diameter
        && let Extra::Mosaic(settings) = &mut extra
    {
        settings.diameter = diameter;
    }
    let replacement = Replacement::new(
        index,
        Content {
            erased: &current.erased,
            kind: current.kind,
            bounds,
            points,
            style: &current.style,
            extra: &extra,
        },
    );
    monitors.iter().fold(epoch, |key, &(bounds, scale)| {
        key.rotate_left(7) ^ signatures.document_signature(bounds, scale, Some(&replacement))
    })
}

pub(super) struct Notice {
    pub revision: u64,
    pub bounds: Rect,
    message: String,
}

/// Remove unvalidated masks without leaving a gesture pointing at a deleted index.
pub(super) fn cancel_pending(r: &mut Registry) {
    if r.mosaic.pending.is_empty() {
        return;
    }
    let editing_pending = r.geometry_drag().is_some_and(|drag| {
        r.session
            .object_index(drag.target())
            .and_then(|index| r.session.objects.get(index))
            .is_some_and(
                |o| matches!(&o.extra, Extra::Mosaic(m) if r.mosaic.pending.contains(&m.id)),
            )
    });
    if editing_pending {
        super::cancel_active_gesture(r);
    }
    // Gesture rollback can replay accepted parameter input. Complete it before
    // removing provisional objects from every Core history snapshot.
    r.finish_style_edit();
    r.mosaic.cancel_pending(&mut r.session);
    r.hovered = None;
    if let Some(main) = r.main.get() {
        main.set_can_undo(r.session.can_undo());
    }
}

/// A failed first sample never leaves an invisible mask in the document or undo.
fn fail(r: &mut Registry, detail: &str) {
    tracing::warn!(error = detail, "Annotation screen capture failed");
    cancel_pending(r);
    if super::gestures::display_draft(&r.gesture, &r.pending_drawing)
        .is_some_and(|o| matches!(o.extra, Extra::Mosaic(_)))
    {
        // Retire through the same owner as Escape and tool switching, so a
        // capture failure cannot leave a transaction or deferred input behind.
        super::cancel_active_gesture(r);
    }
    r.mosaic.waiting_since = None;
    notify(r, "Screen capture unavailable. Try again.");
}

/// Shared transient feedback; export failures must not cancel live mosaics.
pub(super) fn notify(r: &mut Registry, message: &str) {
    notify_on(r, message, None);
}

pub(super) fn notify_on(r: &mut Registry, message: &str, monitor: Option<Rect>) {
    r.mosaic.notice_revision = r.mosaic.notice_revision.wrapping_add(1);
    let revision = r.mosaic.notice_revision;
    let generation = r.generation;
    let instance = r.mosaic.instance;
    if let Some(canvas) = r
        .canvases
        .iter()
        .find(|c| monitor.map_or(c.last_pointer.is_some(), |bounds| c.bounds == bounds))
        .or_else(|| r.canvases.first())
    {
        r.mosaic.notice = Some(Notice {
            revision,
            bounds: canvas.bounds,
            message: crate::i18n::tr(message),
        });
        r.mosaic.tasks.notice_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(4),
            move || {
                let changed = clear_notice(generation, instance, revision);
                if changed {
                    schedule_render(generation);
                }
            },
        );
    }
}

fn clear_notice(generation: u64, instance: Instance, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.mosaic.instance == instance)
        else {
            return false;
        };
        if r.mosaic
            .notice
            .as_ref()
            .is_some_and(|n| n.revision == revision)
        {
            r.mosaic.notice = None;
            r.mosaic.tasks.notice_timer.stop();
            true
        } else {
            false
        }
    })
}

/// Paint feedback in the existing layered surface, without creating or focusing
/// another native window. It never enters object snapshots or effect sampling.
pub(super) fn draw_notice(
    frame: &mut tiny_skia::Pixmap,
    notice: &Notice,
    toolbar: &AnnotationToolbar,
    monitor: Rect,
    scale: f32,
) {
    if monitor != notice.bounds {
        return;
    }
    let colors = toolbar.global::<crate::Colors>();
    let rgb = |color: slint::Color| {
        let c = color.to_argb_u8();
        [c.red, c.green, c.blue]
    };
    let text = TextAnnotation {
        content: notice.message.clone(),
        font_family: String::new(),
        font_size: toolbar.get_notice_font_size(),
        bold: true,
        italic: false,
        outline: None,
        background: None,
        rotation: 0.,
        linked_arrow: None,
        linked_arrow_control: None,
        layout_scale: 1.,
    };
    let initial = crate::annotation_text::text_bounds((0., 0.), &text, scale);
    let margin = toolbar.get_notice_margin() * scale;
    let width = (toolbar.get_notice_width() * scale)
        .max(initial.width() + 2. * margin)
        .min(frame.width() as f32 - 2. * margin);
    let height = (toolbar.get_notice_height() * scale).max(initial.height() + margin);
    let left = (frame.width() as f32 - width) / 2.;
    let top = margin;
    let bounds = Bounds::from_corners((left, top), (left + width, top + height));
    if let Some(path) =
        crate::annotation_render::notice_path(bounds, toolbar.get_notice_radius() * scale)
    {
        let mut paint = tiny_skia::Paint::default();
        let [red, green, blue] = rgb(colors.get_surface());
        paint.set_color_rgba8(red, green, blue, 255);
        frame.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
        let [red, green, blue] = rgb(colors.get_outline());
        paint.set_color_rgba8(red, green, blue, 255);
        frame.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: scale,
                ..Default::default()
            },
            tiny_skia::Transform::identity(),
            None,
        );
    }
    let at = (
        monitor.left as f32 + left + (width - initial.width()) / 2.,
        monitor.top as f32 + top + (height - initial.height()) / 2.,
    );
    let object = Object {
        erased: Vec::new(),
        id: Default::default(),
        kind: Kind::Text,
        bounds: crate::annotation_text::text_bounds(at, &text, scale),
        style: Style {
            color: rgb(colors.get_error()),
            ..Style::default()
        },
        points: Vec::new(),
        extra: Extra::Text(text),
    };
    crate::annotation_text::draw_text(frame, &object, monitor, scale);
}

#[cfg(test)]
mod tests {
    use super::super::{canvas::Layer as CanvasLayer, defaults, init};
    use super::*;
    use crate::{
        AnnotationCanvas,
        bridge::{PassiveWindowPreparation, WindowLifecycleCallbacks},
    };
    use std::rc::Rc;

    #[test]
    fn borrowed_effect_signature_matches_owned_job_and_ignores_selection() {
        let mut document = Session::new();
        let object = Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-20., -10.), (30., 40.)),
            style: Style::default(),
            points: vec![(-20., -10.), (30., 40.)],
            extra: Extra::Mosaic(lexift_core::domain::annotation::MosaicAnnotation::new(
                lexift_core::domain::annotation::MosaicEffect::Blur,
                70,
                12.,
                1.25,
            )),
        };
        document.add(object.clone());
        let monitor = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let borrowed = document_signature(&document, monitor, 1.25, Some(&object), None);
        let mut owned = Session::new();
        owned.objects = document.objects.clone();
        owned.objects.push(object);
        owned.spotlight_opacity = document.spotlight_opacity;
        assert_eq!(
            borrowed,
            super::super::render_signature(&owned, monitor, 1.25, None, None)
        );
        document.selected = None;
        assert_eq!(
            borrowed,
            document_signature(&document, monitor, 1.25, owned.objects.last(), None)
        );
        document.objects[0].style.color = [12, 34, 56];
        assert_ne!(
            borrowed,
            document_signature(&document, monitor, 1.25, owned.objects.last(), None)
        );
    }

    #[test]
    fn stopping_software_retires_poll_tokens_without_retiring_notice_owner() {
        let mut state = State::default();
        let retained = state.snapshot.get(&Session::new(), None);
        let released = Arc::downgrade(&retained);
        drop(retained);
        let token = state.poll_token();
        let instance = state.instance;
        state.stop_software(None);
        assert!(state.poll_token() != token);
        assert!(state.instance == instance);
        assert!(!state.polling);
        assert!(
            released.upgrade().is_none(),
            "stopping software retained a document snapshot"
        );
    }
    #[test]
    fn capture_requests_follow_brush_cells_and_skip_empty_interior() {
        let monitor = Rect {
            left: -520,
            top: -520,
            right: 520,
            bottom: 520,
        };
        let mut brush = Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-450., -450.), (450., 450.)),
            points: vec![(-450., -450.), (-450., 450.), (450., 450.)],
            style: Style::default(),
            extra: settings(&defaults(11), 1.25),
        };
        brush.refresh_bounds();
        let expected: Vec<_> = crate::annotation_render::mosaic::work_regions(&brush, &[monitor])
            .into_iter()
            .map(|(_, sample)| sample)
            .collect();
        let actual = capture_regions(&[], Some(&brush), &[monitor]);
        assert_eq!(actual, expected);
        assert!(actual.len() > 1);
        assert!(
            !actual
                .iter()
                .any(|r| r.left <= 0 && r.right > 0 && r.top <= 0 && r.bottom > 0)
        );
        assert_eq!(
            capture_regions(&[brush.clone(), brush.clone()], None, &[monitor]),
            actual,
            "identical requests must not duplicate capture work"
        );
        let Extra::Mosaic(m) = &mut brush.extra else {
            unreachable!()
        };
        m.strength = 0;
        assert!(capture_regions(&[brush], None, &[monitor]).is_empty());
    }

    #[test]
    fn draft_preview_adopts_only_its_own_released_identity() {
        let object = Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((0., 0.), (20., 20.)),
            points: vec![(5., 5.), (15., 15.)],
            style: Style::default(),
            extra: settings(&defaults(11), 1.),
        };
        let mut state = State {
            gesture_epoch: 7,
            preview: Some(Preview {
                index: 0,
                original: object.clone(),
                start: object.clone(),
                current: object.clone(),
                remask: true,
                draft_epoch: Some(7),
            }),
            ..State::default()
        };
        let mut session = Session::new();
        let mut final_draft = object.clone();
        final_draft.points.push((25., 25.));
        final_draft.refresh_bounds();
        state.stage(&mut session, final_draft);
        let preview = state.preview.as_ref().unwrap();
        assert_eq!(preview.current, session.objects[0]);
        assert!(matches!(&preview.current.extra, Extra::Mosaic(m) if m.id > 0));
        assert!(matches!(&preview.original.extra, Extra::Mosaic(m) if m.id == 0));
        state.gesture_epoch = 8;
        state.stage(&mut session, object);
        assert_eq!(
            state.preview.as_ref().unwrap().index,
            0,
            "a later zero-id draft must not adopt the old texture"
        );
        assert_eq!(state.preview.as_ref().unwrap().current, session.objects[0]);
    }
    #[test]
    fn software_handoff_requires_complete_current_document_and_context() {
        let mut state = State {
            context: 9,
            ..State::default()
        };
        let mut document = Session::new();
        let first = state.snapshot.get(&document, None);
        state.effects = Some(Arc::new(crate::annotation_render::mosaic_worker::Frame {
            key: 1,
            context: 9,
            serial: 1,
            tiles: Vec::new(),
            snapshot: first.clone(),
            complete: false,
        }));
        assert!(!state.handoff_ready());
        Arc::get_mut(state.effects.as_mut().unwrap())
            .unwrap()
            .complete = true;
        assert!(state.handoff_ready());
        state.context = 10;
        assert!(!state.handoff_ready());
        Arc::get_mut(state.effects.as_mut().unwrap())
            .unwrap()
            .context = 10;
        assert!(state.handoff_ready());
        document.displays.push(Rect {
            left: -1000,
            top: -1000,
            right: -900,
            bottom: -900,
        });
        let current = state.snapshot.get(&document, None);
        assert!(
            !state.handoff_ready(),
            "matching visible key cannot authorize an old document"
        );
        assert!(first.displays.is_empty());
        Arc::get_mut(state.effects.as_mut().unwrap())
            .unwrap()
            .snapshot = current;
        assert!(state.handoff_ready());
        state.snapshot.clear();
        assert!(!state.handoff_ready());
    }

    #[test]
    fn partial_capture_does_not_confirm_pending_objects_or_reset_timeout_when_stale() {
        let mut state = State {
            revision: 10,
            pending: vec![7],
            ..State::default()
        };
        let frame = Arc::new(ScreenCaptureFrame {
            revision: 9,
            serial: 1,
            regions: vec![],
        });
        assert!(state.update_capture(frame.clone()));
        assert_eq!(state.pending, vec![7]);
        let waiting = Instant::now() - Duration::from_secs(6);
        state.waiting_since = Some(waiting);
        assert!(!state.update_capture(frame));
        assert_eq!(state.waiting_since, Some(waiting));
        assert!(state.update_capture(Arc::new(ScreenCaptureFrame {
            revision: 10,
            serial: 2,
            regions: vec![]
        })));
        assert!(state.pending.is_empty());
        assert!(state.waiting_since.is_none());
    }
    #[test]
    fn older_capture_is_usable_only_within_the_current_capture_epoch() {
        let state = State {
            revision: 10,
            capture_floor: 8,
            ..State::default()
        };
        let mut frame = ScreenCaptureFrame {
            revision: 9,
            serial: 1,
            regions: vec![],
        };
        assert!(accepts_capture(&state, &frame));
        frame.revision = 10;
        assert!(accepts_capture(&state, &frame));
        frame.revision = 11;
        assert!(!accepts_capture(&state, &frame));
        frame.revision = 7;
        assert!(!accepts_capture(&state, &frame));
        frame.revision = 0;
        assert!(!accepts_capture(&state, &frame));
    }

    #[test]
    fn mosaic_gesture_context_preserves_jobs_and_tracks_parameters_and_instances() {
        let monitors = [(
            Rect {
                left: -1000,
                top: -500,
                right: 1000,
                bottom: 500,
            },
            1.25,
        )];
        let start = Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::MosaicBrush,
            bounds: Bounds::from_corners((-100., -50.), (100., 50.)),
            points: vec![(-90., -40.), (90., 40.)],
            style: Style::default(),
            extra: settings(&defaults(11), 1.25),
        };
        let mut snapshot = Session::new();
        snapshot.objects.push(start.clone());
        let original = gesture_context(&snapshot, None, &monitors, 1, 0, &start, true);
        snapshot.objects[0].bounds = Bounds::from_corners((-200., -100.), (200., 100.));
        snapshot.objects[0].points = vec![(-180., -80.), (180., 80.)];
        let Extra::Mosaic(m) = &mut snapshot.objects[0].extra else {
            unreachable!()
        };
        m.diameter *= 2.;
        let resized = snapshot.objects.clone();
        assert_eq!(
            gesture_context(&snapshot, None, &monitors, 1, 0, &start, true),
            original
        );
        assert_eq!(
            snapshot.objects, resized,
            "normalization must not alter the actual job"
        );
        assert_ne!(
            gesture_context(&snapshot, None, &monitors, 2, 0, &start, true),
            original
        );
        let Extra::Mosaic(m) = &mut snapshot.objects[0].extra else {
            unreachable!()
        };
        m.strength = 80;
        let changed = gesture_context(&snapshot, None, &monitors, 1, 0, &start, true);
        assert_ne!(changed, original, "parameter edits cancel old filter work");
        snapshot.objects.push(Object {
            erased: Vec::new(),
            id: Default::default(),
            kind: Kind::Rectangle,
            extra: Extra::None,
            ..start.clone()
        });
        assert_ne!(
            gesture_context(&snapshot, None, &monitors, 1, 0, &start, true),
            changed
        );
    }

    #[test]
    fn first_capture_timeout_discards_draft_without_undo_and_notice_is_instance_scoped() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        struct NeverReady;
        impl lexift_core::ports::screen_capture::ScreenCapturePort for NeverReady {
            fn is_available(&self) -> bool {
                true
            }
            fn request(&self, _: ScreenCaptureRequest) -> lexift_core::Result<()> {
                Ok(())
            }
            fn latest(&self) -> lexift_core::Result<Option<Arc<ScreenCaptureFrame>>> {
                Ok(None)
            }
            fn clear(&self) {}
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            )
            .with_annotation_screen_capture(Some(Arc::new(NeverReady))),
        );
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.main.open(AnnotationToolbar::new().unwrap());
            let mut canvas = CanvasLayer::new(
                AnnotationCanvas::new().unwrap(),
                Rect {
                    left: -1000,
                    top: -500,
                    right: 0,
                    bottom: 200,
                },
            );
            canvas.last_pointer = Some((10., 10.));
            r.canvases.push(canvas);
            let draft = Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((-900., -400.), (-800., -300.)),
                style: Style::default(),
                points: Vec::new(),
                extra: settings(&defaults(11), 1.25),
            };
            r.mosaic.stage(&mut r.session, draft.clone());
            let values = defaults(11);
            let mut press = super::super::drawing::Press::begin(
                (-900., -400.),
                Kind::MosaicArea,
                Style::default(),
                1.25,
                &values,
            );
            press.update((-800., -300.), 1.25);
            r.gesture = Some(Active::Drawing(press));
            sync(r);
            r.mosaic.waiting_since = Some(Instant::now() - Duration::from_secs(6));
            r.generation
        });
        let instance = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().mosaic.instance);
        let token = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().mosaic.poll_token());
        poll(generation, token);
        let revision = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(
                r.mosaic.pending.is_empty()
                    && super::super::gestures::display_draft(&r.gesture, &r.pending_drawing)
                        .is_none()
                    && r.gesture().is_none()
            );
            assert!(r.session.objects.is_empty() && !r.session.can_undo());
            let notice = r.mosaic.notice.as_ref().expect("visible failure feedback");
            for scale in [1., 1.25] {
                for dark in [false, true] {
                    let toolbar = r.main.get().unwrap();
                    toolbar
                        .global::<crate::Colors>()
                        .set_preference(i32::from(dark));
                    let mut frame = tiny_skia::Pixmap::new(1000, 700).unwrap();
                    draw_notice(&mut frame, notice, toolbar, notice.bounds, scale);
                    assert!(
                        frame
                            .data()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .filter(|p| p[3] > 0)
                            .count()
                            > 1000
                    );
                    if let Ok(directory) = std::env::var("MOSAIC_SNAPSHOT_DIR") {
                        crate::i18n::write_test_bitmap(
                            &directory,
                            &format!("mosaic-capture-error-{scale}-{dark}"),
                            frame.width(),
                            frame.height(),
                            frame.data().as_chunks::<4>().0.iter().copied(),
                        );
                    }
                }
            }
            notice.revision
        });
        assert!(!clear_notice(
            generation.wrapping_add(1),
            instance,
            revision
        ));
        assert!(!clear_notice(
            generation,
            instance,
            revision.wrapping_sub(1)
        ));
        assert!(clear_notice(generation, instance, revision));
        let last_good = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let original = Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicArea,
                bounds: Bounds::from_corners((-900., -400.), (-800., -300.)),
                style: Style::default(),
                points: Vec::new(),
                extra: settings(&defaults(11), 1.25),
            };
            r.session.add(original.clone());
            let last_good = Arc::new(ScreenCaptureFrame {
                revision: 0,
                serial: 42,
                regions: vec![lexift_core::ports::screen_capture::CapturedRegion {
                    bounds: Rect {
                        left: -950,
                        top: -450,
                        right: -750,
                        bottom: -250,
                    },
                    pixels: [40, 50, 60, 255].repeat(40000),
                }],
            });
            r.mosaic.frame = Some(last_good.clone());
            let mut next = original;
            next.bounds = next.bounds.moved(200., 0.);
            r.mosaic.stage(&mut r.session, next);
            sync(r);
            r.mosaic.waiting_since = Some(Instant::now() - Duration::from_secs(6));
            last_good
        });
        poll(generation, token);
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(r.mosaic.pending.is_empty());
            assert!(Arc::ptr_eq(r.mosaic.frame.as_ref().unwrap(), &last_good));
            assert_eq!(r.session.objects.len(), 1);
            assert!(r.mosaic.notice.is_some());
            assert!(r.session.undo());
            assert!(r.session.objects.is_empty());
            assert!(
                !r.session.can_undo(),
                "failed stroke must not add a second undo"
            );
            r.mosaic = State::default();
            r.mosaic.polling = true;
            r.mosaic.notice = Some(Notice {
                revision,
                bounds: r.canvases[0].bounds,
                message: "replacement owner".into(),
            });
        });
        poll(generation, token);
        assert!(!clear_notice(generation, instance, revision));
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert!(
                r.mosaic.polling,
                "old poll must not modify the replacement owner"
            );
            assert_eq!(
                r.mosaic.notice.as_ref().unwrap().message,
                "replacement owner"
            );
            assert!(r.mosaic.instance != instance);
            r.close();
            slot.take();
        });
        assert!(!clear_notice(generation, instance, revision));
    }
    #[test]
    fn released_strokes_keep_drawing_order_and_separate_undo_before_capture() {
        let mut state = State::default();
        let mut session = Session::new();
        for (x, scale) in [(-100., 1.), (100., 1.25)] {
            let mut object = Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::MosaicBrush,
                bounds: Bounds::from_corners((x, 20.), (x, 20.)),
                style: Style::default(),
                points: vec![(x, 20.)],
                extra: settings(&defaults(11), scale),
            };
            object.refresh_bounds();
            state.stage(&mut session, object);
        }
        assert_eq!(state.pending.len(), 2);
        assert_eq!(session.objects.len(), 2);
        assert_eq!(session.objects[0].points[0].0, -100.);
        let Extra::Mosaic(value) = &session.objects[1].extra else {
            panic!()
        };
        assert_eq!(value.creation_scale, 1.25);
        assert!(session.undo());
        assert_eq!(session.objects.len(), 1);
        assert!(session.undo());
        assert!(session.objects.is_empty());
        assert!(!session.can_undo());
        state.cancel_pending(&mut session);
        assert!(state.pending.is_empty());
    }
}
