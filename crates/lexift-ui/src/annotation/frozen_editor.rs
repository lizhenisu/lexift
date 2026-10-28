//! Shared immutable-image editor preview and presentation, independent of capture sessions.
use super::*;
use lexift_core::{
    domain::{region_selection::RegionEditor, screenshot::ScreenshotEvent},
    ports::screen_capture::{CapturedRegion, ScreenCaptureFrame},
};
use std::sync::{Arc, atomic::AtomicBool};
pub(super) struct Editing {
    pub pin: Option<(u64, u64)>,
    pub id: u64,
    pub frames: Arc<Vec<CapturedRegion>>,
    pub region: RegionEditor,
    pub capture: Arc<ScreenCaptureFrame>,
    pub(super) preview: Option<screenshot_preview::Preview>,
    pub(super) revision: u64,
    pub(super) presented: u64,
    pub(super) accepted_after: u64,
    pub(super) image_content: Option<std::sync::Weak<tiny_skia::Pixmap>>,
    pub(super) feedback_placement: Option<(Point, slint::PhysicalSize)>,
    pub(super) toolbar_adjustment: u64,
    pub(super) keyboard: screenshot::keyboard::State,
    pub(super) initial_focus: Option<crate::screen_interaction::selection::SubmissionFocus>,
    pub(super) size_input: screenshot::size_input::State,
    pub(super) help: crate::screen_interaction::screenshot_help::State,
}
impl Editing {
    pub(super) fn image(id: u64, bounds: Rect, frames: Arc<Vec<CapturedRegion>>) -> Self {
        Self {
            pin: None,
            id,
            region: RegionEditor::new(bounds),
            capture: Arc::new(ScreenCaptureFrame {
                revision: id,
                serial: 1,
                regions: frames.as_ref().clone(),
            }),
            frames,
            preview: None,
            revision: 0,
            accepted_after: 0,
            image_content: None,
            presented: 0,
            feedback_placement: None,
            toolbar_adjustment: 0,
            keyboard: Default::default(),
            initial_focus: None,
            size_input: Default::default(),
            help: Default::default(),
        }
    }
}
fn input(r: &Registry, index: usize) -> Option<screenshot_preview::Input> {
    let editing = r.screenshot.as_ref()?;
    let layer = r.canvas_state.canvases.get(index)?;
    let mut snapshot = export::snapshot_at(r, layer.bounds, layer.scale())?;
    if screenshot::long::image_editing() {
        snapshot.scale = 1.;
        snapshot.monitors = vec![(layer.bounds, 1.)];
    }
    if history_editor::active() {
        history_editor::source_metadata(&mut snapshot);
        if history_editor::hidden() {
            let visible = Arc::get_mut(&mut snapshot.session)?;
            visible.objects.clear();
            visible.watermark = None;
            visible.watermark_erased.clear();
            visible.spotlight_erased.clear();
        }
    }
    if let Some((id, _)) = editing.pin {
        pin_editor::source_metadata(id, &mut snapshot);
    }
    if let Some(candidate) = image_transform::preview(r) {
        snapshot.session = Arc::new(candidate);
    }
    let session = Arc::get_mut(&mut snapshot.session)?;
    if image_crop::active() {
        session.image_scale = Default::default();
    }
    session.set_edit_scope(EditScope::All);
    session.selected = r.document.session.selected;
    if let Some((index, object)) = edited_document_object(r) {
        let index = index.unwrap_or_else(|| session.objects.len().saturating_sub(1));
        if let Some(saved) = session.objects.get_mut(index) {
            *saved = object;
        }
    }
    Some(screenshot_preview::Input {
        snapshot,
        capture: editing.capture.clone(),
        crop: editing.region.bounds()?,
        label: screenshot::label_feedback(r).filter(|label| label.host == layer.bounds),
        help: screenshot::help_feedback(r).filter(|help| help.host == layer.bounds),
        picker: crate::screen_interaction::pixel_picker::location()
            .filter(|p| {
                p.x >= layer.bounds.left
                    && p.x < layer.bounds.right
                    && p.y >= layer.bounds.top
                    && p.y < layer.bounds.bottom
            })
            .and_then(|_| screenshot::picker_feedback(r)),
        hovered: r.hovered,
        crop_handles: r.document.mode.is_mouse()
            && editing.pin.is_none()
            && !screenshot::long::image_editing(),
        crop_border: editing.pin.is_none() && !screenshot::long::image_editing(),
        erase: r
            .document
            .session
            .rectangle_erase_geometry()
            .map(|g| (g, r.document.session.rectangle_erase_show_handles())),
    })
}
pub(super) fn frame(r: &Registry, index: usize) -> Option<tiny_skia::Pixmap> {
    screenshot_preview::paint(&input(r, index)?, &AtomicBool::new(false))
}
pub(super) fn render(r: &mut Registry) {
    if screenshot::long::busy()
        || screenshot::long::image_editing() && screenshot::long::editor().is_none()
    {
        return;
    }
    pin_editor::sync_watermark_anchor(r);
    screenshot::sync_region_feedback(r);
    let inputs = (0..r.canvas_state.canvases.len())
        .filter_map(|index| input(r, index))
        .collect::<Vec<_>>();
    if inputs.len() != r.canvas_state.canvases.len() {
        return;
    }
    let Some(editing) = r.screenshot.as_mut() else {
        return;
    };
    if editing.preview.is_none() {
        match screenshot_preview::Preview::new() {
            Ok(worker) => editing.preview = Some(worker),
            Err(_) => {
                let id = editing.id;
                slint::Timer::single_shot(Duration::ZERO, move || report_failure(id));
                return;
            }
        }
    }
    editing.revision = editing.revision.wrapping_add(1);
    image_transform::track(r.generation, editing.id, editing.revision);
    editing
        .preview
        .as_ref()
        .unwrap()
        .submit(screenshot_preview::Job {
            submitted: Instant::now(),
            generation: r.generation,
            session: editing.id,
            revision: editing.revision,
            canvases: r
                .canvas_state
                .canvases
                .iter()
                .map(|c| c.revision())
                .collect(),
            inputs,
        });
}
#[cfg(test)]
pub(super) fn present(
    generation: u64,
    id: u64,
    revision: u64,
    frames: Option<Vec<tiny_skia::Pixmap>>,
    labels: Vec<(u64, Option<screenshot::size_input::Hit>)>,
) {
    present_shared(
        generation,
        id,
        revision,
        frames.map(|frames| frames.into_iter().map(Arc::new).collect()),
        labels,
    );
}
pub(super) fn present_shared(
    generation: u64,
    id: u64,
    revision: u64,
    frames: Option<Vec<Arc<tiny_skia::Pixmap>>>,
    labels: Vec<(u64, Option<screenshot::size_input::Hit>)>,
) {
    let mut failed = false;
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(editing) = r
            .screenshot
            .as_mut()
            .filter(|e| e.id == id && revision > e.presented)
        else {
            return;
        };
        // Capture suspension and image-view replacement retire the previous frame owners.
        // Validate those identities even for failed jobs, before reporting a session failure.
        if screenshot::long::busy()
            || labels.len() != r.canvas_state.canvases.len()
            || r.canvas_state
                .canvases
                .iter()
                .zip(&labels)
                .any(|(c, (revision, _))| c.revision() != *revision)
            || (frames.is_none() && revision < editing.revision)
        {
            return;
        }
        if revision < editing.accepted_after || !image_transform::current(generation, id, revision)
        {
            return;
        }
        let Some(frames) = frames else {
            if image_transform::failed(r, revision) {
                return;
            }
            failed = true;
            return;
        };
        if let Some((pin, _)) = editing.pin {
            let mut frames = frames.into_iter();
            let (Some(image), Some(controls)) = (frames.next(), frames.next()) else {
                failed = true;
                return;
            };
            if revision != editing.revision || pin_crop::waiting() {
                return;
            }
            editing.presented = revision;
            if !pin_crop::preview_ready(r, pin) {
                return;
            }
            pin_editor::presented(pin, &r.document.session);
            crate::pinned_screenshot::present_edit(
                pin,
                Arc::try_unwrap(image).unwrap_or_else(|image| image.as_ref().clone()),
                Arc::try_unwrap(controls).unwrap_or_else(|controls| controls.as_ref().clone()),
            );
            return;
        }
        if screenshot::long::image_editing() {
            if let Some(window) = screenshot::long::editor() {
                let mut frames = frames.into_iter();
                if let (Some(base), Some(controls)) = (frames.next(), frames.next()) {
                    editing.presented = revision;
                    let installed_at = Instant::now();
                    let changed = editing
                        .image_content
                        .as_ref()
                        .is_none_or(|old| !old.ptr_eq(&Arc::downgrade(&base)));
                    if changed {
                        editing.image_content = Some(Arc::downgrade(&base));
                    }
                    let content_bytes = base.data().len();
                    let feedback_bytes = controls.data().len();
                    let as_image = |p: Arc<tiny_skia::Pixmap>| {
                        slint::Image::from_rgba8_premultiplied(slint::SharedPixelBuffer::<
                            slint::Rgba8Pixel,
                        >::clone_from_slice(
                            p.data(), p.width(), p.height()
                        ))
                    };
                    let crop_transition = window.get_image_cropping() != image_crop::active();
                    window.set_frame_resized(
                        !image_crop::active()
                            && image_transform::preview(r)
                                .as_ref()
                                .unwrap_or(&r.document.session)
                                .image_scale
                                != Default::default(),
                    );
                    if changed {
                        // Drop the transfer raster as soon as Slint owns its pixels. Large
                        // uncached content must not overlap the controls upload unnecessarily.
                        window.set_frame(as_image(base));
                    } else {
                        drop(base);
                    }
                    window.set_controls(as_image(controls));
                    if changed {
                        tracing::debug!(
                            generation,
                            session = id,
                            revision,
                            install_ms = installed_at.elapsed().as_secs_f64() * 1000.,
                            content_bytes,
                            feedback_bytes,
                            "Frozen image installed (not compositor presentation)"
                        );
                        image_installed(generation, id, revision);
                    }
                    image_transform::ready(r, revision);
                    if crop_transition {
                        image_crop::sync(r, true);
                    }
                    if let Some(canvas) = r.canvas_state.canvases.first() {
                        let canvas_revision = canvas.revision();
                        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                            if canvas_current(generation, 0, canvas_revision) {
                                screenshot::long::editor_presented(id);
                            }
                        });
                    }
                }
            }
            return;
        }
        if frames.len() != r.canvas_state.canvases.len() {
            return;
        };
        if labels.len() != frames.len()
            || r.canvas_state
                .canvases
                .iter()
                .zip(&labels)
                .any(|(c, (rev, _))| c.revision() != *rev)
        {
            return;
        }
        editing.presented = revision;
        for ((layer, frame), (canvas, label)) in
            r.canvas_state.canvases.iter_mut().zip(frames).zip(labels)
        {
            let window = layer.window.clone_strong();
            if !layer.presented.submit(
                Arc::try_unwrap(frame).unwrap_or_else(|frame| frame.as_ref().clone()),
                |frame| {
                    (r.lifecycle.annotation_frame_presenter)(
                        window.window(),
                        frame.data(),
                        frame.width(),
                        frame.height(),
                    )
                },
            ) {
                failed = true;
            } else {
                editing.size_input.presented.insert(canvas, label);
            }
        }
    });
    if failed {
        report_failure(id);
    }
}
fn report_failure(id: u64) {
    tracing::debug!(session = id, "Frozen editor presentation failed");
    if let Some((window, editor)) = history_editor::identity().filter(|(_, editor)| *editor == id) {
        history_editor::unavailable(window, editor);
        return;
    }
    if pin_editor::current().is_some_and(|p| p.1 == id)
        && REGISTRY.with(|s| s.borrow_mut().as_mut().is_some_and(pin_crop::cancel))
    {
        return;
    }
    if let Some((pin, edit)) = pin_editor::current().filter(|p| p.1 == id) {
        emit_pin(lexift_core::domain::pinned_image::Event::EditFailed {
            id: pin,
            edit,
            error: crate::i18n::tr("Screenshot unavailable"),
        });
    } else {
        screenshot::emit(ScreenshotEvent::Failed {
            session: id,
            error: crate::i18n::tr("Screenshot unavailable"),
        });
    }
}

thread_local! { static IMAGE_RENDER:RefCell<Option<(u64,u64,u64,Instant)>>=const {RefCell::new(None)}; }
fn image_installed(generation: u64, session: u64, revision: u64) {
    IMAGE_RENDER
        .with(|slot| *slot.borrow_mut() = Some((generation, session, revision, Instant::now())));
}
/// Renderer callback completion is distinct from desktop compositor presentation.
pub(super) fn image_rendered(generation: u64, session: u64) {
    IMAGE_RENDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|p| p.0 == generation && p.1 == session)
        {
            let (_, _, revision, started) = slot.take().unwrap();
            tracing::debug!(
                generation,
                session,
                revision,
                render_wait_ms = started.elapsed().as_secs_f64() * 1000.,
                "Frozen image render returned (not compositor presentation)"
            );
        }
    });
}

pub(super) fn clear_image_timing() {
    IMAGE_RENDER.with(|slot| slot.borrow_mut().take());
}
