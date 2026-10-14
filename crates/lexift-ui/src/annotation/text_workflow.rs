//! Annotation text workflow; scoped to its owning session.
use super::*;
pub(super) fn reset_text_caret(r: &mut Registry) {
    let Some(edit) = r.editor.edit_mut() else {
        return;
    };
    let (generation, id) = (r.generation, edit.id);
    edit.reset_caret(move || tick_text_caret(generation, id));
}

pub(super) fn tick_text_caret(generation: u64, id: u64) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && text_edit_matches(r, id))
        else {
            return false;
        };
        let composing = r
            .editor
            .window()
            .is_some_and(|e| !e.get_preedit().is_empty());
        let edit = r.editor.edit_mut().unwrap();
        edit.caret
            .tick(composing || edit.selection_anchor.is_some())
    });
    if changed {
        schedule_render(generation);
    }
}

pub(super) fn text_edit_matches(r: &Registry, id: u64) -> bool {
    r.editor.edit().is_some_and(|edit| edit.id == id)
}
pub(super) fn editing_sequence(r: &Registry) -> bool {
    r.editor
        .edit()
        .and_then(|e| e.index(&r.document.session))
        .and_then(|i| r.document.session.objects.get(i))
        .is_some_and(|o| o.kind == Kind::Sequence)
}

pub(super) fn finish_text_edit_registry(r: &mut Registry, commit: bool) {
    r.focus.cancel();
    let Some(closed) = r.editor.take() else {
        return;
    };
    choice::close_if_context_changed(r);
    if let Some(panel) = r
        .toolbar_state
        .panel
        .get()
        .filter(|panel| !panel.get_menu() && panel.get_tool() == 8)
    {
        panel.set_values(r.values(8).clone());
    }
    // Retiring the editor changes the canvas even when no document edit is committed.
    // Every caller, including native preparation failures, must invalidate that projection.
    r.queue_render();
    let target_kind = closed
        .edit
        .index(&r.document.session)
        .map(|index| r.document.session.objects[index].kind);
    let Some(change) = closed.resolve(commit, target_kind) else {
        return;
    };
    match change {
        editor::EditChange::Caption { target, text } => {
            let Some(index) = r.document.session.object_index(target) else {
                return;
            };
            if !r
                .document
                .session
                .can_edit(&r.document.session.objects[index])
            {
                return;
            }
            r.document.session.selected = Some(index);
            r.document.session.update_selected(|object| {
                if let Extra::Sequence(sequence) = &mut object.extra {
                    sequence.caption = text;
                }
                crate::annotation_sequence::layout(object);
            });
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_can_undo(r.document.session.can_undo());
                if let Some(panel) = r.toolbar_state.panel.get() {
                    panel.set_can_clear(r.document.session.has_content());
                }
            }
            return;
        }
        editor::EditChange::Text {
            target,
            text,
            bounds,
        } => {
            let Some(index) = r.document.session.object_index(target) else {
                return;
            };
            if !r
                .document
                .session
                .can_edit(&r.document.session.objects[index])
            {
                return;
            }
            r.document.session.selected = Some(index);
            r.document.session.update_selected(|object| {
                object.bounds = bounds;
                object.extra = Extra::Text(text);
            });
        }
        editor::EditChange::New(object) => {
            r.document.session.add(*object);
        }
    }
    restore_parameter_projection(r);
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_can_undo(r.document.session.can_undo());
    }
}

pub(super) fn finish_text_edit(generation: u64, commit: bool) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(r) = slot.as_mut().filter(|r| r.generation == generation) {
            finish_text_edit_registry(r, commit);
        }
    });
}

pub(super) fn finish_matching_text_edit(generation: u64, id: u64, commit: bool) {
    if REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|r| r.generation == generation && text_edit_matches(r, id))
    }) {
        finish_text_edit(generation, commit);
    }
}

/// Native and forwarded mouse events enter through the same Slint TouchArea.
pub(super) fn editor_pointer(generation: u64, id: u64, event: i32, x: f32, y: f32) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && text_edit_matches(r, id))
        else {
            return;
        };
        let Some(object) = editing_text_object(r) else {
            return;
        };
        let editor = r.editor.window().unwrap();
        let origin = editor.window().position();
        let dpi = editor.window().scale_factor();
        let point =
            pin_editor::view_point((origin.x as f32 + x * dpi, origin.y as f32 + y * dpi), true);
        let handle = if editing_sequence(r) {
            None
        } else {
            text_handle_at(&object, point, dpi)
        };
        editor.set_control_hover(
            handle.is_some() || matches!(r.gesture(), Some(Gesture::TextHandle { .. })),
        );
        if matches!(r.gesture(), Some(Gesture::TextHandle { editing: true, .. })) {
            if event == 1 || event == 2 {
                update_text_handle(r, point);
            }
            if event == 2 {
                finish_text_handle(r, point);
            }
            if event == 3 {
                cancel_active_gesture(r);
            }
            r.queue_render();
            return;
        }
        if event == 0
            && let Some(handle) = handle
        {
            begin_text_handle(r, object, handle, point, dpi, true);
            return;
        }
        let offset = crate::annotation_text::caret_at(&object, point);
        if event == 0 {
            restore_editor_focus(r);
        }
        let (editor, edit) = r.editor.parts_mut().unwrap();
        let selecting = event == 0 || edit.selection_anchor.is_some();
        match event {
            0 => {
                edit.selection_anchor = Some(offset);
                editor.invoke_place_selection(offset as i32, offset as i32);
            }
            1 | 2 => {
                if let Some(anchor) = edit.selection_anchor {
                    editor.invoke_place_selection(anchor as i32, offset as i32);
                }
                if event == 2 {
                    edit.selection_anchor = None;
                }
            }
            _ => edit.selection_anchor = None,
        }
        if selecting {
            reset_text_caret(r);
            r.queue_render();
        }
    });
}

/// Reads the latest input state: IME commit can change value and preedit together.
pub(super) fn resize_text_editor(generation: u64, id: u64) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        if !text_edit_matches(r, id) {
            return None;
        }
        let (editor, edit) = r.editor.parts_mut()?;
        let mut text = edit.original.clone();
        text.content = editor_preview(editor).0;
        let b = if let Some(mut object) = edit
            .index(&r.document.session)
            .and_then(|i| r.document.session.objects.get(i))
            .filter(|o| o.kind == Kind::Sequence)
            .cloned()
        {
            if let Extra::Sequence(s) = &mut object.extra {
                s.caption = text.clone();
            }
            crate::annotation_sequence::layout(&mut object);
            let b = object.sequence_caption().unwrap().bounds;
            edit.at = (b.left, b.top);
            b
        } else {
            crate::annotation_text::text_bounds(edit.at, &text, edit.scale)
        };
        edit.bounds = b;
        edit.laid_out_content = text.content.clone();
        place_text_editor(
            editor,
            b,
            edit.scale,
            &text,
            &r.canvas_state.canvases,
            r.screenshot.as_ref().and_then(|e| e.region.bounds()),
        );
        reset_text_caret(r);
        Some(r.generation)
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
}

/// Canvas owners supply desktop bounds; the editor owns component placement.
pub(super) fn place_text_editor(
    editor: &AnnotationTextInput,
    bounds: Bounds,
    scale: f32,
    text: &TextAnnotation,
    canvases: &[CanvasLayer],
    crop: Option<Rect>,
) {
    let displays: Vec<_> = crop
        .map(|bounds| vec![bounds])
        .unwrap_or_else(|| canvases.iter().map(|layer| layer.bounds).collect());
    editor::place(editor, bounds, scale, text, &displays);
}

#[cfg(test)]
pub(super) fn open_text_editor_object(
    generation: u64,
    at: (f32, f32),
    target: Option<ObjectId>,
    scale: f32,
) {
    open_text_editor_with_settings(generation, at, target, scale, None);
}
pub(super) fn open_text_editor_with_settings(
    generation: u64,
    at: (f32, f32),
    target: Option<ObjectId>,
    scale: f32,
    creation: Option<AnnotationValues>,
) {
    let clicked = at;
    let origin = current_focus_origin();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let index = if let Some(target) = target {
            let Some(index) = r.document.session.object_index(target) else {
                return;
            };
            if !r
                .document
                .session
                .can_edit(&r.document.session.objects[index])
            {
                return;
            }
            if !matches!(
                r.document.session.objects[index].kind,
                Kind::Text | Kind::Sequence
            ) {
                return;
            }
            Some(index)
        } else {
            if !r.parameter_entry_active(8) {
                return;
            }
            None
        };
        if !r.ensure_fonts() {
            return;
        }
        finish_text_edit_registry(r, true);
        if index.is_none()
            && let Some(values) = &creation
        {
            r.document.parameter_values.remember_creation(8, values);
        }
        let scale = index
            .and_then(|i| r.document.session.objects.get(i))
            .map_or(scale, |o| {
                if let Extra::Sequence(s) = &o.extra {
                    s.scale
                } else {
                    crate::annotation_text::object_scale(o)
                }
            });
        let original = index
            .and_then(|i| r.document.session.objects.get(i))
            .and_then(|o| {
                if let Extra::Text(text) = &o.extra {
                    Some(text.clone())
                } else if let Extra::Sequence(s) = &o.extra {
                    Some(s.caption.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| text_from_values(r, String::new()));
        let at = index
            .and_then(|i| r.document.session.objects.get(i))
            .map_or(at, |object| {
                let b = object
                    .sequence_caption()
                    .map_or(object.bounds, |o| o.bounds);
                (b.left, b.top)
            });
        let Ok(editor) = AnnotationTextInput::new() else {
            return;
        };
        crate::theme::apply(&editor);
        let generation = r.generation;
        let id = editor::next_instance();
        editor.set_value(original.content.clone().into());
        let color = index
            .and_then(|i| r.document.session.objects.get(i))
            .map_or(current_style(r, 8).color, |object| {
                if let Extra::Sequence(s) = &object.extra {
                    s.caption_color
                } else {
                    object.style.color
                }
            });
        editor::configure(&editor, &original, color);
        editor.on_pointer(move |event, x, y| editor_pointer(generation, id, event, x, y));
        let b = crate::annotation_text::text_bounds(at, &original, scale);
        place_text_editor(
            &editor,
            b,
            scale,
            &original,
            &r.canvas_state.canvases,
            r.screenshot.as_ref().and_then(|e| e.region.bounds()),
        );
        if let Some(object) = index.and_then(|i| r.document.session.objects.get(i)) {
            let caption = object.sequence_caption();
            let offset =
                crate::annotation_text::caret_at(caption.as_ref().unwrap_or(object), clicked)
                    as i32;
            editor.invoke_place_selection(offset, offset);
        }
        let edit = TextEdit::new(
            id,
            index
                .and_then(|i| r.document.session.objects.get(i))
                .map(|o| o.id),
            at,
            scale,
            original,
            b,
            editor.get_value().to_string(),
        );
        let values = index.is_none().then(|| {
            let mut values = r.document.parameter_values.defaults(8).clone();
            text_dimensions_to_values(&mut values, &edit.original);
            values
        });
        edit.bind_input(
            &editor,
            move || resize_text_editor(generation, id),
            move |commit| finish_matching_text_edit(generation, id, commit),
        );
        edit.bind_escape(&editor, move || {
            if REGISTRY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .is_some_and(|r| r.generation == generation && text_edit_matches(r, id))
            }) && !choice::dismiss_from(current_focus_origin())
            {
                finish_matching_text_edit(generation, id, false);
            }
        });
        r.editor.open(editor, edit, values);
        choice::close_if_context_changed(r);
        reset_text_caret(r);
        r.editor
            .edit()
            .unwrap()
            .prepare_after(Duration::ZERO, move || {
                show_text_editor(generation, id, 0, origin)
            });
    });
    let generation = REGISTRY.with(|slot| slot.borrow().as_ref().map(|r| r.generation));
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

#[cfg(test)]
pub(super) fn open_text_editor(at: (f32, f32), index: Option<usize>, scale: f32) {
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        let target = match index {
            Some(index) => Some(r.document.session.objects.get(index)?.id),
            None => None,
        };
        Some((r.generation, target))
    });
    if let Some((generation, target)) = request {
        open_text_editor_object(generation, at, target, scale);
    }
}

pub(super) fn show_text_editor(generation: u64, id: u64, attempt: u8, origin: focus::Origin) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        if !text_edit_matches(r, id) {
            return;
        }
        let Some(editor) = r.editor.window() else {
            return;
        };
        match (r.prepare)(editor.window()) {
            PassiveWindowPreparation::Ready => {
                let edit = r.editor.edit().unwrap();
                if !attach_text_editor_owner(r) {
                    retry_text_owner(r, generation, id, attempt, origin);
                    return;
                }
                place_text_editor(
                    editor,
                    edit.bounds,
                    edit.scale,
                    &edit.original,
                    &r.canvas_state.canvases,
                    r.screenshot.as_ref().and_then(|e| e.region.bounds()),
                );
                if editor.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        editor.window(),
                        r.editor.edit().unwrap().pointer_sink(editor),
                    )
                {
                    r.focus.cancel();
                    finish_text_edit_registry(r, false);
                    return;
                }
                if let Some(edit) = r.editor.edit() {
                    place_text_editor(
                        editor,
                        edit.bounds,
                        edit.scale,
                        &edit.original,
                        &r.canvas_state.canvases,
                        r.screenshot.as_ref().and_then(|e| e.region.bounds()),
                    );
                }
                if !attach_text_editor_owner(r) {
                    let _ = editor.hide();
                    retry_text_owner(r, generation, id, attempt, origin);
                    return;
                }
                if permits_focus_activation(r, origin) {
                    (r.lifecycle.activate_user_requested_window)(editor.window());
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = editor.show();
                let _ = editor.hide();
                r.editor
                    .edit()
                    .unwrap()
                    .prepare_after(Duration::from_millis(16), move || {
                        show_text_editor(generation, id, attempt + 1, origin)
                    });
            }
            _ => {
                r.focus.cancel();
                finish_text_edit_registry(r, false);
            }
        }
    });
}

pub(super) fn attach_text_editor_owner(r: &Registry) -> bool {
    let (Some(editor), Some(edit)) = (r.editor.window(), r.editor.edit()) else {
        return false;
    };
    let displays: Vec<_> = r
        .canvas_state
        .canvases
        .iter()
        .map(|layer| layer.bounds)
        .collect();
    let viewport = editor::viewport(edit.bounds, &displays);
    // Ownership follows the clipped visible editor, whose original bounds can be outside the canvas.
    r.canvas_state
        .canvases
        .iter()
        .find(|layer| {
            viewport.left >= layer.bounds.left as f32
                && viewport.left < layer.bounds.right as f32
                && viewport.top >= layer.bounds.top as f32
                && viewport.top < layer.bounds.bottom as f32
        })
        .is_none_or(|owner| {
            (r.lifecycle.attach_tool_window)(editor.window(), owner.window.window())
        })
}

pub(super) fn retry_text_owner(
    r: &mut Registry,
    generation: u64,
    id: u64,
    attempt: u8,
    origin: focus::Origin,
) {
    if attempt < 20 {
        r.editor
            .edit()
            .unwrap()
            .prepare_after(Duration::from_millis(16), move || {
                show_text_editor(generation, id, attempt + 1, origin)
            });
    } else {
        tracing::warn!("could not attach annotation editor after preparation retries");
        r.focus.cancel();
        finish_text_edit_registry(r, false);
    }
}

pub(super) fn text_handle_at(object: &Object, point: (f32, f32), scale: f32) -> Option<usize> {
    object
        .edit_handles(scale)
        .iter()
        .position(|p| (p.0 - point.0).abs() <= 9. * scale && (p.1 - point.1).abs() <= 9. * scale)
}

pub(super) fn editing_body_contains(object: &Object, point: (f32, f32)) -> bool {
    let Extra::Text(text) = &object.extra else {
        return false;
    };
    let local =
        lexift_core::domain::annotation::rotate_text_point(point, object.bounds, -text.rotation);
    local.0 >= object.bounds.left
        && local.0 <= object.bounds.right
        && local.1 >= object.bounds.top
        && local.1 <= object.bounds.bottom
}

pub(super) fn select_editing_text(r: &Registry, point: (f32, f32), anchor: Option<usize>) -> usize {
    let Some(object) = editing_text_object(r) else {
        return 0;
    };
    let offset = crate::annotation_text::caret_at(&object, point);
    if let Some(editor) = r.editor.window() {
        editor.invoke_place_selection(anchor.unwrap_or(offset) as i32, offset as i32);
    }
    offset
}

/// Restore the existing edit only after its originating toolbar interaction completes.
pub(super) fn restore_editor_focus(r: &Registry) {
    restore_editor_focus_with_activation(r, focus::Activation::UserRequested);
}

pub(super) fn begin_text_handle(
    r: &mut Registry,
    object: Object,
    handle: usize,
    point: (f32, f32),
    scale: f32,
    editing: bool,
) {
    let target = if editing {
        r.editor.edit().and_then(|edit| edit.object)
    } else {
        r.document
            .session
            .selected
            .and_then(|index| r.document.session.objects.get(index))
            .map(|object| object.id)
    };
    let handle = gestures::TextHandle::new(object, handle, point, scale, editing, target);
    r.gesture = Some(Active::Text(handle));
}

/// Coordinate accepted parameters and history before the handle adopts its press geometry.
pub(super) fn update_text_handle(r: &mut Registry, point: (f32, f32)) {
    let Some((info, position)) = r.text_handle().and_then(|handle| {
        handle
            .position(point)
            .map(|position| (handle.info(), position))
    }) else {
        return;
    };
    if !info.active {
        let current = if info.editing {
            editing_text_object(r)
        } else {
            info.target
                .and_then(|id| r.document.session.object_index(id))
                .and_then(|index| r.document.session.objects.get(index))
                .cloned()
        };
        let Some(current) = current else {
            return;
        };
        r.finish_style_edit();
        if !info.editing {
            r.begin_document_gesture();
        }
        let Some(handle) = r.gesture.as_mut().and_then(Active::text_mut) else {
            return;
        };
        handle.activate(current);
    }
    let Some(object) = r.text_handle().map(|handle| handle.object_at(position)) else {
        return;
    };
    if info.editing {
        apply_editing_object(r, object);
    } else if let Some(target) = info.target {
        gestures::write(&mut r.document.session, target, object);
    }
}

pub(super) fn apply_editing_object(r: &mut Registry, object: Object) {
    let Extra::Text(mut text) = object.extra else {
        return;
    };
    let Some((editor, edit)) = r.editor.parts_mut() else {
        return;
    };
    edit.bounds = object.bounds;
    edit.at = (object.bounds.left, object.bounds.top);
    edit.laid_out_content = text.content.clone();
    place_text_editor(
        editor,
        object.bounds,
        edit.scale,
        &text,
        &r.canvas_state.canvases,
        r.screenshot.as_ref().and_then(|e| e.region.bounds()),
    );
    text.content = editor.get_value().to_string();
    edit.original = text;
}

pub(super) fn finish_text_handle(r: &mut Registry, point: (f32, f32)) {
    let Some(operation) = Active::take_text(&mut r.gesture) else {
        return;
    };
    let gestures::TextHandleInfo {
        handle,
        editing,
        target,
        active,
        scale,
    } = operation.info();
    if editing
        && matches!(handle, 1 | 2)
        && let Some(mut object) = editing_text_object(r)
        && text_handle_at(&object, point, scale) == Some(handle)
    {
        if handle == 1 {
            finish_text_edit_registry(r, false);
            if let Some(index) = target.and_then(|id| r.document.session.object_index(id)) {
                r.document.session.selected = Some(index);
                r.document.session.delete_selected();
            }
        } else {
            object.toggle_text_arrow(scale);
            apply_editing_object(r, object);
        }
    }
    operation.finish(&mut r.document.session);
    r.gesture = None;
    r.flush_deferred_parameters();
    if !editing
        && active
        && let Some(index) = target.and_then(|id| r.document.session.object_index(id))
    {
        adopt_selected_style(r, index);
    }
    let parameter_object = r.parameter_object(8);
    if editing && active {
        if !r.editor.sync_draft_dimensions()
            && let Some(edit) = r.editor.edit()
        {
            text_dimensions_to_values(
                r.document.parameter_values.edit(8, parameter_object),
                &edit.original,
            );
        }
        if let Some(panel) = r.toolbar_state.panel.get()
            && panel.get_tool() == 8
        {
            panel.set_values(r.values(8).clone());
        }
    }
    r.gesture = None;

    if editing {
        restore_editor_focus(r);
    }
    cursor::refresh(r);
}
