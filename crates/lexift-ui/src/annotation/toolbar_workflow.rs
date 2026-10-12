//! Annotation toolbar workflow; scoped to its owning session.
use super::*;
pub(super) fn show_main(generation: u64, revision: u64, attempt: u8) {
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.toolbar_state.main.revision() == revision)
        else {
            return;
        };
        let Some(main) = r
            .toolbar_state
            .main
            .get()
            .map(ComponentHandle::clone_strong)
        else {
            return;
        };
        let prepare_start = Instant::now();
        let preparation = (r.prepare)(main.window());
        if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
            let elapsed = prepare_start.elapsed();
            timing.toolbar_prepare += elapsed;
            timing.largest_stage = timing.largest_stage.max(elapsed);
        }
        match preparation {
            PassiveWindowPreparation::Ready => {
                let place_start = Instant::now();
                let anchor = (r.lifecycle.toolbar_cursor_position)().unwrap_or_default();
                if let Some(area) = (r.lifecycle.popup_work_area)(anchor) {
                    let scale = main.window().scale_factor();
                    let width = (main.get_toolbar_width() * scale)
                        .min((area.right - area.left) as f32)
                        .max(1.);
                    main.window()
                        .set_size(slint::PhysicalSize::new(width as u32, (54. * scale) as u32));
                    let size = main.window().size();
                    let p = placement::place_popup(
                        anchor,
                        size.width,
                        size.height,
                        area,
                        (16. * scale) as i32,
                        (16. * scale) as i32,
                    )
                    .position;
                    main.window()
                        .set_position(slint::PhysicalPosition::new(p.x, p.y));
                }
                if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
                    let elapsed = place_start.elapsed();
                    timing.toolbar_place += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                }
                let show_start = Instant::now();
                if !sync_toolbar_owner(r) {
                    retry_main_owner(r, generation, revision, attempt);
                    return;
                }
                if main.show().is_err() {
                    r.close();
                    return;
                }
                if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
                    let elapsed = show_start.elapsed();
                    timing.toolbar_show += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                }
                let native_start = Instant::now();
                if !(r.lifecycle.complete_toolbar_show)(
                    main.window(),
                    pointer_main(
                        main.as_weak(),
                        generation,
                        revision,
                        r.toolbar_state.main.input(),
                    ),
                ) {
                    r.close();
                    return;
                }
                if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
                    let elapsed = native_start.elapsed();
                    timing.toolbar_native_complete += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                }
                let finish_start = Instant::now();
                r.toolbar_state.toolbar_owner = None;
                if !sync_toolbar_owner(r) {
                    let _ = main.hide();
                    retry_main_owner(r, generation, revision, attempt);
                    return;
                }
                settle_main_position(r, generation, revision, anchor, 2);
                r.timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(33),
                    reposition_panel,
                );
                r.toolbar_state.layout_timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_secs(1),
                    check_display_layout,
                );
                r.render_state.magnifier_timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(66),
                    refresh_magnifiers,
                );
                r.canvas_state.annotation_ready = true;
                r.canvas_state.pending_displays.clear();
                // Build the empty native scene while the canvases are ready,
                // before the first drag has to pay for software/native handoff.
                // Submission remains asynchronous; no capture or UI wait is added.
                if r.render_state
                    .native_presenter
                    .available(r.lifecycle.annotation_renderer.as_ref())
                {
                    r.queue_render();
                }
                if let Some(mut timing) = r.canvas_state.cold_open_timing.take() {
                    let elapsed = finish_start.elapsed();
                    timing.toolbar_finish += elapsed;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                    timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
                    timing.report("ready", r.canvas_state.canvases.len());
                    r.canvas_state.cold_open_measured = true;
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let retry_start = Instant::now();
                let _ = main.show();
                let _ = main.hide();
                if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
                    let elapsed = retry_start.elapsed();
                    timing.toolbar_retry += elapsed;
                    timing.toolbar_retries += 1;
                    timing.largest_stage = timing.largest_stage.max(elapsed);
                    timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
                }
                r.toolbar_state
                    .main
                    .prepare_after(Duration::from_millis(16), move || {
                        show_main(generation, revision, attempt + 1)
                    });
            }
            _ => r.close(),
        }
    });
}

pub(super) fn refresh_magnifiers() {
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let Some(r) = slot.as_ref() else { return };
        if r.render_state.native_presenter.in_use() {
            return;
        }
        if r.session
            .objects
            .iter()
            .chain(gestures::display_draft(&r.gesture, &r.pending_drawing))
            .any(|object| object.kind == Kind::Magnifier)
        {
            (r.lifecycle.annotation_magnifier_refresh)();
        }
    });
}

pub(super) fn check_display_layout() {
    let changed = REGISTRY.with(|s| {
        let slot = s.borrow();
        let Some(r) = slot
            .as_ref()
            .filter(|r| r.toolbar_state.main.get().is_some())
        else {
            return false;
        };
        (r.lifecycle.annotation_displays)()
            != r.canvas_state
                .canvases
                .iter()
                .map(|c| c.bounds)
                .collect::<Vec<_>>()
    });
    if changed {
        close();
    }
}

/// Winit applies the destination monitor DPI after showing a native window.
/// Reconcile the anchor using realized physical size; the window owns and cancels retries.
pub(super) fn settle_main_position(
    r: &Registry,
    generation: u64,
    revision: u64,
    anchor: Point,
    remaining: u8,
) {
    r.toolbar_state
        .main
        .position_after(Duration::from_millis(32), move || {
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let Some(r) = slot.as_ref().filter(|r| {
                    r.generation == generation && r.toolbar_state.main.revision() == revision
                }) else {
                    return;
                };
                let Some(main) = r.toolbar_state.main.get() else {
                    return;
                };
                if let Some(area) = (r.lifecycle.popup_work_area)(anchor) {
                    let size = main.window().size();
                    let gap = (16. * main.window().scale_factor()) as i32;
                    let point =
                        placement::place_popup(anchor, size.width, size.height, area, gap, gap)
                            .position;
                    main.window()
                        .set_position(slint::PhysicalPosition::new(point.x, point.y));
                }
                if remaining > 0 {
                    settle_main_position(r, generation, revision, anchor, remaining - 1);
                }
            });
        });
}

pub(super) fn open_panel(group: usize, menu: bool) {
    let requested_at = Instant::now();
    if group >= GROUPS.len() {
        return;
    }
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.toolbar_state
            .main
            .get()
            .is_none_or(|main| main.get_collapsed())
        {
            return;
        }
        if !menu && !r.set_mode(InteractionMode::Tool(group)) {
            return;
        }
        let same = r.toolbar_state.group == Some(group)
            && r.toolbar_state
                .panel
                .get()
                .is_some_and(|p| p.get_menu() == menu);
        r.close_panel();
        if same {
            return;
        }
        let create_at = Instant::now();
        let Ok(panel) = AnnotationPanel::new() else {
            return;
        };
        let create = create_at.elapsed();
        crate::theme::apply(&panel);
        let tool = r.selected[group];
        let fonts_at = Instant::now();
        // A tool picker has no font controls or parameter transaction. Keep the
        // installed-font scan on the paths that actually need editable text.
        if !menu {
            if matches!(tool, 8..=10) {
                r.ensure_fonts();
            }
            if tool == 9 {
                adopt_watermark_style(r);
            }
            panel.set_font_families(ModelRc::new(VecModel::from(
                r.fonts
                    .names()
                    .iter()
                    .cloned()
                    .map(Into::into)
                    .collect::<Vec<_>>(),
            )));
        }
        let fonts = if menu {
            Duration::ZERO
        } else {
            fonts_at.elapsed()
        };
        if !menu {
            panel.set_can_clear(r.session.has_content());
        }
        panel.set_tool(tool);
        panel.set_menu(menu);
        panel.set_heading(crate::i18n::tr(NAMES[tool as usize]).into());
        if !menu {
            panel.set_values(r.values(tool as usize).clone());
        }
        if !menu {
            panel.set_mosaic_available(
                r.lifecycle
                    .annotation_screen_capture
                    .as_ref()
                    .is_some_and(|p| p.is_available()),
            );
            let [red, green, blue] = current_style(r, tool).color;
            panel.set_custom_preview(slint::Color::from_rgb_u8(red, green, blue));
            panel.set_fields(ModelRc::new(VecModel::from(fields(tool))));
        }
        panel.set_menu_tools(ModelRc::new(VecModel::from(GROUPS[group].to_vec())));
        panel.set_menu_labels(ModelRc::new(VecModel::from(
            GROUPS[group]
                .iter()
                .map(|&t| crate::i18n::tr(NAMES[t as usize]).into())
                .collect::<Vec<_>>(),
        )));
        let generation = r.generation;
        let dismissal = r.lifecycle.set_popup_dismissal.clone();
        let panel_revision = r
            .toolbar_state
            .panel
            .open_with_retirement(panel, move |window| {
                dismissal(window, false);
            });
        if !menu {
            r.parameter_values.bind(r.parameter_object(tool as usize));
        }
        let panel = r.toolbar_state.panel.get().unwrap();
        if !menu && group <= 6 {
            panel.on_numeric_scrolled(move |field, delta| {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    if let Some(r) = slot.as_mut().filter(|r| {
                        r.generation == generation
                            && r.toolbar_state.panel.revision() == panel_revision
                            && r.parameter_entry_active(tool)
                    }) && let Some(parameter) = numeric::Parameter::from_field(field)
                    {
                        let target = r.parameter_target(tool);
                        r.scroll_numeric(target, parameter, delta);
                    }
                });
            });
            let input = r.toolbar_state.panel.input();
            let layout_input = input.clone();
            panel.on_numeric_layout_changed(move || {
                layout_input.post(move || {
                    REGISTRY.with(|slot| {
                        if let Some(r) = slot.borrow().as_ref().filter(|r| {
                            r.generation == generation
                                && r.toolbar_state.panel.revision() == panel_revision
                        }) && r.toolbar_state.panel_preparation.ready
                        {
                            position_panel(r);
                        }
                    })
                });
            });
            let observed = parameters::Notifications::new(panel.get_values());
            panel.on_values_changed(move |values| {
                let target = REGISTRY.with(|slot| {
                    let slot = slot.try_borrow().ok()?;
                    let r = slot.as_ref().filter(|r| {
                        r.generation == generation
                            && r.toolbar_state.panel.revision() == panel_revision
                    })?;
                    Some(r.parameter_target(tool))
                });
                let patch = observed.observe(values, target.is_some());
                let (Some(target), Some(patch)) = (target, patch) else {
                    return;
                };
                input.post(move || {
                    if panel_current(generation, panel_revision) {
                        update_geometry_patch(generation, target, patch);
                    }
                })
            });
        }
        let lifecycle = r.lifecycle.clone();
        panel::bind_input(
            panel,
            r.toolbar_state.panel.input(),
            Rc::new(move || focus_origin(&lifecycle)),
            Rc::new(move |event| apply_panel_input(generation, panel_revision, group, event)),
        );
        r.toolbar_state.panel_preparation.timing = Some(toolbar::PanelTiming::new(
            requested_at,
            create,
            fonts,
            menu,
            generation,
            panel_revision,
        ));
        if !menu && matches!(tool, 8..=10) {
            r.ensure_fonts();
            font_workflow::sync_panel(r);
            r.toolbar_state
                .panel
                .get()
                .unwrap()
                .on_font_retry_requested(move || font_workflow::retry(generation, panel_revision));
        }
        r.toolbar_state.group = Some(group);
        r.toolbar_state
            .panel
            .prepare_after(Duration::ZERO, move || {
                show_panel(generation, panel_revision, 0)
            });
    });
}

/// The panel's FIFO owner delivers typed events here; target and window identity
/// remain session decisions rather than component callback responsibilities.
pub(super) fn apply_panel_input(generation: u64, revision: u64, group: usize, event: panel::Input) {
    match event {
        panel::Input::Finished { confirmed, origin } => REGISTRY.with(|slot| {
            if let Some(r) = slot.borrow().as_ref().filter(|r| {
                r.generation == generation && r.toolbar_state.panel.revision() == revision
            }) {
                restore_editor_focus_from(
                    r,
                    if confirmed {
                        focus::Activation::UserRequested
                    } else {
                        focus::Activation::Local
                    },
                    origin,
                );
            }
        }),
        panel::Input::Watermark(origin) => {
            if panel_current(generation, revision) {
                open_watermark_input_from(generation, origin);
            }
        }
        panel::Input::Dragging { active, origin } => REGISTRY.with(|slot| {
            if let Some(r) = slot.borrow_mut().as_mut().filter(|r| {
                r.generation == generation && r.toolbar_state.panel.revision() == revision
            }) {
                if active {
                    r.finish_style_edit();
                }
                r.parameter_edit.set_dragging(active);
                if !active {
                    r.finish_style_edit();
                    restore_editor_focus_from(r, focus::Activation::UserRequested, origin);
                }
            }
        }),
        panel::Input::Tool(tool) => {
            let selected = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(r) = slot.as_mut().filter(|r| {
                    r.generation == generation
                        && r.toolbar_state.panel.revision() == revision
                        && r.toolbar_state.group == Some(group)
                        && r.toolbar_state
                            .panel
                            .get()
                            .is_some_and(|panel| panel.get_menu())
                }) else {
                    return false;
                };
                r.select_tool_variant(group, tool)
            });
            if selected {
                open_panel(group, false);
            }
        }
        panel::Input::StyleChoice { request, origin } => {
            choice::toggle_style(generation, revision, request, origin);
        }
        panel::Input::Choice { request, origin } => {
            choice::open_from(generation, revision, request, origin);
        }
        panel::Input::Escape(origin) => {
            if panel_current(generation, revision) {
                escape_from(origin);
            }
        }
        panel::Input::ClearAll => {
            if panel_current(generation, revision) {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(r) = slot.as_mut() else { return };
                    if r.mode != InteractionMode::Tool(6) {
                        return;
                    }
                    cancel_active_gesture(r);
                    r.finish_style_edit();
                    if r.session.clear_all() {
                        deselect_object(r);
                        r.hovered = None;
                        sync_sequence_counter(r);
                        r.queue_render();
                    }
                    if let Some(panel) = r.toolbar_state.panel.get() {
                        panel.set_can_clear(r.session.has_content());
                    }
                    if let Some(main) = r.toolbar_state.main.get() {
                        main.set_can_undo(r.session.can_undo());
                    }
                });
            }
        }
        panel::Input::Undo => {
            let current = REGISTRY.with(|slot| {
                slot.borrow().as_ref().is_some_and(|r| {
                    r.generation == generation
                        && r.toolbar_state.panel.revision() == revision
                        && r.toolbar_state
                            .panel
                            .get()
                            .is_some_and(|p| matches!(p.get_tool(), 11..=13))
                })
            });
            if current {
                undo();
            }
        }
    }
}

#[cfg(test)]
pub(super) fn open_watermark_input(generation: u64) {
    open_watermark_input_from(generation, current_focus_origin());
}

pub(super) fn open_watermark_input_from(generation: u64, request_origin: focus::Origin) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        if !r.ensure_fonts() {
            return;
        }
        r.close_watermark_input();
        let Some(main) = r.toolbar_state.main.get() else {
            return;
        };
        let origin = main.window().position();
        // Seed the owner monitor before native preparation establishes the editor DPI.
        let Some(editor) = watermark::build_window(watermark::Request {
            value: r.values(9).text.clone(),
            position: Point {
                x: origin.x,
                y: origin.y,
            },
        }) else {
            return;
        };
        let revision = r.toolbar_state.watermark.open(editor);
        watermark::bind_input(
            r.toolbar_state.watermark.get().unwrap(),
            r.toolbar_state.watermark.input(),
            Rc::new(move |event| apply_watermark_input(generation, revision, event)),
        );
        r.toolbar_state
            .watermark
            .prepare_after(Duration::ZERO, move || {
                show_watermark_input(generation, revision, 0, request_origin)
            });
    });
}

pub(super) fn apply_watermark_input(generation: u64, revision: u64, event: watermark::Input) {
    match event {
        watermark::Input::Presets {
            x,
            y,
            width,
            height,
        } => {
            choice::open_watermark(generation, revision, x, y, width, height);
        }
        watermark::Input::DismissPresets => choice::dismiss_watermark(generation, revision),
        watermark::Input::Apply(template) => apply_watermark(generation, revision, template),
        watermark::Input::Cancel => close_watermark_input(generation, revision),
    }
}

pub(super) fn show_watermark_input(
    generation: u64,
    revision: u64,
    attempt: u8,
    origin: focus::Origin,
) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation && r.toolbar_state.watermark.revision() == revision
        }) else {
            return;
        };
        let Some(editor) = r.toolbar_state.watermark.get() else {
            return;
        };
        match (r.prepare)(editor.window()) {
            PassiveWindowPreparation::Ready => {
                let owner_ready = r.toolbar_state.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(editor.window(), main.window())
                });
                if !owner_ready {
                    retry_watermark_owner(r, generation, revision, attempt, origin);
                    return;
                }
                position_watermark_input(r);
                if editor.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        editor.window(),
                        watermark::pointer_sink(
                            editor.as_weak(),
                            move || {
                                REGISTRY.with(|slot| {
                                    slot.borrow().as_ref().is_some_and(|r| {
                                        r.generation == generation
                                            && r.toolbar_state.watermark.revision() == revision
                                            && r.toolbar_state.watermark.get().is_some()
                                    })
                                })
                            },
                            r.toolbar_state.watermark.input(),
                            choice::watermark_pointer,
                        ),
                    )
                {
                    r.close_watermark_input();
                    return;
                }
                if !r.toolbar_state.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(editor.window(), main.window())
                }) {
                    let _ = editor.hide();
                    retry_watermark_owner(r, generation, revision, attempt, origin);
                    return;
                }
                position_watermark_input(r);
                schedule_watermark_position(r, generation, revision, 2);
                if permits_focus_activation(r, origin) {
                    (r.lifecycle.activate_user_requested_window)(editor.window());
                }
                if r.render_state
                    .native_presenter
                    .exclusions_changed(&capture_exclusions(r))
                {
                    r.queue_render();
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = editor.show();
                let _ = editor.hide();
                r.toolbar_state
                    .watermark
                    .prepare_after(Duration::from_millis(16), move || {
                        show_watermark_input(generation, revision, attempt + 1, origin)
                    });
            }
            _ => {
                r.close_watermark_input();
            }
        }
    });
}

pub(super) fn position_watermark_input(r: &Registry) {
    let (Some(main), Some(editor)) = (r.toolbar_state.main.get(), r.toolbar_state.watermark.get())
    else {
        return;
    };
    let origin = main.window().position();
    let toolbar_size = main.window().size();
    let toolbar = Rect {
        left: origin.x,
        top: origin.y,
        right: origin
            .x
            .saturating_add(toolbar_size.width.min(i32::MAX as u32) as i32),
        bottom: origin
            .y
            .saturating_add(toolbar_size.height.min(i32::MAX as u32) as i32),
    };
    let area = (r.lifecycle.popup_work_area)(Point {
        x: origin.x,
        y: origin.y,
    });
    let target = watermark::position(
        toolbar,
        editor.window().size(),
        editor.window().scale_factor(),
        area,
    );
    let target = slint::PhysicalPosition::new(target.x, target.y);
    if editor.window().position() != target {
        editor.window().set_position(target);
    }
}

pub(super) fn retry_watermark_owner(
    r: &mut Registry,
    generation: u64,
    revision: u64,
    attempt: u8,
    origin: focus::Origin,
) {
    if attempt < 20 {
        r.toolbar_state
            .watermark
            .prepare_after(Duration::from_millis(16), move || {
                show_watermark_input(generation, revision, attempt + 1, origin)
            });
    } else {
        tracing::warn!("could not attach watermark editor after preparation retries");
        r.close_watermark_input();
    }
}

pub(super) fn schedule_watermark_position(
    r: &Registry,
    generation: u64,
    revision: u64,
    remaining: u8,
) {
    r.toolbar_state
        .watermark
        .position_after(Duration::from_millis(32), move || {
            settle_watermark_position(generation, revision, remaining);
        });
}

pub(super) fn settle_watermark_position(generation: u64, revision: u64, remaining: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.toolbar_state.watermark.revision() == revision
                && r.toolbar_state.watermark.get().is_some()
        }) else {
            return;
        };
        position_watermark_input(r);
        if remaining > 1 {
            schedule_watermark_position(r, generation, revision, remaining - 1);
        }
        // Window replacement changes capture exclusions; movement alone does not.
        if r.render_state
            .native_presenter
            .exclusions_changed(&capture_exclusions(r))
        {
            r.queue_render();
        }
    });
}

pub(super) fn close_watermark_input(generation: u64, revision: u64) {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut().filter(|r| {
            r.generation == generation && r.toolbar_state.watermark.revision() == revision
        }) {
            r.close_watermark_input();
        }
    });
}

pub(super) fn apply_watermark(generation: u64, revision: u64, template: String) {
    let changed = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.toolbar_state.watermark.revision() == revision
                && r.toolbar_state.watermark.get().is_some()
        })?;
        r.close_watermark_input();
        r.finish_style_edit();
        let content =
            crate::annotation_text::resolve_watermark_template(&template, chrono::Local::now());
        let watermark =
            (!content.trim().is_empty()).then(|| watermark_from_values(r, template, content));
        r.session.set_watermark(watermark);
        adopt_watermark_style(r);
        sync_history_controls(r);
        Some(r.generation)
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
}

pub(super) fn show_panel(generation: u64, panel_revision: u64, attempt: u8) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation && r.toolbar_state.panel.revision() == panel_revision
        }) else {
            return;
        };
        let Some(panel) = r.toolbar_state.panel.get() else {
            return;
        };
        if r.toolbar_state.panel_preparation.ready {
            return;
        }
        let prepare_at = Instant::now();
        let preparation = (r.prepare)(panel.window());
        if let Some(timing) = &mut r.toolbar_state.panel_preparation.timing {
            timing.native += prepare_at.elapsed();
            timing.retries = attempt;
        }
        match preparation {
            PassiveWindowPreparation::Ready => {
                if !panel.get_menu() && !r.toolbar_state.panel_preparation.measured {
                    // Evaluate the hidden component with its actual renderer.
                    // Text/layout caches may otherwise acquire their final metrics
                    // only during show(), after our width measurement has finished.
                    let render_at = Instant::now();
                    let _ = panel.window().take_snapshot();
                    if let Some(timing) = &mut r.toolbar_state.panel_preparation.timing {
                        timing.hidden_render += render_at.elapsed();
                    }
                    r.toolbar_state.panel_preparation.measured = true;
                }
                if !position_panel(r) {
                    r.close_panel();
                    return;
                }
                let panel = r.toolbar_state.panel.get().unwrap();
                // Parameter panels need final-width Flexbox evaluation; pickers
                // have fixed dimensions and need only this final-size hidden render.
                let render_at = Instant::now();
                let _ = panel.window().take_snapshot();
                if let Some(timing) = &mut r.toolbar_state.panel_preparation.timing {
                    timing.hidden_render += render_at.elapsed();
                }
                if !r.toolbar_state.main.get().is_some_and(|main| {
                    (r.lifecycle.attach_tool_window)(panel.window(), main.window())
                }) {
                    retry_panel_preparation(r, generation, panel_revision, attempt);
                    return;
                }
                let layout = toolbar::Layout {
                    token: (r.lifecycle.annotation_window_token)(panel.window()),
                    scale: panel.window().scale_factor(),
                    position: panel.window().position(),
                    size: panel.window().size(),
                    content: (
                        panel.get_tool(),
                        panel.get_menu(),
                        if panel.get_menu() {
                            220.
                        } else {
                            panel.get_numeric_content_width()
                        },
                        if panel.get_menu() {
                            46. + GROUPS[r.toolbar_state.group.unwrap_or(0)].len() as f32 * 42.
                        } else {
                            panel.get_numeric_content_height()
                        },
                    ),
                    values: if panel.get_menu() {
                        Default::default()
                    } else {
                        panel.get_values()
                    },
                    language: panel.get_language_revision(),
                };
                if !r.toolbar_state.panel_preparation.observe(layout) {
                    retry_panel_preparation(r, generation, panel_revision, attempt);
                    return;
                }
                // Configure the frame and input bridge while hidden. Winit paints
                // the final transparent Slint frame before mapping this HWND.
                if !(r.lifecycle.complete_passive_window_show)(
                    panel.window(),
                    pointer_panel(
                        panel.as_weak(),
                        generation,
                        panel_revision,
                        r.toolbar_state.panel.input(),
                    ),
                ) {
                    r.close_panel();
                    return;
                }
                (r.lifecycle.set_popup_dismissal)(panel.window(), panel.get_menu());
                // Snapshot renders happened while hidden. Observe only the first
                // render after mapping; the queued report runs after render returns.
                let requested_at = r.toolbar_state.panel_preparation.timing.as_ref()
                    .map(|timing| timing.requested_at());
                let rendered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let _ = panel.window().set_rendering_notifier(move |state, _| {
                    if matches!(state, slint::RenderingState::AfterRendering)
                        && !rendered.swap(true, std::sync::atomic::Ordering::Relaxed)
                        && let Some(requested_at) = requested_at
                    {
                        let _ = slint::invoke_from_event_loop(move || {
                            if panel_current(generation, panel_revision) {
                                tracing::debug!(generation, revision = panel_revision,
                                    total_ms = requested_at.elapsed().as_secs_f64() * 1000.,
                                    "Annotation panel first render returned (not compositor presentation)");
                            }
                        });
                    }
                });
                let show_at = Instant::now();
                if panel.show().is_err() {
                    r.close_panel();
                    return;
                }
                if let Some(timing) = r.toolbar_state.panel_preparation.timing.take() {
                    timing.report(show_at.elapsed());
                }
                r.toolbar_state.panel_preparation.ready = true;
            }
            PassiveWindowPreparation::Pending => {
                // The backend creates registered adapters with a hidden HWND on
                // its next event-loop turn; never prime it with a visible frame.
                retry_panel_preparation(r, generation, panel_revision, attempt);
            }
            _ => r.close_panel(),
        }
    });
}

/// Collapse only presentation: tool mode, drawing drafts and document history stay live.
pub(super) fn toggle_toolbar_collapse() {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(main) = r
            .toolbar_state
            .main
            .get()
            .map(ComponentHandle::clone_strong)
        else {
            return;
        };
        r.toolbar_state.main.cancel_position();
        r.toolbar_state.toolbar_drag = None;
        r.toolbar_state.toolbar_press = None;
        if !main.get_collapsed() {
            r.pending_edit_panel = false;
            r.close_panel();
        }
        main.set_collapsed(!main.get_collapsed());
        fit_toolbar_layout(r, true);
        sync_toolbar_owner(r);
        if r.render_state
            .native_presenter
            .exclusions_changed(&capture_exclusions(r))
        {
            r.queue_render();
        }
    });
}

/// Follow the realized monitor DPI without resetting user placement during a drag.
pub(super) fn fit_toolbar_layout(r: &Registry, constrain_position: bool) {
    let Some(main) = r.toolbar_state.main.get() else {
        return;
    };
    let position = main.window().position();
    let area = (r.lifecycle.popup_work_area)(Point {
        x: position.x,
        y: position.y,
    });
    let scale = main.window().scale_factor().max(0.1);
    let (size, position) = toolbar::fit_layout(position, main.get_toolbar_width(), scale, area);
    if main.window().size() != size {
        main.window().set_size(size);
    }
    if constrain_position && main.window().position() != position {
        main.window().set_position(position);
    }
}

pub(super) fn reposition_panel() {
    let changed = REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            fit_toolbar_layout(r, false);
            if let Some(drag) = r.toolbar_state.toolbar_drag
                && let Some(main) = r.toolbar_state.main.get()
                && (r.lifecycle.cancel_toolbar_drag_on_escape)(main.window())
            {
                main.window().set_position(drag.origin);
                r.toolbar_state.toolbar_drag = None;
                r.toolbar_state.toolbar_press = None;
            }
            move_toolbar(r, None);
            sync_toolbar_owner(r);
            if r.toolbar_state.panel_preparation.ready {
                position_panel(r);
            }
            choice::close_if_context_changed(r);
            if r.render_state
                .native_presenter
                .exclusions_changed(&capture_exclusions(r))
            {
                return Some(r.generation);
            }
        }
        None
    });
    if let Some(generation) = changed {
        schedule_render(generation);
    }
}

/// Both input and the existing follow timer use this path, so a native DPI
/// change can settle the grab offset without waiting for another mouse move.
pub(super) fn move_toolbar(r: &mut Registry, client_fallback: Option<(f32, f32)>) -> bool {
    if r.toolbar_state.toolbar_drag.is_none() {
        return false;
    }
    let Some(main) = r.toolbar_state.main.get() else {
        return false;
    };
    let current = main.window().position();
    let cursor = (r.lifecycle.toolbar_cursor_position)()
        .map(|p| slint::PhysicalPosition::new(p.x, p.y))
        .or_else(|| {
            client_fallback.map(|(x, y)| {
                slint::PhysicalPosition::new(
                    current.x.saturating_add(x.round() as i32),
                    current.y.saturating_add(y.round() as i32),
                )
            })
        });
    let Some(cursor) = cursor else { return false };
    let area = (r.lifecycle.popup_work_area)(Point {
        x: cursor.x,
        y: cursor.y,
    });
    let target = r
        .toolbar_state
        .toolbar_drag
        .as_mut()
        .unwrap()
        .snapped_position(
            cursor,
            main.window().scale_factor(),
            main.window().size(),
            area,
        );
    if target == current {
        return false;
    }
    main.window().set_position(target);
    true
}

/// Keep the toolbar owned by the canvas under its physical center so activating
/// the canvas during drawing cannot place its transparent input surface above the controls.
pub(super) fn sync_toolbar_owner(r: &mut Registry) -> bool {
    let Some(main) = r.toolbar_state.main.get() else {
        return false;
    };
    let position = main.window().position();
    let size = main.window().size();
    let center = Point {
        x: position.x.saturating_add((size.width / 2) as i32),
        y: position.y.saturating_add((size.height / 2) as i32),
    };
    let owner = canvas_at(
        r.canvas_state.canvases.iter().map(|canvas| canvas.bounds),
        center,
    )
    .or(r.toolbar_state.toolbar_owner.map(|owner| owner.0))
    .or_else(|| (!r.canvas_state.canvases.is_empty()).then_some(0));
    let Some(owner) = owner else { return false };
    let identity = (
        owner,
        r.canvas_state.canvases[owner].revision(),
        r.toolbar_state.main.revision(),
    );
    if r.toolbar_state.toolbar_owner == Some(identity) {
        return true;
    }
    if (r.lifecycle.attach_tool_window)(
        main.window(),
        r.canvas_state.canvases[owner].window.window(),
    ) {
        r.toolbar_state.toolbar_owner = Some(identity);
        return true;
    }
    false
}

pub(super) fn retry_main_owner(r: &mut Registry, generation: u64, revision: u64, attempt: u8) {
    if attempt < 20 {
        r.toolbar_state
            .main
            .prepare_after(Duration::from_millis(16), move || {
                show_main(generation, revision, attempt + 1)
            });
    } else {
        tracing::warn!("could not attach annotation toolbar after preparation retries");
        r.close();
    }
}

pub(super) fn retry_panel_preparation(
    r: &mut Registry,
    generation: u64,
    revision: u64,
    attempt: u8,
) {
    if attempt < 20 {
        r.toolbar_state
            .panel
            .prepare_after(Duration::from_millis(16), move || {
                show_panel(generation, revision, attempt + 1)
            });
    } else {
        tracing::warn!(
            "annotation panel did not finish hidden native/layout preparation after retries"
        );
        r.close_panel();
    }
}

pub(super) fn canvas_at(bounds: impl IntoIterator<Item = Rect>, point: Point) -> Option<usize> {
    bounds.into_iter().position(|bounds| {
        point.x >= bounds.left
            && point.x < bounds.right
            && point.y >= bounds.top
            && point.y < bounds.bottom
    })
}

pub(super) fn position_panel(r: &Registry) -> bool {
    let (Some(main), Some(panel)) = (r.toolbar_state.main.get(), r.toolbar_state.panel.get())
    else {
        // Ownership preparation decides whether a missing main window can retry.
        return true;
    };
    let p = main.window().position();
    let m = main.window().size();
    let Some(area) = (r.lifecycle.popup_work_area)(Point { x: p.x, y: p.y }) else {
        return !panel.get_menu();
    };
    let scale = panel.window().scale_factor().max(0.1);
    let available = ((area.right - area.left) as f32 / scale).max(1.);
    let width = if panel.get_menu() {
        220f32.min(available)
    } else {
        // Wrapped preferred widths are clamped by Slint; measure the same row without wrapping.
        panel.set_numeric_measuring(true);
        let width = ((panel.get_numeric_content_width() * scale).ceil() / scale).min(available);
        panel.set_numeric_measuring(false);
        width
    };
    if !panel.get_menu() {
        let current = panel.window().size().to_logical(scale);
        panel
            .window()
            .set_size(slint::LogicalSize::new(width, current.height));
    }
    let columns = ((width - 24.) / 165.).floor().max(1.) as i32;
    panel.set_columns(columns);
    let height = if panel.get_menu() {
        46. + GROUPS[r.toolbar_state.group.unwrap_or(0)].len() as f32 * 42.
    } else {
        panel.get_numeric_content_height()
    };
    if panel.get_menu() {
        let main_rect = Rect {
            left: p.x,
            top: p.y,
            right: p.x.saturating_add(m.width as i32),
            bottom: p.y.saturating_add(m.height as i32),
        };
        let desired = toolbar::physical_size(width, height, scale);
        let Some(placed) = placement::place_annotation_popup(
            main_rect,
            (desired.width, desired.height),
            area,
            &[main_rect],
            false,
            (8. * scale).ceil() as i32,
        ) else {
            tracing::warn!("no unobstructed space for annotation tool group");
            return false;
        };
        panel
            .window()
            .set_size(slint::PhysicalSize::new(placed.width, placed.height));
        panel.window().set_position(slint::PhysicalPosition::new(
            placed.position.x,
            placed.position.y,
        ));
        return true;
    }
    let size = slint::LogicalSize::new(width, height.min((area.bottom - area.top) as f32 / scale));
    let physical = toolbar::physical_size(size.width, size.height, scale);
    if panel.window().size() != physical {
        panel.window().set_size(physical);
    }
    let target = panel_position(
        Point { x: p.x, y: p.y },
        m.height,
        physical.width,
        physical.height,
        area,
        (8. * scale) as i32,
    );
    if panel.window().position() != slint::PhysicalPosition::new(target.x, target.y) {
        panel
            .window()
            .set_position(slint::PhysicalPosition::new(target.x, target.y));
    }
    true
}

pub(super) fn panel_position(
    main: Point,
    main_height: u32,
    width: u32,
    height: u32,
    area: Rect,
    gap: i32,
) -> Point {
    placement::place_toolbar_popup(
        Rect {
            left: main.x,
            right: main.x,
            top: main.y,
            bottom: main
                .y
                .saturating_add(main_height.min(i32::MAX as u32) as i32),
        },
        width,
        height,
        Some(area),
        gap,
        placement::Side::After,
    )
}

pub(super) fn main_current(generation: u64, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        slot.borrow().as_ref().is_some_and(|r| {
            r.generation == generation
                && r.toolbar_state.main.revision() == revision
                && r.toolbar_state.main.get().is_some()
        })
    })
}

pub(super) fn later_main(
    input: &Rc<window::InputWork>,
    generation: u64,
    revision: u64,
    action: impl FnOnce() + 'static,
) {
    input.post(move || {
        if main_current(generation, revision) {
            action();
        }
    });
}

/// Native preparation can pump input while a session update owns Registry.
/// Queue only that reentrant delivery on the originating window; retirement and
/// pointer pauses remain checked when the event is replayed.
pub(super) fn registry_pointer(
    input_work: Rc<window::InputWork>,
    receive: crate::bridge::PopupPointerSink,
) -> crate::bridge::PopupPointerSink {
    input_work.pointer_sink_when_ready(
        Rc::new(|| REGISTRY.with(|slot| slot.try_borrow().is_ok())),
        receive,
    )
}

pub(super) fn pointer_main(
    weak: slint::Weak<AnnotationToolbar>,
    generation: u64,
    revision: u64,
    input_work: Rc<window::InputWork>,
) -> crate::bridge::PopupPointerSink {
    registry_pointer(
        input_work,
        Rc::new(move |input| {
            if main_current(generation, revision)
                && let Some(w) = weak.upgrade()
            {
                let moved = REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(r) = slot.as_mut() else { return false };
                    match input {
                        PopupPointerInput::LeftPressed { x, y } => {
                            r.toolbar_state.toolbar_press = Some(toolbar::Drag::new(
                                w.window().position(),
                                x,
                                y,
                                w.window().scale_factor(),
                            ));
                        }
                        PopupPointerInput::Moved { x, y } => {
                            return move_toolbar(r, Some((x, y)));
                        }
                        PopupPointerInput::LeftReleased { .. }
                        | PopupPointerInput::LeftCancelled { .. } => {
                            r.toolbar_state.toolbar_press = None;
                            r.toolbar_state.toolbar_drag = None;
                        }
                        _ => {}
                    }
                    false
                });
                if moved {
                    reposition_panel();
                }
                // Native capture keeps delivery on this toolbar. Keep Slint's
                // hover on the grip as well, even when a queued move was reported
                // relative to the toolbar's previous screen position.
                let input = REGISTRY.with(|slot| {
                    if let PopupPointerInput::Moved { .. } = input
                        && let Some(drag) = slot
                            .borrow()
                            .as_ref()
                            .and_then(|r| r.toolbar_state.toolbar_drag)
                    {
                        let (x, y) = drag.client_position(w.window().scale_factor());
                        PopupPointerInput::Moved { x, y }
                    } else {
                        input
                    }
                });
                dispatch(w.window(), input);
            }
        }),
    )
}
pub(super) fn panel_current(generation: u64, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        slot.borrow().as_ref().is_some_and(|r| {
            r.generation == generation
                && r.toolbar_state.panel.revision() == revision
                && r.toolbar_state.panel.get().is_some()
        })
    })
}

pub(super) fn pointer_panel(
    weak: slint::Weak<AnnotationPanel>,
    generation: u64,
    panel_revision: u64,
    input_work: Rc<window::InputWork>,
) -> crate::bridge::PopupPointerSink {
    registry_pointer(
        input_work.clone(),
        Rc::new(move |input| {
            if !panel_current(generation, panel_revision) {
                return;
            }
            if let PopupPointerInput::DismissRequested { cause } = input {
                input_work.post(move || {
                    REGISTRY.with(|s| {
                        if let Some(r) = s.borrow_mut().as_mut().filter(|r| {
                            r.generation == generation
                                && r.toolbar_state.panel.revision() == panel_revision
                                && r.toolbar_state
                                    .panel
                                    .get()
                                    .is_some_and(|panel| panel.get_menu())
                        }) {
                            if cause == PopupDismissCause::PointerOutside
                                && choice::cursor_inside(r)
                            {
                                choice::rearm_panel(r);
                                return;
                            }
                            let on_main = (r.lifecycle.toolbar_cursor_position)()
                                .zip(r.toolbar_state.main.get())
                                .is_some_and(|(cursor, main)| {
                                    let p = main.window().position();
                                    let size = main.window().size();
                                    cursor.x >= p.x
                                        && cursor.y >= p.y
                                        && cursor.x < p.x + size.width as i32
                                        && cursor.y < p.y + size.height as i32
                                });
                            let on_panel = (r.lifecycle.toolbar_cursor_position)()
                                .zip(r.toolbar_state.panel.get())
                                .is_some_and(|(cursor, panel)| {
                                    let p = panel.window().position();
                                    let size = panel.window().size();
                                    cursor.x >= p.x
                                        && cursor.y >= p.y
                                        && cursor.x < p.x + size.width as i32
                                        && cursor.y < p.y + size.height as i32
                                });
                            // Returning keyboard focus to the editor is not an outside click.
                            if cause == PopupDismissCause::PointerOutside && (on_main || on_panel) {
                                if let Some(panel) = r.toolbar_state.panel.get() {
                                    (r.lifecycle.set_popup_dismissal)(panel.window(), false);
                                    (r.lifecycle.set_popup_dismissal)(panel.window(), true);
                                }
                            } else {
                                r.close_panel();
                            }
                        }
                    })
                });
            } else if let Some(w) = weak.upgrade() {
                let origin = if matches!(input, PopupPointerInput::LeftReleased { .. }) {
                    current_focus_origin()
                } else {
                    focus::Origin::Unobserved
                };
                if let PopupPointerInput::LeftPressed { x, y } = input {
                    let scale = w.window().scale_factor().max(0.1);
                    let (x, y) = (x / scale, y / scale);
                    if x < w.get_parameter_input_x()
                        || y < w.get_parameter_input_y()
                        || x >= w.get_parameter_input_x() + w.get_parameter_input_width()
                        || y >= w.get_parameter_input_y() + w.get_parameter_input_height()
                    {
                        w.invoke_release_parameter_input();
                    }
                }
                choice::panel_pointer(input);
                dispatch(w.window(), input);
                if matches!(input, PopupPointerInput::LeftReleased { .. }) {
                    input_work.post(move || {
                        REGISTRY.with(|slot| {
                            if let Some(r) = slot.borrow().as_ref().filter(|r| {
                                r.generation == generation
                                    && r.toolbar_state.panel.revision() == panel_revision
                            }) {
                                restore_editor_focus_from(
                                    r,
                                    focus::Activation::UserRequested,
                                    origin,
                                );
                            }
                        });
                    });
                }
            }
        }),
    )
}

impl Registry {
    pub(super) fn sync_mode_display(&self) {
        if let Some(main) = self.toolbar_state.main.get() {
            main.set_operate(self.mode.is_mouse());
            main.set_active_group(self.mode.selected_group());
        }
    }

    /// Invalidate callbacks before releasing the native editor and its input bridge.
    pub(super) fn close_watermark_input(&mut self) {
        // Child hook teardown can emit native input before the owner is destroyed.
        self.toolbar_state.watermark.retire_input();
        self.focus.cancel_for(focus::Target::Watermark {
            editor: self.toolbar_state.watermark.revision(),
        });
        choice::close_watermark(self);
        self.toolbar_state.watermark.clear();
        if self
            .render_state
            .native_presenter
            .exclusions_changed(&capture_exclusions(self))
        {
            self.queue_render();
        }
    }

    pub(super) fn close_panel(&mut self) {
        // Reject reentrant owner events before dropping child windows and hooks.
        self.fonts.subscription = None;
        self.toolbar_state.panel_preparation = Default::default();
        self.toolbar_state.panel.retire_input();
        self.deferred_parameters.clear();
        self.focus.cancel();
        self.parameter_edit.set_dragging(false);
        choice::close(self, choice::CloseReason::OwnerRetired);
        self.finish_style_edit();
        if let Some(panel) = self.toolbar_state.panel.get()
            && !panel.get_menu()
        {
            let tool = panel.get_tool() as usize;
            if self.parameter_values.binding().get() == self.parameter_object(tool) {
                self.replace_values(tool, panel.get_values());
            }
        }
        self.toolbar_state.panel.clear();
        if self
            .render_state
            .native_presenter
            .exclusions_changed(&capture_exclusions(self))
        {
            self.queue_render();
        }
        self.toolbar_state.group = None;
        self.sync_mode_display();
        // Closing its menu may request focus; that request belongs to the retired panel.
        self.focus.cancel();
    }
}
