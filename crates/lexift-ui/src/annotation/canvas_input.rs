//! Annotation canvas input; scoped to its owning session.
use super::*;
pub(super) fn scroll_eraser_size(r: &mut Registry, delta: f32) {
    if !delta.is_finite()
        || delta == 0.
        || r.gesture().is_some()
        || r.mode != InteractionMode::Tool(6)
        || r.values(13).shape != 1
    {
        return;
    }
    let mut values = r.values(13).clone();
    let Some(size) = numeric::Parameter::Size.step(values.size, 13, delta) else {
        return;
    };
    if size == values.size {
        return;
    }
    values.size = size;
    r.replace_values(13, values.clone());
    if let Some(panel) = r
        .toolbar_state
        .panel
        .get()
        .filter(|panel| !panel.get_menu() && panel.get_tool() == 13)
    {
        panel.set_values(values);
    }
    choice::sync_numeric(r);
    cursor::refresh(r);
}

pub(super) fn brush_size_scrolled(index: usize, delta: f32) {
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if !delta.is_finite()
            || delta == 0.
            || r.gesture().is_some()
            || (!matches!(active_kind(r), Some(Kind::MosaicBrush | Kind::StickerBrush))
                && !(r.mode == InteractionMode::Tool(6) && r.values(13).shape == 1))
            || index >= r.canvas_state.canvases.len()
        {
            return None;
        }
        if r.mode == InteractionMode::Tool(6) {
            scroll_eraser_size(r, delta);
            return None;
        }
        let tool = if active_kind(r) == Some(Kind::StickerBrush) {
            12
        } else {
            11
        };
        let mut values = r.values(tool).clone();
        values.size = (values.size + if delta > 0. { 1 } else { -1 }).clamp(1, 100);
        if let Some(panel) = r
            .toolbar_state
            .panel
            .get()
            .filter(|p| !p.get_menu() && p.get_tool() == tool as i32)
        {
            panel.set_values(values.clone());
            let target = r.parameter_target(tool as i32);
            apply_geometry_values(r, target, values);
        } else {
            r.replace_values(tool, values);
            let size = r.values(tool).size;
            r.session.update_selected(|object| {
                match &mut object.extra {
                    Extra::Mosaic(value) if object.kind == Kind::MosaicBrush => {
                        value.diameter = size as f32 * value.creation_scale
                    }
                    Extra::Sticker(value) if object.kind == Kind::StickerBrush => {
                        value.diameter = size as f32 * value.creation_scale
                    }
                    _ => return,
                }
                object.refresh_bounds();
            });
            cursor::refresh(r);
        }
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

pub(super) fn canvas_pointer(index: usize, event: i32, x: f32, y: f32) {
    let mut text_to_open = None;
    let result = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if !r.canvas_state.annotation_ready || r.mode.is_mouse() {
            return None;
        }
        let layer = r.canvas_state.canvases.get(index)?;
        let scale = layer.window.window().scale_factor().max(0.1);
        let point = cursor::screen_point(layer.bounds, (x, y), scale);
        let _input_timing =
            drag_timing::pointer(event, r.generation, index, r.session.objects.len());
        if event == 4 {
            r.canvas_state.canvases[index].last_pointer = None;
            let preview_changed = r.pending_drawing.pointer_left();
            if r.hovered.take().is_some() || preview_changed {
                return Some((r.generation, false));
            }
            return None;
        }
        r.canvas_state.canvases[index].last_pointer = Some(point);
        let mut open_geometry_panel = false;
        match event {
            0 => {
                // A second press cannot reuse an unfinished object's document transaction.
                if r.gesture.as_ref().is_some_and(Active::is_manipulation) {
                    cancel_active_gesture(r);
                }
                r.render_state.mosaic.gesture_epoch =
                    r.render_state.mosaic.gesture_epoch.wrapping_add(1);
                if r.editor.window().is_some() {
                    if let Some(object) = editing_text_object(r) {
                        if !editing_sequence(r)
                            && let Some(handle) = text_handle_at(&object, point, scale)
                        {
                            begin_text_handle(r, object, handle, point, scale, true);
                            return Some((r.generation, false));
                        }
                        if editing_body_contains(&object, point) {
                            let anchor = select_editing_text(r, point, None);
                            r.gesture = Some(Active::Action(Action::TextSelection { anchor }));
                            restore_editor_focus(r);
                            return Some((r.generation, false));
                        }
                    }
                    let empty = r
                        .editor
                        .window()
                        .is_some_and(|editor| editor.get_value().trim().is_empty());
                    let sequence = editing_sequence(r);
                    finish_text_edit_registry(r, true);
                    if empty && !sequence {
                        // Losing focus on an empty draft ends this gesture; the
                        // same click must not start another blank editor.
                        r.gesture = None;
                        return Some((r.generation, false));
                    }
                }
                if r.mode == InteractionMode::Tool(6) {
                    let values = r.values(13).clone();
                    let views = r
                        .canvas_state
                        .canvases
                        .iter()
                        .map(|c| (c.bounds, c.window.window().scale_factor()))
                        .collect::<Vec<_>>();
                    r.begin_document_gesture();
                    deselect_object(r);
                    r.gesture = Some(Active::Erasing(eraser::Press::begin(
                        &mut r.session,
                        point,
                        values.shape == 1,
                        values.size.clamp(1, 100) as f32 * scale,
                        &views,
                    )));
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                r.hovered = None;
                r.finish_style_edit();

                // A pending arrow may end over another object; finish it before hit testing.
                if r.arrow_pending().is_some() {
                    r.gesture = Some(Active::Action(Action::FinishArrow));
                } else if r.polyline().is_some()
                    || (r.mode == InteractionMode::Tool(2)
                        && r.selected[2] == 6
                        && r.session.hit(point, scale).is_none())
                {
                    if r.polyline().is_none() {
                        let values = capture_creation(r, 6);
                        let style = parameters::style(&values, 6);
                        r.pending_drawing.start_polyline(point, style, &values);
                    }
                    deselect_object(r);
                    r.gesture = Some(Active::Action(Action::PlacePoint));
                } else {
                    match r.session.hit(point, scale) {
                        Some(Hit::Handle(handle)) => {
                            let index = r.session.selected.unwrap();
                            if r.session.objects[index].kind == Kind::Sequence && handle < 5 {
                                r.gesture = Some(Active::Action(Action::SequenceControl {
                                    target: r.session.objects[index].id,
                                    handle,
                                }));
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 3 {
                                begin_text_handle(
                                    r,
                                    r.session.objects[index].clone(),
                                    handle,
                                    point,
                                    scale,
                                    false,
                                );
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 2 {
                                r.gesture = Some(Active::Action(Action::ToggleTextArrow {
                                    target: r.session.objects[index].id,
                                }));
                                return Some((r.generation, false));
                            }
                            if r.session.objects[index].kind == Kind::Text && handle == 1 {
                                r.session.delete_selected();
                                refresh_parameter_target(r);
                                r.gesture = Some(Active::Action(Action::DeleteText));
                                return Some((r.generation, false));
                            }
                            r.begin_document_gesture();
                            r.gesture = (gestures::GeometryDrag::resizing(
                                &r.session,
                                r.session.objects[index].id,
                                handle,
                            ))
                            .map(Active::Geometry);
                        }
                        Some(Hit::Object(index)) => {
                            r.session.selected = Some(index);
                            r.pending_edit_panel = adopt_selected_style(r, index);
                            r.begin_document_gesture();
                            r.gesture = (gestures::GeometryDrag::moving(
                                &r.session,
                                r.session.objects[index].id,
                                point,
                            ))
                            .map(Active::Geometry);
                        }
                        None => {
                            r.pending_edit_panel = false;
                            if r.mode == InteractionMode::Tool(4) {
                                begin_sequence_placement(r, point, scale);
                                cursor::refresh(r);
                                return Some((r.generation, false));
                            }
                            let tool = r.selected[r.mode.selected_group() as usize] as usize;
                            let creation = capture_creation(r, tool);
                            deselect_object(r);
                            if r.mode == InteractionMode::Tool(3) && r.selected[3] == 8 {
                                r.gesture = Some(Active::Action(Action::CreateText {
                                    at: point,
                                    scale,
                                    values: Box::new(creation.clone()),
                                }));
                            } else if matches!(r.mode, InteractionMode::Tool(0..=2 | 5))
                                && let Some(kind) = active_kind(r)
                            {
                                if kind == Kind::Magnifier
                                    && matches!(
                                        r.render_state.magnifier_sync,
                                        MagnifierSync::Pending(_)
                                    )
                                    && gestures::display_draft(&r.gesture, &r.pending_drawing)
                                        .is_none()
                                    && !r
                                        .session
                                        .objects
                                        .iter()
                                        .any(|object| object.kind == Kind::Magnifier)
                                {
                                    // A cancelled preview may still have a queued native
                                    // callback. A new gesture must receive its own border frame.
                                    r.render_state.magnifier_sync_revision =
                                        r.render_state.magnifier_sync_revision.wrapping_add(1);
                                    r.render_state.magnifier_sync = MagnifierSync::Cold;
                                    r.render_state.first_magnifier_timing = None;
                                }
                                let style = parameters::style(&creation, kind.tool() as i32);
                                let values = &creation;
                                r.gesture = Some(Active::Drawing(drawing::Press::begin(
                                    point, kind, style, scale, values,
                                )));
                            }
                        }
                    }
                }
            }
            1 => match r.gesture() {
                Some(Gesture::Erase) => {
                    let views = r
                        .canvas_state
                        .canvases
                        .iter()
                        .map(|c| (c.bounds, c.window.window().scale_factor()))
                        .collect::<Vec<_>>();
                    if let Some(Active::Erasing(press)) = &mut r.gesture {
                        press.update(&mut r.session, point, &views);
                    }
                }
                Some(Gesture::PlaceSequence) => update_sequence_placement(r, point),
                Some(Gesture::SequenceControl { .. }) => {}
                Some(Gesture::DeleteText | Gesture::ToggleTextArrow { .. }) => {}
                Some(Gesture::CreateText { .. }) => {}
                Some(Gesture::PlacePoint | Gesture::FinishArrow) => {
                    let expected = if matches!(r.gesture(), Some(Gesture::PlacePoint)) {
                        Kind::Polyline
                    } else {
                        Kind::Arrow
                    };
                    r.pending_drawing
                        .pointer_moved(point, scale, Some(expected));
                }
                Some(Gesture::Draw { kind }) => {
                    if kind == Kind::Magnifier
                        && r.render_state.magnifier_sync == MagnifierSync::Cold
                        && !r.render_state.first_magnifier_measured
                        && r.render_state.first_magnifier_timing.is_none()
                    {
                        r.render_state.first_magnifier_timing = Some(FirstMagnifierTiming::new());
                    }
                    if let Some(press) = r.gesture.as_mut().and_then(Active::drawing_mut) {
                        press.update(point, scale);
                    }
                }

                Some(Gesture::Move | Gesture::Resize { .. }) => {
                    if let Some(drag) = r.gesture.as_ref().and_then(Active::geometry) {
                        drag.update(&mut r.session, point, scale);
                    }
                }
                Some(Gesture::TextHandle { .. }) => {
                    update_text_handle(r, point);
                }
                Some(Gesture::TextSelection { anchor }) => {
                    select_editing_text(r, point, Some(anchor));
                }
                None => {
                    if !r.pending_drawing.is_none() {
                        r.pending_drawing.pointer_moved(point, scale, None);
                        return Some((r.generation, false));
                    }
                    let hovered = hovered_geometry_at(&r.session, point, scale);
                    let changed = r.hovered != hovered;
                    r.hovered = hovered;
                    cursor::refresh(r);
                    return changed.then_some((r.generation, false));
                }
            },
            2 => {
                // Native release can arrive after the gesture retired or from another control.
                // It cannot finish a snapshot owned by a continuous parameter operation.
                r.gesture()?;
                if matches!(r.gesture(), Some(Gesture::Erase)) {
                    let views = r
                        .canvas_state
                        .canvases
                        .iter()
                        .map(|c| (c.bounds, c.window.window().scale_factor()))
                        .collect::<Vec<_>>();
                    if let Some(Active::Erasing(mut press)) = r.gesture.take() {
                        press.update(&mut r.session, point, &views);
                    }
                    r.session.finish_drag();
                    r.flush_deferred_parameters();
                    sync_history_controls(r);
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if matches!(r.gesture(), Some(Gesture::PlaceSequence)) {
                    let placement = Active::take_sequence(&mut r.gesture)?;
                    let target = placement.finish(&mut r.session, point);
                    r.gesture = None;
                    r.flush_deferred_parameters();
                    r.session.selected = r.session.object_index(target);
                    if let Some(main) = r.toolbar_state.main.get() {
                        main.set_can_undo(r.session.can_undo());
                        if let Some(panel) = r.toolbar_state.panel.get() {
                            panel.set_can_clear(r.session.has_content());
                        }
                    }
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::SequenceControl { target, handle }) = r.gesture() {
                    r.gesture = None;
                    let Some(index) = r.session.object_index(target) else {
                        return Some((r.generation, false));
                    };
                    if r.session.selected == Some(index)
                        && r.session.hit(point, scale) == Some(Hit::Handle(handle))
                    {
                        if handle == 4 {
                            text_to_open =
                                Some((point, Some(r.session.objects[index].id), scale, None));
                        } else if handle == 2 {
                            r.session.delete_selected();
                        } else {
                            r.session.update_selected(|object| {
                                if handle == 3 {
                                    object.toggle_text_arrow(scale);
                                } else if let Extra::Sequence(s) = &mut object.extra {
                                    s.value = if handle == 0 {
                                        s.value.saturating_add(1)
                                    } else {
                                        (s.value - 1).max(1)
                                    };
                                }
                                crate::annotation_sequence::layout(object);
                            });
                        }
                    }
                    if let Some(main) = r.toolbar_state.main.get() {
                        main.set_can_undo(r.session.can_undo());
                        if let Some(panel) = r.toolbar_state.panel.get() {
                            panel.set_can_clear(r.session.has_content());
                        }
                    }
                    cursor::refresh(r);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::TextSelection { anchor }) = r.gesture() {
                    select_editing_text(r, point, Some(anchor));
                    r.gesture = None;
                    restore_editor_focus(r);
                    return Some((r.generation, false));
                }
                if matches!(r.gesture(), Some(Gesture::TextHandle { .. })) {
                    update_text_handle(r, point);
                    finish_text_handle(r, point);
                    return Some((r.generation, false));
                }
                if let Some(Gesture::ToggleTextArrow { target }) = r.gesture() {
                    let Some(index) = r.session.object_index(target) else {
                        r.gesture = None;
                        return Some((r.generation, false));
                    };
                    if r.session.selected == Some(index)
                        && r.session.objects.get(index).is_some()
                        && r.session.hit(point, scale) == Some(Hit::Handle(2))
                    {
                        r.session
                            .update_selected(|object| object.toggle_text_arrow(scale));
                    }
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                if matches!(r.gesture(), Some(Gesture::DeleteText)) {
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                if let Some(Gesture::CreateText { at, scale }) = r.gesture().as_ref() {
                    let values = match &r.gesture {
                        Some(Active::Action(Action::CreateText { values, .. })) => {
                            Some((**values).clone())
                        }
                        _ => None,
                    };
                    text_to_open = Some((*at, None, *scale, values));
                    r.gesture = None;
                    return Some((r.generation, false));
                }
                open_geometry_panel = std::mem::take(&mut r.pending_edit_panel);
                if matches!(r.gesture(), Some(Gesture::FinishArrow)) {
                    r.gesture = None;
                    if let Some(object) = r.pending_drawing.release_arrow(point) {
                        r.session.add(object);
                    }
                } else if matches!(r.gesture(), Some(Gesture::PlacePoint)) {
                    r.gesture = None;
                    let style = current_style(r, 6);
                    let object = r.parameter_object(6);
                    let values = r.parameter_values.values(6, object);
                    if let Some(drawing::Progress::Retired(Some(object))) = r
                        .pending_drawing
                        .place_polyline(point, scale, Instant::now(), style, values)
                    {
                        r.session.add(*object);
                    }
                } else if matches!(r.gesture(), Some(Gesture::Draw { .. })) {
                    let press = Active::take_drawing(&mut r.gesture)?;
                    match press.release(point, scale) {
                        Some(drawing::Release::Add(object)) => {
                            r.session.add(object);
                        }
                        Some(drawing::Release::StageMosaic(object)) => {
                            r.render_state.mosaic.stage(&mut r.session, object);
                        }
                        Some(drawing::Release::AwaitArrow(arrow)) => {
                            r.pending_drawing.await_arrow(arrow);
                        }
                        Some(drawing::Release::Magnifier {
                            source,
                            style,
                            values,
                        }) => {
                            if r.render_state.magnifier_sync == MagnifierSync::Cold
                                && !r.render_state.first_magnifier_measured
                                && r.render_state.first_magnifier_timing.is_none()
                            {
                                r.render_state.first_magnifier_timing =
                                    Some(FirstMagnifierTiming::new());
                            }
                            if let Some(object) = magnifier_from_source(source, style, &values) {
                                r.session.add(object);
                            }
                        }
                        None => {}
                    }
                } else {
                    if let Some(drag) = r.geometry_drag()
                        && let Some(start) = drag.move_start()
                        && let Some(index) = r.session.object_index(drag.target())
                        && (r.session.objects[index].kind == Kind::Text
                            || (r.session.objects[index].kind == Kind::Sequence
                                && r.session.objects[index]
                                    .sequence_caption()
                                    .is_some_and(|o| {
                                        point.0 >= o.bounds.left
                                            && point.0 <= o.bounds.right
                                            && point.1 >= o.bounds.top
                                            && point.1 <= o.bounds.bottom
                                    })))
                        && (point.0 - start.0).hypot(point.1 - start.1) <= 3. * scale
                    {
                        text_to_open =
                            Some((point, Some(r.session.objects[index].id), scale, None));
                        open_geometry_panel = false;
                    }
                    if let Some(drag) = Active::take_geometry(&mut r.gesture) {
                        drag.finish(&mut r.session);
                    }
                    r.gesture = None;
                    r.flush_deferred_parameters();
                    if let Some(index) = r.session.selected {
                        adopt_selected_style(r, index);
                    }
                }
            }
            3 => {
                cancel_active_gesture(r);
                mosaic::cancel_pending(r);
                r.hovered = None;
            }
            _ => return None,
        }
        sync_history_controls(r);
        refresh_parameter_target(r);
        cursor::refresh(r);
        Some((r.generation, open_geometry_panel))
    });
    if let Some((generation, open)) = result {
        schedule_render(generation);
        if let Some((point, target, scale, values)) = text_to_open {
            open_text_editor_with_settings(generation, point, target, scale, values);
        }
        if open {
            queue_geometry_panel(generation, index);
        }
    }
}

pub(super) fn queue_geometry_panel(generation: u64, index: usize) {
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().filter(|r| r.generation == generation)?;
        let layer = r.canvas_state.canvases.get(index)?;
        let target = r
            .session
            .selected
            .and_then(|selected| r.session.objects.get(selected))
            .map(|object| object.id)?;
        let group = r.mode.selected_group().max(0) as usize;
        Some((layer.input(), layer.revision(), target, group))
    });
    if let Some((input, revision, target, group)) = request {
        input.post(move || {
            let valid = REGISTRY.with(|slot| {
                slot.borrow().as_ref().is_some_and(|r| {
                    r.generation == generation
                        && !r.mode.is_mouse()
                        && r.canvas_state
                            .canvases
                            .get(index)
                            .is_some_and(|layer| layer.revision() == revision)
                        && r.mode.selected_group().max(0) as usize == group
                        && r.session
                            .selected
                            .and_then(|selected| r.session.objects.get(selected))
                            .is_some_and(|object| object.id == target)
                })
            });
            if valid {
                open_panel(group, false);
            }
        });
    }
}

pub(super) fn hovered_geometry_at(
    session: &Session,
    point: (f32, f32),
    scale: f32,
) -> Option<usize> {
    match session.hit(point, scale) {
        Some(Hit::Object(index))
            if session.selected != Some(index)
                && matches!(
                    session.objects[index].kind,
                    Kind::Rectangle | Kind::Ellipse | Kind::HighlightRectangle
                ) =>
        {
            Some(index)
        }
        _ => None,
    }
}

pub(super) fn refresh_hovered(r: &mut Registry) {
    r.hovered = if r.mode.is_mouse() {
        None
    } else {
        r.canvas_state.canvases.iter().find_map(|layer| {
            let point = layer.last_pointer?;
            hovered_geometry_at(
                &r.session,
                point,
                layer.window.window().scale_factor().max(0.1),
            )
        })
    };
}

pub(super) fn activate_mouse_mode() {
    let generation = REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            if r.mode == InteractionMode::Mouse {
                r.close_panel();
                return None;
            }
            if !r.set_mode(InteractionMode::Mouse) {
                return None;
            }
            r.finish_style_edit();
            r.close_panel();
            // set_mode already retired the active gesture through its owner.
            deselect_object(r);
            return Some(r.generation);
        }
        None
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

pub(super) fn undo() {
    let generation = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if cancel_object_gesture(r) {
            cursor::refresh(r);
            return Some(r.generation);
        }
        if r.editor.window().is_some() {
            finish_text_edit_registry(r, true);
        }
        r.finish_style_edit();
        let erased = eraser::has_coverage(&r.session);
        if !r.session.undo() {
            return None;
        }
        if erased || eraser::has_coverage(&r.session) {
            // Restoring alpha coverage must retire the accepted native candidate,
            // not leave a pre-undo presentation eligible for composition reuse.
            r.render_state
                .native_presenter
                .suspend(r.lifecycle.annotation_renderer.as_ref());
            for layer in &mut r.canvas_state.canvases {
                layer.gpu_input_cleared = false;
                layer.signature = None;
            }
        }
        restore_parameter_projection(r);
        sync_sequence_counter(r);
        adopt_watermark_style(r);
        refresh_hovered(r);
        sync_history_controls(r);
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}
pub(super) fn delete_selected() {
    let generation = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if cancel_sequence_placement(r) {
            cursor::refresh(r);
            return Some(r.generation);
        }
        cancel_object_gesture(r);
        if r.editor.window().is_some() {
            finish_text_edit_registry(r, false);
        }
        r.session.delete_selected();
        refresh_parameter_target(r);
        refresh_hovered(r);
        sync_history_controls(r);
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

impl Registry {
    pub(super) fn arrow_pending(&self) -> Option<&ArrowPending> {
        self.pending_drawing.arrow()
    }

    pub(super) fn polyline(&self) -> Option<&Polyline> {
        self.pending_drawing.polyline()
    }

    pub(super) fn gesture(&self) -> Option<Gesture> {
        self.gesture.as_ref().map(Active::route)
    }

    #[cfg(test)]
    pub(super) fn sequence_placement(&self) -> Option<&gestures::SequencePlacement> {
        self.gesture.as_ref().and_then(Active::sequence)
    }

    pub(super) fn geometry_drag(&self) -> Option<&gestures::GeometryDrag> {
        self.gesture.as_ref().and_then(Active::geometry)
    }

    pub(super) fn text_handle(&self) -> Option<&gestures::TextHandle> {
        self.gesture.as_ref().and_then(Active::text)
    }

    #[cfg(test)]
    pub(super) fn drawing_press(&self) -> Option<&drawing::Press> {
        self.gesture.as_ref().and_then(Active::drawing)
    }

    /// Keep selection and parameter projection within the active toolbar entry.
    pub(super) fn sync_edit_scope(&mut self) {
        let scope = if self.mode.is_mouse() {
            EditScope::None
        } else {
            tools::scope(self.selected[self.mode.selected_group() as usize] as usize)
        };
        let previous = self.session.selected;
        let hovered = self.hovered.take();
        self.session.set_edit_scope(scope);
        if previous != self.session.selected {
            deselect_object(self);
        }
        if previous != self.session.selected || hovered.is_some() {
            self.queue_render();
        }
    }

    /// Apply the native input policy before exposing a new mode in the toolbar.
    pub(super) fn set_mode(&mut self, mode: InteractionMode) -> bool {
        if self.mode == mode {
            self.sync_edit_scope();
            cursor::refresh(self);
            self.sync_mode_display();
            return true;
        }
        cancel_active_gesture(self);
        self.finish_style_edit();
        self.close_watermark_input();
        if self.editor.window().is_some() {
            finish_text_edit_registry(self, true);
        }
        if self.mode.is_mouse() != mode.is_mouse() {
            let mut changed = Vec::new();
            for (index, layer) in self.canvas_state.canvases.iter().enumerate() {
                if !(self.lifecycle.annotation_click_through)(
                    layer.host.window.window(),
                    mode.is_mouse(),
                ) {
                    for index in changed {
                        let layer: &CanvasLayer = &self.canvas_state.canvases[index];
                        (self.lifecycle.annotation_click_through)(
                            layer.host.window.window(),
                            self.mode.is_mouse(),
                        );
                    }
                    return false;
                }
                changed.push(index);
            }
        }
        self.mode = mode;
        self.sync_edit_scope();
        if mode != InteractionMode::Tool(5) {
            mosaic::cancel_pending(self);
        }
        self.hovered = None;
        cursor::refresh(self);
        self.sync_mode_display();
        true
    }

    /// A variant change retires the same capture as a group change, including
    /// when set_mode is a no-op because this group was already active.
    pub(super) fn select_tool_variant(&mut self, group: usize, tool: i32) -> bool {
        if !self.set_mode(InteractionMode::Tool(group)) {
            return false;
        }
        if self.selected[group] != tool {
            cancel_active_gesture(self);
            self.finish_style_edit();
            self.close_watermark_input();
            if self.editor.window().is_some() {
                finish_text_edit_registry(self, true);
            }
        }
        self.close_panel();
        self.selected[group] = tool;
        self.sync_edit_scope();
        cursor::refresh(self);
        if let Some(main) = self.toolbar_state.main.get() {
            main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(self))));
        }
        true
    }

    /// Finish parameter history before a geometry snapshot takes ownership.
    /// Keeping the handoff here prevents callers from depending on pointer routing.
    pub(super) fn begin_document_gesture(&mut self) {
        self.finish_style_edit();
        self.session.begin_drag();
    }
}
