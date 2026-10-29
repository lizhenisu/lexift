//! One auxiliary browser instance. Pixel resizing and image outputs stay in App.
use super::*;
use crate::{ImageHistoryRow, ImageHistoryWindow};
use lexift_core::domain::image_history::{
    Entry, Kind, Ticket,
    workflow::{Action, Event},
};
use lexift_core::ports::screen_capture::CapturedRegion;
use slint::ComponentHandle;

pub(crate) fn emit(event: Event) {
    let handler = REGISTRY.with(|r| r.borrow().as_ref().and_then(|r| r.handler.clone()));
    if let Some(handler) = handler {
        handler(AppEvent::ImageHistory(event));
    }
}
pub(crate) fn output_current(key: lexift_core::domain::image_history::workflow::OutputKey) -> bool {
    REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|r| r.workflow.ticket(key).is_some())
    })
}
struct Registry {
    toast: crate::screen_interaction::window::Slot<crate::ScreenshotToastWindow>,
    toast_timer: slint::Timer,
    notice: Option<Ticket>,
    import_notice: Option<u64>,
    editor_feedback: bool,
    workflow: lexift_core::domain::image_history::workflow::Workflow,
    lifecycle: WindowLifecycleCallbacks,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    window: Option<ImageHistoryWindow>,
    foreground: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    area: Option<lexift_core::domain::geometry::Rect>,
    id: u64,
    entries: Vec<Entry>,
    images: HashMap<Ticket, slint::Image>,
}
thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

pub(super) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    REGISTRY.with(|r| {
        *r.borrow_mut() = Some(Registry {
            toast: Default::default(),
            toast_timer: Default::default(),
            notice: None,
            import_notice: None,
            editor_feedback: false,
            workflow: Default::default(),
            lifecycle,
            prepare,
            handler: None,
            window: None,
            foreground: None,
            area: None,
            id: 0,
            entries: vec![],
            images: HashMap::new(),
        })
    });
}
pub(super) fn set_handler(handler: Rc<dyn Fn(AppEvent)>) {
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.handler = Some(handler);
        }
    });
}
pub(crate) fn anchor() -> Option<Point> {
    REGISTRY.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        r.window
            .as_ref()
            .map(|w| {
                let p = w.window().position();
                Point { x: p.x, y: p.y }
            })
            .or_else(|| r.workflow.window.as_ref().map(|w| w.anchor))
    })
}
pub(super) fn active() -> bool {
    REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|r| r.window.is_some() || r.toast.get().is_some())
    })
}
pub(crate) fn exclusions() -> Vec<lexift_core::ports::magnifier::WindowToken> {
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let Some(r) = slot.as_ref() else {
            return vec![];
        };
        r.window
            .as_ref()
            .map(ComponentHandle::window)
            .into_iter()
            .chain(r.toast.get().map(ComponentHandle::window))
            .filter_map(|w| (r.lifecycle.annotation_window_token)(w))
            .collect()
    })
}
/// Account only for a foreground change caused synchronously by retiring this exact
/// browser. Later external activation is still rejected by the screenshot host.
fn suspend() {
    let context = REGISTRY.with(|slot| {
        let r = slot.borrow();
        let r = r.as_ref()?;
        let token = (r.lifecycle.annotation_window_token)(r.window.as_ref()?.window())?;
        let query = r.lifecycle.annotation_foreground.clone()?;
        (query() == Some(token)).then_some((token, query))
    });
    close();
    if let Some((token, query)) = context {
        crate::annotation::screenshot::retired_foreground(token, query());
    }
}
pub(super) fn close() {
    let closed = REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            let closed = r.window.is_some();
            if let Some(window) = r.window.take() {
                (r.lifecycle.set_popup_dismissal)(window.window(), false);
                let _ = window.hide();
            }
            r.entries.clear();
            r.images.clear();
            r.id = 0;
            r.foreground = None;
            return closed;
        }
        false
    });
    if closed {
        schedule_idle_memory_trim();
    }
}
fn post(id: u64, event: Event) {
    let _ = slint::invoke_from_event_loop(move || {
        let handler = REGISTRY.with(|r| {
            r.borrow()
                .as_ref()
                .filter(|r| r.id == id)
                .and_then(|r| r.handler.clone())
        });
        if let Some(handler) = handler {
            handler(AppEvent::ImageHistory(event));
        }
    });
}
fn selected(r: &Registry) -> Option<Ticket> {
    let selection = r.window.as_ref()?.get_selection();
    r.entries
        .iter()
        .find(|e| e.ticket.origin.0.to_string() == selection.as_str())
        .map(|e| e.ticket)
}
fn action(id: u64, index: i32) {
    let event = REGISTRY.with(|slot| {
        let r = slot.borrow();
        let r = r.as_ref().filter(|r| r.id == id)?;
        let ticket = selected(r)?;
        if index == 4 {
            return Some(Event::Delete { ticket });
        }
        let action = match index {
            0 => Action::Edit,
            1 => Action::Copy,
            2 => Action::Save,
            3 => Action::Pin,
            _ => return None,
        };
        Some(Event::Action {
            window: id,
            ticket,
            action,
            owner: r
                .window
                .as_ref()
                .and_then(|w| (r.lifecycle.annotation_window_token)(w.window())),
            title: crate::i18n::tr("Save image"),
        })
    });
    if let Some(event) = event {
        post(id, event);
    }
}

pub(super) fn sync(state: &AppState) {
    notification(state);
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.workflow = state.image_history.clone();
        }
    });
    let Some(view) = &state.image_history.window else {
        if matches!(
            state.screenshot.phase,
            lexift_core::domain::screenshot::ScreenshotPhase::CountingDown
                | lexift_core::domain::screenshot::ScreenshotPhase::Preparing
                | lexift_core::domain::screenshot::ScreenshotPhase::Refreshing
        ) {
            suspend();
        } else {
            close();
        }
        return;
    };
    let current = REGISTRY.with(|r| r.borrow().as_ref().is_some_and(|r| r.id == view.id));
    if !current {
        close();
        open(view);
    }
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut().filter(|r| r.id == view.id)
            && let Some(w) = &r.window
        {
            crate::theme::apply(w);
            w.set_blocked(lexift_core::domain::image_history::workflow::blocked(state));
            w.set_busy(view.busy);
            w.set_failed(view.error);
            w.set_selection(
                view.selected
                    .map_or_else(String::new, |s| s.0.to_string())
                    .into(),
            );
            r.entries = state.image_history.cache.entries().to_vec();
            render(r);
        }
    });
}

fn notification(state: &AppState) {
    let ticket = state.image_history.skipped;
    let display = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if matches!(
            state.screenshot.phase,
            lexift_core::domain::screenshot::ScreenshotPhase::CountingDown
                | lexift_core::domain::screenshot::ScreenshotPhase::Preparing
                | lexift_core::domain::screenshot::ScreenshotPhase::Refreshing
        ) {
            r.toast_timer.stop();
            r.toast.clear();
        }
        let import = state.image_import.notice;
        let import_changed = import.map(|n| n.0) != r.import_notice;
        if ticket == r.notice && !import_changed {
            return None;
        }
        r.notice = ticket;
        r.import_notice = import.map(|n| n.0);
        r.toast_timer.stop();
        r.toast.clear();
        let imported = import.filter(|_| import_changed);
        if ticket.is_none() && imported.is_none() {
            return None;
        }
        let point = imported
            .map(|n| n.1)
            .unwrap_or_else(|| (r.lifecycle.toolbar_cursor_position)().unwrap_or_default());
        let area = (r.lifecycle.popup_work_area)(point)?;
        let w = crate::screen_interaction::create(&r.lifecycle, crate::ScreenshotToastWindow::new)
            .ok()?;
        crate::theme::apply(&w);
        w.set_passive(true);
        w.set_error(imported.is_some());
        let message = imported.map_or(
            "Image exceeds the history limit and was not added",
            |(_, _, feedback)| match feedback {
                lexift_core::domain::image_import::Feedback::Failed => "Could not open image",
                lexift_core::domain::image_import::Feedback::NoImage => "No image in the clipboard",
                lexift_core::domain::image_import::Feedback::Conflict => {
                    "Finish the current editing or operation first"
                }
            },
        );
        w.set_message(crate::i18n::tr(message).into());
        w.window()
            .set_position(slint::PhysicalPosition::new(area.left, area.top));
        let revision = r.toast.open(w);
        Some((revision, area))
    });
    if let Some((revision, area)) = display {
        cancel_idle_memory_trim();
        prepare_toast(revision, area, 0);
    }
}
/// Reuse the passive, revision-guarded toast lifecycle for the live image editor.
pub(crate) fn show_editor_feedback(point: Point, outcome: lexift_core::domain::ocr::OcrOutcome) {
    show_passive_feedback(
        point,
        outcome.message(),
        outcome != lexift_core::domain::ocr::OcrOutcome::Copied,
    );
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.editor_feedback = true;
        }
    });
}
/// Shared passive notice ownership for image operations without a surviving canvas.
pub(crate) fn show_passive_feedback(point: Point, message: &str, error: bool) {
    let display = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        r.toast_timer.stop();
        r.toast.clear();
        let area = (r.lifecycle.popup_work_area)(point)?;
        let w = crate::screen_interaction::create(&r.lifecycle, crate::ScreenshotToastWindow::new)
            .ok()?;
        crate::theme::apply(&w);
        w.set_passive(true);
        w.set_error(error);
        w.set_message(crate::i18n::tr(message).into());
        w.window()
            .set_position(slint::PhysicalPosition::new(area.left, area.top));
        r.editor_feedback = false;
        Some((r.toast.open(w), area))
    });
    if let Some((revision, area)) = display {
        cancel_idle_memory_trim();
        prepare_toast(revision, area, 0);
    }
}
pub(crate) fn clear_editor_feedback() {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut()
            && r.editor_feedback
        {
            r.editor_feedback = false;
            r.toast_timer.stop();
            r.toast.clear();
        }
    });
}
fn prepare_toast(revision: u64, area: lexift_core::domain::geometry::Rect, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            let result = REGISTRY.with(|slot| {
                let mut slot = slot.borrow_mut();
                let r = slot.as_mut()?;
                if r.toast.revision() != revision {
                    return None;
                }
                let w = r.toast.get()?;
                match (r.prepare)(w.window()) {
                    PassiveWindowPreparation::Ready => {
                        for _ in 0..2 {
                            w.window().set_size(slint::LogicalSize::new(
                                w.get_content_width(),
                                w.get_content_height(),
                            ));
                            let _ = w.window().take_snapshot();
                        }
                        let size = w.window().size();
                        let dpi = w.window().scale_factor();
                        w.window().set_position(slint::PhysicalPosition::new(
                            area.left + ((area.right - area.left - size.width as i32) / 2).max(0),
                            (area.bottom - size.height as i32 - (24. * dpi) as i32).max(area.top),
                        ));
                        if !(r.lifecycle.configure_hint_window)(w.window())
                            || w.show().is_err()
                            || !(r.lifecycle.configure_hint_window)(w.window())
                        {
                            return Some(false);
                        }
                        r.toast_timer.start(
                            slint::TimerMode::SingleShot,
                            Duration::from_secs(3),
                            move || {
                                REGISTRY.with(|slot| {
                                    if let Some(r) = slot.borrow_mut().as_mut()
                                        && r.toast.revision() == revision
                                    {
                                        r.toast.clear();
                                    }
                                });
                                schedule_idle_memory_trim();
                            },
                        );
                        Some(true)
                    }
                    PassiveWindowPreparation::Pending if attempt < 20 => {
                        prepare_toast(revision, area, attempt + 1);
                        None
                    }
                    _ => Some(false),
                }
            });
            if result == Some(false) {
                REGISTRY.with(|slot| {
                    if let Some(r) = slot.borrow_mut().as_mut()
                        && r.toast.revision() == revision
                    {
                        r.toast.clear();
                    }
                });
                schedule_idle_memory_trim();
            }
        },
    );
}

fn open(view: &lexift_core::domain::image_history::workflow::Window) {
    let id = view.id;
    let created = REGISTRY.with(|slot| {
        let mut r = slot.borrow_mut();
        let r = r.as_mut()?;
        let w = crate::screen_interaction::create(&r.lifecycle, ImageHistoryWindow::new).ok()?;
        crate::theme::apply(&w);
        let area = (r.lifecycle.popup_work_area)(view.anchor)?;
        let dpi = w.window().scale_factor();
        let width = (800. * dpi)
            .min((area.right - area.left) as f32 * 0.9)
            .max(1.) as u32;
        let height = (600. * dpi)
            .min((area.bottom - area.top) as f32 * 0.9)
            .max(1.) as u32;
        w.set_limit_width((area.right - area.left) as f32 * 0.9 / dpi);
        w.set_limit_height((area.bottom - area.top) as f32 * 0.9 / dpi);
        w.window().set_position(slint::PhysicalPosition::new(
            area.left + (area.right - area.left - width as i32) / 2,
            area.top + (area.bottom - area.top - height as i32) / 2,
        ));
        w.window().set_size(slint::PhysicalSize::new(width, height));
        w.on_close_requested(move || post(id, Event::Close { window: id }));
        w.window().on_close_requested(move || {
            post(id, Event::Close { window: id });
            slint::CloseRequestResponse::KeepWindowShown
        });
        w.on_select(move |identity| {
            let ticket = REGISTRY.with(|r| {
                r.borrow().as_ref().filter(|r| r.id == id).and_then(|r| {
                    r.entries
                        .iter()
                        .find(|e| e.ticket.origin.0.to_string() == identity.as_str())
                        .map(|e| e.ticket)
                })
            });
            if let Some(ticket) = ticket {
                post(id, Event::Select { window: id, ticket });
            }
        });
        w.on_action(move |index| action(id, index));
        w.on_clear(move || post(id, Event::Clear));
        w.on_navigate(move |direction| {
            let ticket = REGISTRY.with(|slot| {
                let r = slot.borrow();
                let r = r.as_ref().filter(|r| r.id == id)?;
                let selected = selected(r)?;
                let index = r.entries.iter().position(|e| e.ticket == selected)? as i32;
                let index = (index + direction).clamp(0, r.entries.len() as i32 - 1) as usize;
                Some(r.entries[index].ticket)
            });
            if let Some(ticket) = ticket {
                post(id, Event::Select { window: id, ticket });
            }
        });
        let weak = w.as_weak();
        let begin = r.lifecycle.begin_window_drag;
        w.on_move_requested(move || {
            let weak = weak.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(w) = weak.upgrade() {
                    begin(w.window());
                }
            });
        });
        let weak = w.as_weak();
        let begin = r.lifecycle.begin_window_resize.clone();
        w.on_resize_requested(move |edge| {
            let weak = weak.clone();
            let begin = begin.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let (Some(w), Some(edge)) = (weak.upgrade(), PopupResizeEdge::from_index(edge)) {
                    begin(w.window(), edge, false);
                }
            });
        });
        r.id = id;
        r.area = Some(area);
        r.foreground = r.lifecycle.annotation_foreground.as_ref().map(|q| q());
        r.window = Some(w);
        Some(())
    });
    if created.is_some() {
        cancel_idle_memory_trim();
        prepare(id, 0);
    } else {
        slint::Timer::single_shot(Duration::ZERO, move || emit(Event::Close { window: id }));
    }
}

fn prepare(id: u64, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            let result = REGISTRY.with(|slot| {
                let r = slot.borrow();
                let r = r.as_ref().filter(|r| r.id == id)?;
                let w = r.window.as_ref()?;
                match (r.prepare)(w.window()) {
                    PassiveWindowPreparation::Ready => {
                        if let Some(area) = r.area {
                            let dpi = w.window().scale_factor();
                            let width = (800. * dpi)
                                .min((area.right - area.left) as f32 * 0.9)
                                .max(1.) as u32;
                            let height = (600. * dpi)
                                .min((area.bottom - area.top) as f32 * 0.9)
                                .max(1.) as u32;
                            w.set_limit_width((area.right - area.left) as f32 * 0.9 / dpi);
                            w.set_limit_height((area.bottom - area.top) as f32 * 0.9 / dpi);
                            w.window().set_size(slint::PhysicalSize::new(width, height));
                            w.window().set_position(slint::PhysicalPosition::new(
                                area.left + (area.right - area.left - width as i32) / 2,
                                area.top + (area.bottom - area.top - height as i32) / 2,
                            ));
                        }
                        let color = w.global::<crate::Colors>().get_surface();
                        (r.lifecycle.configure_resize_background)(
                            w.window(),
                            [color.red(), color.green(), color.blue()],
                        );
                        let weak = w.as_weak();
                        (r.lifecycle.configure_window_paint_repair)(
                            w.window(),
                            Rc::new(move || {
                                if let Some(w) = weak.upgrade() {
                                    repair_window_paint(w.window());
                                }
                            }),
                        );
                        if !(r.lifecycle.configure_menu_window)(w.window()) || w.show().is_err() {
                            return Some(false);
                        }
                        let weak = w.as_weak();
                        let sink = Rc::new(move |input| {
                            if let Some(w) = weak.upgrade() {
                                dispatch_pointer_input(w.window(), input);
                            }
                        });
                        let success = (r.lifecycle.configure_menu_window)(w.window())
                            && (r.lifecycle.complete_passive_window_show)(w.window(), sink);
                        if success
                            && r.foreground
                                .zip(r.lifecycle.annotation_foreground.as_ref().map(|q| q()))
                                .is_some_and(|(a, b)| a.is_some() && a == b)
                        {
                            (r.lifecycle.activate_user_requested_window)(w.window());
                        }
                        Some(success)
                    }
                    PassiveWindowPreparation::Failed => Some(false),
                    _ if attempt < 20 => {
                        prepare(id, attempt + 1);
                        None
                    }
                    _ => Some(false),
                }
            });
            if result == Some(false) {
                post(id, Event::Close { window: id });
            }
        },
    );
}

pub(super) fn present(rows: Vec<(Entry, Arc<CapturedRegion>)>) {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow_mut().as_mut().filter(|r| r.window.is_some()) {
            let identities: HashSet<_> = rows.iter().map(|(e, _)| e.ticket).collect();
            r.images.retain(|ticket, _| identities.contains(ticket));
            for (entry, preview) in rows {
                r.images.entry(entry.ticket).or_insert_with(|| {
                    slint::Image::from_rgba8_premultiplied(slint::SharedPixelBuffer::<
                        slint::Rgba8Pixel,
                    >::clone_from_slice(
                        &preview.pixels,
                        (preview.bounds.right - preview.bounds.left) as u32,
                        (preview.bounds.bottom - preview.bounds.top) as u32,
                    ))
                });
            }
            render(r);
        }
    });
}
fn render(r: &Registry) {
    let Some(w) = &r.window else {
        return;
    };
    let rows: Vec<_> = r
        .entries
        .iter()
        .map(|e| ImageHistoryRow {
            identity: e.ticket.origin.0.to_string().into(),
            preview: r.images.get(&e.ticket).cloned().unwrap_or_default(),
            kind: crate::i18n::tr(match e.kind {
                Kind::Screenshot => "Screenshot",
                Kind::LongImage => "Long screenshot",
                Kind::Imported => "Imported image",
            })
            .into(),
            dimensions: format!(
                "{} × {}",
                i64::from(e.bounds.right) - i64::from(e.bounds.left),
                i64::from(e.bounds.bottom) - i64::from(e.bounds.top)
            )
            .into(),
            updated: chrono::DateTime::from_timestamp_millis(e.updated_ms as i64)
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_default()
                .into(),
        })
        .collect();
    w.set_entries(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_preview(
        selected(r)
            .and_then(|t| r.images.get(&t).cloned())
            .unwrap_or_default(),
    );
}
