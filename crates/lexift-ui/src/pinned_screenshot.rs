//! Independent pinned result windows. Each instance owns only its cropped image.
use crate::{
    PassiveWindowPreparation, PinnedScreenshotWindow, WindowLifecycleCallbacks,
    screen_interaction::window::Slot,
};
use lexift_core::{
    domain::screenshot::ScreenshotEvent,
    ports::{annotation_save::SaveOutcome, screen_capture::CapturedRegion},
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Visible,
    Closing,
}
#[derive(Default)]
struct Gesture {
    serial: Cell<u64>,
    pressed: Cell<bool>,
    queued: Cell<Option<u64>>,
    closed: Cell<bool>,
    menu_epoch: Cell<u64>,
}
impl Gesture {
    fn press(&self) {
        self.menu_epoch.set(
            self.menu_epoch
                .get()
                .checked_add(1)
                .expect("pin interaction epoch exhausted"),
        );
        if !self.closed.get() {
            self.serial.set(
                self.serial
                    .get()
                    .checked_add(1)
                    .expect("pin press identity exhausted"),
            );
            self.pressed.set(true);
            self.queued.set(None);
        }
    }
    fn cancel(&self) {
        self.menu_epoch.set(
            self.menu_epoch
                .get()
                .checked_add(1)
                .expect("pin interaction epoch exhausted"),
        );
        self.pressed.set(false);
        self.queued.set(None);
    }
    fn request(&self) -> Option<u64> {
        if self.closed.get() || !self.pressed.get() || self.queued.get().is_some() {
            return None;
        }
        let serial = self.serial.get();
        self.queued.set(Some(serial));
        Some(serial)
    }
    fn take(&self, serial: u64) -> bool {
        if self.closed.get()
            || !self.pressed.get()
            || self.serial.get() != serial
            || self.queued.get() != Some(serial)
        {
            return false;
        }
        self.queued.set(None);
        true
    }
    fn retire(&self) {
        self.closed.set(true);
        self.cancel();
    }
}
struct Pin {
    window: Slot<PinnedScreenshotWindow>,
    image: Arc<CapturedRegion>,
    session: u64,
    operation: u64,
    document: Option<lexift_core::domain::annotation_export::ExportRequest>,
    revision: u64,
    phase: Phase,
    prepared_inset: i32,
    topmost: bool,
    shadow: bool,
    gesture: Rc<Gesture>,
    close_task: slint::Timer,
    output_error: Option<String>,
    feedback: Slot<crate::ScreenshotToastWindow>,
    feedback_timer: slint::Timer,
}
struct Registry {
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    next: u64,
    interactive: bool,
    windows: BTreeMap<u64, Pin>,
}
thread_local! {static STATE:RefCell<Option<Registry>>=const{RefCell::new(None)};}
pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    let next = STATE.with(|s| s.borrow().as_ref().map_or(0, |r| r.next));
    close_all();
    STATE.with(|s| {
        *s.borrow_mut() = Some(Registry {
            prepare,
            lifecycle,
            next,
            interactive: true,
            windows: BTreeMap::new(),
        })
    });
}
fn emit_pin(event: lexift_core::domain::pinned_image::Event) {
    crate::annotation::emit_pin(event);
}
fn native_scale(window: &slint::Window) -> f32 {
    use slint::winit_030::WinitWindowAccessor;
    window
        .with_winit_window(|native| native.scale_factor() as f32)
        .unwrap_or_else(|| window.scale_factor())
}
fn apply_policy(id: u64) -> bool {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        Some((p.window.get()?.clone_strong(), r.lifecycle.clone()))
    });
    context.is_some_and(|(w, l)| {
        (l.configure_pinned_image)(
            w.window(),
            w.get_topmost(),
            w.get_interactive(),
            (w.get_frame_inset() * native_scale(w.window())).round() as i32,
        )
    })
}
fn change_option(id: u64, kind: i32, value: bool) {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        Some((
            p.window.get()?.clone_strong(),
            r.interactive && p.phase == Phase::Visible,
            p.topmost,
            p.shadow,
        ))
    });
    let Some((window, allowed, topmost, shadow)) = context else {
        return;
    };
    if !allowed || !matches!(kind, 0 | 1) {
        window.set_topmost(topmost);
        window.set_shadow_enabled(shadow);
        return;
    }
    if kind == 1 && !apply_policy(id) {
        window.set_topmost(topmost);
        apply_policy(id);
        show_feedback(id, "Screenshot unavailable", 0, None);
        return;
    }
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
            if kind == 0 {
                p.shadow = value;
            } else {
                p.topmost = value;
            }
        }
    });
}
pub(crate) fn apply_theme() {
    let windows = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| {
                r.windows
                    .values()
                    .filter_map(|p| p.window.get().map(ComponentHandle::clone_strong))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    for window in windows {
        crate::theme::apply(&window);
    }
}
pub(crate) fn close_menus() {
    let windows = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| {
                r.windows
                    .values()
                    .filter_map(|p| {
                        p.window
                            .get()
                            .map(|w| (w.clone_strong(), r.lifecycle.clone()))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    for (window, lifecycle) in windows {
        window.invoke_close_menu();
        (lifecycle.cancel_window_interaction)(window.window());
    }
}
pub(crate) fn set_annotation_interaction(allowed: bool) {
    let contexts = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return vec![] };
        if r.interactive == allowed {
            return vec![];
        };
        r.interactive = allowed;
        r.windows
            .iter()
            .filter_map(|(id, p)| {
                p.window.get().map(|w| {
                    (
                        *id,
                        w.clone_strong(),
                        p.gesture.clone(),
                        r.lifecycle.clone(),
                    )
                })
            })
            .collect::<Vec<_>>()
    });
    for (id, w, g, l) in contexts {
        g.cancel();
        w.invoke_close_menu();
        (l.cancel_window_interaction)(w.window());
        w.set_interactive(allowed);
        apply_policy(id);
    }
}
pub(crate) fn request_output(id: u64, kind: i32) {
    if !matches!(kind, 0 | 1) {
        return;
    }
    let owner = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        if p.phase != Phase::Visible || !r.interactive {
            return None;
        };
        let w = p.window.get()?;
        if w.get_output_busy() {
            return None;
        };
        Some((r.lifecycle.annotation_window_token)(w.window()))
    });
    if let Some(owner) = owner {
        emit_pin(lexift_core::domain::pinned_image::Event::Requested {
            id,
            kind: if kind == 0 {
                lexift_core::domain::pinned_image::Output::Copy
            } else {
                lexift_core::domain::pinned_image::Output::Save
            },
            owner,
            title: crate::i18n::tr("Save image"),
        });
    }
}
pub(crate) fn sync_outputs(states: &lexift_core::domain::pinned_image::States) {
    let errors = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return vec![] };
        let mut errors = vec![];
        for (id, p) in &mut r.windows {
            if let Some(state) = states.get(id) {
                if let Some(w) = p.window.get() {
                    w.set_output_busy(state.busy);
                    w.set_annotations_hidden(state.hidden);
                }
                p.revision = state.revision;
                if p.output_error != state.error {
                    p.output_error = state.error.clone();
                    errors.push((*id, state.error.clone()));
                }
            }
        }
        errors
    });
    crate::annotation::pin_editor::sync_outputs(states);
    for (id, message) in errors {
        clear_feedback(id, None);
        if let Some(message) = message {
            show_feedback(id, &message, 0, None);
        }
    }
}
fn clear_feedback(id: u64, expected: Option<u64>) {
    let retired = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.as_mut()?.windows.get_mut(&id)?;
        if expected.is_some_and(|revision| p.feedback.revision() != revision) {
            return None;
        }
        p.feedback_timer.stop();
        Some(std::mem::take(&mut p.feedback))
    });
    drop(retired);
}
fn show_feedback(id: u64, message: &str, attempt: u8, expected: Option<u64>) {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        if p.phase != Phase::Visible || expected.is_some_and(|rev| p.feedback.revision() != rev) {
            return None;
        }
        Some((
            r.prepare,
            r.lifecycle.clone(),
            p.window.get()?.window().position(),
        ))
    });
    let Some((prepare, lifecycle, position)) = context else {
        return;
    };
    if attempt == 0 {
        clear_feedback(id, None);
        let Ok(w) =
            crate::screen_interaction::create(&lifecycle, crate::ScreenshotToastWindow::new)
        else {
            return;
        };
        crate::theme::apply(&w);
        w.set_message(crate::i18n::error(message).into());
        let revision = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let p = s.as_mut()?.windows.get_mut(&id)?;
            Some(p.feedback.open(w.clone_strong()))
        });
        let Some(revision) = revision else { return };
        w.on_dismissed(move || {
            slint::Timer::single_shot(Duration::ZERO, move || clear_feedback(id, Some(revision)))
        });
    }
    let current = STATE.with(|s| {
        let s = s.borrow();
        let p = s.as_ref()?.windows.get(&id)?;
        Some((p.feedback.get()?.clone_strong(), p.feedback.revision()))
    });
    let Some((w, revision)) = current else { return };
    match prepare(w.window()) {
        PassiveWindowPreparation::Ready => {
            if let Some(area) = (lifecycle.popup_work_area)(lexift_core::domain::geometry::Point {
                x: position.x,
                y: position.y,
            }) {
                let size = w.window().size();
                let width = size.width.min((area.right - area.left - 16).max(1) as u32);
                let height = size.height.min((area.bottom - area.top - 16).max(1) as u32);
                w.window().set_size(slint::PhysicalSize::new(width, height));
                w.window().set_position(slint::PhysicalPosition::new(
                    position.x.clamp(
                        area.left + 8,
                        (area.right - 8 - width as i32).max(area.left + 8),
                    ),
                    position.y.clamp(
                        area.top + 8,
                        (area.bottom - 8 - height as i32).max(area.top + 8),
                    ),
                ));
            }
            let _ = w.show();
            STATE.with(|s| {
                if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
                    p.feedback_timer.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_secs(4),
                        move || clear_feedback(id, Some(revision)),
                    );
                }
            });
        }
        PassiveWindowPreparation::Pending if attempt < 20 => {
            let message = message.to_string();
            STATE.with(|s| {
                if let Some(p) = s.borrow().as_ref().and_then(|r| r.windows.get(&id)) {
                    p.feedback
                        .prepare_after(Duration::from_millis(16), move || {
                            show_feedback(id, &message, attempt + 1, Some(revision))
                        });
                }
            });
        }
        _ => clear_feedback(id, Some(revision)),
    }
}

/// The frozen screenshot already contains existing pins. Hide their live windows
/// during editing so only the snapshot receives gestures; restore without activation.
pub(crate) fn set_frozen(frozen: bool) {
    let windows = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| {
                r.windows
                    .values()
                    .filter(|p| p.phase == Phase::Visible)
                    .map(|p| {
                        p.gesture.cancel();
                        (p.window.get().unwrap().clone_strong(), r.lifecycle.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    for (window, lifecycle) in windows {
        if frozen {
            window.invoke_close_menu();
            (lifecycle.cancel_window_interaction)(window.window());
            let _ = window.hide();
        } else {
            let _ = window.show();
        }
    }
}
pub(crate) fn active() -> bool {
    STATE.with(|s| s.borrow().as_ref().is_some_and(|s| !s.windows.is_empty()))
}
pub(crate) fn close_all() {
    let retired = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        Some((std::mem::take(&mut r.windows), r.lifecycle.clone()))
    });
    if let Some((mut windows, lifecycle)) = retired {
        // Native cancellation of one window can send input to another. Retire
        // the whole group before beginning any synchronous native teardown.
        for pin in windows.values_mut() {
            pin.gesture.retire();
            pin.window.retire_input();
            pin.close_task.stop();
        }
        for (id, pin) in windows {
            emit_pin(lexift_core::domain::pinned_image::Event::Closed { id });
            destroy(pin, &lifecycle);
        }
    }
}
pub(crate) fn request_close(id: u64) {
    let window = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let pin = s.as_mut()?.windows.get_mut(&id)?;
        if pin.phase == Phase::Closing {
            return None;
        }
        pin.phase = Phase::Closing;
        pin.gesture.retire();
        pin.window.retire_input();
        pin.close_task
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                close(id)
            });
        pin.window.get().map(ComponentHandle::clone_strong)
    });
    if let Some(window) = window {
        emit_pin(lexift_core::domain::pinned_image::Event::Closed { id });
        window.invoke_close_menu();
    }
}
fn close(id: u64) {
    crate::annotation::pin_editor::forget(id);
    let retired = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        Some((r.windows.remove(&id)?, r.lifecycle.clone()))
    });
    emit_pin(lexift_core::domain::pinned_image::Event::Closed { id });
    if let Some((pin, lifecycle)) = retired {
        destroy(pin, &lifecycle);
    }
    crate::bridge::schedule_idle_memory_trim();
}
fn destroy(mut pin: Pin, lifecycle: &WindowLifecycleCallbacks) {
    pin.gesture.retire();
    pin.window.retire_input();
    pin.close_task.stop();
    pin.feedback_timer.stop();
    pin.feedback.clear();
    if let Some(window) = pin.window.get() {
        window.invoke_close_menu();
        (lifecycle.cancel_window_interaction)(window.window());
    }
    pin.window.clear();
}
pub(crate) fn input(id: u64) -> Option<Rc<crate::screen_interaction::window::InputWork>> {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .map(|p| p.window.input())
    })
}
pub(crate) fn edit_window(id: u64, editing: bool) -> Option<PinnedScreenshotWindow> {
    let context = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        let p = r.windows.get_mut(&id)?;
        if p.phase != Phase::Visible && editing {
            return None;
        }
        p.gesture.cancel();
        Some((p.window.get()?.clone_strong(), r.lifecycle.clone()))
    });
    let (window, lifecycle) = context?;
    window.invoke_close_menu();
    (lifecycle.cancel_window_interaction)(window.window());
    window.set_editing(editing);
    if !editing {
        window.set_decoration(slint::Image::default());
    }
    window.set_interactive(true);
    apply_policy(id);
    Some(window)
}
pub(crate) fn present(id: u64, revision: u64, image: Arc<CapturedRegion>) {
    STATE.with(|s| {
        let s = s.borrow();
        let Some(p) = s
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .filter(|p| p.phase == Phase::Visible && p.revision == revision)
        else {
            return;
        };
        if let Some(w) = p.window.get().filter(|w| !w.get_editing()) {
            set_picture(w, &image);
        }
    });
}
fn set_picture(window: &PinnedScreenshotWindow, image: &CapturedRegion) {
    window.set_picture(slint::Image::from_rgba8_premultiplied(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &image.pixels,
            (image.bounds.right - image.bounds.left) as u32,
            (image.bounds.bottom - image.bounds.top) as u32,
        ),
    ));
}
pub(crate) fn present_edit(id: u64, frame: tiny_skia::Pixmap, controls: tiny_skia::Pixmap) {
    STATE.with(|s| {
        let s = s.borrow();
        if let Some(w) = s
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .filter(|p| p.phase == Phase::Visible)
            .and_then(|p| p.window.get())
            .filter(|w| w.get_editing())
        {
            w.set_decoration(slint::Image::from_rgba8_premultiplied(
                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    controls.data(),
                    controls.width(),
                    controls.height(),
                ),
            ));
            w.set_picture(slint::Image::from_rgba8_premultiplied(
                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    frame.data(),
                    frame.width(),
                    frame.height(),
                ),
            ));
        }
    });
}
pub(crate) fn show(session: u64, operation: u64, image: Arc<CapturedRegion>) {
    show_document(session, operation, image, None);
}
pub(crate) fn show_document(
    session: u64,
    operation: u64,
    image: Arc<CapturedRegion>,
    document: Option<(
        Arc<CapturedRegion>,
        Arc<lexift_core::ports::screen_capture::ScreenCaptureFrame>,
        lexift_core::domain::annotation_export::ExportRequest,
    )>,
) {
    if !crate::annotation::screenshot::output_current(session, operation) {
        return;
    }
    let result = STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().ok_or(())?;
        let w = image.bounds.right - image.bounds.left;
        let h = image.bounds.bottom - image.bounds.top;
        if w <= 0
            || h <= 0
            || (w as usize)
                .checked_mul(h as usize)
                .and_then(|n| n.checked_mul(4))
                != Some(image.pixels.len())
        {
            return Err(());
        }
        let window = crate::screen_interaction::create(&r.lifecycle, PinnedScreenshotWindow::new)
            .map_err(|_| ())?;
        crate::theme::apply(&window);
        window.set_picture(slint::Image::from_rgba8_premultiplied(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &image.pixels,
                w as u32,
                h as u32,
            ),
        ));
        r.next = r.next.checked_add(1).ok_or(())?;
        let id = r.next;
        let mut owned = Slot::default();
        owned.open(window);
        let window = owned.get().ok_or(())?;
        window.on_close_requested(move || request_close(id));
        window.on_output_requested(move |kind| request_output(id, kind));
        window.on_option_requested(move |kind, value| change_option(id, kind, value));
        window.on_annotate_requested(move || {
            emit_pin(lexift_core::domain::pinned_image::Event::EditRequested { id })
        });
        window.on_visibility_requested(move |hidden| {
            emit_pin(lexift_core::domain::pinned_image::Event::VisibilityChanged { id, hidden })
        });
        window.on_edit_pointer(move |event, x, y| {
            crate::annotation::pin_editor::pointer(id, event, x, y)
        });
        window.on_edit_key(move |key| crate::annotation::pin_editor::key(id, key));
        window.on_edit_scroll(move |delta| crate::annotation::pin_editor::scroll(id, delta));
        window.on_escape_requested(move || {
            if crate::annotation::pin_editor::current().is_some_and(|p| p.0 == id) {
                crate::annotation::pin_editor::escape(id);
            } else {
                request_close(id);
            }
        });
        window.window().on_close_requested(move || {
            request_close(id);
            slint::CloseRequestResponse::KeepWindowShown
        });
        let gesture = Rc::new(Gesture::default());
        let tracking = gesture.clone();
        window.on_press_changed(move |pressed| {
            if pressed {
                tracking.press();
            } else {
                tracking.cancel();
            }
        });
        let input = owned.input();
        let tracking = gesture.clone();
        window.on_drag_requested(move || {
            if let Some(serial) = tracking.request() {
                input.post(move || begin_drag(id, serial));
            }
        });
        window.on_zoom_requested(move |delta, x, y| zoom(id, delta, x, y));
        window.window().set_position(slint::PhysicalPosition::new(
            image.bounds.left,
            image.bounds.top,
        ));
        window
            .window()
            .set_size(slint::PhysicalSize::new(w as u32, h as u32));
        let inset = (window.get_frame_inset() * window.window().scale_factor()).round() as i32;
        window.window().set_position(slint::PhysicalPosition::new(
            image.bounds.left - inset,
            image.bounds.top - inset,
        ));
        window.window().set_size(slint::PhysicalSize::new(
            (w + 2 * inset) as u32,
            (h + 2 * inset) as u32,
        ));
        window.set_interactive(r.interactive);
        r.windows.insert(
            id,
            Pin {
                window: owned,
                image,
                session,
                operation,
                document: document.as_ref().map(|(_, _, r)| r.clone()),
                revision: 0,
                phase: Phase::Preparing,
                prepared_inset: inset,
                topmost: true,
                shadow: true,
                gesture,
                close_task: slint::Timer::default(),
                output_error: None,
                feedback: Slot::default(),
                feedback_timer: slint::Timer::default(),
            },
        );
        Ok(id)
    });
    if let (Ok(id), Some((base, samples, request))) = (&result, document) {
        crate::annotation::pin_editor::install(*id, base, samples, request);
    }
    match result {
        Ok(id) => prepare(id, 0),
        Err(()) => crate::annotation::screenshot::emit(ScreenshotEvent::Completed {
            session,
            operation,
            result: Err(lexift_core::Error::new(crate::i18n::tr(
                "Screenshot unavailable",
            ))),
        }),
    }
}
/// ReleaseCapture may synchronously deliver Slint cancellation. Enter native dragging
/// only after the component callback and registry borrow have both returned.
fn begin_drag(id: u64, serial: u64) {
    let pair = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let pin = r.windows.get(&id)?;
        if !r.interactive
            || pin.window.get().is_some_and(|w| w.get_editing())
            || pin.phase != Phase::Visible
            || !pin.gesture.take(serial)
        {
            return None;
        }
        Some((pin.window.get()?.clone_strong(), r.lifecycle.clone()))
    });
    if let Some((window, lifecycle)) = pair.filter(|(window, _)| window.window().is_visible()) {
        (lifecycle.activate_user_requested_window)(window.window());
        (lifecycle.begin_window_drag)(window.window());
    }
}
fn prepare(id: u64, attempt: u8) {
    let value = STATE.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        let pin = r.windows.get(&id)?;
        if pin.phase != Phase::Preparing {
            return None;
        }
        Some((
            pin.window.get()?.clone_strong(),
            r.prepare,
            r.lifecycle.clone(),
            pin.session,
            pin.operation,
            pin.window.input(),
            pin.gesture.clone(),
        ))
    });
    let Some((window, prepare_window, lifecycle, session, operation, input, gesture)) = value
    else {
        return;
    };
    if !crate::annotation::screenshot::output_current(session, operation) {
        close(id);
        return;
    }
    match prepare_window(window.window()) {
        PassiveWindowPreparation::Ready => {
            fit(id, None);
            let weak = window.as_weak();
            let deferred = input.clone();
            let sink = input.pointer_sink(move |event| {
                if matches!(
                    event,
                    crate::PopupPointerInput::RightPressed { .. }
                        | crate::PopupPointerInput::RightReleased { .. }
                ) {
                    // A native menu can enter a nested User32 loop. Leave the input
                    // bridge callback before opening it, and reject stale interaction epochs.
                    let weak = weak.clone();
                    let gate = gesture.clone();
                    let epoch = gate.menu_epoch.get();
                    deferred.post(move || {
                        if gate.closed.get() || gate.menu_epoch.get() != epoch {
                            return;
                        }
                        if let Some(w) = weak
                            .upgrade()
                            .filter(|w| w.get_interactive() && w.window().is_visible())
                        {
                            crate::bridge::dispatch_pointer_input(w.window(), event);
                        }
                    });
                    return;
                }
                if matches!(
                    event,
                    crate::PopupPointerInput::LeftReleased { .. }
                        | crate::PopupPointerInput::LeftCancelled { .. }
                ) {
                    gesture.cancel();
                }
                if let Some(window) = weak.upgrade() {
                    if matches!(event, crate::PopupPointerInput::LeftCancelled { .. }) {
                        if window.get_editing() {
                            crate::annotation::pin_editor::pointer(id, 3, 0., 0.);
                        }
                        // Capture loss is not a click. End Slint's grab outside the
                        // image so a cancelled close-button press cannot activate it.
                        crate::bridge::dispatch_pointer_input(
                            window.window(),
                            crate::PopupPointerInput::Exited,
                        );
                        crate::bridge::dispatch_pointer_input(
                            window.window(),
                            crate::PopupPointerInput::LeftReleased {
                                x: -window.window().scale_factor(),
                                y: -window.window().scale_factor(),
                            },
                        );
                    } else {
                        crate::bridge::dispatch_pointer_input(window.window(), event);
                    }
                }
            });
            let success = (lifecycle.prepare_pinned_image)(window.window())
                && (lifecycle.complete_passive_window_show)(window.window(), sink)
                && crate::annotation::screenshot::output_current(session, operation)
                && apply_policy(id)
                && window.show().is_ok();
            if !success {
                close(id);
            } else {
                STATE.with(|s| {
                    if let Some(pin) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id))
                    {
                        pin.phase = Phase::Visible;
                    }
                });
            }
            if success {
                emit_pin(lexift_core::domain::pinned_image::Event::Opened {
                    id,
                    source: (session, operation),
                });
                let document = STATE.with(|s| {
                    s.borrow()
                        .as_ref()
                        .and_then(|r| r.windows.get(&id))
                        .and_then(|p| p.document.clone())
                });
                if let Some(document) = document {
                    emit_pin(lexift_core::domain::pinned_image::Event::DocumentChanged {
                        id,
                        document,
                    });
                }
            }
            crate::annotation::screenshot::emit(ScreenshotEvent::Completed {
                session,
                operation,
                result: if success {
                    Ok(SaveOutcome::Saved)
                } else {
                    Err(lexift_core::Error::new(crate::i18n::tr(
                        "Screenshot unavailable",
                    )))
                },
            });
        }
        PassiveWindowPreparation::Pending if attempt < 20 => STATE.with(|s| {
            if let Some(pin) = s.borrow().as_ref().and_then(|r| r.windows.get(&id)) {
                pin.window
                    .prepare_after(Duration::from_millis(16), move || prepare(id, attempt + 1));
            }
        }),
        _ => {
            close(id);
            crate::annotation::screenshot::emit(ScreenshotEvent::Completed {
                session,
                operation,
                result: Err(lexift_core::Error::new(crate::i18n::tr(
                    "Screenshot unavailable",
                ))),
            });
        }
    }
}
fn zoom(id: u64, delta: f32, x: f32, y: f32) {
    let allowed = STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            r.interactive
                && r.windows
                    .get(&id)
                    .is_some_and(|p| p.phase == Phase::Visible)
        })
    });
    if allowed
        && delta != 0.
        && !crate::annotation::pin_editor::current().is_some_and(|p| p.0 == id)
    {
        fit(id, Some((if delta > 0. { 1.1 } else { 1. / 1.1 }, x, y)));
    }
}
fn fit(id: u64, zoom: Option<(f32, f32, f32)>) {
    let value = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        if p.phase == Phase::Closing {
            return None;
        }
        Some((
            p.window.get()?.clone_strong(),
            r.lifecycle.clone(),
            p.image.bounds,
            p.phase,
            p.prepared_inset,
        ))
    });
    let Some((window, lifecycle, image, phase, prepared_inset)) = value else {
        return;
    };
    let pos = window.window().position();
    let size = window.window().size();
    let scale = native_scale(window.window());
    let margin = (window.get_frame_inset() * scale).round() as i32;
    let previous = if phase == Phase::Preparing {
        prepared_inset
    } else {
        margin
    };
    // A glow can cross a monitor seam; choose the work area from the image itself.
    let Some(area) = (lifecycle.popup_work_area)(lexift_core::domain::geometry::Point {
        x: pos.x + previous + 1,
        y: pos.y + previous + 1,
    }) else {
        return;
    };
    let inner_pos = slint::PhysicalPosition::new(pos.x + previous, pos.y + previous);
    let inner_size = slint::PhysicalSize::new(
        size.width.saturating_sub(2 * previous as u32),
        size.height.saturating_sub(2 * previous as u32),
    );
    let area = lexift_core::domain::geometry::Rect {
        left: area.left + margin,
        top: area.top + margin,
        right: area.right - margin,
        bottom: area.bottom - margin,
    };
    let zoom = zoom.map(|(factor, x, y)| {
        (
            factor,
            x - window.get_frame_inset(),
            y - window.get_frame_inset(),
        )
    });
    let (size, position) = placement(inner_pos, inner_size, image, area, scale, zoom);
    window.window().set_size(slint::PhysicalSize::new(
        size.width + 2 * margin as u32,
        size.height + 2 * margin as u32,
    ));
    window.window().set_position(slint::PhysicalPosition::new(
        position.x - margin,
        position.y - margin,
    ));
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut().and_then(|r| r.windows.get_mut(&id)) {
            p.prepared_inset = margin;
        }
    });
    apply_policy(id);
}
/// Preserve the physical mouse anchor while fitting an aspect-preserving result to its work area.
fn placement(
    pos: slint::PhysicalPosition,
    size: slint::PhysicalSize,
    image: lexift_core::domain::geometry::Rect,
    area: lexift_core::domain::geometry::Rect,
    scale: f32,
    zoom: Option<(f32, f32, f32)>,
) -> (slint::PhysicalSize, slint::PhysicalPosition) {
    let (iw, ih) = (
        (image.right - image.left) as f32,
        (image.bottom - image.top) as f32,
    );
    let max = ((area.right - area.left - 16).max(1) as f32 / iw)
        .min((area.bottom - area.top - 16).max(1) as f32 / ih);
    let min = (32. * scale / iw.min(ih)).min(max);
    let current = (size.width as f32 / iw).max(f32::EPSILON);
    let factor = (current * zoom.map_or(1., |z| z.0)).clamp(min, max);
    let (w, h) = (
        (iw * factor).round().max(1.) as u32,
        (ih * factor).round().max(1.) as u32,
    );
    let (x, y) = zoom.map_or((0., 0.), |(_, x, y)| (x * scale, y * scale));
    let ratio = factor / current;
    let px = (pos.x as f32 + x * (1. - ratio)).round() as i32;
    let py = (pos.y as f32 + y * (1. - ratio)).round() as i32;
    (
        slint::PhysicalSize::new(w, h),
        slint::PhysicalPosition::new(
            px.clamp(area.left + 8, area.right - 8 - w as i32),
            py.clamp(area.top + 8, area.bottom - 8 - h as i32),
        ),
    )
}

#[cfg(debug_assertions)]
pub(crate) fn diagnostic_windows() -> Vec<(u64, PinnedScreenshotWindow)> {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| {
                r.windows
                    .iter()
                    .filter_map(|(id, p)| p.window.get().map(|w| (*id, w.clone_strong())))
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::{
        domain::{
            geometry::Rect,
            screenshot::{ScreenshotPhase, ScreenshotState},
        },
        ports::screen_capture::CapturedRegion,
    };
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::rc::Rc;
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    fn state(session: u64, operation: u64) -> ScreenshotState {
        ScreenshotState {
            session,
            operation,
            phase: ScreenshotPhase::Output,
            ..Default::default()
        }
    }
    thread_local! { static DRAGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    fn native_drag(window: &slint::Window) -> bool {
        // Native ReleaseCapture reenters the input bridge. Neither a Slint
        // pointer callback nor the pin registry may still be borrowed here.
        STATE.with(|s| {
            let _guard = s.borrow_mut();
        });
        DRAGS.set(DRAGS.get() + 1);
        window.dispatch_event(slint::platform::WindowEvent::PointerExited);
        window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(50., 50.),
            button: slint::platform::PointerEventButton::Left,
        });
        true
    }
    #[test]
    fn image_press_defers_native_drag_and_retirement_cancels_queued_work() {
        use slint::platform::{PointerEventButton, WindowEvent};
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            native_drag,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        init(|_| PassiveWindowPreparation::Ready, lifecycle);
        crate::annotation::screenshot::sync(&state(1, 1));
        let image = || {
            Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 200,
                    bottom: 100,
                },
                pixels: [220, 180, 100, 255].repeat(200 * 100),
            })
        };
        show(1, 1, image());
        let window = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[&1]
                .window
                .get()
                .unwrap()
                .clone_strong()
        });
        window.window().dispatch_event(WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(50., 50.),
            button: PointerEventButton::Left,
        });
        assert_eq!(
            DRAGS.get(),
            0,
            "native drag must not reenter the press callback"
        );
        slint::platform::update_timers_and_animations();
        assert_eq!(DRAGS.get(), 1);
        window.invoke_press_changed(true);
        window.invoke_drag_requested();
        close(1);
        show(1, 1, image());
        window.invoke_drag_requested();
        slint::platform::update_timers_and_animations();
        assert_eq!(DRAGS.get(), 1, "retired input must not drag either window");
        let second = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[&2]
                .window
                .get()
                .unwrap()
                .clone_strong()
        });
        second.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(50., 50.),
        });
        for pressed in [true, false] {
            let position = slint::LogicalPosition::new(184., 16.);
            second.window().dispatch_event(if pressed {
                WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                }
            } else {
                WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                }
            });
        }
        slint::platform::update_timers_and_animations();
        assert_eq!(DRAGS.get(), 1, "a released click must cancel queued drag");
        assert!(
            active(),
            "the former close corner must no longer close a pin"
        );
        second.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        slint::platform::update_timers_and_animations();
        assert!(!active());
        crate::annotation::screenshot::shutdown();
    }
    #[test]
    fn three_pins_keep_gestures_close_tasks_and_late_callbacks_independent() {
        use crate::PopupPointerInput as Input;
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let sinks = Rc::new(RefCell::new(Vec::<crate::PopupPointerSink>::new()));
        let received = sinks.clone();
        let retired = Rc::new(Cell::new(0));
        let cleaned = retired.clone();
        let lifecycle = WindowLifecycleCallbacks::new(
            move |_, sink| {
                received.borrow_mut().push(sink);
                true
            },
            |_| true,
            native_drag,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        )
        .with_window_interaction_cancellation(move |_| {
            STATE.with(|s| {
                let _guard = s.borrow_mut();
            });
            cleaned.set(cleaned.get() + 1);
            true
        });
        init(|_| PassiveWindowPreparation::Ready, lifecycle);
        let mut refs = vec![];
        for session in 1..=3 {
            crate::annotation::screenshot::sync(&state(session, 1));
            let image = Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 200,
                    bottom: 100,
                },
                pixels: [220, 180, 100, 255].repeat(200 * 100),
            });
            refs.push(Arc::downgrade(&image));
            show(session, 1, image);
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        let emitted = events.clone();
        crate::annotation::set_handler(Rc::new(move |event| emitted.borrow_mut().push(event)));
        let first = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[&1]
                .window
                .get()
                .unwrap()
                .clone_strong()
        });
        assert!(first.get_shadow_enabled() && first.get_topmost());
        first.set_shadow_enabled(false);
        first.invoke_option_requested(0, false);
        first.set_topmost(false);
        first.invoke_option_requested(1, false);
        STATE.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert!(!r.windows[&1].shadow);
            assert!(r.windows[&2].shadow && r.windows[&2].topmost);
        });
        set_annotation_interaction(false);
        let old_size = first.window().size();
        zoom(1, 120., 50., 50.);
        assert_eq!(first.window().size(), old_size);
        first.invoke_output_requested(0);
        assert!(events.borrow().is_empty());
        first.set_topmost(true);
        first.invoke_option_requested(1, true);
        assert!(
            !first.get_topmost(),
            "stale menu action cannot change a drawing-mode pin"
        );
        set_annotation_interaction(true);
        first.invoke_output_requested(0);
        assert!(matches!(
            events.borrow().last(),
            Some(lexift_core::AppEvent::PinnedImage(
                lexift_core::domain::pinned_image::Event::Requested { id: 1, .. }
            ))
        ));
        let mut states = lexift_core::domain::pinned_image::States::new();
        states.insert(
            1,
            lexift_core::domain::pinned_image::State {
                operation: 1,
                busy: true,
                error: None,
                ..Default::default()
            },
        );
        sync_outputs(&states);
        let count = events.borrow().len();
        first.invoke_output_requested(1);
        assert_eq!(
            events.borrow().len(),
            count,
            "busy pin must reject repeat output"
        );
        states.get_mut(&1).unwrap().busy = false;
        sync_outputs(&states);
        first.set_topmost(true);
        first.invoke_option_requested(1, true);
        let interaction_cancels = retired.get();
        let sinks = sinks.borrow().clone();
        // New output identity must not expire already-visible older windows.
        crate::annotation::screenshot::sync(&ScreenshotState {
            session: 4,
            ..Default::default()
        });
        for id in 1..=3 {
            prepare(id, 0);
        }
        assert_eq!(
            STATE.with(|s| s.borrow().as_ref().unwrap().windows.len()),
            3
        );
        let press = |i: usize| {
            sinks[i](Input::Moved { x: 50., y: 50. });
            sinks[i](Input::LeftPressed { x: 50., y: 50. });
        };
        let release = |i: usize| sinks[i](Input::LeftReleased { x: 50., y: 50. });
        press(0);
        release(0);
        slint::platform::update_timers_and_animations();
        assert_eq!(DRAGS.get(), 0, "released gesture cannot start later");
        press(0);
        sinks[0](Input::LeftCancelled { x: 50., y: 50. });
        press(1);
        release(0);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            DRAGS.get(),
            1,
            "A cancellation/up must not move A or cancel B"
        );
        release(1);
        for _ in 0..10 {
            for index in [0, 2, 1] {
                press(index);
                slint::platform::update_timers_and_animations();
                release(index);
                zoom(index as u64 + 1, 120., 50., 50.);
            }
        }
        assert_eq!(DRAGS.get(), 31);
        sinks[0](Input::Moved { x: 184., y: 16. });
        sinks[0](Input::LeftPressed { x: 184., y: 16. });
        sinks[0](Input::LeftCancelled { x: 184., y: 16. });
        slint::platform::update_timers_and_animations();
        assert_eq!(
            STATE.with(|s| s.borrow().as_ref().unwrap().windows.len()),
            3,
            "cancelled close press must not close a window"
        );
        press(0);
        request_close(1);
        request_close(1);
        slint::platform::update_timers_and_animations();
        assert_eq!(DRAGS.get(), 31);
        assert_eq!(retired.get(), interaction_cancels + 1);
        assert!(refs[0].upgrade().is_none());
        press(0);
        slint::platform::update_timers_and_animations();
        assert_eq!(DRAGS.get(), 31, "retired native sink is inert");
        press(1);
        slint::platform::update_timers_and_animations();
        release(1);
        assert_eq!(DRAGS.get(), 32);
        request_close(2);
        slint::platform::update_timers_and_animations();
        assert!(refs[1].upgrade().is_none());
        assert!(active());
        close_all();
        assert_eq!(retired.get(), interaction_cancels + 3);
        assert!(refs[2].upgrade().is_none());
        assert!(!active());
        crate::annotation::screenshot::shutdown();
    }
    #[test]
    fn pins_are_independent_keep_aspect_ratio_and_reject_expired_preparation() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        lifecycle.popup_work_area = Rc::new(|_| {
            Some(Rect {
                left: -500,
                top: -300,
                right: 500,
                bottom: 500,
            })
        });
        let image = || {
            Arc::new(CapturedRegion {
                bounds: Rect {
                    left: -100,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                pixels: [220, 180, 100, 255].repeat(200 * 100),
            })
        };
        init(|_| PassiveWindowPreparation::Ready, lifecycle.clone());
        crate::annotation::screenshot::sync(&state(1, 1));
        let first = image();
        let weak = Arc::downgrade(&first);
        show(1, 1, first);
        crate::annotation::screenshot::sync(&state(2, 1));
        show(2, 1, image());
        assert_eq!(
            STATE.with(|s| s.borrow().as_ref().unwrap().windows.len()),
            2
        );
        zoom(1, 120., 100., 50.);
        STATE.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            let window = r.windows[&1].window.get().unwrap();
            let size = window.window().size();
            let margin = (window.get_frame_inset() * window.window().scale_factor()).round() as u32;
            assert_eq!(size.width - 2 * margin, 2 * (size.height - 2 * margin));
        });
        close(1);
        assert!(weak.upgrade().is_none());
        assert!(active());
        close_all();
        assert!(!active());
        init(|_| PassiveWindowPreparation::Pending, lifecycle);
        crate::annotation::screenshot::sync(&state(3, 1));
        show(3, 1, image());
        crate::annotation::screenshot::sync(&ScreenshotState {
            session: 3,
            ..Default::default()
        });
        let pending = STATE.with(|s| s.borrow().as_ref().unwrap().next);
        prepare(pending, 1);
        assert!(!active(), "late preparation must destroy an unshown pin");
        show(3, 1, image());
        assert!(!active(), "late output must not create a new window");
        crate::annotation::screenshot::shutdown();
    }
}

#[cfg(test)]
#[test]
fn zoom_keeps_the_mouse_anchor_and_clamps_negative_work_areas() {
    use lexift_core::domain::geometry::Rect;
    let image = Rect {
        left: -100,
        top: 0,
        right: 100,
        bottom: 100,
    };
    let area = Rect {
        left: -500,
        top: -300,
        right: 500,
        bottom: 500,
    };
    let (size, position) = placement(
        slint::PhysicalPosition::new(-100, 0),
        slint::PhysicalSize::new(200, 100),
        image,
        area,
        1.25,
        Some((1.1, 80., 40.)),
    );
    assert_eq!(size, slint::PhysicalSize::new(220, 110));
    assert_eq!(position, slint::PhysicalPosition::new(-110, -5));
    let (size, position) = placement(
        slint::PhysicalPosition::new(-490, -290),
        size,
        image,
        area,
        1.25,
        Some((100., 0., 0.)),
    );
    assert!(position.x >= area.left + 8 && position.y >= area.top + 8);
    assert!(position.x + size.width as i32 <= area.right - 8);
    assert_eq!(size.width, 2 * size.height);
}

#[cfg(test)]
#[test]
fn pin_glow_without_hover_border_renders_at_both_scales_and_themes() {
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType},
    };
    use std::rc::Rc;
    struct RenderPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for RenderPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(window.clone());
            Ok(window)
        }
    }
    let captured = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(RenderPlatform(captured.clone()))).unwrap();
    let pin = crate::PinnedScreenshotWindow::new().unwrap();
    let adapter = captured.borrow().as_ref().unwrap().clone();
    let rgba = (0..200 * 100)
        .flat_map(|i| {
            if (i % 200) / 20 % 2 == (i / 200) / 20 % 2 {
                [195, 220, 240, 255]
            } else {
                [235, 240, 245, 255]
            }
        })
        .collect::<Vec<u8>>();
    pin.set_picture(slint::Image::from_rgba8_premultiplied(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, 200, 100),
    ));
    for dark in [false, true] {
        for scale in [1., 1.25] {
            pin.global::<crate::Colors>()
                .set_preference(i32::from(dark));
            adapter
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged {
                    scale_factor: scale,
                });
            for (logical_width, logical_height) in [(216., 116.), (96., 256.)] {
                let width = (logical_width * scale) as u32;
                let height = (logical_height * scale) as u32;
                adapter.set_size(slint::PhysicalSize::new(width, height));
                for shadow in [true, false] {
                    pin.set_shadow_enabled(shadow);
                    let mut previous_pixels = None;
                    for hovered in [false, true, false] {
                        adapter.window().dispatch_event(if hovered {
                            WindowEvent::PointerMoved {
                                position: slint::LogicalPosition::new(
                                    logical_width / 2.,
                                    logical_height / 2.,
                                ),
                            }
                        } else {
                            WindowEvent::PointerExited
                        });
                        let mut pixels =
                            vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                        adapter.request_redraw();
                        adapter.draw_if_needed(|renderer| {
                            renderer.render(&mut pixels, width as usize);
                        });
                        if let Some(previous) = &previous_pixels {
                            assert_eq!(&pixels, previous, "hover must not add any decoration");
                        }
                        // Image boundaries must retain source pixels, without a blue outline.
                        let left = (8. * scale) as u32;
                        let right = width - left - 1;
                        for x in [left, right] {
                            let content = pixels[(height / 2 * width + x) as usize];
                            assert!(
                                content.r >= 195,
                                "image edge must not be covered by a border"
                            );
                        }
                        let edge = pixels[(height / 2 * width + (4. * scale) as u32) as usize];
                        if shadow {
                            assert!(
                                edge.b > edge.r && edge.b > 0,
                                "blue glow must actually render in software"
                            );
                            let falloff = (0..left)
                                .map(|x| pixels[(height / 2 * width + x) as usize].b)
                                .collect::<Vec<_>>();
                            assert_eq!(falloff[0], 0, "outer edge must fade to transparent");
                            assert!(
                                falloff.windows(2).all(|p| p[0] <= p[1]),
                                "glow must fade continuously, without concentric ridges"
                            );
                            assert!(
                                falloff[left as usize - 1] > falloff[2],
                                "glow must become brighter toward the image"
                            );
                        } else {
                            assert_eq!(
                                [edge.r, edge.g, edge.b],
                                [0, 0, 0],
                                "disabled glow must disappear"
                            );
                        }
                        if let Ok(directory) = std::env::var("LEXIFT_SCREENSHOT_SNAPSHOT_DIR") {
                            crate::i18n::write_test_bitmap(
                                &directory,
                                &format!(
                                    "pin-shadow-{shadow}-{dark}-{scale}-{width}x{height}-hover-{hovered}"
                                ),
                                width,
                                height,
                                pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                            );
                            let mut transparent =
                                vec![PremultipliedRgbaColor::default(); (width * height) as usize];
                            adapter.request_redraw();
                            adapter.draw_if_needed(|renderer| {
                                renderer.render(&mut transparent, width as usize);
                            });
                            for background in [24u8, 245] {
                                crate::i18n::write_test_bitmap(
                                    &directory,
                                    &format!(
                                        "pin-composite-{shadow}-{dark}-{scale}-{width}x{height}-{background}-hover-{hovered}"
                                    ),
                                    width,
                                    height,
                                    transparent.iter().map(|p| {
                                        let composite = |channel: u8| {
                                            (u16::from(channel)
                                                + u16::from(background)
                                                    * (255 - u16::from(p.alpha))
                                                    / 255)
                                                as u8
                                        };
                                        [
                                            composite(p.red),
                                            composite(p.green),
                                            composite(p.blue),
                                            255,
                                        ]
                                    }),
                                );
                            }
                        }
                        previous_pixels = Some(pixels);
                    }
                }
            }
        }
    }
}
