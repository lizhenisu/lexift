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
    pub(super) feedback_placement: Option<(Point, slint::PhysicalSize)>,
    pub(super) toolbar_adjustment: u64,
    pub(super) keyboard: screenshot::keyboard::State,
    pub(super) initial_focus: Option<focus::Origin>,
    pub(super) size_input: screenshot::size_input::State,
    pub(super) help: crate::screen_interaction::screenshot_help::State,
}
fn input(r: &Registry, index: usize) -> Option<screenshot_preview::Input> {
    let editing = r.screenshot.as_ref()?;
    let layer = r.canvas_state.canvases.get(index)?;
    let mut snapshot = export::snapshot_at(r, layer.bounds, layer.scale())?;
    if let Some((id, _)) = editing.pin {
        pin_editor::source_metadata(id, &mut snapshot);
    }
    let session = Arc::get_mut(&mut snapshot.session)?;
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
        hovered: r.hovered,
        crop_handles: r.document.mode.is_mouse() && editing.pin.is_none(),
        crop_border: editing.pin.is_none(),
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
    editing
        .preview
        .as_ref()
        .unwrap()
        .submit(screenshot_preview::Job {
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
pub(super) fn present(
    generation: u64,
    id: u64,
    revision: u64,
    frames: Option<Vec<tiny_skia::Pixmap>>,
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
        let Some(frames) = frames else {
            failed = true;
            return;
        };
        if let Some((pin, _)) = editing.pin {
            let mut frames = frames.into_iter();
            let (Some(image), Some(controls)) = (frames.next(), frames.next()) else {
                failed = true;
                return;
            };
            editing.presented = revision;
            crate::pinned_screenshot::present_edit(pin, image, controls);
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
            if !layer.presented.submit(frame, |frame| {
                (r.lifecycle.annotation_frame_presenter)(
                    window.window(),
                    frame.data(),
                    frame.width(),
                    frame.height(),
                )
            }) {
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
