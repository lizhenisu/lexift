//! Annotation render workflow; scoped to its owning session.
use super::*;
pub(super) fn schedule_render(generation: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        r.queue_render();
    });
}

/// Shares the editor's current geometry between painting and cursor hit testing.
pub(super) fn editing_text_object(r: &Registry) -> Option<Object> {
    let (Some(edit), Some(editor)) = (r.editor.edit(), r.editor.window()) else {
        return None;
    };
    if edit.object.is_some() && edit.index(&r.document.session).is_none() {
        return None;
    }
    let mut visible = edit.original.clone();
    visible.content = editor_preview(editor).0;
    if editor.get_native_visual() {
        visible.content.clear();
        visible.outline = None;
    }
    let mut object = edit
        .index(&r.document.session)
        .and_then(|index| r.document.session.objects.get(index))
        .cloned()
        .unwrap_or_else(|| Object {
            erased: Vec::new(),
            id: Default::default(),
            bounds: edit.bounds,
            kind: Kind::Text,
            style: current_style(r, 8),
            points: Vec::new(),
            extra: Extra::None,
        });
    object.bounds = edit.bounds;
    if let Extra::Sequence(s) = &object.extra {
        object.style.color = s.caption_color;
    }
    object.kind = Kind::Text;
    object.extra = Extra::Text(visible);
    Some(object)
}

/// Own only the edited object; every other native scene object stays borrowed.
pub(super) fn edited_document_object(r: &Registry) -> Option<(Option<usize>, Object)> {
    let edit = r.editor.edit()?;
    let visible = editing_text_object(r)?;
    let index = edit.index(&r.document.session);
    if let Some(index) = index
        && matches!(r.document.session.objects[index].extra, Extra::Sequence(_))
    {
        let mut object = r.document.session.objects[index].clone();
        if let (Extra::Sequence(sequence), Extra::Text(text)) = (&mut object.extra, visible.extra) {
            sequence.caption = text;
            sequence.caption_bounds = visible.bounds;
        }
        return Some((Some(index), object));
    }
    Some((index, visible))
}

pub(super) fn render_canvases(generation: u64) {
    let _render_timing = drag_timing::Span::new(drag_timing::Phase::Render);
    let deferred_revision = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        r.render_state.frame_tasks.cancel_render();
        if r.canvas_state.canvases.is_empty() {
            return None;
        }
        if r.screenshot.is_some() {
            screenshot::render(r);
            return None;
        }
        let edited_object = edited_document_object(r);
        let display_document = crate::annotation_scene::Document {
            session: &r.document.session,
            edit: edited_object
                .as_ref()
                .map(|(index, object)| (*index, object)),
        };
        let confirmed = r
            .editor
            .window()
            .filter(|editor| !editor.get_preedit().is_empty())
            .map(|editor| editor.get_value().to_string());
        let caret = r
            .editor
            .window()
            .filter(|editor| !editor.get_native_visual())
            .and_then(|editor| {
                display_document.selected().map(|index| {
                    let (_, preedit) = editor_preview(editor);
                    let offset = if preedit.is_empty() {
                        editor.get_cursor_offset().max(0) as usize
                    } else {
                        preedit.end
                    };
                    (
                        index,
                        offset,
                        preedit,
                        editor.get_anchor_offset().max(0) as usize,
                        r.editor.edit().is_some_and(|e| e.caret.visible),
                    )
                })
            });
        let editor_overlay =
            caret
                .as_ref()
                .and_then(|(index, offset, preedit, anchor, visible)| {
                    display_document.get(*index).map(|object| {
                        crate::annotation_scene::EditorOverlay {
                            object: object.sequence_caption().unwrap_or_else(|| object.clone()),
                            offset: *offset,
                            anchor: *anchor,
                            preedit: preedit.clone(),
                            visible: *visible,
                            reference: confirmed.clone(),
                        }
                    })
                });
        let active_canvas = r
            .canvas_state
            .canvases
            .iter()
            .position(|layer| layer.last_pointer.is_some())
            .unwrap_or(0);
        let canvas_order = std::iter::once(active_canvas)
            .chain((0..r.canvas_state.canvases.len()).filter(|&index| index != active_canvas));
        let has_effects = display_document
            .objects()
            .chain(gestures::display_draft(&r.gesture, &r.pending_drawing))
            .any(|object| {
                matches!(
                    object.kind,
                    Kind::MosaicArea | Kind::MosaicBrush | Kind::Magnifier
                )
            });
        let excluded = capture_exclusions(r);
        let native_scene = if has_effects
            && !r
                .render_state
                .native_presenter
                .available(r.lifecycle.annotation_renderer.as_ref())
        {
            None
        } else {
            use lexift_core::ports::annotation_render::{RenderStamp, RenderSurface};
            let surfaces: Option<Vec<_>> = r
                .canvas_state
                .canvases
                .iter()
                .map(|layer| {
                    Some(RenderSurface {
                        input_window: (r.lifecycle.annotation_window_token)(layer.window.window())?,
                        bounds: layer.bounds,
                        scale: layer.window.window().scale_factor(),
                    })
                })
                .collect();
            surfaces.and_then(|surfaces| {
                let _compile_timing = drag_timing::Span::new(drag_timing::Phase::Compile);
                r.render_state.scene_revision = r.render_state.scene_revision.wrapping_add(1);
                r.render_state
                    .scene_compiler
                    .compile(crate::annotation_scene::SceneInput {
                        stamp: RenderStamp {
                            session: generation,
                            revision: r.render_state.scene_revision,
                        },
                        session: &r.document.session,
                        edit: display_document.edit,
                        surfaces: &surfaces,
                        draft: gestures::display_draft(&r.gesture, &r.pending_drawing),
                        hovered: r.hovered,
                        reference: confirmed.as_deref(),
                        editor: editor_overlay.as_ref(),
                        paused_regions: &[],
                        excluded_windows: &excluded,
                    })
            })
        };
        // Successful native compilation already pruned the same viewport caches.
        // Keep software cleanup for unavailable or invalid native surfaces.
        if native_scene.is_none() {
            r.render_state.scene_compiler.retain_viewports(
                &r.canvas_state
                    .canvases
                    .iter()
                    .map(|layer| layer.bounds)
                    .collect::<Vec<_>>(),
            );
        }
        if native_scene.is_none() && r.render_state.native_presenter.active {
            r.render_state
                .native_presenter
                .suspend(r.lifecycle.annotation_renderer.as_ref());
        }
        if let Some(scene) = &native_scene
            && r.render_state
                .native_presenter
                .available(r.lifecycle.annotation_renderer.as_ref())
        {
            r.render_state
                .mosaic
                .stop_software(r.lifecycle.annotation_screen_capture.as_ref());
            let port = r.lifecycle.annotation_renderer.as_ref().unwrap();
            let _submit_timing = drag_timing::Span::new(drag_timing::Phase::Submit);
            match r
                .render_state
                .native_presenter
                .submit(port.as_ref(), std::sync::Arc::new(scene.clone()))
            {
                Ok(()) => r
                    .render_state
                    .native_presenter
                    .arm(generation, poll_native_renderer),
                Err(error) => {
                    tracing::warn!(%error, "native annotation submission failed; using software");
                    r.render_state
                        .native_presenter
                        .fail(r.lifecycle.annotation_renderer.as_ref());
                }
            }
            if r.render_state.native_presenter.active {
                let mut cleared = true;
                for layer in &mut r.canvas_state.canvases {
                    if layer.gpu_input_cleared {
                        continue;
                    }
                    let frame = tiny_skia::Pixmap::new(
                        (i64::from(layer.bounds.right) - i64::from(layer.bounds.left)) as u32,
                        (i64::from(layer.bounds.bottom) - i64::from(layer.bounds.top)) as u32,
                    );
                    let success = frame.is_some_and(|frame| {
                        layer.presented.submit(frame, |frame| {
                            (r.lifecycle.annotation_frame_presenter)(
                                layer.host.window.window(),
                                frame.data(),
                                frame.width(),
                                frame.height(),
                            )
                        })
                    });
                    layer.gpu_input_cleared = success;
                    layer.signature = None;
                    cleared &= success;
                }
                if cleared {
                    return None;
                }
                r.render_state
                    .native_presenter
                    .fail(r.lifecycle.annotation_renderer.as_ref());
                tracing::warn!("native annotation input surface handoff failed; using software");
            }
        }
        let native_waiting = native_scene.is_some()
            && r.render_state
                .native_presenter
                .available(r.lifecycle.annotation_renderer.as_ref());
        let (deferred_revision, measure_first_frame) = if native_waiting {
            (None, false)
        } else {
            mosaic::sync(r);
            let views = magnifier_views(
                &r.document.session,
                gestures::display_draft(&r.gesture, &r.pending_drawing),
            );
            let drawing_magnifier = matches!(
                r.gesture(),
                Some(Gesture::Draw {
                    kind: Kind::Magnifier,
                    ..
                })
            );
            let had_native_state = r.render_state.magnifier_sync != MagnifierSync::Cold;
            let deferred_revision = match magnifier_frame_action(
                &mut r.render_state.magnifier_sync,
                &mut r.render_state.magnifier_sync_revision,
                !views.is_empty(),
            ) {
                MagnifierFrameAction::Clear => {
                    r.render_state.frame_tasks.cancel_lens();
                    if !drawing_magnifier {
                        r.render_state.first_magnifier_timing = None;
                    }
                    if had_native_state {
                        sync_magnifier_views(r, &views);
                    }
                    None
                }
                MagnifierFrameAction::Defer(revision) => {
                    if !r.render_state.first_magnifier_measured {
                        let timing = r
                            .render_state
                            .first_magnifier_timing
                            .get_or_insert_with(FirstMagnifierTiming::new);
                        timing.frame_start.get_or_insert_with(Instant::now);
                    }
                    Some(revision)
                }
                MagnifierFrameAction::Sync => {
                    sync_magnifier_views(r, &views);
                    None
                }
                MagnifierFrameAction::Wait => None,
            };
            let measure_first_frame = r
                .render_state
                .first_magnifier_timing
                .as_ref()
                .is_some_and(|timing| timing.border_submitted.is_none());
            if measure_first_frame
                && let Some(timing) = r.render_state.first_magnifier_timing.as_mut()
            {
                timing.frame_start.get_or_insert_with(Instant::now);
            }
            // A failed first request may discard provisional objects; rebuild
            // only the editor substitution after all document mutations.
            (deferred_revision, measure_first_frame)
        };
        let edited_object = edited_document_object(r);
        let display_document = crate::annotation_scene::Document {
            session: &r.document.session,
            edit: edited_object
                .as_ref()
                .map(|(index, object)| (*index, object)),
        };
        let signatures = crate::annotation_signature::Frame::new(
            display_document,
            gestures::display_draft(&r.gesture, &r.pending_drawing),
        );
        if !native_waiting {
            let monitors = r
                .canvas_state
                .canvases
                .iter()
                .map(|canvas| (canvas.bounds, canvas.window.window().scale_factor()));
            r.render_state.mosaic.submit(
                &signatures,
                r.gesture().as_ref(),
                r.gesture.as_ref().and_then(Active::geometry),
                r.gesture.as_ref().and_then(Active::drawing),
                monitors,
            );
        }
        let mosaic_effects = r.render_state.mosaic.effects.clone();
        let mosaic_preview = r.render_state.mosaic.preview.as_ref();
        let mosaic_view =
            mosaic_effects
                .as_deref()
                .map(|frame| crate::annotation_render::mosaic_worker::View {
                    frame,
                    preview: mosaic_preview.as_ref().map(|p| (p.index, &p.original)),
                    remask: mosaic_preview.as_ref().is_some_and(|p| p.remask),
                });
        let mut software_presented = true;
        for index in canvas_order {
            let layer = &mut r.canvas_state.canvases[index];
            if layer.gpu_input_cleared {
                layer.gpu_input_cleared = false;
                layer.signature = None;
            }
            let mut signature = signatures.display(
                layer.bounds,
                layer.window.window().scale_factor(),
                r.hovered,
            );
            if let Some(view) = mosaic_view {
                signature ^= view.raster_signature_with(layer.bounds, &signatures);
            }
            if let Some((_, offset, ref preedit, anchor, visible)) = caret {
                signature ^= u64::from(visible).wrapping_mul(0xa24b_aed4_963e_e407);
                signature ^= (offset as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                signature ^= (preedit.start as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
                signature ^= (preedit.len() as u64).wrapping_mul(0x94d0_49bb_1331_11eb);
                signature ^= (anchor as u64).wrapping_mul(0xd6e8_feb8_6659_fd93);
            }
            if let Some(notice) = &r.render_state.mosaic.notice {
                signature ^= notice.revision.wrapping_mul(0x83b4c87f91ec7401);
            }
            if layer.signature == Some(signature) {
                continue;
            }
            layer.signature = Some(signature);
            let raster_start = Instant::now();
            let raster_timing = drag_timing::Span::new(drag_timing::Phase::Raster);
            let frame = if has_effects {
                crate::annotation_render::render_document_with_effects(
                    display_document,
                    layer.bounds,
                    layer.window.window().scale_factor(),
                    gestures::display_draft(&r.gesture, &r.pending_drawing),
                    r.hovered,
                    confirmed.as_deref(),
                    mosaic_view,
                )
            } else if let Some(scene) = &native_scene {
                crate::annotation_scene::compose_document(scene, layer.bounds)
            } else {
                r.render_state
                    .scene_compiler
                    .render_scene(crate::annotation_scene::RenderInput {
                        session: &r.document.session,
                        edit: display_document.edit,
                        viewport: layer.bounds,
                        scale: layer.window.window().scale_factor(),
                        draft: gestures::display_draft(&r.gesture, &r.pending_drawing),
                        hovered: r.hovered,
                        reference: confirmed.as_deref(),
                        editor: editor_overlay.as_ref(),
                    })
            };
            drop(raster_timing);
            if let Some(mut frame) = frame {
                if has_effects && let Some(editor) = &editor_overlay {
                    editor.draw(
                        &mut frame,
                        layer.bounds,
                        layer.window.window().scale_factor(),
                    );
                }
                if measure_first_frame
                    && let Some(timing) = r.render_state.first_magnifier_timing.as_mut()
                {
                    timing.raster += raster_start.elapsed();
                }
                if let (Some(notice), Some(toolbar)) =
                    (&r.render_state.mosaic.notice, r.toolbar_state.main.get())
                {
                    mosaic::draw_notice(
                        &mut frame,
                        notice,
                        toolbar,
                        layer.bounds,
                        layer.window.window().scale_factor(),
                    );
                }
                let present_start = Instant::now();
                let submit_timing = drag_timing::Span::new(drag_timing::Phase::Submit);
                if !layer.presented.submit(frame, |frame| {
                    (r.lifecycle.annotation_frame_presenter)(
                        layer.host.window.window(),
                        frame.data(),
                        frame.width(),
                        frame.height(),
                    )
                }) {
                    // Keep the previous successful pixels and allow this render
                    // signature to retry after a transient native failure.
                    layer.signature = None;
                    software_presented = false;
                }
                drop(submit_timing);
                if measure_first_frame
                    && let Some(timing) = r.render_state.first_magnifier_timing.as_mut()
                {
                    timing.present += present_start.elapsed();
                }
            } else {
                layer.signature = None;
                software_presented = false;
            }
        }
        if r.render_state.native_presenter.retaining() {
            let monitors: Vec<_> = r.canvas_state.canvases.iter().map(|c| c.bounds).collect();
            let needs_mosaic = display_document
                .objects()
                .chain(gestures::display_draft(&r.gesture, &r.pending_drawing))
                .any(|object| {
                    matches!(&object.extra, Extra::Mosaic(settings) if settings.strength > 0)
                        && !crate::annotation_render::mosaic::work_regions(object, &monitors)
                            .is_empty()
                });
            let needs_lens = !magnifier_views(
                &r.document.session,
                gestures::display_draft(&r.gesture, &r.pending_drawing),
            )
            .is_empty();
            if software_presented
                && (!needs_mosaic || r.render_state.mosaic.handoff_ready())
                && (!needs_lens || r.render_state.magnifier_sync == MagnifierSync::Live)
            {
                r.render_state
                    .native_presenter
                    .finish_software_handoff(r.lifecycle.annotation_renderer.as_ref());
            } else {
                r.render_state
                    .native_presenter
                    .arm(generation, retry_software_handoff);
            }
        }
        if measure_first_frame && let Some(timing) = r.render_state.first_magnifier_timing.as_mut()
        {
            timing.border_submitted = Some(Instant::now());
        }
        deferred_revision
    });
    if let Some(revision) = deferred_revision {
        // Returning to the event loop lets the compositor show the border
        // before the cold native Magnification API initialization begins.
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            if let Some(r) = slot.as_ref().filter(|r| {
                deferred_sync_is_current(
                    r.generation,
                    r.render_state.magnifier_sync,
                    generation,
                    revision,
                )
            }) {
                r.render_state
                    .frame_tasks
                    .defer_lens(generation, revision, sync_first_magnifier);
            }
        });
    }
}

pub(super) fn retry_software_handoff(generation: u64) {
    render_canvases(generation);
}

pub(super) fn poll_native_renderer(generation: u64) {
    let _confirm_timing = drag_timing::Span::new(drag_timing::Phase::Confirm);
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return false;
        };
        let Some(port) = r.lifecycle.annotation_renderer.as_ref() else {
            return false;
        };
        match r.render_state.native_presenter.poll(port.as_ref()) {
            Ok(presenter::Poll::Waiting) => {
                r.render_state
                    .native_presenter
                    .arm(generation, poll_native_renderer);
                false
            }
            Ok(presenter::Poll::Ready { activated, pending }) => {
                r.render_state
                    .mosaic
                    .confirm_native(r.render_state.native_presenter.confirmed_objects());
                if pending || r.render_state.native_presenter.live() {
                    r.render_state
                        .native_presenter
                        .arm(generation, poll_native_renderer);
                }
                activated
            }
            Err(error) => {
                tracing::warn!(%error, "native annotation presentation failed; using software");
                r.render_state
                    .native_presenter
                    .fail(r.lifecycle.annotation_renderer.as_ref());
                for layer in &mut r.canvas_state.canvases {
                    layer.signature = None;
                }
                true
            }
        }
    });
    if changed {
        schedule_render(generation);
    }
}

pub(super) fn sync_magnifier_views(
    r: &mut Registry,
    views: &[lexift_core::ports::magnifier::MagnifierSpec],
) -> bool {
    if r.screenshot.is_some() {
        return true;
    }
    let mut overlay_windows = r
        .canvas_state
        .canvases
        .iter()
        .map(|layer| layer.window.window())
        .collect::<Vec<_>>();
    if let Some(main) = r.toolbar_state.main.get() {
        overlay_windows.push(main.window());
    }
    if let Some(panel) = r.toolbar_state.panel.get() {
        overlay_windows.push(panel.window());
    }
    if let Some(choice) = &r.toolbar_state.menus.current {
        overlay_windows.push(choice.window.window());
    }
    let success = (r.lifecycle.annotation_magnifier_sync)(views, &overlay_windows);
    r.render_state.magnifier_sync = if views.is_empty() {
        MagnifierSync::Cold
    } else if success {
        MagnifierSync::Live
    } else {
        // A failed native setup may have left a partially created view.
        (r.lifecycle.annotation_magnifier_sync)(&[], &[]);
        MagnifierSync::Failed
    };
    success
}

pub(super) fn sync_first_magnifier(generation: u64, revision: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            deferred_sync_is_current(
                r.generation,
                r.render_state.magnifier_sync,
                generation,
                revision,
            )
        }) else {
            return;
        };
        let views = magnifier_views(
            &r.document.session,
            gestures::display_draft(&r.gesture, &r.pending_drawing),
        );
        if views.is_empty() {
            r.render_state.first_magnifier_timing = None;
            sync_magnifier_views(r, &[]);
            return;
        }
        let native_start = Instant::now();
        let success = sync_magnifier_views(r, &views);
        let native = native_start.elapsed();
        if let Some(timing) = r.render_state.first_magnifier_timing.take() {
            let border = timing
                .border_submitted
                .map(|submitted| submitted.duration_since(timing.first_move));
            tracing::debug!(
                native_ok = success,
                queue_ms = timing
                    .frame_start
                    .map(|started| started.duration_since(timing.first_move).as_millis() as u64),
                raster_ms = timing.raster.as_millis() as u64,
                present_ms = timing.present.as_millis() as u64,
                border_ms = border.map(|elapsed| elapsed.as_millis() as u64),
                native_ms = native.as_millis() as u64,
                "first annotation magnifier frame"
            );
            r.render_state.first_magnifier_measured = true;
        }
    });
}

pub(super) fn deferred_sync_is_current(
    current_generation: u64,
    state: MagnifierSync,
    callback_generation: u64,
    callback_revision: u64,
) -> bool {
    current_generation == callback_generation && state == MagnifierSync::Pending(callback_revision)
}

/// Maps committed magnifiers to stable native window IDs.
pub(super) fn magnifier_views(
    session: &Session,
    draft: Option<&Object>,
) -> Vec<lexift_core::ports::magnifier::MagnifierSpec> {
    session
        .objects
        .iter()
        .chain(draft)
        .enumerate()
        .filter_map(|(index, object)| {
            let Extra::Magnifier {
                output,
                zoom,
                ellipse,
                antialias,
                ..
            } = object.extra
            else {
                return None;
            };
            Some(lexift_core::ports::magnifier::MagnifierSpec {
                id: index as u64,
                preview: index == session.objects.len(),
                source: object.bounds,
                output,
                zoom,
                ellipse,
                antialias,
            })
        })
        .collect()
}

/// Both render backends exclude annotation UI by exact native identity, not process ownership.
pub(super) fn capture_exclusions(r: &Registry) -> Vec<lexift_core::ports::magnifier::WindowToken> {
    let mut windows: Vec<_> = r
        .canvas_state
        .canvases
        .iter()
        .map(|layer| layer.window.window())
        .chain(r.hints.window.get().map(ComponentHandle::window))
        .chain(r.toolbar_state.main.get().map(ComponentHandle::window))
        .chain(r.toolbar_state.panel.get().map(ComponentHandle::window))
        .chain(r.editor.window().map(ComponentHandle::window))
        .chain(r.toolbar_state.watermark.get().map(ComponentHandle::window))
        .chain(
            r.toolbar_state
                .menus
                .current
                .as_ref()
                .map(|menu| menu.window.window()),
        )
        .filter_map(|window| (r.lifecycle.annotation_window_token)(window))
        .collect();
    windows.sort_unstable_by_key(|window| window.0);
    windows.dedup();
    windows
}

pub(super) fn render_signature<'a>(
    document: impl Into<crate::annotation_scene::Document<'a>>,
    monitor: Rect,
    scale: f32,
    draft: Option<&'a Object>,
    hovered: Option<usize>,
) -> u64 {
    crate::annotation_signature::Frame::new(document, draft).display(monitor, scale, hovered)
}

#[cfg(test)]
pub(super) fn document_signature<'a>(
    document: impl Into<crate::annotation_scene::Document<'a>>,
    monitor: Rect,
    scale: f32,
    draft: Option<&'a Object>,
    replacement: Option<(usize, &Object)>,
) -> u64 {
    let frame = crate::annotation_signature::Frame::new(document, draft);
    let replacement = replacement
        .map(|(index, object)| crate::annotation_signature::Replacement::new(index, object.into()));
    frame.document_signature(monitor, scale, replacement.as_ref())
}

pub(super) fn magnifier_from_source(
    source: Bounds,
    style: Style,
    values: &AnnotationValues,
) -> Option<Object> {
    let zoom = (values.zoom as f32 / 100.).clamp(1., 8.);
    if source.width() < 2. || source.height() < 2. {
        return None;
    }
    let output = magnifier_output_for_source(source, zoom);
    Some(Object {
        erased: Vec::new(),
        id: Default::default(),
        bounds: source,
        kind: Kind::Magnifier,
        style,
        points: Vec::new(),
        extra: Extra::Magnifier {
            output,
            zoom,
            ellipse: values.shape == 1,
            connector: magnifier_connector(values.connector_style),
            erase_annotations: !values.include_annotations,
            antialias: values.antialias,
            shadow: values.shadow,
        },
    })
}

impl Registry {
    pub(super) fn queue_render(&mut self) {
        cursor::refresh(self);
        self.render_state
            .frame_tasks
            .queue_render(self.generation, render_canvases);
    }
}
