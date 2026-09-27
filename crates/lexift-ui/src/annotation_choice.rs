//! Independent parameter menus: Winit's embedded PopupWindow clips at panel bounds.
use super::*;
use crate::{AnnotationChoiceRequest, AnnotationChoiceWindow};
use slint::Model;

pub(super) struct Menu {
    pub window: AnnotationChoiceWindow,
    request: AnnotationChoiceRequest,
    panel_position: slint::PhysicalPosition,
    dragging: bool,
}

pub(super) fn color_dragging(r: &Registry) -> bool {
    r.choice.as_ref().is_some_and(|menu| menu.dragging)
}

pub(super) fn close(r: &mut Registry) {
    r.choice_revision = r.choice_revision.wrapping_add(1);
    if let Some(menu) = r.choice.take() {
        (r.lifecycle.set_popup_dismissal)(menu.window.window(), false);
        let _ = menu.window.hide();
        if menu.request.field == -2 {
            r.finish_style_edit();
        }
    }
    if let Some(panel) = &r.panel {
        panel.set_active_choice(-1);
    }
}

pub(super) fn dismiss() -> bool {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.choice.is_some()) else {
            return false;
        };
        close(r);
        true
    })
}

pub(super) fn open(generation: u64, panel_revision: u64, request: AnnotationChoiceRequest) {
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
        close(r);
        if same || (request.field != -2 && request.options.row_count() == 0) {
            return;
        }
        let Some(panel) = &r.panel else { return };
        let Ok(window) = AnnotationChoiceWindow::new() else {
            return;
        };
        crate::theme::apply(&window);
        window.set_options(request.options.clone());
        window.set_lines(request.lines);
        window.set_selected(request.selected);
        window.set_color_mode(request.field == -2);
        if request.field == -2 {
            set_color_controls(
                &window,
                color::Hsv::from_rgb(current_style(r, panel.get_tool()).color),
                true,
            );
        }
        let revision = r.choice_revision;
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
        });
        later(move || show(generation, panel_revision, revision, 0));
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
        if menu.request.field != -2 {
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
            values.color_index = 8;
            values.custom_color = color::hex(hsv.rgb()).into();
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
        if let (Some(index), Some(menu), Some(panel)) = (index, &r.choice, &r.panel)
            && index >= 0
            && (index as usize) < menu.request.options.row_count()
        {
            let mut values = panel.get_values();
            set_value(&mut values, menu.request.field, index);
            panel.set_values(values);
        }
        close(r);
        rearm_panel(r);
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
        _ => {}
    }
}

fn valid(r: &Registry, generation: u64, panel_revision: u64, revision: u64) -> bool {
    r.generation == generation
        && r.panel_revision == panel_revision
        && r.choice_revision == revision
        && r.choice.is_some()
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
                let panel = r.panel.as_ref().unwrap();
                let weak = menu.window.as_weak();
                let sink = Rc::new(move |input| {
                    if input == PopupPointerInput::DismissRequested {
                        later(move || outside(generation, panel_revision, revision));
                    } else if let Some(w) = weak.upgrade() {
                        dispatch(w.window(), input);
                    }
                });
                if !(r.lifecycle.configure_menu_window)(menu.window.window())
                    || !(r.lifecycle.attach_tool_window)(menu.window.window(), panel.window())
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
                .borrow()
                .as_ref()
                .filter(|r| valid(r, generation, panel_revision, revision))
            {
                place(r);
                if remaining > 1 {
                    settle(generation, panel_revision, revision, remaining - 1);
                }
            }
        });
    });
}

fn place(r: &Registry) {
    let (Some(menu), Some(panel)) = (&r.choice, &r.panel) else {
        return;
    };
    let scale = panel.window().scale_factor().max(0.1);
    let p = panel.window().position();
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
    let minimum_width = if q.field == -2 {
        320.
    } else if q.lines {
        112.
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
                || r.panel
                    .as_ref()
                    .is_some_and(|panel| contains(panel.window(), p))
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
        .zip(r.panel.as_ref())
        .is_some_and(|(m, p)| m.panel_position != p.window().position())
    {
        close(r);
    }
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
                _ => unreachable!(),
            };
            assert_eq!(selected, index);
            assert_eq!(values.size, original.size);
            assert_eq!(values.color_index, original.color_index);
        }
    }
}
