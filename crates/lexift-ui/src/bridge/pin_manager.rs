//! Transient previews and native feedback for the Core-owned pin manager.
use super::*;
use crate::{PinLocateOutline, PinManagerRow, PinManagerWindow};
use lexift_core::{
    domain::pinned_image::manager::{self, Action, Command, Event, PinId, Request},
    ports::screen_capture::CapturedRegion,
};
use slint::ComponentHandle;
use std::{collections::BTreeMap, sync::Weak};

type Handler = Rc<dyn Fn(AppEvent)>;
type Preparation = fn(&slint::Window) -> PassiveWindowPreparation;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Row {
    pin: PinId,
    live: bool,
    paused: bool,
    hidden: bool,
    recover: bool,
    actions: [bool; 4],
    revision: u64,
}
struct Thumb {
    source: Weak<CapturedRegion>,
    live: bool,
    image: slint::Image,
}
struct View {
    key: manager::Window,
    window: PinManagerWindow,
    selection: Option<PinId>,
    confirmation: Option<PinId>,
    rows: Vec<Row>,
    photos: BTreeMap<PinId, Thumb>,
    outline: Option<(Request, PinLocateOutline)>,
    timer: slint::Timer,
    pending: Option<Request>,
}
impl Drop for View {
    fn drop(&mut self) {
        self.timer.stop();
        if let Some((_, outline)) = self.outline.take() {
            let _ = outline.hide();
        }
        let _ = self.window.hide();
    }
}
struct Registry {
    lifecycle: WindowLifecycleCallbacks,
    prepare: Preparation,
    handler: Option<Handler>,
    view: Option<View>,
    retired: u64,
}

thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

pub(super) fn init(prepare: Preparation, lifecycle: WindowLifecycleCallbacks) {
    REGISTRY.with(|r| {
        *r.borrow_mut() = Some(Registry {
            lifecycle,
            prepare,
            handler: None,
            view: None,
            retired: 0,
        })
    });
}
pub(super) fn set_handler(handler: Handler) {
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.handler = Some(handler);
        }
    });
}
fn emit(event: Event) {
    let handler = REGISTRY.with(|r| r.borrow().as_ref()?.handler.clone());
    if let Some(handler) = handler {
        handler(AppEvent::PinManager(event));
    }
}
pub(super) fn active() -> bool {
    REGISTRY.with(|r| r.borrow().as_ref().is_some_and(|r| r.view.is_some()))
}
pub(super) fn shutdown() {
    if let Some(mut retired) = REGISTRY.with(|r| r.borrow_mut().take()) {
        drop(retired.view.take());
    }
}
pub(super) fn apply_theme() {
    REGISTRY.with(|r| {
        if let Some(v) = r.borrow().as_ref().and_then(|r| r.view.as_ref()) {
            crate::theme::apply(&v.window);
            if let Some((_, outline)) = &v.outline {
                crate::theme::apply(outline);
            }
        }
    });
}
pub(super) fn suspend() {
    let context = REGISTRY.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        let v = r.view.as_ref()?;
        let token = (r.lifecycle.annotation_window_token)(v.window.window());
        Some((v.key.id, token, r.lifecycle.annotation_foreground.clone()))
    });
    if let Some((id, token, query)) = context {
        let owned = query
            .as_ref()
            .is_some_and(|q| token.is_some() && q() == token);
        retire(id);
        if owned && let (Some(token), Some(query)) = (token, query) {
            crate::annotation::screenshot::retired_foreground(token, query());
        }
        emit(Event::Closed(id));
    }
}
fn retire(id: u64) {
    let view = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        r.retired = r.retired.max(id);
        if r.view.as_ref().is_some_and(|v| v.key.id == id) {
            r.view.take()
        } else {
            None
        }
    });
    if view.is_some() {
        drop(view);
        schedule_idle_memory_trim();
    }
}
fn close_later(id: u64) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        retire(id);
        emit(Event::Closed(id));
    });
}
fn identity(pin: PinId) -> String {
    match pin {
        PinId::Image(id) => format!("p{id}"),
        PinId::Text(id) => format!("t{id}"),
    }
}

pub(super) fn sync(s: &AppState, language_changed: bool) {
    let Some(key) = s.pin_manager.window else {
        if active() {
            suspend();
        }
        return;
    };
    let needs_window = REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|r| r.view.is_none() && key.id > r.retired)
    });
    if needs_window {
        open(key);
    }
    let rows = s.pin_manager.order.iter().filter_map(|&pin| {
        let hidden = pin.hidden(s)?;
        let image = match pin { PinId::Image(id) => s.pinned_images.get(&id), PinId::Text(_) => None };
        let live = image.is_some_and(|p| p.live);
        let actions = [
            Action::Visibility(!hidden), Action::Locate, Action::RestoreMouse, Action::Close { confirmed_live: live },
        ].map(|action| manager::eligible(s, pin, action));
        Some(Row {
            pin,
            live,
            paused: matches!(pin, PinId::Image(id) if s.live_pin.is_some_and(|p| p.id == Some(id) && p.pause.is_some())),
            hidden,
            recover: image.is_some_and(|p| p.interaction.applied.passthrough),
            revision: image.map_or(0, |p| p.revision),
            actions,
        })
    }).collect::<Vec<_>>();
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let Some(v) = r
            .as_mut()
            .and_then(|r| r.view.as_mut())
            .filter(|v| v.key == key)
        else {
            return;
        };
        v.pending = s.pin_manager.pending;
        if rows == v.rows && !language_changed {
            return;
        }
        let previous = v
            .rows
            .iter()
            .position(|r| Some(r.pin) == v.selection)
            .unwrap_or_default();
        if !rows.iter().any(|r| Some(r.pin) == v.selection) {
            v.selection = rows
                .get(previous.min(rows.len().saturating_sub(1)))
                .map(|r| r.pin);
        }
        if !rows.iter().any(|r| r.live && Some(r.pin) == v.confirmation) {
            v.confirmation = None;
        }
        if v.outline
            .as_ref()
            .is_some_and(|(q, _)| !rows.iter().any(|r| r.pin == q.pin && !r.hidden))
        {
            clear_outline(v);
        }
        v.photos.retain(|id, _| rows.iter().any(|r| r.pin == *id));
        let model: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| project(v, i, *row))
            .collect();
        v.rows = rows;
        v.window
            .set_rows(slint::ModelRc::new(slint::VecModel::from(model)));
        sync_selection(v);
    });
}

fn project(v: &mut View, index: usize, row: Row) -> PinManagerRow {
    let (kind, kind_index, summary, preview) = match row.pin {
        PinId::Text(id) => {
            let text = super::text_pin::manager_summary(id).unwrap_or_default();
            let title = text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim();
            let mut chars = title.chars();
            let mut sample: String = chars
                .by_ref()
                .take(80)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            if chars.next().is_some() {
                sample.push('…');
            }
            ("Pinned text", 1, sample, slint::Image::default())
        }
        PinId::Image(id) => {
            let mut size = String::new();
            let mut preview = slint::Image::default();
            if let Some(image) = crate::pinned_screenshot::manager_image(id) {
                let width = image.bounds.right - image.bounds.left;
                let height = image.bounds.bottom - image.bounds.top;
                size = format!("{width} × {height} px");
                let source = Arc::downgrade(&image);
                let is_current = v.photos.get(&row.pin).is_some_and(|old| {
                    old.live == row.live && (row.live || old.source.ptr_eq(&source))
                });
                if !is_current {
                    v.photos.insert(
                        row.pin,
                        Thumb {
                            source,
                            live: row.live,
                            image: thumbnail(&image),
                        },
                    );
                }
                preview = v.photos[&row.pin].image.clone();
            }
            (
                if row.live {
                    "Window pin"
                } else {
                    "Pinned screenshot"
                },
                if row.live { 2 } else { 0 },
                size,
                preview,
            )
        }
    };
    let mut status = crate::i18n::tr(if row.hidden {
        "Pin hidden"
    } else {
        "Pin visible"
    });
    for key in [
        row.paused.then_some("Source paused"),
        row.recover.then_some("Mouse passthrough"),
    ]
    .into_iter()
    .flatten()
    {
        status.push_str(" · ");
        status.push_str(&crate::i18n::tr(key));
    }
    PinManagerRow {
        identity: identity(row.pin).into(),
        number: format!("{}", index + 1).into(),
        kind: crate::i18n::tr(kind).into(),
        kind_index,
        summary: summary.into(),
        status: status.into(),
        preview,
        hidden: row.hidden,
    }
}

/// Sample the already installed picture at bounded thumbnail dimensions.
fn thumbnail(image: &CapturedRegion) -> slint::Image {
    let Ok(w) = u32::try_from(image.bounds.right - image.bounds.left) else {
        return Default::default();
    };
    let Ok(h) = u32::try_from(image.bounds.bottom - image.bounds.top) else {
        return Default::default();
    };
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) != image.pixels.len() as u64 / 4 {
        return Default::default();
    }
    let scale = (116. / f64::from(w)).min(96. / f64::from(h)).min(1.);
    let tw = (f64::from(w) * scale).round().max(1.) as u32;
    let th = (f64::from(h) * scale).round().max(1.) as u32;
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(tw, th);
    let bytes = buffer.make_mut_bytes();
    for y in 0..th {
        for x in 0..tw {
            let source = ((u64::from(y) * u64::from(h) / u64::from(th)) * u64::from(w)
                + u64::from(x) * u64::from(w) / u64::from(tw)) as usize
                * 4;
            let target = (y * tw + x) as usize * 4;
            bytes[target..target + 4].copy_from_slice(&image.pixels[source..source + 4]);
        }
    }
    slint::Image::from_rgba8_premultiplied(buffer)
}

fn sync_selection(v: &View) {
    let index = v.rows.iter().position(|r| Some(r.pin) == v.selection);
    let row = index.map(|i| v.rows[i]);
    let w = &v.window;
    w.set_selected(index.map_or(-1, |i| i as i32));
    w.set_selected_hidden(row.is_some_and(|r| r.hidden));
    w.set_show_recovery(row.is_some_and(|r| r.recover));
    let [toggle, locate, recover, close] = row.map_or([false; 4], |r| r.actions);
    w.set_can_toggle(toggle);
    w.set_can_locate(locate);
    w.set_can_recover(recover);
    w.set_can_close(close);
    show_confirmation(w, v.confirmation.is_some());
}

fn show_confirmation(w: &PinManagerWindow, confirming: bool) {
    if w.get_confirming() != confirming {
        w.set_confirming(confirming);
        // The action button can disappear while holding Slint's keyboard focus.
        w.invoke_focus_list();
    }
}

fn choose(id: u64, identity_to_select: &str) {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let Some(v) = r
            .as_mut()
            .and_then(|r| r.view.as_mut())
            .filter(|v| v.key.id == id && v.confirmation.is_none())
        else {
            return;
        };
        v.selection = v
            .rows
            .iter()
            .find(|row| identity(row.pin) == identity_to_select)
            .map(|r| r.pin)
            .or(v.selection);
        sync_selection(v);
    });
}
fn navigate(id: u64, step: i32) {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let Some(v) = r
            .as_mut()
            .and_then(|r| r.view.as_mut())
            .filter(|v| v.key.id == id && v.confirmation.is_none() && !v.rows.is_empty())
        else {
            return;
        };
        let i = v
            .rows
            .iter()
            .position(|r| Some(r.pin) == v.selection)
            .unwrap_or_default() as i32;
        v.selection =
            Some(v.rows[(i + step.signum()).clamp(0, v.rows.len() as i32 - 1) as usize].pin);
        sync_selection(v);
    });
}

fn native_idle(id: PinId) -> bool {
    match id {
        PinId::Image(id) => crate::pinned_screenshot::manager_idle(id),
        PinId::Text(id) => super::text_pin::manager_idle(id),
    }
}
fn action(id: u64, index: i32) {
    let request = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let v = r.as_mut()?.view.as_mut().filter(|v| v.key.id == id)?;
        if index == 4 {
            v.confirmation = None;
            sync_selection(v);
            return None;
        }
        let pin = v.selection?;
        let row = *v.rows.iter().find(|r| r.pin == pin)?;
        let action = match index {
            0 if row.actions[0] => Action::Visibility(!row.hidden),
            1 if row.actions[1] => Action::Locate,
            2 if row.actions[2] => Action::RestoreMouse,
            3 if row.actions[3] && row.live => {
                v.confirmation = Some(pin);
                sync_selection(v);
                return None;
            }
            3 if row.actions[3] => Action::Close {
                confirmed_live: false,
            },
            5 if row.actions[3] && row.live && v.confirmation == Some(pin) => {
                v.confirmation = None;
                sync_selection(v);
                Action::Close {
                    confirmed_live: true,
                }
            }
            _ => return None,
        };
        Some((pin, action, v.key.anchor))
    });
    if let Some((pin, action, anchor)) = request {
        if !native_idle(pin) {
            super::image_history::show_passive_feedback(
                anchor,
                "Operation failed. Try again.",
                true,
            );
            return;
        }
        emit(Event::Requested {
            window: id,
            pin,
            action,
        });
    }
}

fn open(key: manager::Window) {
    let services = REGISTRY.with(|r| {
        let r = r.borrow();
        Some(r.as_ref()?.lifecycle.clone())
    });
    let Some(lifecycle) = services else { return };
    let Ok(w) = crate::screen_interaction::create(&lifecycle, PinManagerWindow::new) else {
        close_later(key.id);
        return;
    };
    crate::theme::apply(&w);
    crate::i18n::apply();
    w.window()
        .set_position(slint::PhysicalPosition::new(key.anchor.x, key.anchor.y));
    let id = key.id;
    w.on_select(move |text| choose(id, text.as_str()));
    w.on_navigate(move |step| navigate(id, step));
    w.on_action(move |index| action(id, index));
    w.on_close_manager(move || close_later(id));
    w.window().on_close_requested(move || {
        close_later(id);
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = w.as_weak();
    let move_window = lifecycle.begin_window_drag;
    w.on_move_requested(move || {
        if let Some(w) = weak.upgrade() {
            move_window(w.window());
        }
    });
    let weak = w.as_weak();
    let resize_window = lifecycle.begin_window_resize.clone();
    w.on_resize_requested(move |value| {
        if let (Some(w), Some(edge)) = (weak.upgrade(), PopupResizeEdge::from_index(value)) {
            resize_window(w.window(), edge, false);
        }
    });
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.view = Some(View {
                key,
                window: w,
                selection: None,
                confirmation: None,
                rows: vec![],
                photos: BTreeMap::new(),
                outline: None,
                timer: Default::default(),
                pending: None,
            });
        }
    });
    cancel_idle_memory_trim();
    let foreground = lifecycle
        .annotation_foreground
        .as_ref()
        .map(|query| query());
    prepare(key, foreground, 0);
}

fn prepare(
    key: manager::Window,
    foreground: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    attempt: u8,
) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            let context = REGISTRY.with(|r| {
                let r = r.borrow();
                let r = r.as_ref()?;
                let v = r.view.as_ref().filter(|v| v.key == key)?;
                Some((v.window.clone_strong(), r.lifecycle.clone(), r.prepare))
            });
            let Some((w, l, prep)) = context else { return };
            match prep(w.window()) {
                PassiveWindowPreparation::Ready => {
                    if let Some(area) = (l.popup_work_area)(key.anchor) {
                        let scale = (l.screenshot_displays)()
                            .into_iter()
                            .find(|(b, _)| {
                                key.anchor.x >= b.left
                                    && key.anchor.x < b.right
                                    && key.anchor.y >= b.top
                                    && key.anchor.y < b.bottom
                            })
                            .map_or_else(|| w.window().scale_factor(), |(_, d)| d)
                            .max(0.1);
                        let mw = ((area.right - area.left).max(1) as f32 * 0.9).max(1.);
                        let mh = ((area.bottom - area.top).max(1) as f32 * 0.9).max(1.);
                        let (width, height) =
                            ((640. * scale).min(mw) as u32, (480. * scale).min(mh) as u32);
                        w.set_limit_width(mw / scale);
                        w.set_limit_height(mh / scale);
                        w.window().set_position(slint::PhysicalPosition::new(
                            area.left + (area.right - area.left - width as i32) / 2,
                            area.top + (area.bottom - area.top - height as i32) / 2,
                        ));
                        w.window().set_size(slint::PhysicalSize::new(width, height));
                    }
                    let weak = w.as_weak();
                    let sink = Rc::new(move |input| {
                        if let Some(w) = weak.upgrade() {
                            dispatch_pointer_input(w.window(), input);
                        }
                    });
                    let valid = (l.configure_menu_window)(w.window())
                        && (l.exclude_view_from_capture)(w.window())
                        && w.show().is_ok()
                        && (l.complete_toolbar_show)(w.window(), sink);
                    if !valid {
                        close_later(key.id);
                        return;
                    }
                    w.invoke_focus_list();
                    if foreground
                        .zip(l.annotation_foreground.as_ref().map(|query| query()))
                        .is_some_and(|(before, now)| before.is_some() && before == now)
                    {
                        (l.activate_user_requested_window)(w.window());
                    }
                }
                PassiveWindowPreparation::Pending if attempt < 19 => {
                    prepare(key, foreground, attempt + 1)
                }
                _ => close_later(key.id),
            }
        },
    );
}

pub(super) fn execute(command: Command) {
    match command {
        Command::Retire(id) => retire(id),
        Command::Failed { window, anchor } => {
            if REGISTRY.with(|r| {
                r.borrow()
                    .as_ref()
                    .and_then(|r| r.view.as_ref())
                    .is_some_and(|v| v.key.id == window)
            }) {
                super::image_history::show_passive_feedback(
                    anchor,
                    "Operation failed. Try again.",
                    true,
                );
            }
        }
        Command::Execute(request) if current(request) => {
            if !native_idle(request.pin) {
                emit(Event::Finished {
                    request,
                    success: false,
                });
                return;
            }
            match request.action {
                Action::Locate => locate(request),
                Action::Close { .. } => {
                    match request.pin {
                        PinId::Image(id) => crate::pinned_screenshot::request_close(id),
                        PinId::Text(id) => super::text_pin::manager_close(id),
                    }
                    emit(Event::Finished {
                        request,
                        success: true,
                    });
                }
                _ => emit(Event::Finished {
                    request,
                    success: false,
                }),
            }
        }
        Command::Execute(_) => {}
    }
}

fn current(request: Request) -> bool {
    REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .and_then(|r| r.view.as_ref())
            .is_some_and(|v| v.pending == Some(request))
    })
}
fn bounds(pin: PinId) -> Option<Rect> {
    match pin {
        PinId::Image(id) => crate::pinned_screenshot::geometry(id).map(|(_, bounds)| bounds),
        PinId::Text(id) => super::text_pin::manager_bounds(id),
    }
}
fn locate(request: Request) {
    let lifecycle = REGISTRY.with(|r| r.borrow().as_ref().map(|r| r.lifecycle.clone()));
    let Some(lifecycle) = lifecycle else { return };
    let Ok(w) = crate::screen_interaction::create(&lifecycle, PinLocateOutline::new) else {
        emit(Event::Finished {
            request,
            success: false,
        });
        return;
    };
    crate::theme::apply(&w);
    if let Some(b) = bounds(request.pin) {
        w.window()
            .set_position(slint::PhysicalPosition::new(b.left, b.top));
    }
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        if let Some(v) = r.as_mut().and_then(|r| r.view.as_mut()) {
            clear_outline(v);
            v.outline = Some((request, w));
        }
    });
    prepare_outline(request, 0);
}

fn clear_outline(v: &mut View) {
    v.timer.stop();
    if let Some((_, w)) = v.outline.take() {
        let _ = w.hide();
    }
}

fn prepare_outline(request: Request, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            let context = REGISTRY.with(|r| {
                let r = r.borrow();
                let r = r.as_ref()?;
                let (key, w) = r.view.as_ref()?.outline.as_ref()?;
                (*key == request).then(|| (w.clone_strong(), r.lifecycle.clone(), r.prepare))
            });
            let Some((w, l, prep)) = context else { return };
            let ready = current(request) && native_idle(request.pin);
            let success = match (ready, prep(w.window())) {
                (true, PassiveWindowPreparation::Ready) => bounds(request.pin)
                    .and_then(|b| {
                        let work = (l.popup_work_area)(Point {
                            x: b.left.saturating_add((b.right.saturating_sub(b.left)) / 2),
                            y: b.top.saturating_add((b.bottom.saturating_sub(b.top)) / 2),
                        })?;
                        let extra = (4. * w.window().scale_factor()).round() as i32;
                        let clipped = Rect {
                            left: b.left.saturating_sub(extra).max(work.left),
                            top: b.top.saturating_sub(extra).max(work.top),
                            right: b.right.saturating_add(extra).min(work.right),
                            bottom: b.bottom.saturating_add(extra).min(work.bottom),
                        };
                        if clipped.right <= clipped.left || clipped.bottom <= clipped.top {
                            return None;
                        }
                        w.window()
                            .set_position(slint::PhysicalPosition::new(clipped.left, clipped.top));
                        w.window().set_size(slint::PhysicalSize::new(
                            (clipped.right - clipped.left) as u32,
                            (clipped.bottom - clipped.top) as u32,
                        ));
                        Some(
                            (l.configure_hint_window)(w.window())
                                && (l.exclude_view_from_capture)(w.window())
                                && w.show().is_ok(),
                        )
                    })
                    .unwrap_or(false),
                (true, PassiveWindowPreparation::Pending) if attempt < 19 => {
                    prepare_outline(request, attempt + 1);
                    return;
                }
                _ => false,
            };
            REGISTRY.with(|r| {
                let mut r = r.borrow_mut();
                let Some(v) = r.as_mut().and_then(|r| r.view.as_mut()) else {
                    return;
                };
                if !success {
                    clear_outline(v);
                    return;
                }
                v.timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(1400),
                    move || {
                        REGISTRY.with(|r| {
                            let mut r = r.borrow_mut();
                            if let Some(v) = r
                                .as_mut()
                                .and_then(|r| r.view.as_mut())
                                .filter(|v| v.outline.as_ref().is_some_and(|(k, _)| *k == request))
                            {
                                clear_outline(v);
                            }
                        });
                    },
                );
            });
            emit(Event::Finished { request, success });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Key, Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };

    struct Backend(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for Backend {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(window.clone());
            Ok(window)
        }
    }

    #[test]
    fn bounded_thumbnail_keeps_aspect_and_accepts_tiny_images() {
        let mut image = CapturedRegion {
            bounds: Rect {
                left: -240,
                top: -8,
                right: 0,
                bottom: 112,
            },
            pixels: [30, 55, 80, 255].repeat(240 * 120),
        };
        let size = thumbnail(&image).size();
        assert_eq!((size.width, size.height), (116, 58));
        image.pixels.clear();
        let size = thumbnail(&image).size();
        assert_eq!((size.width, size.height), (0, 0));
    }

    #[test]
    fn pin_manager_renders_and_routes_selection_and_confirm_keys() {
        let slot = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(Backend(slot.clone()))).unwrap();
        let w = PinManagerWindow::new().unwrap();
        let adapter = slot.borrow().as_ref().unwrap().clone();
        adapter.set_size(slint::PhysicalSize::new(640, 480));
        w.set_selected(0);
        w.set_rows(slint::ModelRc::new(slint::VecModel::from(vec![
            PinManagerRow {
                identity: "p3".into(),
                number: "1".into(),
                kind: "Image".into(),
                summary: "1280 × 720 px".into(),
                status: "Visible".into(),
                ..Default::default()
            },
            PinManagerRow {
                identity: "t3".into(),
                number: "2".into(),
                kind: "Text".into(),
                summary: "A preserved note · 文字".into(),
                status: "Hidden".into(),
                ..Default::default()
            },
        ])));
        let selection = Rc::new(RefCell::new(None));
        let seen = selection.clone();
        w.on_select(move |id| *seen.borrow_mut() = Some(id));
        let arrows = Rc::new(Cell::new(0));
        let seen = arrows.clone();
        w.on_navigate(move |step| seen.set(step));
        let action = Rc::new(Cell::new(-1));
        let seen = action.clone();
        let weak = w.as_weak();
        w.on_action(move |index| {
            seen.set(index);
            show_confirmation(&weak.unwrap(), index == 3);
        });
        let click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            w.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            w.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        };
        click(120., 160.);
        assert_eq!(
            selection.borrow().as_ref().map(slint::SharedString::as_str),
            Some("t3")
        );
        let key = |text: Key| {
            w.window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
            w.window()
                .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
        };
        w.invoke_focus_list();
        key(Key::UpArrow);
        assert_eq!(arrows.get(), -1);
        w.set_can_close(true);
        click(573., 448.);
        assert_eq!(action.get(), 3);
        assert!(w.get_confirming());
        key(Key::Escape);
        assert_eq!(action.get(), 4);
        assert!(!w.get_confirming());

        w.set_show_recovery(true);
        w.set_can_recover(true);
        w.set_can_close(true);
        w.set_can_locate(true);
        w.set_can_toggle(true);
        for lang in lexift_core::domain::ui_language::UiLanguage::ALL {
            crate::i18n::select(lang);
            for dark in [0, 1] {
                w.global::<crate::Colors>().set_preference(dark);
                for scale in [1., 1.25] {
                    w.window().dispatch_event(WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                    for (width, height) in [(640., 480.), (420., 340.)] {
                        for confirming in [false, true] {
                            w.set_confirming(confirming);
                            let (width, height) = ((width * scale) as u32, (height * scale) as u32);
                            adapter.set_size(slint::PhysicalSize::new(width, height));
                            let mut pixels =
                                vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                            adapter.request_redraw();
                            assert!(adapter.draw_if_needed(|renderer| {
                                renderer.render(&mut pixels, width as usize);
                            }));
                            assert!(pixels.windows(2).any(|pair| pair[0] != pair[1]));
                            if let Ok(dir) = std::env::var("LEXIFT_PIN_MANAGER_RENDER_DIR") {
                                crate::i18n::write_test_bitmap(
                                    &dir,
                                    &format!("{}-{dark}-{scale}-{width}-{confirming}", lang.code()),
                                    width,
                                    height,
                                    pixels.into_iter().map(|p| [p.r, p.g, p.b, 255]),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
