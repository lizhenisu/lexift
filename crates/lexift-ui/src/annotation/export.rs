//! Freezes visible annotation state and presents Core-owned export results.
use super::{REGISTRY, Registry};
use lexift_core::domain::{
    annotation::{Extra, Session},
    annotation_export::{ExportEvent, ExportRequest, ExportSnapshot as Snapshot},
    geometry::Rect,
};
use slint::ComponentHandle;
use std::sync::Arc;
#[derive(Default)]
pub(super) struct State {
    pending: bool,
    editor: Option<u64>,
    request: u64,
    origin: Option<super::focus::Origin>,
    monitor: Option<Rect>,
}
impl State {
    pub(super) fn pending(&self) -> bool {
        self.pending
    }
    pub fn cancel(&mut self) {
        self.pending = false;
        self.editor = None;
        self.monitor = None;
    }
}
pub(super) fn available(r: &Registry) -> bool {
    r.lifecycle.annotation_export_supported
}
/// The toolbar center selects a complete display, never its work area.
fn display_at(center: (i64, i64), displays: &[(Rect, f32)]) -> Option<(Rect, f32)> {
    displays.iter().copied().find(|(r, _)| {
        center.0 >= i64::from(r.left)
            && center.0 < i64::from(r.right)
            && center.1 >= i64::from(r.top)
            && center.1 < i64::from(r.bottom)
    })
}

pub(super) fn snapshot(r: &Registry) -> Option<Snapshot> {
    let main = r.toolbar_state.main.get()?;
    let position = main.window().position();
    let size = main.window().size();
    let monitors: Vec<_> = r
        .canvas_state
        .canvases
        .iter()
        .map(|c| (c.bounds, c.window.window().scale_factor()))
        .collect();
    let (monitor, scale) = display_at(
        (
            i64::from(position.x) + i64::from(size.width) / 2,
            i64::from(position.y) + i64::from(size.height) / 2,
        ),
        &monitors,
    )?;
    snapshot_at(r, monitor, scale)
}

/// Freeze editor content independently of toolbar placement (including hidden startup).
pub(super) fn snapshot_at(r: &Registry, monitor: Rect, scale: f32) -> Option<Snapshot> {
    let main = r.toolbar_state.main.get()?;
    let monitors = r
        .canvas_state
        .canvases
        .iter()
        .map(|c| (c.bounds, c.window.window().scale_factor()))
        .collect();
    // Copy visible state, not the undo stack or parameter bindings.
    let mut session = Session::new();
    session.objects = r.document.session.objects.clone();
    session.displays = r.document.session.displays.clone();
    session.watermark = r.document.session.watermark.clone();
    session.watermark_erased = r.document.session.watermark_erased.clone();
    session.spotlight_erased = r.document.session.spotlight_erased.clone();
    session.spotlight_opacity = r.document.session.spotlight_opacity;
    if let Some((index, mut object)) = super::edited_document_object(r) {
        // Native editors omit their glyphs from the canvas, but their HWND is
        // excluded from export. Restore visible draft glyphs in the snapshot.
        if let Some(editor) = r.editor.window() {
            let content = super::editor_preview(editor).0;
            match &mut object.extra {
                Extra::Text(text) => {
                    text.content = content;
                    text.outline = r.editor.edit()?.original.outline.clone();
                }
                Extra::Sequence(sequence) => {
                    sequence.caption.content = content;
                    sequence.caption.outline = r.editor.edit()?.original.outline.clone();
                }
                _ => {}
            }
        }
        if let Some(index) = index {
            session.objects[index] = object;
        } else {
            session.objects.push(object);
        }
    }
    if let Some(draft) = super::gestures::display_draft(&r.gesture, &r.pending_drawing) {
        session.objects.push(draft.clone());
    }
    identify_draft_effects(&mut session)?;
    Some(Snapshot {
        session: Arc::new(session),
        monitor,
        scale,
        monitors,
        excluded: super::capture_exclusions(r),
        owner: (r.lifecycle.annotation_window_token)(main.window()),
        name: format!(
            "Lexift-{}.png",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ),
        title: crate::i18n::tr("Save annotation screenshot"),
    })
}

/// Live drafts have no completed effect identity. The frozen export needs its
/// own identity for tile matching, without assigning an object in the real document.
fn identify_draft_effects(session: &mut Session) -> Option<()> {
    let mut used: std::collections::HashSet<_> = session
        .objects
        .iter()
        .filter_map(|o| {
            if let Extra::Mosaic(m) = &o.extra {
                Some(m.id)
            } else {
                None
            }
        })
        .collect();
    let mut next = 1u64;
    for object in &mut session.objects {
        if let Extra::Mosaic(m) = &mut object.extra
            && m.id == 0
        {
            while used.contains(&next) {
                next = next.checked_add(1)?;
            }
            m.id = next;
            used.insert(next);
        }
    }
    Some(())
}

pub(super) fn start() {
    let request = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot
            .as_mut()
            .filter(|r| r.canvas_state.annotation_ready && !r.export.pending && available(r))?;
        let snapshot = snapshot(r)?;
        r.export.request = super::window::next_revision();
        r.focus.cancel();
        r.export.origin = Some(super::focus_origin(&r.lifecycle));
        r.export.editor = r.editor.edit().map(|edit| edit.id);
        r.export.monitor = Some(snapshot.monitor);
        r.export.pending = true;
        r.toolbar_state.main.get().unwrap().set_can_download(false);
        Some(ExportRequest {
            session: r.generation,
            request: r.export.request,
            snapshot: Arc::new(snapshot),
        })
    });
    if let Some(request) = request {
        super::emit_export(ExportEvent::Requested(request));
    }
}
pub(crate) fn sync(state: &lexift_core::domain::annotation_export::ExportState) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            state.session == Some(r.generation)
                && state.completed == Some(r.export.request)
                && r.export.pending
        }) else {
            return;
        };
        let editor = r.export.editor;
        let monitor = r.export.monitor;
        let origin = r.export.origin.unwrap_or(super::focus::Origin::Unobserved);
        r.export.cancel();
        if let Some(main) = r.toolbar_state.main.get() {
            main.set_can_download(available(r));
        }
        if let Some(Err(error)) = &state.result {
            tracing::warn!(%error, "Annotation export failed");
            super::mosaic::notify_on(r, "Could not save screenshot. Try again.", monitor);
            r.queue_render();
        }
        if editor.is_some() && r.editor.edit().map(|e| e.id) == editor {
            super::restore_editor_focus_from(r, super::focus::Activation::UserRequested, origin);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation_export_render::{compose, regions};
    use lexift_core::domain::{
        annotation::{
            Bounds, Kind, MagnifierConnector, MosaicAnnotation, MosaicEffect, Object, Style,
        },
        annotation_erase::{EraseRegion, ErasedCoverage},
    };
    use lexift_core::ports::magnifier::WindowToken;
    use lexift_core::ports::screen_capture::{CapturedRegion, ScreenCaptureFrame};
    use std::sync::atomic::AtomicBool;
    fn fixture() -> Snapshot {
        let monitor = Rect {
            left: -16,
            top: -16,
            right: 16,
            bottom: 16,
        };
        Snapshot {
            session: Arc::new(Session::new()),
            monitor,
            scale: 1.,
            monitors: vec![(monitor, 1.)],
            excluded: vec![WindowToken(100)],
            owner: None,
            name: "test.png".into(),
            title: String::new(),
        }
    }
    fn rectangle(a: (f32, f32), b: (f32, f32), color: [u8; 3]) -> Object {
        Object {
            id: Default::default(),
            erased: Vec::new(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners(a, b),
            style: Style {
                fill: true,
                outline: false,
                rounding: 0.,
                color,
                ..Style::default()
            },
            points: Vec::new(),
            extra: Extra::None,
        }
    }
    fn frame(bounds: Rect, color: [u8; 4]) -> Arc<ScreenCaptureFrame> {
        Arc::new(ScreenCaptureFrame {
            revision: 1,
            serial: 1,
            regions: vec![CapturedRegion {
                bounds,
                pixels: color
                    .repeat(((bounds.right - bounds.left) * (bounds.bottom - bounds.top)) as usize),
            }],
        })
    }
    fn pixel(image: &CapturedRegion, x: i32, y: i32) -> &[u8] {
        let i = ((y - image.bounds.top) * (image.bounds.right - image.bounds.left) + x
            - image.bounds.left) as usize
            * 4;
        &image.pixels[i..i + 4]
    }
    #[test]
    fn export_composes_frozen_pixels_and_erasures_without_selection_handles() {
        for scale in [1., 1.25] {
            let mut snapshot = fixture();
            snapshot.scale = scale;
            let session = Arc::get_mut(&mut snapshot.session).unwrap();
            let mut rect = rectangle((-8., -8.), (8., 8.), [210, 0, 0]);
            rect.erased.push(ErasedCoverage::new(EraseRegion::Rectangle(
                Bounds::from_corners((-4., -4.), (4., 4.)),
            )));
            session.objects.push(rect);
            session.selected = Some(0);
            let output = compose(
                &snapshot,
                frame(snapshot.monitor, [0, 120, 0, 255]),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(pixel(&output, 0, 0), [0, 120, 0, 255]);
            assert_eq!(pixel(&output, 6, 6), [210, 0, 0, 255]);
            assert_eq!(
                pixel(&output, -8, -8),
                [210, 0, 0, 255],
                "selection controls must not enter export"
            );
            assert_eq!(pixel(&output, 12, 12), [0, 120, 0, 255]);
            assert!(!snapshot.session.can_undo());
        }
    }
    #[test]
    fn export_effects_use_this_capture_and_keep_later_marks_clear() {
        for effect in [MosaicEffect::Pixelate, MosaicEffect::Blur] {
            let mut snapshot = fixture();
            let session = Arc::get_mut(&mut snapshot.session).unwrap();
            let mut area = rectangle((-8., -8.), (8., 8.), [0, 0, 0]);
            area.kind = Kind::MosaicArea;
            let mut settings = MosaicAnnotation::new(effect, 50, 12., 1.);
            settings.id = 1;
            area.extra = Extra::Mosaic(settings);
            session.objects.push(area);
            session
                .objects
                .push(rectangle((2., 2.), (6., 6.), [0, 0, 230]));
            let sampling = Rect {
                left: -100,
                top: -100,
                right: 100,
                bottom: 100,
            };
            let output = compose(
                &snapshot,
                frame(sampling, [170, 30, 20, 255]),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(pixel(&output, 0, 0), [170, 30, 20, 255]);
            assert_eq!(pixel(&output, 4, 4), [0, 0, 230, 255]);
        }
    }

    #[test]
    fn uncommitted_mosaic_draft_uses_export_identity_without_mutating_the_draft() {
        let mut snapshot = fixture();
        let mut draft = rectangle((-8., -8.), (8., 8.), [0, 0, 0]);
        draft.kind = Kind::MosaicArea;
        draft.extra = Extra::Mosaic(MosaicAnnotation::new(MosaicEffect::Pixelate, 100, 12., 1.));
        let session = Arc::get_mut(&mut snapshot.session).unwrap();
        session.objects.push(draft.clone());
        identify_draft_effects(session).unwrap();
        assert!(matches!(draft.extra,Extra::Mosaic(m) if m.id==0));
        assert!(!session.can_undo());
        let sampling = Rect {
            left: -100,
            top: -100,
            right: 100,
            bottom: 100,
        };
        let mut capture = frame(sampling, [10, 30, 90, 255]);
        let frame = Arc::get_mut(&mut capture).unwrap();
        // Distinct adjacent colors must actually be pixelated, not just reveal
        // the unchanged base through a missing draft tile.
        for y in 0..200usize {
            for x in 0..200usize {
                if x % 2 == 0 {
                    frame.regions[0].pixels[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4]
                        .copy_from_slice(&[200, 40, 20, 255]);
                }
            }
        }
        let output = compose(&snapshot, capture, &AtomicBool::new(false)).unwrap();
        let color = pixel(&output, 0, 0);
        assert!(
            color[0] > 30 && color[0] < 180,
            "uncommitted mosaic was omitted: {color:?}"
        );
    }
    #[test]
    fn magnifier_samples_cross_display_pixels_and_includes_annotations() {
        let mut snapshot = fixture();
        let session = Arc::get_mut(&mut snapshot.session).unwrap();
        session
            .objects
            .push(rectangle((-40., 0.), (-36., 4.), [230, 0, 0]));
        let mut lens = rectangle((-40., 0.), (-36., 4.), [0, 0, 0]);
        lens.kind = Kind::Magnifier;
        lens.style.outline = false;
        lens.style.width = 0.;
        lens.extra = Extra::Magnifier {
            output: Bounds::from_corners((0., 0.), (8., 8.)),
            zoom: 2.,
            ellipse: false,
            antialias: false,
            erase_annotations: false,
            shadow: false,
            connector: MagnifierConnector::None,
        };
        session.objects.push(lens);
        assert!(
            regions(&snapshot)
                .iter()
                .any(|r| r.left <= -40 && r.right >= -36)
        );
        let sampling = Rect {
            left: -50,
            top: -30,
            right: 30,
            bottom: 30,
        };
        let output = compose(
            &snapshot,
            frame(sampling, [0, 90, 0, 255]),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(pixel(&output, 4, 4), [230, 0, 0, 255]);
        if let Extra::Magnifier {
            erase_annotations, ..
        } = &mut Arc::get_mut(&mut snapshot.session).unwrap().objects[1].extra
        {
            *erase_annotations = true;
        }
        let output = compose(
            &snapshot,
            frame(sampling, [0, 90, 0, 255]),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(pixel(&output, 4, 4), [0, 90, 0, 255]);
    }
    #[test]
    fn toolbar_center_uses_full_display_and_negative_coordinates() {
        let displays = [
            (
                Rect {
                    left: -1920,
                    top: 0,
                    right: 0,
                    bottom: 1080,
                },
                1.,
            ),
            (
                Rect {
                    left: 0,
                    top: 0,
                    right: 2560,
                    bottom: 1440,
                },
                1.25,
            ),
        ];
        assert_eq!(display_at((-1, 1050), &displays), Some(displays[0]));
        assert_eq!(display_at((0, 1400), &displays), Some(displays[1]));
        assert!(display_at((3000, 0), &displays).is_none());
    }
    #[test]
    fn snapshot_preserves_text_draft_and_does_not_commit_it() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::rc::Rc;
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(
                &self,
            ) -> std::result::Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        super::super::init(
            |_| super::super::PassiveWindowPreparation::Ready,
            super::super::WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.toolbar_state
                .main
                .open(crate::AnnotationToolbar::new().unwrap());
            r.toolbar_state
                .main
                .get()
                .unwrap()
                .window()
                .set_size(slint::PhysicalSize::new(650, 54));
            let monitor = Rect {
                left: 0,
                top: 0,
                right: 1024,
                bottom: 768,
            };
            r.canvas_state.canvases.push(super::super::CanvasLayer::new(
                crate::AnnotationCanvas::new().unwrap(),
                monitor,
            ));
            let window = crate::AnnotationTextInput::new().unwrap();
            window.set_value("draft 中文".into());
            window.invoke_place_selection(2, 5);
            let text = lexift_core::domain::annotation::TextAnnotation {
                content: String::new(),
                font_family: "Segoe UI".into(),
                font_size: 22.,
                bold: false,
                italic: false,
                outline: None,
                background: None,
                rotation: 0.,
                linked_arrow: None,
                layout_scale: 1.,
                linked_arrow_control: None,
            };
            let edit = super::super::TextEdit::new(
                88,
                None,
                (100., 100.),
                1.,
                text,
                Bounds::from_corners((100., 100.), (260., 140.)),
                String::new(),
            );
            r.editor
                .open(window, edit, Some(super::super::parameters::defaults(8)));
            let frozen = snapshot(r).unwrap();
            assert_eq!(frozen.session.objects.len(), 1);
            assert!(
                matches!(&frozen.session.objects[0].extra,Extra::Text(t) if t.content=="draft 中文")
            );
            assert!(r.document.session.objects.is_empty() && !r.document.session.can_undo());
            assert_eq!(r.editor.edit().unwrap().id, 88);
            assert_eq!(r.editor.window().unwrap().get_cursor_offset(), 5);
            assert_eq!(r.editor.window().unwrap().get_anchor_offset(), 2);
            r.editor.window().unwrap().set_value("new content".into());
            assert!(
                matches!(&frozen.session.objects[0].extra,Extra::Text(t) if t.content=="draft 中文")
            );
            r.export.pending = true;
            r.export.cancel();
            assert!(!r.export.pending);
            r.close();
        });
        // A delayed completion from a retired request must leave the next job busy.
        let generation = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.export.pending = true;
            r.export.request = 42;
            r.generation
        });
        let mut state = lexift_core::domain::annotation_export::ExportState {
            session: Some(generation),
            completed: Some(41),
            result: Some(Ok(lexift_core::ports::annotation_save::SaveOutcome::Saved)),
            ..Default::default()
        };
        sync(&state);
        REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().export.pending));
        state.completed = Some(42);
        sync(&state);
        REGISTRY.with(|slot| assert!(!slot.borrow().as_ref().unwrap().export.pending));
    }
}
