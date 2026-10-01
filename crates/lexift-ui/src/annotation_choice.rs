//! Independent parameter menus: Winit's embedded PopupWindow clips at panel bounds.
use super::*;
use crate::{AnnotationChoiceRequest, AnnotationChoiceWindow};
use slint::Model;

const TEXT_OUTLINE: i32 = -3;
const TEXT_BACKGROUND: i32 = -4;
const OUTLINE_COLOR: i32 = -5;
const BACKGROUND_COLOR: i32 = -6;
const WATERMARK_PRESET: i32 = -7;
const SEQUENCE_TEXT_COLOR: i32 = -8;

fn is_color(field: i32) -> bool {
    matches!(
        field,
        -2 | OUTLINE_COLOR | BACKGROUND_COLOR | SEQUENCE_TEXT_COLOR
    )
}

fn target_color(values: &AnnotationValues, palette: [[u8; 3]; 8], field: i32) -> [u8; 3] {
    match field {
        SEQUENCE_TEXT_COLOR => color::parse_hex(&values.sequence_text_color)
            .unwrap_or_else(|| palette_color(palette, values.color_index, &values.custom_color)),
        OUTLINE_COLOR => palette_color(
            palette,
            values.text_outline_color_index,
            &values.text_outline_custom_color,
        ),
        BACKGROUND_COLOR => palette_color(
            palette,
            values.text_bg_color_index,
            &values.text_bg_custom_color,
        ),
        _ => palette_color(palette, values.color_index, &values.custom_color),
    }
}

fn set_target_color(values: &mut AnnotationValues, field: i32, rgb: [u8; 3]) {
    let hex = color::hex(rgb).into();
    match field {
        SEQUENCE_TEXT_COLOR => values.sequence_text_color = hex,
        OUTLINE_COLOR => {
            values.text_outline_color_index = 8;
            values.text_outline_custom_color = hex;
        }
        BACKGROUND_COLOR => {
            values.text_bg_color_index = 8;
            values.text_bg_custom_color = hex;
        }
        _ => {
            values.color_index = 8;
            values.custom_color = hex;
        }
    }
}

fn text_style_mode(field: i32) -> i32 {
    match field {
        TEXT_OUTLINE => 1,
        TEXT_BACKGROUND => 2,
        _ => 0,
    }
}

pub(super) struct Menu {
    pub window: AnnotationChoiceWindow,
    request: AnnotationChoiceRequest,
    panel_position: slint::PhysicalPosition,
    dragging: bool,
    return_request: Option<AnnotationChoiceRequest>,
    transition_region: Option<Rect>,
    return_pending: bool,
    watermark_revision: Option<u64>,
}

/// Resolve the menu's actual owner, including its current editor instance.
fn owner<'a>(r: &'a Registry, menu: &Menu) -> Option<&'a slint::Window> {
    if let Some(revision) = menu.watermark_revision {
        r.watermark_input
            .as_ref()
            .filter(|_| r.watermark_input_revision == revision)
            .map(ComponentHandle::window)
    } else {
        r.panel.as_ref().map(ComponentHandle::window)
    }
}

pub(super) fn color_dragging(r: &Registry) -> bool {
    r.choice.as_ref().is_some_and(|menu| menu.dragging)
}

pub(super) fn close(r: &mut Registry) {
    r.choice_revision = r.choice_revision.wrapping_add(1);
    if let Some(menu) = r.choice.take() {
        if menu.watermark_revision == Some(r.watermark_input_revision)
            && let Some(editor) = &r.watermark_input
        {
            editor.set_presets_open(false);
        }
        (r.lifecycle.set_popup_dismissal)(menu.window.window(), false);
        let _ = menu.window.hide();
        if is_color(menu.request.field) || text_style_mode(menu.request.field) != 0 {
            r.finish_style_edit();
        }
        if let Some(panel) = &r.panel {
            panel.invoke_release_parameter_input();
        }
        restore_text_focus(r);
    }
    if let Some(panel) = &r.panel {
        panel.set_active_choice(-1);
    }
}

pub(super) fn dismiss() -> bool {
    let current = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|r| r.choice.is_some())
            .map(|r| (r.generation, r.panel_revision, r.choice_revision))
    });
    if let Some((generation, panel_revision, revision)) = current {
        finish(generation, panel_revision, revision, None);
        true
    } else {
        false
    }
}

pub(super) fn open(generation: u64, panel_revision: u64, request: AnnotationChoiceRequest) {
    open_menu(generation, panel_revision, request, None);
}

pub(super) fn open_watermark(
    generation: u64,
    watermark_revision: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.watermark_input_revision == watermark_revision
                && r.watermark_input.is_some()
        }) else {
            return;
        };
        let same = r
            .choice
            .as_ref()
            .is_some_and(|m| m.watermark_revision == Some(watermark_revision));
        close(r);
        if same {
            return;
        }
        let editor = r.watermark_input.as_ref().unwrap();
        let Ok(window) = AnnotationChoiceWindow::new() else {
            return;
        };
        crate::theme::apply(&window);
        let request = AnnotationChoiceRequest {
            field: WATERMARK_PRESET,
            options: editor.get_preset_labels(),
            selected: editor.get_selected_preset(),
            x,
            y,
            width,
            height,
            lines: false,
            connectors: false,
        };
        window.set_options(request.options.clone());
        window.set_selected(request.selected);
        let (panel_revision, revision) = (r.panel_revision, r.choice_revision);
        window.on_chosen(move |index| {
            later(move || finish(generation, panel_revision, revision, Some(index)))
        });
        window.on_cancel(move || later(move || finish(generation, panel_revision, revision, None)));
        window.window().on_close_requested(move || {
            later(move || finish(generation, panel_revision, revision, None));
            slint::CloseRequestResponse::KeepWindowShown
        });
        editor.set_presets_open(true);
        r.choice = Some(Menu {
            window,
            request,
            panel_position: editor.window().position(),
            dragging: false,
            return_request: None,
            transition_region: None,
            return_pending: false,
            watermark_revision: Some(watermark_revision),
        });
        later(move || show(generation, panel_revision, revision, 0));
    });
}

pub(super) fn close_watermark(r: &mut Registry) {
    if r.choice
        .as_ref()
        .is_some_and(|menu| menu.watermark_revision.is_some())
    {
        close(r);
    }
}

pub(super) fn dismiss_watermark(generation: u64, watermark_revision: u64) {
    let current = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|r| {
                r.generation == generation && r.watermark_input_revision == watermark_revision
            })
            .and_then(|r| {
                r.choice
                    .as_ref()
                    .filter(|m| m.watermark_revision == Some(watermark_revision))
                    .map(|_| (r.panel_revision, r.choice_revision))
            })
    });
    if let Some((panel_revision, revision)) = current {
        finish(generation, panel_revision, revision, None);
    }
}

fn open_menu(
    generation: u64,
    panel_revision: u64,
    request: AnnotationChoiceRequest,
    return_request: Option<AnnotationChoiceRequest>,
) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.panel_revision == panel_revision)
        else {
            return;
        };
        let same = r
            .choice
            .as_ref()
            .is_some_and(|m| m.request.field == request.field);
        // The triggering click can still have a queued dismissal from the old window.
        // Keep its bounds until the new popup has settled, without keeping the window alive.
        let transition_region = r
            .choice
            .as_ref()
            .filter(|menu| {
                return_request.is_some()
                    || (is_color(menu.request.field) && text_style_mode(request.field) != 0)
            })
            .map(|menu| {
                let p = menu.window.window().position();
                let size = menu.window.window().size();
                Rect {
                    left: p.x,
                    top: p.y,
                    right: p.x + size.width as i32,
                    bottom: p.y + size.height as i32,
                }
            });
        close(r);
        let mode = text_style_mode(request.field);
        if same || (!is_color(request.field) && mode == 0 && request.options.row_count() == 0) {
            return;
        }
        let Some(panel) = &r.panel else { return };
        let values = panel.get_values();
        if mode != 0
            && (!matches!(panel.get_tool(), 8 | 10)
                || (mode == 1 && !values.text_outline)
                || (mode == 2 && !values.text_background))
        {
            return;
        }
        let Ok(window) = AnnotationChoiceWindow::new() else {
            return;
        };
        crate::theme::apply(&window);
        window.set_options(request.options.clone());
        window.set_sequence_shapes(request.field == 23);
        window.set_lines(request.lines);
        window.set_connectors(request.connectors);
        window.set_disabled_index(if panel.get_tool() == 4 && request.field == 6 {
            1
        } else {
            -1
        });
        window.set_selected(request.selected);
        window.set_color_mode(is_color(request.field));
        window.set_font_mode(request.field == 12);
        window.set_style_values(values.clone());
        window.set_text_style_mode(mode);
        if is_color(request.field) {
            set_color_controls(
                &window,
                color::Hsv::from_rgb(target_color(
                    &values,
                    palette(panel.get_tool() as usize),
                    request.field,
                )),
                true,
            );
        }
        let revision = r.choice_revision;
        window.on_custom_requested(move |x, y, width, height| {
            later(move || {
                open_style_color(generation, panel_revision, revision, x, y, width, height)
            });
        });
        window.on_text_style_edited(move |values| {
            later(move || edit_text_style(generation, panel_revision, revision, values))
        });
        window.on_chosen(move |index| {
            later(move || finish(generation, panel_revision, revision, Some(index)))
        });
        window.on_cancel(move || later(move || finish(generation, panel_revision, revision, None)));
        window.window().on_close_requested(move || {
            later(move || finish(generation, panel_revision, revision, None));
            slint::CloseRequestResponse::KeepWindowShown
        });
        window.on_hsv_edited(move |hue, saturation, value| {
            later(move || {
                edit_color(
                    generation,
                    panel_revision,
                    revision,
                    ColorEdit::Hsv(color::Hsv {
                        hue,
                        saturation,
                        value,
                    }),
                )
            });
        });
        window.on_hex_edited(move |text, commit| {
            later(move || {
                edit_color(
                    generation,
                    panel_revision,
                    revision,
                    ColorEdit::Hex(text.to_string(), commit),
                )
            });
        });
        window.on_color_dragging(move |active| {
            later(move || set_dragging(generation, panel_revision, revision, active));
        });
        panel.set_active_choice(request.field);
        r.choice = Some(Menu {
            window,
            request,
            panel_position: panel.window().position(),
            dragging: false,
            return_request,
            transition_region,
            return_pending: false,
            watermark_revision: None,
        });
        later(move || show(generation, panel_revision, revision, 0));
    });
}

/// Convert the style popup's local button anchor into the toolbar's logical coordinates.
fn open_style_color(
    generation: u64,
    panel_revision: u64,
    revision: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot
            .as_ref()
            .filter(|r| valid(r, generation, panel_revision, revision))?;
        let menu = r.choice.as_ref()?;
        let field = match menu.request.field {
            TEXT_OUTLINE => OUTLINE_COLOR,
            TEXT_BACKGROUND => BACKGROUND_COLOR,
            _ => return None,
        };
        let panel = r.panel.as_ref()?;
        let scale = panel.window().scale_factor().max(0.1);
        let popup_scale = menu.window.window().scale_factor().max(0.1);
        let p = menu.window.window().position();
        let origin = panel.window().position();
        let mut request = menu.request.clone();
        request.field = field;
        request.x = (p.x - origin.x) as f32 / scale + x * popup_scale / scale;
        request.y = (p.y - origin.y) as f32 / scale + y * popup_scale / scale;
        request.width = width * popup_scale / scale;
        request.height = height * popup_scale / scale;
        Some((request, menu.request.clone()))
    });
    if let Some((request, parent)) = request {
        open_menu(generation, panel_revision, request, Some(parent));
    }
}

/// Merge only this popup's parameters; its snapshot must never overwrite another edit.
fn merge_text_style(values: &mut AnnotationValues, source: &AnnotationValues, mode: i32) {
    if mode == 1 {
        values.text_outline_width = source.text_outline_width;
        values.text_outline_color_index = source.text_outline_color_index;
        values.text_outline_custom_color = source.text_outline_custom_color.clone();
    } else if mode == 2 {
        values.text_bg_opacity = source.text_bg_opacity;
        values.text_bg_rounding = source.text_bg_rounding;
        values.text_bg_padding = source.text_bg_padding;
        values.text_bg_color_index = source.text_bg_color_index;
        values.text_bg_custom_color = source.text_bg_custom_color.clone();
    }
}

fn edit_text_style(generation: u64, panel_revision: u64, revision: u64, source: AnnotationValues) {
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let Some(r) = slot
            .as_ref()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        let mode = text_style_mode(r.choice.as_ref().unwrap().request.field);
        if mode != 0
            && let Some(panel) = &r.panel
        {
            let mut values = panel.get_values();
            merge_text_style(&mut values, &source, mode);
            panel.set_values(values);
        }
    });
}

enum ColorEdit {
    Hsv(color::Hsv),
    Hex(String, bool),
}

fn set_color_controls(window: &AnnotationChoiceWindow, hsv: color::Hsv, update_text: bool) {
    let rgb = hsv.rgb();
    window.set_hue(hsv.hue);
    window.set_saturation(hsv.saturation);
    window.set_value(hsv.value);
    window.set_preview(slint::Color::from_rgb_u8(rgb[0], rgb[1], rgb[2]));
    let bright = color::Hsv { value: 1., ..hsv }.rgb();
    window.set_bright(slint::Color::from_rgb_u8(bright[0], bright[1], bright[2]));
    if update_text {
        window.set_hex_text(color::hex(rgb).into());
    }
    window.set_invalid(false);
}

fn edit_color(generation: u64, panel_revision: u64, revision: u64, edit: ColorEdit) {
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let Some(r) = slot
            .as_ref()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        let menu = r.choice.as_ref().unwrap();
        if !is_color(menu.request.field) {
            return;
        }
        let (hsv, normalize) = match edit {
            ColorEdit::Hsv(hsv) => (hsv, true),
            ColorEdit::Hex(text, commit) => {
                let Some(rgb) = color::parse_hex(&text) else {
                    menu.window.set_invalid(commit);
                    return;
                };
                (color::Hsv::from_rgb(rgb), commit)
            }
        };
        set_color_controls(&menu.window, hsv, normalize);
        if let Some(panel) = &r.panel {
            let mut values = panel.get_values();
            set_target_color(&mut values, menu.request.field, hsv.rgb());
            panel.set_values(values);
        }
    });
}

fn set_dragging(generation: u64, panel_revision: u64, revision: u64, active: bool) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        if active {
            r.finish_style_edit();
        }
        r.choice.as_mut().unwrap().dragging = active;
    });
    if !active {
        // Values-changed posts its update; finish after that update has reached the session.
        later(move || {
            later(move || {
                REGISTRY.with(|slot| {
                    if let Some(r) = slot.borrow_mut().as_mut().filter(|r| {
                        valid(r, generation, panel_revision, revision) && !color_dragging(r)
                    }) {
                        r.finish_style_edit();
                    }
                })
            })
        });
    }
}

fn finish(generation: u64, panel_revision: u64, revision: u64, index: Option<i32>) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        if let Some(watermark_revision) = r.choice.as_ref().and_then(|menu| menu.watermark_revision)
        {
            if let Some(index) = index.filter(|i| {
                *i >= 0 && (*i as usize) < r.choice.as_ref().unwrap().request.options.row_count()
            }) {
                r.watermark_input
                    .as_ref()
                    .unwrap()
                    .invoke_choose_preset(index);
            }
            // Native and Slint routing can both observe the same Escape.
            r.choice.as_mut().unwrap().return_pending = true;
            next_frame(move || {
                REGISTRY.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    if let Some(r) = slot.as_mut().filter(|r| {
                        r.generation == generation
                            && r.choice_revision == revision
                            && r.watermark_input_revision == watermark_revision
                    }) {
                        close(r);
                        if let Some(editor) = &r.watermark_input {
                            editor.invoke_focus_input();
                        }
                    }
                });
            });
            return;
        }
        if let (Some(index), Some(menu), Some(panel)) = (index, &r.choice, &r.panel)
            && index >= 0
            && (index as usize) < menu.request.options.row_count()
        {
            if panel.get_tool() == 4 && menu.request.field == 6 && index == 1 {
                return;
            }
            let mut values = panel.get_values();
            set_value(&mut values, menu.request.field, index);
            panel.set_values(values);
        }
        let parent = r
            .choice
            .as_ref()
            .and_then(|menu| menu.return_request.clone());
        if parent.is_some()
            || r.choice
                .as_ref()
                .is_some_and(|menu| is_color(menu.request.field))
        {
            // Native routing and the focused Slint window can both observe Escape.
            // Keep this instance as a guard until the next frame: one key closes one level.
            r.choice.as_mut().unwrap().return_pending = true;
            next_frame(move || {
                let valid_return = REGISTRY.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|r| {
                        r.generation == generation
                            && r.panel_revision == panel_revision
                            && r.choice_revision == revision
                            && r.choice.as_ref().is_some_and(|menu| menu.return_pending)
                    })
                });
                if valid_return {
                    if let Some(parent) = parent {
                        open(generation, panel_revision, parent);
                    } else {
                        REGISTRY.with(|slot| {
                            if let Some(r) = slot.borrow_mut().as_mut() {
                                close(r);
                                rearm_panel(r);
                            }
                        });
                    }
                }
            });
        } else {
            close(r);
            rearm_panel(r);
        }
    });
}

fn set_value(values: &mut AnnotationValues, field: i32, index: i32) {
    match field {
        1 | 17 => values.shape = index,
        2 => values.style = index,
        6 | 14 | 18 => values.mode = index,
        7 => values.zoom = index,
        12 => values.font = index,
        16 => values.format = index,
        19 => values.line_start = index,
        20 => values.line_end = index,
        21 => values.connector_style = index,
        23 => values.shape = index,
        24 => values.sequence_ratio = index,
        _ => {}
    }
}

fn valid(r: &Registry, generation: u64, panel_revision: u64, revision: u64) -> bool {
    r.generation == generation
        && r.panel_revision == panel_revision
        && r.choice_revision == revision
        && r.choice
            .as_ref()
            .is_some_and(|menu| !menu.return_pending && owner(r, menu).is_some())
}

fn show(generation: u64, panel_revision: u64, revision: u64, attempt: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        let menu = r.choice.as_ref().unwrap();
        match (r.prepare)(menu.window.window()) {
            PassiveWindowPreparation::Ready => {
                place(r);
                let menu = r.choice.as_ref().unwrap();
                let Some(parent) = owner(r, menu) else { return };
                let weak = menu.window.as_weak();
                let sink = Rc::new(move |input| {
                    if input == PopupPointerInput::DismissRequested {
                        later(move || outside(generation, panel_revision, revision));
                    } else if let Some(w) = weak.upgrade() {
                        dispatch(w.window(), input);
                    }
                });
                if !(r.lifecycle.configure_menu_window)(menu.window.window())
                    || !(r.lifecycle.attach_tool_window)(menu.window.window(), parent)
                    || menu.window.show().is_err()
                    || !(r.lifecycle.configure_menu_window)(menu.window.window())
                    || !(r.lifecycle.complete_passive_window_show)(menu.window.window(), sink)
                    || !(r.lifecycle.set_popup_dismissal)(menu.window.window(), true)
                {
                    tracing::warn!("could not show annotation parameter menu");
                    close(r);
                } else {
                    place(r);
                    settle(generation, panel_revision, revision, 2);
                }
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show(generation, panel_revision, revision, attempt + 1)
                });
            }
            _ => close(r),
        }
    });
}

// Moving the native menu to a different-DPI monitor updates its scale asynchronously.
fn settle(generation: u64, panel_revision: u64, revision: u64, remaining: u8) {
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        REGISTRY.with(|slot| {
            if let Some(r) = slot
                .borrow_mut()
                .as_mut()
                .filter(|r| valid(r, generation, panel_revision, revision))
            {
                place(r);
                if remaining == 1 {
                    r.choice.as_mut().unwrap().transition_region = None;
                }
                if remaining > 1 {
                    settle(generation, panel_revision, revision, remaining - 1);
                }
            }
        });
    });
}

fn place(r: &Registry) {
    let Some(menu) = &r.choice else { return };
    let Some(parent) = owner(r, menu) else { return };
    let scale = parent.scale_factor().max(0.1);
    let p = parent.position();
    let q = &menu.request;
    let anchor = Rect {
        left: p.x + (q.x * scale).round() as i32,
        top: p.y + (q.y * scale).round() as i32,
        right: p.x + ((q.x + q.width) * scale).round() as i32,
        bottom: p.y + ((q.y + q.height) * scale).round() as i32,
    };
    let Some(area) = (r.lifecycle.popup_work_area)(Point {
        x: anchor.left,
        y: anchor.top,
    }) else {
        return;
    };
    let menu_scale = menu.window.window().scale_factor().max(0.1);
    let minimum_width = if is_color(q.field) {
        320.
    } else if q.field == TEXT_OUTLINE {
        330.
    } else if q.field == TEXT_BACKGROUND {
        360.
    } else if q.field == 12 {
        300.
    } else if q.field == 23 {
        240.
    } else if q.lines {
        112.
    } else if q.connectors {
        88.
    } else {
        180.
    };
    let width = ((q.width * scale).max(minimum_width * menu_scale).ceil() as i32)
        .min(area.right - area.left);
    let height =
        ((menu.window.get_menu_height() * menu_scale).ceil() as i32).min(area.bottom - area.top);
    let position = menu_position(anchor, width, height, area, (3. * menu_scale).ceil() as i32);
    menu.window
        .window()
        .set_size(slint::PhysicalSize::new(width as u32, height as u32));
    menu.window
        .window()
        .set_position(slint::PhysicalPosition::new(position.x, position.y));
}

/// Use physical coordinates throughout, including negative monitor origins.
fn menu_position(anchor: Rect, width: i32, height: i32, area: Rect, gap: i32) -> Point {
    let below = anchor.bottom + gap;
    let y = if below + height <= area.bottom {
        below
    } else {
        anchor.top - gap - height
    };
    Point {
        x: anchor
            .left
            .clamp(area.left, (area.right - width).max(area.left)),
        y: y.clamp(area.top, (area.bottom - height).max(area.top)),
    }
}

fn contains(window: &slint::Window, p: Point) -> bool {
    let origin = window.position();
    let size = window.size();
    p.x >= origin.x
        && p.y >= origin.y
        && p.x < origin.x + size.width as i32
        && p.y < origin.y + size.height as i32
}

pub(super) fn cursor_inside(r: &Registry) -> bool {
    r.choice
        .as_ref()
        .zip((r.lifecycle.toolbar_cursor_position)())
        .is_some_and(|(menu, p)| {
            contains(menu.window.window(), p)
                || menu.transition_region.is_some_and(|area| {
                    p.x >= area.left && p.x < area.right && p.y >= area.top && p.y < area.bottom
                })
                || r.panel
                    .as_ref()
                    .is_some_and(|panel| contains(panel.window(), p))
                || (menu.watermark_revision.is_some()
                    && owner(r, menu).is_some_and(|parent| contains(parent, p)))
        })
}

pub(super) fn rearm_panel(r: &Registry) {
    if let Some(panel) = &r.panel {
        (r.lifecycle.set_popup_dismissal)(panel.window(), false);
        (r.lifecycle.set_popup_dismissal)(panel.window(), true);
    }
}

fn outside(generation: u64, panel_revision: u64, revision: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        // A click on the trigger must reach its toggle callback before dismissal.
        let in_panel = r
            .panel
            .as_ref()
            .zip((r.lifecycle.toolbar_cursor_position)())
            .is_some_and(|(p, cursor)| contains(p.window(), cursor));
        if in_panel || cursor_inside(r) {
            if let Some(menu) = &r.choice {
                (r.lifecycle.set_popup_dismissal)(menu.window.window(), false);
                (r.lifecycle.set_popup_dismissal)(menu.window.window(), true);
            }
        } else {
            close(r);
        }
    });
}

pub(super) fn close_if_moved(r: &mut Registry) {
    if r.choice
        .as_ref()
        .is_some_and(|m| owner(r, m).is_none_or(|p| m.panel_position != p.position()))
    {
        close(r);
    }
}

pub(super) fn watermark_pointer(input: PopupPointerInput) {
    let PopupPointerInput::LeftPressed { x, y } = input else {
        return;
    };
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(menu) = r.choice.as_ref().filter(|m| m.watermark_revision.is_some()) else {
            return;
        };
        let Some(parent) = owner(r, menu) else { return };
        let scale = parent.scale_factor().max(0.1);
        let (x, y) = (x / scale, y / scale);
        let q = &menu.request;
        if x < q.x || x >= q.x + q.width || y < q.y || y >= q.y + q.height {
            close(r);
        }
    });
}

pub(super) fn panel_pointer(input: PopupPointerInput) {
    let PopupPointerInput::LeftPressed { x, y } = input else {
        return;
    };
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(menu) = &r.choice else { return };
        let scale = r
            .panel
            .as_ref()
            .map_or(1., |p| p.window().scale_factor().max(0.1));
        let (x, y) = (x / scale, y / scale);
        let q = &menu.request;
        if x < q.x || x >= q.x + q.width || y < q.y || y >= q.y + q.height {
            close(r);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_colors_round_trip_and_update_only_their_target() {
        let original = defaults(8);
        let rgb = [37, 83, 149];
        let (index, custom) = color_to_values(palette(8), rgb);
        assert_eq!(index, 8);
        assert_eq!(palette_color(palette(8), index, &custom), rgb);
        for field in [-2, OUTLINE_COLOR, BACKGROUND_COLOR] {
            let mut values = original.clone();
            set_target_color(&mut values, field, rgb);
            assert_eq!(target_color(&values, palette(8), field), rgb);
            let mut restored = original.clone();
            if field == OUTLINE_COLOR {
                restored.text_outline_color_index = 8;
                restored.text_outline_custom_color = custom.clone();
            } else if field == BACKGROUND_COLOR {
                restored.text_bg_color_index = 8;
                restored.text_bg_custom_color = custom.clone();
            } else {
                restored.color_index = 8;
                restored.custom_color = custom.clone();
            }
            assert_eq!(values, restored);
        }
    }

    #[test]
    fn text_color_buttons_and_centered_popup_custom_button_are_hittable_at_both_scales() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        for dpi in [1., 1.25] {
            let click = |window: &slint::Window, x: f32, y: f32| {
                dispatch(
                    window,
                    PopupPointerInput::Moved {
                        x: x * dpi,
                        y: y * dpi,
                    },
                );
                dispatch(
                    window,
                    PopupPointerInput::LeftPressed {
                        x: x * dpi,
                        y: y * dpi,
                    },
                );
                dispatch(
                    window,
                    PopupPointerInput::LeftReleased {
                        x: x * dpi,
                        y: y * dpi,
                    },
                );
            };
            for (tool, width, height, x, y) in [
                (8, 780., 58., 748., 29.),
                (8, 640., 106., 282., 72.),
                (9, 1000., 58., 962., 29.),
                (9, 640., 154., 282., 120.),
            ] {
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(tool);
                panel.set_values(defaults(tool));
                panel
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                panel
                    .window()
                    .set_size(slint::LogicalSize::new(width, height));
                panel.show().unwrap();
                let requested = Rc::new(Cell::new(0));
                let result = requested.clone();
                panel.on_choice_requested(move |request| result.set(request.field));
                click(panel.window(), x, y);
                assert_eq!(requested.get(), -2, "tool {tool}, width {width}");
            }
            for (mode, width, height, y) in [(1, 330., 80., 57.5), (2, 360., 160., 135.5)] {
                let window = AnnotationChoiceWindow::new().unwrap();
                window.set_text_style_mode(mode);
                window.set_style_values(defaults(8));
                window
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                window
                    .window()
                    .set_size(slint::LogicalSize::new(width, height));
                window.show().unwrap();
                let anchor = Rc::new(Cell::new(None));
                let result = anchor.clone();
                window.on_custom_requested(move |x, y, w, h| result.set(Some((x, y, w, h))));
                click(window.window(), (width - 283.) / 2. + 256. + 13.5, y);
                let (x, _, w, _) = anchor.get().expect("custom button was clipped");
                let realized_width = window.window().size().width as f32 / dpi;
                assert!(
                    (realized_width - x - w - (x - 256.)).abs() < 0.01,
                    "palette margins differ"
                );
            }
        }
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
            ),
        );
        let (generation, panel_revision) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let panel = AnnotationPanel::new().unwrap();
            panel.set_tool(8);
            let mut values = defaults(8);
            values.text_background = true;
            panel.set_values(values);
            r.panel = Some(panel);
            (r.generation, r.panel_revision)
        });
        open(
            generation,
            panel_revision,
            AnnotationChoiceRequest {
                field: TEXT_BACKGROUND,
                ..Default::default()
            },
        );
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().choice_revision);
        open_style_color(generation, panel_revision, revision, 250., 122., 27., 27.);
        let child_revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().choice_revision);
        finish(generation, panel_revision, child_revision, None);
        assert!(dismiss()); // Duplicate native Escape must not close the pending parent.
        std::thread::sleep(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(r.choice.as_ref().unwrap().request.field, TEXT_BACKGROUND);
            assert!(r.panel.is_some());
            assert!(r.panel.as_ref().unwrap().get_values().text_background);
        });
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().choice_revision);
        open_style_color(generation, panel_revision, revision, 250., 122., 27., 27.);
        let child_revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().choice_revision);
        finish(generation, panel_revision, child_revision, None);
        REGISTRY.with(|slot| close(slot.borrow_mut().as_mut().unwrap()));
        std::thread::sleep(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(
                slot.as_ref().unwrap().choice.is_none(),
                "closed chain was resurrected"
            );
            slot.take();
        });
    }

    #[test]
    fn text_style_popup_merges_only_its_own_parameters() {
        let original = defaults(8);
        let mut source = defaults(8);
        source.text_size = 900;
        source.font = 17;
        source.text = "stale content".into();
        source.text_outline = !original.text_outline;
        source.text_background = !original.text_background;
        source.text_outline_width = 9;
        source.text_outline_color_index = 3;
        source.text_outline_custom_color = "#256395".into();
        source.text_bg_opacity = 25;
        source.text_bg_padding = 13;
        source.text_bg_rounding = 19;
        source.text_bg_color_index = 5;
        source.text_bg_custom_color = "#731458".into();
        for mode in [0, 1, 2] {
            let mut values = original.clone();
            let mut expected = original.clone();
            if mode == 1 {
                expected.text_outline_width = 9;
                expected.text_outline_color_index = 3;
                expected.text_outline_custom_color = "#256395".into();
            } else if mode == 2 {
                expected.text_bg_opacity = 25;
                expected.text_bg_padding = 13;
                expected.text_bg_rounding = 19;
                expected.text_bg_color_index = 5;
                expected.text_bg_custom_color = "#731458".into();
            }
            merge_text_style(&mut values, &source, mode);
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn text_style_windows_fit_near_negative_screen_edges_at_both_dpi_scales() {
        let area = Rect {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        let anchor = Rect {
            left: -60,
            top: 800,
            right: -20,
            bottom: 840,
        };
        for dpi in [1., 1.25] {
            for (width, height) in [(330., 80.), (360., 160.)] {
                let (w, h) = ((width * dpi) as i32, (height * dpi) as i32);
                let p = menu_position(anchor, w, h, area, (3. * dpi) as i32);
                assert!(p.y + h < anchor.top);
                assert!(p.x >= area.left && p.x + w <= area.right);
                assert!(p.y >= area.top && p.y + h <= area.bottom);
            }
        }
    }
    #[test]
    fn placement_flips_and_clamps_at_work_area_edges() {
        let area = Rect {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        let anchor = Rect {
            left: -100,
            top: 700,
            right: 0,
            bottom: 730,
        };
        assert_eq!(
            menu_position(anchor, 140, 230, area, 4),
            Point { x: -140, y: 466 }
        );
        let anchor = Rect {
            left: -1800,
            top: 0,
            right: -1660,
            bottom: 40,
        };
        assert_eq!(
            menu_position(anchor, 140, 230, area, 4),
            Point { x: -1800, y: 44 }
        );
    }
    #[test]
    fn parameter_selection_updates_only_the_requested_field() {
        let original = defaults(0);
        for (field, index) in [
            (1, 1),
            (2, 4),
            (6, 1),
            (7, 3),
            (12, 2),
            (14, 1),
            (16, 2),
            (17, 1),
            (18, 1),
            (21, 3),
        ] {
            let mut values = original.clone();
            set_value(&mut values, field, index);
            let selected = match field {
                1 | 17 => values.shape,
                2 => values.style,
                6 | 14 | 18 => values.mode,
                7 => values.zoom,
                12 => values.font,
                16 => values.format,
                21 => values.connector_style,
                _ => unreachable!(),
            };
            assert_eq!(selected, index);
            assert_eq!(values.size, original.size);
            assert_eq!(values.color_index, original.color_index);
        }
    }
}
