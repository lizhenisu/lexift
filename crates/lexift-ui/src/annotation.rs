//! Owns the ephemeral annotation preview windows and per-tool UI values.
use crate::{
    AnnotationPanel, AnnotationToolbar, AnnotationValues,
    bridge::{PassiveWindowPreparation, PopupPointerInput, WindowLifecycleCallbacks},
    placement,
};
use lexift_core::domain::geometry::{Point, Rect};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc, time::Duration};

thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

struct Registry {
    main: Option<AnnotationToolbar>,
    panel: Option<AnnotationPanel>,
    selected: [i32; 7],
    values: Vec<AnnotationValues>,
    group: Option<usize>,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    timer: slint::Timer,
    generation: u64,
    status: String,
}

const GROUPS: [&[i32]; 7] = [
    &[0, 1, 2],
    &[3, 4],
    &[5, 6, 7],
    &[8, 9],
    &[10],
    &[11, 12],
    &[13],
];
const NAMES: [&str; 14] = [
    "Rectangle",
    "Ellipse",
    "Spotlight",
    "Pencil",
    "Highlighter",
    "Arrow",
    "Polyline",
    "Magnifier",
    "Text",
    "Watermark",
    "Sequence",
    "Mosaic",
    "Automatic mosaic",
    "Eraser",
];

fn fields(tool: i32) -> Vec<i32> {
    match tool {
        0 | 1 => vec![1, 4, 0, 2, 3],
        2 => vec![1, 5],
        3 => vec![0],
        4 => vec![0, 6],
        5 | 6 => vec![0, 2],
        7 => vec![1, 0, 2, 7, 8, 9, 10],
        8 => vec![11, 12, 13],
        9 => vec![11, 14, 12, 13, 5],
        10 => vec![15, 16, 13],
        11 | 12 => vec![17, 0, 18, 5],
        _ => vec![17, 0],
    }
}

fn defaults(tool: i32) -> AnnotationValues {
    AnnotationValues {
        size: if tool == 7 {
            2
        } else if matches!(tool, 3 | 4 | 11..=13) {
            12
        } else {
            4
        },
        rounding: 21,
        text_size: if tool == 10 { 16 } else { 22 },
        start: 1,
        strength: 50.,
        shape: i32::from(matches!(tool, 1 | 7)),
        mode: i32::from(tool == 4),
        erase: true,
        antialias: true,
        color_index: if tool == 4 { 6 } else { 0 },
        ..Default::default()
    }
}

pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    REGISTRY.with(|slot| {
        *slot.borrow_mut() = Some(Registry {
            main: None,
            panel: None,
            selected: [0, 4, 7, 9, 10, 11, 13],
            values: (0..14).map(defaults).collect(),
            group: None,
            prepare,
            lifecycle,
            timer: slint::Timer::default(),
            generation: 0,
            status: "Unavailable".into(),
        })
    });
}

pub(crate) fn status() -> String {
    REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| r.status.clone())
            .unwrap_or_else(|| "Unavailable".into())
    })
}
pub(crate) fn set_status(value: String) {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.status = value;
        }
    });
}
pub(crate) fn is_open() -> bool {
    REGISTRY.with(|s| s.borrow().as_ref().is_some_and(|r| r.main.is_some()))
}

fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}

impl Registry {
    fn close_panel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(panel) = self.panel.take() {
            if !panel.get_menu() {
                self.values[panel.get_tool() as usize] = panel.get_values();
            }
            (self.lifecycle.set_popup_dismissal)(panel.window(), false);
            let _ = panel.hide();
        }
        self.group = None;
        if let Some(main) = &self.main {
            main.set_active_group(-1);
        }
    }
    fn close(&mut self) {
        self.close_panel();
        self.timer.stop();
        if let Some(main) = self.main.take() {
            let _ = main.hide();
        }
        self.values = (0..14).map(defaults).collect();
        self.selected = [0, 4, 7, 9, 10, 11, 13];
    }
}

pub(crate) fn shutdown() {
    REGISTRY.with(|s| {
        if let Some(mut r) = s.borrow_mut().take() {
            r.close();
        }
    });
}
pub(crate) fn close() {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.close();
        }
    });
    crate::bridge::schedule_idle_memory_trim();
}
pub(crate) fn escape() {
    let has_panel = REGISTRY.with(|s| s.borrow().as_ref().is_some_and(|r| r.panel.is_some()));
    if has_panel {
        let target = REGISTRY.with(|s| {
            let mut slot = s.borrow_mut();
            let r = slot.as_mut()?;
            r.close_panel();
            Some((
                r.main.as_ref()?.clone_strong(),
                r.lifecycle.activate_user_requested_window,
            ))
        });
        // Escape is an explicit keyboard action; keep the next Escape in this UI session.
        if let Some((main, activate)) = target {
            activate(main.window());
        }
    } else {
        close();
    }
}

pub(crate) fn toggle() {
    if is_open() {
        close();
        return;
    }
    crate::bridge::cancel_idle_memory_trim();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Ok(main) = AnnotationToolbar::new() else {
            return;
        };
        main.on_tool_clicked(|group, menu| later(move || open_panel(group as usize, menu)));
        main.on_finish_requested(|| later(close));
        main.on_escape_requested(|| later(escape));
        main.on_drag_requested(|| {
            later(|| {
                let target = REGISTRY.with(|s| {
                    let slot = s.borrow();
                    let r = slot.as_ref()?;
                    Some((
                        r.main.as_ref()?.clone_strong(),
                        r.lifecycle.begin_window_drag,
                    ))
                });
                if let Some((main, drag)) = target {
                    drag(main.window());
                    reposition_panel();
                }
            })
        });
        main.window().on_close_requested(|| {
            later(close);
            slint::CloseRequestResponse::KeepWindowShown
        });
        main.window().set_size(slint::LogicalSize::new(590., 54.));
        r.main = Some(main);
        r.generation = r.generation.wrapping_add(1);
        let generation = r.generation;
        later(move || show_main(generation, 0));
    });
}

fn show_main(generation: u64, attempt: u8) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(main) = r.main.as_ref() else { return };
        match (r.prepare)(main.window()) {
            PassiveWindowPreparation::Ready => {
                let anchor = (r.lifecycle.toolbar_cursor_position)().unwrap_or_default();
                if let Some(area) = (r.lifecycle.popup_work_area)(anchor) {
                    let scale = main.window().scale_factor();
                    let width = (590. * scale).min((area.right - area.left) as f32).max(1.);
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
                if main.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        main.window(),
                        pointer_main(main.as_weak()),
                    )
                {
                    r.close();
                    return;
                }
                settle_main_position(
                    main.as_weak(),
                    Rc::clone(&r.lifecycle.popup_work_area),
                    anchor,
                    2,
                );
                r.timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(33),
                    reposition_panel,
                );
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = main.show();
                let _ = main.hide();
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_main(generation, attempt + 1)
                });
            }
            _ => r.close(),
        }
    });
}

/// Winit applies the destination monitor DPI after showing a native window.
/// Reconcile the initial anchor using the realized physical size; weak ownership cancels on close.
fn settle_main_position(
    weak: slint::Weak<AnnotationToolbar>,
    work_area: Rc<dyn Fn(Point) -> Option<Rect>>,
    anchor: Point,
    remaining: u8,
) {
    slint::Timer::single_shot(Duration::from_millis(32), move || {
        let Some(main) = weak.upgrade() else { return };
        if let Some(area) = work_area(anchor) {
            let size = main.window().size();
            let gap = (16. * main.window().scale_factor()) as i32;
            let point =
                placement::place_popup(anchor, size.width, size.height, area, gap, gap).position;
            main.window()
                .set_position(slint::PhysicalPosition::new(point.x, point.y));
        }
        if remaining > 0 {
            settle_main_position(weak, work_area, anchor, remaining - 1);
        }
    });
}

fn open_panel(group: usize, menu: bool) {
    if group >= GROUPS.len() {
        return;
    }
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.main.is_none() {
            return;
        }
        let same = r.group == Some(group) && r.panel.as_ref().is_some_and(|p| p.get_menu() == menu);
        r.close_panel();
        if same {
            return;
        }
        let Ok(panel) = AnnotationPanel::new() else {
            return;
        };
        let tool = r.selected[group];
        panel.set_tool(tool);
        panel.set_menu(menu);
        panel.set_heading(NAMES[tool as usize].into());
        panel.set_values(r.values[tool as usize].clone());
        panel.set_fields(ModelRc::new(VecModel::from(fields(tool))));
        panel.set_menu_tools(ModelRc::new(VecModel::from(GROUPS[group].to_vec())));
        panel.set_menu_labels(ModelRc::new(VecModel::from(
            GROUPS[group]
                .iter()
                .map(|&t| NAMES[t as usize].into())
                .collect::<Vec<_>>(),
        )));
        panel.on_selected(move |tool| {
            later(move || {
                REGISTRY.with(|s| {
                    if let Some(r) = s.borrow_mut().as_mut() {
                        r.close_panel();
                        r.selected[group] = tool;
                        if let Some(main) = &r.main {
                            main.set_tools(ModelRc::new(VecModel::from(r.selected.to_vec())));
                        }
                    }
                });
                open_panel(group, false);
            })
        });
        panel.on_escape_requested(|| later(escape));
        panel.window().on_close_requested(|| {
            later(escape);
            slint::CloseRequestResponse::KeepWindowShown
        });
        r.group = Some(group);
        r.main.as_ref().unwrap().set_active_group(group as i32);
        r.panel = Some(panel);
        let generation = r.generation;
        later(move || show_panel(generation, 0));
    });
}

fn show_panel(generation: u64, attempt: u8) {
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| r.generation == generation) else {
            return;
        };
        let Some(panel) = r.panel.as_ref() else {
            return;
        };
        match (r.prepare)(panel.window()) {
            PassiveWindowPreparation::Ready => {
                position_panel(r);
                let panel = r.panel.as_ref().unwrap();
                if let Some(main) = &r.main {
                    (r.lifecycle.attach_tool_window)(panel.window(), main.window());
                }
                if panel.show().is_err()
                    || !(r.lifecycle.complete_passive_window_show)(
                        panel.window(),
                        pointer_panel(panel.as_weak(), generation),
                    )
                {
                    r.close_panel();
                    return;
                }
                (r.lifecycle.set_popup_dismissal)(panel.window(), true);
            }
            PassiveWindowPreparation::Pending if attempt < 20 => {
                let _ = panel.show();
                let _ = panel.hide();
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    show_panel(generation, attempt + 1)
                });
            }
            _ => r.close_panel(),
        }
    });
}

fn reposition_panel() {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow().as_ref() {
            position_panel(r);
        }
    });
}

fn position_panel(r: &Registry) {
    let (Some(main), Some(panel)) = (&r.main, &r.panel) else {
        return;
    };
    let p = main.window().position();
    let m = main.window().size();
    let Some(area) = (r.lifecycle.popup_work_area)(Point { x: p.x, y: p.y }) else {
        return;
    };
    let scale = panel.window().scale_factor().max(0.1);
    let available = ((area.right - area.left) as f32 / scale).max(1.);
    let width = if panel.get_menu() {
        240f32.min(available)
    } else {
        720f32.min(available)
    };
    let columns = ((width - 24.) / 165.).floor().max(1.) as i32;
    panel.set_columns(columns);
    let height = if panel.get_menu() {
        46. + GROUPS[r.group.unwrap_or(0)].len() as f32 * 42.
    } else {
        80. + ((fields(panel.get_tool()).len() as f32 / columns as f32).ceil()) * 62.
    };
    let size = slint::LogicalSize::new(width, height.min((area.bottom - area.top) as f32 / scale));
    let physical = size.to_physical(scale);
    if panel.window().size() != physical {
        panel.window().set_size(size);
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
}

fn panel_position(
    main: Point,
    main_height: u32,
    width: u32,
    height: u32,
    area: Rect,
    gap: i32,
) -> Point {
    let below = main
        .y
        .saturating_add(main_height as i32)
        .saturating_add(gap);
    let y = if below.saturating_add(height as i32) <= area.bottom {
        below
    } else {
        main.y.saturating_sub(height as i32).saturating_sub(gap)
    };
    Point {
        x: main
            .x
            .clamp(area.left, (area.right - width as i32).max(area.left)),
        y: y.clamp(area.top, (area.bottom - height as i32).max(area.top)),
    }
}

fn dispatch(window: &slint::Window, input: PopupPointerInput) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let scale = window.scale_factor().max(0.1);
    let pos = |x, y| slint::LogicalPosition::new(x / scale, y / scale);
    let event = match input {
        PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
            position: pos(x, y),
        },
        PopupPointerInput::Exited => WindowEvent::PointerExited,
        PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
            position: pos(x, y),
            button: PointerEventButton::Left,
        },
        PopupPointerInput::LeftReleased { x, y } => WindowEvent::PointerReleased {
            position: pos(x, y),
            button: PointerEventButton::Left,
        },
        PopupPointerInput::Scrolled {
            x,
            y,
            delta_x,
            delta_y,
        } => WindowEvent::PointerScrolled {
            position: pos(x, y),
            delta_x: delta_x / scale,
            delta_y: delta_y / scale,
        },
        _ => return,
    };
    let _ = window.dispatch_event_with_result(event);
}
fn pointer_main(weak: slint::Weak<AnnotationToolbar>) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if let Some(w) = weak.upgrade() {
            dispatch(w.window(), input);
        }
    })
}
fn pointer_panel(
    weak: slint::Weak<AnnotationPanel>,
    generation: u64,
) -> crate::bridge::PopupPointerSink {
    Rc::new(move |input| {
        if input == PopupPointerInput::DismissRequested {
            later(move || {
                REGISTRY.with(|s| {
                    if let Some(r) = s
                        .borrow_mut()
                        .as_mut()
                        .filter(|r| r.generation == generation)
                    {
                        let on_main = (r.lifecycle.toolbar_cursor_position)()
                            .zip(r.main.as_ref())
                            .is_some_and(|(cursor, main)| {
                                let p = main.window().position();
                                let size = main.window().size();
                                cursor.x >= p.x
                                    && cursor.y >= p.y
                                    && cursor.x < p.x + size.width as i32
                                    && cursor.y < p.y + size.height as i32
                            });
                        if on_main {
                            if let Some(panel) = &r.panel {
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
            dispatch(w.window(), input);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panels_flip_and_clamp_on_negative_monitors() {
        let area = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(
            panel_position(Point { x: -100, y: 1000 }, 54, 720, 204, area, 8),
            Point { x: -720, y: 788 }
        );
    }
    #[test]
    fn independent_tool_defaults_match_preview() {
        assert_eq!(defaults(4).color_index, 6);
        assert_eq!(defaults(7).size, 2);
        assert_eq!(defaults(10).text_size, 16);
        assert_eq!(defaults(0).rounding, 21);
        for group in GROUPS {
            for &tool in group {
                assert!(!fields(tool).is_empty());
            }
        }
    }
}
