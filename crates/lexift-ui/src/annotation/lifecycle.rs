//! Annotation session startup, cancellation and teardown coordination.
use super::*;
pub(crate) fn shutdown() {
    let retired = REGISTRY.with(|s| {
        s.borrow_mut().take().map(|mut r| {
            let session = r.generation;
            r.close();
            session
        })
    });
    if let Some(session) = retired {
        emit_export(lexift_core::domain::annotation_export::ExportEvent::Closed { session });
    }
    EXPORT_HANDLER.with(|h| h.borrow_mut().take());
}
pub(crate) fn close() {
    let retired = REGISTRY.with(|s| {
        s.borrow_mut().as_mut().map(|r| {
            let session = r.generation;
            r.close();
            session
        })
    });
    // Dispatch after releasing the view borrow, before another export worker can start.
    if let Some(session) = retired {
        emit_export(lexift_core::domain::annotation_export::ExportEvent::Closed { session });
    }
    crate::bridge::schedule_idle_memory_trim();
}
#[cfg(test)]
pub(super) fn escape() {
    escape_from(current_focus_origin());
}

pub(super) fn escape_from(origin: focus::Origin) {
    let restored = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return false };
        let Some(drag) = r.toolbar_state.toolbar_drag.take() else {
            return false;
        };
        if let Some(main) = r.toolbar_state.main.get() {
            main.window().set_position(drag.origin);
        }
        true
    });
    if restored {
        reposition_panel();
        return;
    }

    if choice::dismiss_from(origin) {
        return;
    }
    let editing = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        slot.as_ref().and_then(|r| {
            if r.editor.window().is_some() {
                Some((r.generation, true, 0))
            } else if r.toolbar_state.watermark.get().is_some() {
                Some((r.generation, false, r.toolbar_state.watermark.revision()))
            } else {
                None
            }
        })
    });
    if let Some((generation, text, revision)) = editing {
        if text {
            finish_text_edit(generation, false);
        } else {
            close_watermark_input(generation, revision);
        }
        return;
    }
    let first = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if !r.render_state.mosaic.pending.is_empty() {
            cancel_active_gesture(r);
            mosaic::cancel_pending(r);
            return Some(r.generation);
        }
        if cancel_active_gesture(r) {
            refresh_hovered(r);
            return Some(r.generation);
        }
        if r.session.selected.is_some() {
            deselect_object(r);
            refresh_hovered(r);
            cursor::refresh(r);
            return Some(r.generation);
        }
        None
    });
    if let Some(generation) = first {
        schedule_render(generation);
        return;
    }
    let has_panel = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.toolbar_state.panel.get().is_some())
    });
    if has_panel {
        let target = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut()?;
            r.close_panel();
            let main = r.toolbar_state.main.get()?.clone_strong();
            permits_focus_activation(r, origin)
                .then_some((main, r.lifecycle.activate_user_requested_window))
        });
        // Escape is an explicit keyboard action; keep the next Escape in this UI session.
        if let Some((main, activate)) = target {
            activate(main.window());
        }
    } else {
        close();
    }
}

pub(super) fn finish_polyline() {
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if let drawing::Progress::Retired(Some(object)) = r.pending_drawing.finish_polyline()? {
            r.session.add(*object);
        }
        if matches!(r.gesture(), Some(Gesture::PlacePoint)) {
            r.gesture = None;
        }
        sync_history_controls(r);
        cursor::refresh(r);
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

pub(super) fn backspace_polyline() {
    let generation = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if matches!(
            r.pending_drawing.backspace_polyline()?,
            drawing::Progress::Retired(_)
        ) && matches!(r.gesture(), Some(Gesture::PlacePoint))
        {
            r.gesture = None;
        }
        Some(r.generation)
    });
    if let Some(generation) = generation {
        schedule_render(generation);
    }
}

pub(crate) fn toggle(requested_at: Instant) {
    if is_open() {
        close();
        return;
    }
    crate::bridge::cancel_idle_memory_trim();
    let callback_start = Instant::now();
    let opened = REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if !r.canvas_state.cold_open_measured {
            r.canvas_state.cold_open_timing = Some(ColdOpenTiming {
                requested_at: Some(requested_at),
                ui_queue: requested_at.elapsed(),
                ..Default::default()
            });
        }
        let toolbar_start = Instant::now();
        let Ok(main) = AnnotationToolbar::new() else {
            r.close();
            return None;
        };
        crate::theme::apply(&main);
        r.generation = r.generation.wrapping_add(1);
        let generation = r.generation;
        let revision = r.toolbar_state.main.open(main);
        let main = r.toolbar_state.main.get().unwrap();
        hints::bind_toolbar(main, generation, hints::Owner::Toolbar(revision));
        let input = r.toolbar_state.main.input();
        main.on_tool_clicked(move |group, menu| {
            later_main(&input, generation, revision, move || {
                open_panel(group as usize, menu)
            })
        });
        let input = r.toolbar_state.main.input();
        main.on_collapse_requested(move || {
            later_main(&input, generation, revision, toggle_toolbar_collapse)
        });
        let input = r.toolbar_state.main.input();
        main.on_operate_requested(move || {
            later_main(&input, generation, revision, activate_mouse_mode)
        });
        let input = r.toolbar_state.main.input();
        main.on_undo_requested(move || later_main(&input, generation, revision, undo));
        let input = r.toolbar_state.main.input();
        main.on_download_requested(move || later_main(&input, generation, revision, export::start));
        main.set_can_download(export::available(r));
        let input = r.toolbar_state.main.input();
        main.on_delete_requested(move || later_main(&input, generation, revision, delete_selected));
        let input = r.toolbar_state.main.input();
        main.on_finish_path_requested(move || {
            later_main(&input, generation, revision, finish_polyline)
        });
        let input = r.toolbar_state.main.input();
        main.on_backspace_requested(move || {
            later_main(&input, generation, revision, backspace_polyline)
        });
        let input = r.toolbar_state.main.input();
        main.on_finish_requested(move || later_main(&input, generation, revision, close));
        let input = r.toolbar_state.main.input();
        let lifecycle = r.lifecycle.clone();
        main.on_escape_requested(move || {
            let origin = focus_origin(&lifecycle);
            later_main(&input, generation, revision, move || escape_from(origin));
        });
        let lifecycle = r.lifecycle.clone();
        main.on_drag_requested(move || {
            let origin = focus_origin(&lifecycle);
            if !main_current(generation, revision) {
                return;
            }
            choice::dismiss_from(origin);
            REGISTRY.with(|slot| {
                if let Some(r) = slot.borrow_mut().as_mut()
                    && permits_focus_activation(r, origin)
                {
                    r.toolbar_state.main.cancel_position();
                    r.toolbar_state.toolbar_drag = r.toolbar_state.toolbar_press;
                }
            });
        });
        let input = r.toolbar_state.main.input();
        main.window().on_close_requested(move || {
            later_main(&input, generation, revision, close);
            slint::CloseRequestResponse::KeepWindowShown
        });
        main.window()
            .set_size(slint::LogicalSize::new(main.get_toolbar_width(), 54.));
        if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
            let elapsed = toolbar_start.elapsed();
            timing.toolbar_create += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
        }
        r.sync_mode_display();
        let query_start = Instant::now();
        r.canvas_state.pending_displays = (r.lifecycle.annotation_displays)();
        r.session.displays = r.canvas_state.pending_displays.clone();
        if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
            let elapsed = query_start.elapsed();
            timing.display_query += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
            timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
        }
        r.toolbar_state
            .main
            .prepare_after(Duration::from_millis(16), move || {
                bootstrap_tick(generation, revision)
            });
        Some(generation)
    });
    if let Some(session) = opened {
        emit_export(lexift_core::domain::annotation_export::ExportEvent::Opened { session });
    }
}

impl Registry {
    /// Child teardown can synchronously forward native input to any session window.
    /// Keep those callbacks outside the coordinator borrow without retiring owners.
    pub(super) fn pause_native_pointer_inputs(&self) -> Vec<window::PointerPause> {
        let mut guards = self.toolbar_state.pause_inputs();
        guards.extend(self.canvas_state.pause_inputs());
        if let Some(edit) = self.editor.edit() {
            guards.push(edit.pause_pointer());
        }
        guards
    }

    pub(super) fn close(&mut self) {
        self.hints.clear();
        self.fonts.subscription = None;
        defer_export(
            lexift_core::domain::annotation_export::ExportEvent::Closed {
                session: self.generation,
            },
        );
        self.export.cancel();
        drag_timing::clear();
        self.toolbar_state.toolbar_drag = None;
        self.toolbar_state.toolbar_press = None;
        // Invalidate queued input before native teardown can emit callbacks.
        // No parameter replay or editor restoration belongs to a dying session.
        self.generation = self.generation.wrapping_add(1);
        self.toolbar_state.main.retire_input();
        self.toolbar_state.panel.retire_input();
        self.toolbar_state.watermark.retire_input();
        self.editor.retire_input();
        self.toolbar_state.menus.retire_input();
        for canvas in &self.canvas_state.canvases {
            canvas.retire_input();
        }
        self.focus.cancel();
        if let Some(active) = self.gesture.take() {
            let _ = active.cancel(&mut self.session);
        }
        self.pending_drawing.cancel();
        self.render_state
            .native_presenter
            .close(self.lifecycle.annotation_renderer.as_ref());
        self.render_state.scene_compiler.clear();
        self.render_state.scene_revision = 0;
        if let Some(timing) = self.canvas_state.cold_open_timing.take() {
            timing.report("cancelled", self.canvas_state.pending_displays.len());
            self.canvas_state.cold_open_measured = true;
        }
        self.close_panel();
        self.editor.clear();
        self.close_watermark_input();
        (self.lifecycle.annotation_magnifier_sync)(&[], &[]);
        if let Some(capture) = &self.lifecycle.annotation_screen_capture {
            capture.clear();
        }
        self.render_state.mosaic = mosaic::State::default();
        crate::annotation_sticker::clear_cache();
        self.timer.stop();
        self.toolbar_state.layout_timer.stop();
        self.render_state.magnifier_timer.stop();
        self.render_state.frame_tasks.clear();
        self.canvas_state.annotation_ready = false;
        self.canvas_state.pending_displays.clear();
        self.canvas_state.bootstrap_cursor = 0;
        self.render_state.magnifier_sync = MagnifierSync::Cold;
        self.render_state.magnifier_sync_revision =
            self.render_state.magnifier_sync_revision.wrapping_add(1);
        self.render_state.first_magnifier_timing = None;
        self.render_state.first_magnifier_measured = false;
        self.pending_edit_panel = false;
        self.mode = InteractionMode::Tool(0);
        self.session.clear();
        self.toolbar_state.toolbar_owner = None;
        self.canvas_state.canvases.clear();
        self.toolbar_state.main.clear();
        self.parameter_values = parameters::State::new();
        if !self.fonts.names().is_empty() {
            self.reset_font_defaults();
        }
        self.selected = [0, 4, 7, 9, 10, 11, 13];
        self.hovered = None;
        self.sync_edit_scope();
    }
}
