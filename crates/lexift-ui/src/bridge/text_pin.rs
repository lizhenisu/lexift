use super::*;
use lexift_core::domain::{
    pinned_image::visibility::Request,
    text_pin::{Command, Creation, Event, Failure},
};
use slint::ComponentHandle;
use std::collections::BTreeMap;

type Preparation = fn(&slint::Window) -> PassiveWindowPreparation;
type Handler = Rc<dyn Fn(AppEvent)>;

struct PinView {
    window: crate::TextPinWindow,
    sink: PopupPointerSink,
    ready: bool,
    hidden: bool,
    current_batch: u64,
    gesturing: Rc<Cell<bool>>,
}

impl Drop for PinView {
    fn drop(&mut self) {
        let _ = self.window.hide();
    }
}

#[derive(Default)]
struct Registry {
    views: BTreeMap<u64, PinView>,
    creation: Option<Creation>,
    batch: Option<lexift_core::domain::pinned_image::visibility::Batch>,
    available: bool,
    suspended: bool,
    frozen: bool,
    capture_visible: bool,
    exited: bool,
}

thread_local! {
    static SERVICES: RefCell<Option<(Preparation, WindowLifecycleCallbacks)>> = const { RefCell::new(None) };
    static VIEWS: RefCell<Registry> = RefCell::new(Registry::default());
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

pub(super) fn init(prepare: Preparation, lifecycle: WindowLifecycleCallbacks) {
    SERVICES.with(|s| *s.borrow_mut() = Some((prepare, lifecycle)));
}

pub(super) fn set_handler(handler: Handler) {
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}

fn emit(event: Event) {
    let handler = HANDLER.with(|h| h.borrow().clone());
    if let Some(handler) = handler {
        handler(AppEvent::TextPin(event));
    }
}

pub(crate) fn available() -> bool {
    VIEWS.with(|v| {
        let v = v.borrow();
        v.available && !v.frozen && !v.suspended && v.views.values().all(|p| !p.gesturing.get())
    })
}

pub(crate) fn idle() -> bool {
    VIEWS.with(|v| {
        let v = v.borrow();
        v.creation.is_none() && v.views.values().all(|p| !p.gesturing.get())
    })
}

pub(super) fn active() -> bool {
    VIEWS.with(|v| !v.borrow().views.is_empty())
}

pub(super) fn sync(state: &AppState) {
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.creation = state.text_pins.creation;
        v.batch = state.pin_visibility.pending.clone();
        v.available = lexift_core::domain::pinned_image::visibility::idle(state)
            && state.pin_restore.is_none();
    });
}

pub(super) fn apply_theme() {
    VIEWS.with(|v| {
        for p in v.borrow().views.values() {
            crate::theme::apply(&p.window);
        }
    });
}

pub(super) fn close_all() {
    let old = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.exited = true;
        std::mem::take(&mut v.views)
    });
    drop(old);
}

fn services() -> Option<(Preparation, WindowLifecycleCallbacks)> {
    SERVICES.with(|s| s.borrow().clone())
}

fn window(id: u64) -> Option<crate::TextPinWindow> {
    VIEWS.with(|v| v.borrow().views.get(&id).map(|p| p.window.clone_strong()))
}

fn operable(id: u64) -> bool {
    VIEWS.with(|v| {
        let v = v.borrow();
        !v.exited
            && !v.suspended
            && !v.frozen
            && v.batch.is_none()
            && v.views.get(&id).is_some_and(|p| p.ready && !p.hidden)
    })
}

fn remove(id: u64) {
    let view = VIEWS.with(|v| v.borrow_mut().views.remove(&id));
    if let Some(view) = view {
        if let Some((_, lifecycle)) = services() {
            (lifecycle.cancel_window_interaction)(view.window.window());
        }
        drop(view);
        super::schedule_idle_memory_trim();
    }
}

pub(super) fn execute(command: Command, text: Option<Arc<str>>) {
    match command {
        Command::Present(creation) => {
            if let Some(text) = text {
                open(creation, text);
            }
        }
        Command::Drop(id) => remove(id),
        Command::Visibility(request) => apply_visibility(request),
        Command::Feedback { anchor, failure } => {
            let key = match failure {
                Failure::Empty => "The clipboard has no text to pin.",
                Failure::TooLarge => "Text pins are limited to 64 KiB.",
                Failure::Read => "Could not read clipboard text.",
                Failure::Limit => "Close a text pin before adding another. The limit is 20.",
                Failure::Window => "Could not open the text pin.",
                Failure::Copy => "Could not copy clipboard entry.",
            };
            let point =
                anchor.or_else(|| services().and_then(|(_, l)| (l.toolbar_cursor_position)()));
            if let Some(point) = point {
                super::image_history::show_passive_feedback(point, key, true);
            }
        }
        Command::Read(_) | Command::Copy { .. } => {}
    }
}

fn open(creation: Creation, text: Arc<str>) {
    let id = creation.id;
    let accepted = VIEWS.with(|v| {
        let v = v.borrow();
        !v.exited && v.creation == Some(creation) && !v.views.contains_key(&id)
    });
    if !accepted {
        return;
    }
    let Some((_, lifecycle)) = services() else {
        emit(Event::Presented { id, success: false });
        return;
    };
    let Ok(window) = crate::screen_interaction::create(&lifecycle, crate::TextPinWindow::new)
    else {
        emit(Event::Presented { id, success: false });
        return;
    };
    crate::theme::apply(&window);
    crate::i18n::apply();
    window.set_content(text.as_ref().into());
    let native = window.window();
    native.set_position(slint::PhysicalPosition::new(
        creation.anchor.x,
        creation.anchor.y,
    ));
    window.on_close_pin(move || close_later(id));
    native.on_close_requested(move || {
        close_later(id);
        slint::CloseRequestResponse::KeepWindowShown
    });
    window.on_copy_all(move || copy(id, None));
    window.on_copy_text(move |a, c| {
        if a != c {
            copy(id, Some((a, c)));
        }
    });
    window.on_change_topmost(move |topmost| {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(window) = window_for_input(id)
                && let Some((_, lifecycle)) = services()
            {
                if (lifecycle.configure_pinned_image)(window.window(), topmost, true, 0) {
                    window.set_topmost(topmost);
                } else {
                    (lifecycle.configure_pinned_image)(
                        window.window(),
                        window.get_topmost(),
                        true,
                        0,
                    );
                }
            }
        });
    });
    window.on_move_requested(move || start_gesture(id, None));
    window.on_resize_requested(move |edge| {
        if let Some(edge) = PopupResizeEdge::from_index(edge) {
            start_gesture(id, Some(edge));
        }
    });
    let gesturing = Rc::new(Cell::new(false));
    let native_gesture = gesturing.clone();
    let weak = window.as_weak();
    let sink: PopupPointerSink = Rc::new(move |input| {
        if matches!(
            input,
            PopupPointerInput::NativeMoveFinished
                | PopupPointerInput::ResizeFinished { .. }
                | PopupPointerInput::LeftReleased { .. }
                | PopupPointerInput::LeftCancelled { .. }
        ) {
            native_gesture.set(false);
        }
        let Some(window) = weak.upgrade().filter(|w| w.get_input_enabled()) else {
            return;
        };
        match input {
            PopupPointerInput::RightPressed { x, y } => {
                let scale = window.window().scale_factor().max(0.1);
                let weak = window.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if operable(id)
                        && let Some(w) = weak.upgrade()
                    {
                        w.invoke_open_menu(x / scale, y / scale);
                    }
                });
            }
            PopupPointerInput::RightReleased { .. } => {}
            PopupPointerInput::LeftCancelled { .. } => {
                dispatch_pointer_input(
                    window.window(),
                    PopupPointerInput::LeftReleased { x: -1., y: -1. },
                );
            }
            _ => dispatch_pointer_input(window.window(), input),
        }
    });
    VIEWS.with(|v| {
        v.borrow_mut().views.insert(
            id,
            PinView {
                window,
                sink,
                ready: false,
                hidden: false,
                current_batch: 0,
                gesturing,
            },
        )
    });
    super::cancel_idle_memory_trim();
    prepare(creation, 0);
}

pub(crate) fn manager_idle(id: u64) -> bool {
    VIEWS.with(|v| {
        let v = v.borrow();
        !v.frozen
            && !v.suspended
            && v.views
                .get(&id)
                .is_some_and(|p| p.ready && !p.gesturing.get())
    })
}

pub(crate) fn manager_summary(id: u64) -> Option<slint::SharedString> {
    Some(window(id)?.get_content())
}

pub(crate) fn manager_bounds(id: u64) -> Option<Rect> {
    let w = window(id)?;
    let p = w.window().position();
    let size = w.window().size();
    Some(Rect {
        left: p.x,
        top: p.y,
        right: p.x.saturating_add(size.width as i32),
        bottom: p.y.saturating_add(size.height as i32),
    })
}

pub(crate) fn manager_close(id: u64) {
    close_later(id);
}
fn close_later(id: u64) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        if window(id).is_some() {
            emit(Event::Closed(id));
        }
    });
}

fn copy(id: u64, selection: Option<(i32, i32)>) {
    if operable(id) {
        emit(Event::Copy { id, selection });
    }
}

fn window_for_input(id: u64) -> Option<crate::TextPinWindow> {
    if operable(id) { window(id) } else { None }
}

fn start_gesture(id: u64, edge: Option<PopupResizeEdge>) {
    let Some(window) = window_for_input(id) else {
        return;
    };
    let Some((_, lifecycle)) = services() else {
        return;
    };
    let flag = VIEWS.with(|v| v.borrow().views[&id].gesturing.clone());
    flag.set(true);
    slint::Timer::single_shot(Duration::ZERO, move || {
        if !flag.get() || !operable(id) {
            flag.set(false);
            return;
        }
        let win = window.window();
        let started = if let Some(edge) = edge {
            (lifecycle.begin_window_resize)(win, edge, false)
        } else {
            (lifecycle.begin_window_drag)(win)
        };
        if !started {
            flag.set(false);
        }
    });
}

fn prepare(creation: Creation, attempt: u8) {
    let id = creation.id;
    let allowed = VIEWS.with(|v| {
        let v = v.borrow();
        !v.exited && !v.frozen && !v.suspended && v.creation == Some(creation)
    });
    let Some(window) = window(id) else { return };
    let Some((prepare_window, lifecycle)) = services() else {
        return;
    };
    if allowed {
        match prepare_window(window.window()) {
            PassiveWindowPreparation::Ready => {
                if let Some(area) = (lifecycle.popup_work_area)(creation.anchor) {
                    let win = window.window();
                    let scale = (lifecycle.screenshot_displays)()
                        .into_iter()
                        .find(|(bounds, _)| {
                            bounds.left <= creation.anchor.x
                                && creation.anchor.x < bounds.right
                                && bounds.top <= creation.anchor.y
                                && creation.anchor.y < bounds.bottom
                        })
                        .map_or_else(|| win.scale_factor(), |(_, scale)| scale)
                        .max(0.1);
                    let width = (420. * scale)
                        .round()
                        .min((area.right - area.left).max(1) as f32)
                        as u32;
                    let height = (280. * scale)
                        .round()
                        .min((area.bottom - area.top).max(1) as f32)
                        as u32;
                    win.set_size(slint::PhysicalSize::new(width, height));
                    win.set_position(slint::PhysicalPosition::new(
                        creation
                            .anchor
                            .x
                            .saturating_sub(width as i32 / 2)
                            .clamp(area.left, area.right - width as i32),
                        creation
                            .anchor
                            .y
                            .saturating_sub(height as i32 / 2)
                            .clamp(area.top, area.bottom - height as i32),
                    ));
                }
                if show(id, true) {
                    VIEWS.with(|v| {
                        if let Some(pin) = v.borrow_mut().views.get_mut(&id) {
                            pin.ready = true;
                        }
                    });
                    emit(Event::Presented { id, success: true });
                    return;
                }
            }
            PassiveWindowPreparation::Pending if attempt < 19 => {
                slint::Timer::single_shot(Duration::from_millis(16), move || {
                    prepare(creation, attempt + 1)
                });
                return;
            }
            _ => {}
        }
    }
    remove(id);
    emit(Event::Presented { id, success: false });
}

fn show(id: u64, interactive: bool) -> bool {
    let Some((_, l)) = services() else {
        return false;
    };
    let context = VIEWS.with(|v| {
        let v = v.borrow();
        let pin = v.views.get(&id)?;
        Some((pin.window.clone_strong(), pin.sink.clone()))
    });
    let Some((w, sink)) = context else {
        return false;
    };
    w.set_input_enabled(interactive);
    (l.prepare_pinned_image)(w.window())
        && (l.complete_passive_window_show)(w.window(), sink)
        && (l.configure_pinned_image)(w.window(), w.get_topmost(), interactive, 0)
        && w.show().is_ok()
        && w.window().is_visible()
}

fn hide(pin: &crate::TextPinWindow, lifecycle: &WindowLifecycleCallbacks) -> bool {
    pin.invoke_close_menu();
    (lifecycle.cancel_window_interaction)(pin.window());
    pin.hide().is_ok() && !pin.window().is_visible()
}

fn apply_visibility(request: Request) {
    let target = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        if v.exited
            || v.frozen
            || v.suspended
            || !v.batch.as_ref().is_some_and(|b| {
                b.id == request.batch
                    && b.hidden == request.hidden
                    && b.pending_text.contains(&request.id)
            })
        {
            return None;
        }
        let pin = v.views.get_mut(&request.id)?;
        if !pin.ready || request.batch <= pin.current_batch || pin.gesturing.get() {
            return None;
        }
        pin.current_batch = request.batch;
        Some(pin.window.clone_strong())
    });
    let success = target.zip(services()).is_some_and(|(w, (_, l))| {
        if request.hidden {
            hide(&w, &l)
        } else {
            let shown = show(request.id, true);
            if !shown {
                hide(&w, &l);
            }
            shown
        }
    });
    if success {
        VIEWS.with(|v| {
            if let Some(pin) = v.borrow_mut().views.get_mut(&request.id) {
                pin.hidden = request.hidden;
            }
        });
    }
    emit(Event::VisibilityApplied(request, success));
    if success && request.hidden {
        super::schedule_idle_memory_trim();
    } else if success {
        super::cancel_idle_memory_trim();
    }
}

fn apply_capture_state() -> bool {
    let Some((_, l)) = services() else {
        return true;
    };
    let (views, frozen, visible, suspended) = VIEWS.with(|v| {
        let v = v.borrow();
        let views = v
            .views
            .iter()
            .filter(|(_, p)| p.ready && !p.hidden)
            .map(|(&id, p)| (id, p.window.clone_strong(), p.gesturing.clone()))
            .collect::<Vec<_>>();
        (views, v.frozen, v.capture_visible, v.suspended)
    });
    let mut ready = true;
    for (id, w, gesture) in views {
        let interactive = !frozen && !suspended;
        if !interactive {
            gesture.set(false);
            w.invoke_close_menu();
            (l.cancel_window_interaction)(w.window());
        }
        w.set_input_enabled(interactive);
        let success = if frozen && !visible {
            hide(&w, &l)
        } else if !w.window().is_visible() {
            show(id, interactive)
        } else {
            (l.configure_pinned_image)(w.window(), w.get_topmost(), interactive, 0)
        };
        ready &= success;
    }
    ready
}

pub(crate) fn set_frozen(frozen: bool) {
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.frozen = frozen;
        v.capture_visible = false;
    });
    apply_capture_state();
}

pub(crate) fn set_interactive(interactive: bool) {
    VIEWS.with(|v| v.borrow_mut().suspended = !interactive);
    apply_capture_state();
}

pub(crate) fn capture_visibility(visible: bool) -> bool {
    if VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.capture_visible = v.frozen && visible;
        v.frozen
    }) {
        apply_capture_state()
    } else {
        true
    }
}
