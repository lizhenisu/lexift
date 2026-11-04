//! Slint preview ownership and view-only native-surface placement.
use super::*;
use crate::{LongCaptureConfirmation, RecordingPreview};
use lexift_core::domain::recording_preview as p;
#[derive(Default)]
struct Views {
    window: window::Slot<RecordingPreview>,
    confirmation: window::Slot<LongCaptureConfirmation>,
    session: u64,
    operation: u64,
    last_bounds: Option<(p::Target, f32)>,
    last_status: Option<(p::Phase, Phase, bool)>,
}
thread_local! {static VIEWS:RefCell<Views>=RefCell::new(Views::default());}
type Services = (
    WindowLifecycleCallbacks,
    fn(&slint::Window) -> PassiveWindowPreparation,
);
fn services() -> Option<Services> {
    REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .map(|r| (r.lifecycle.clone(), r.prepare))
    })
}
fn state() -> Option<p::State> {
    current()?.preview
}
pub(super) fn input(event: p::Input) {
    if let Some(s) = state() {
        emit_recording(Event::PreviewInput {
            identity: s.identity,
            input: event,
        });
    }
}
fn fail(id: p::Identity) {
    emit_recording(Event::PreviewNotice {
        identity: id,
        notice: p::Notice::Failed("Recording preview unavailable".into()),
    });
}
fn valid(id: p::Identity) -> bool {
    state().is_some_and(|s| s.identity == id)
}
pub(super) fn clear() {
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.confirmation.clear();
        v.window.clear();
        v.last_bounds = None;
        v.last_status = None;
    });
}
pub(super) fn owner() -> Option<lexift_core::ports::magnifier::WindowToken> {
    let (l, _) = services()?;
    VIEWS.with(|v| {
        v.borrow()
            .window
            .get()
            .filter(|w| w.window().is_visible())
            .and_then(|w| (l.annotation_window_token)(w.window()))
    })
}
fn target(w: &RecordingPreview, l: &WindowLifecycleCallbacks) -> Option<p::Target> {
    let d = w.window().scale_factor();
    let x = (w.get_video_x() * d).round() as i32;
    let y = (w.get_video_y() * d).round() as i32;
    Some(p::Target {
        window: (l.annotation_window_token)(w.window())?,
        bounds: Rect {
            left: x,
            top: y,
            right: x + (w.get_video_width() * d).round() as i32,
            bottom: y + (w.get_video_height() * d).round() as i32,
        },
    })
}
fn update_surface() {
    let Some((l, _)) = services() else { return };
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        let Some(w) = v.window.get() else { return };
        let Some(t) = target(w, &l) else { return };
        let d = w.window().scale_factor();
        if v.last_bounds == Some((t, d)) {
            return;
        }
        if (l.recording_preview_surface)(t.window, t.bounds) {
            v.last_bounds = Some((t, d));
        }
    });
}
pub(super) fn execute(command: p::Command) {
    if !valid(command.identity) {
        return;
    }
    match command.action {
        p::Action::PrepareView => open(command.identity),
        p::Action::Handoff => show(command.identity),
        p::Action::Install => install(),
        p::Action::Save => save_request(),
        p::Action::ConfirmDiscard => confirm(command.identity),
        p::Action::DestroyView => clear(),
        _ => {}
    }
}
fn open(id: p::Identity) {
    let Some((l, _)) = services() else {
        fail(id);
        return;
    };
    let existing = VIEWS.with(|v| v.borrow().window.get().is_some());
    if existing {
        prepare(id, 0);
        return;
    }
    let Ok(w) = crate::screen_interaction::create(&l, RecordingPreview::new) else {
        fail(id);
        return;
    };
    crate::theme::apply(&w);
    let Some(s) = current() else { return };
    let Some(area) = (l.popup_work_area)(Point {
        x: s.monitor.left,
        y: s.monitor.top,
    }) else {
        fail(id);
        return;
    };
    w.window()
        .set_position(slint::PhysicalPosition::new(area.left, area.top));
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.session = id.session;
        v.operation = id.operation;
        v.window.open(w);
    });
    VIEWS.with(|v| {
        let v = v.borrow();
        let w = v.window.get().unwrap();
        let revision = v.window.revision();
        let work = v.window.input();
        w.on_action(move |action, value| {
            work.post(move || {
                if VIEWS.with(|v| v.borrow().window.revision() == revision) {
                    act(action, value)
                }
            })
        });
        let work = v.window.input();
        w.on_surface_changed(move || work.post(update_surface));
        let weak = w.as_weak();
        let begin = l.begin_window_drag;
        w.on_move_requested(move || {
            // The bridge posts the native move itself. A second UI queue hop
            // changes the cursor anchor or loses a short press before handoff.
            if let Some(w) = weak.upgrade() {
                let started = begin(w.window());
                tracing::debug!(started, "Recording preview title drag requested");
            }
        });
        let work = v.window.input();
        let weak = w.as_weak();
        let begin = l.begin_window_resize.clone();
        w.on_resize_requested(move |index| {
            let weak = weak.clone();
            let begin = begin.clone();
            work.post(move || {
                if let (Some(w), Some(edge)) = (
                    weak.upgrade(),
                    crate::bridge::PopupResizeEdge::from_index(index),
                ) {
                    begin(w.window(), edge, false);
                }
            })
        });
        let work = v.window.input();
        w.window().on_close_requested(move || {
            work.post(|| act(5, 0.));
            slint::CloseRequestResponse::KeepWindowShown
        });
        let work = v.window.input();
        (l.configure_window_paint_repair)(w.window(), Rc::new(move || work.post(update_surface)));
    });
    prepare(id, 0);
}
fn prepare(id: p::Identity, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            if !valid(id) {
                return;
            }
            let Some((l, prepare)) = services() else {
                fail(id);
                return;
            };
            let result = VIEWS.with(|v| {
                let v = v.borrow();
                let w = v.window.get()?;
                match prepare(w.window()) {
                    PassiveWindowPreparation::Ready => {
                        if !w.window().is_visible() {
                            let s = current()?;
                            let a = (l.popup_work_area)(Point {
                                x: s.monitor.left,
                                y: s.monitor.top,
                            })?;
                            let d = w.window().scale_factor();
                            let width = (960. * d).min((a.right - a.left) as f32 * 0.9);
                            let height = (760. * d).min((a.bottom - a.top) as f32 * 0.9);
                            w.window()
                                .set_size(slint::PhysicalSize::new(width as u32, height as u32));
                            w.window().set_position(slint::PhysicalPosition::new(
                                a.left + (a.right - a.left - width as i32) / 2,
                                a.top + (a.bottom - a.top - height as i32) / 2,
                            ));
                        }
                        if !(l.configure_menu_window)(w.window()) {
                            return Some(Err(()));
                        }
                        let _ = w.window().take_snapshot();
                        target(w, &l).map(Ok)
                    }
                    PassiveWindowPreparation::Pending if attempt < 20 => None,
                    _ => Some(Err(())),
                }
            });
            match result {
                Some(Ok(t)) => emit_recording(Event::PreviewInput {
                    identity: id,
                    input: p::Input::Target(t),
                }),
                Some(Err(())) => fail(id),
                None if attempt < 20 => self::prepare(id, attempt + 1),
                None => fail(id),
            }
        },
    );
}
fn show(id: p::Identity) {
    let Some((l, _)) = services() else {
        fail(id);
        return;
    };
    let okay = VIEWS.with(|v| {
        let v = v.borrow();
        let Some(w) = v.window.get() else {
            return false;
        };
        if w.show().is_err() {
            return false;
        }
        let weak = w.as_weak();
        let geometry = l.clone();
        let sink = v.window.input().pointer_sink(move |input| {
            if let Some(w) = weak.upgrade() {
                let size = match input {
                    PopupPointerInput::Resized { width, height } => Some((width, height)),
                    PopupPointerInput::NativeMoveFinished => {
                        let s = w.window().size();
                        Some((s.width as f32, s.height as f32))
                    }
                    _ => None,
                };
                if let Some((width, height)) = size {
                    sync_native_geometry(w.window(), &geometry, width, height);
                    update_surface();
                    return;
                }
                crate::bridge::dispatch_pointer_input(w.window(), input);
            }
        });
        (l.complete_passive_window_show)(w.window(), sink)
            && l.recording_window
                .as_ref()
                .is_some_and(|f| f(w.window(), crate::RecordingWindowRole::Control))
    });
    if !okay {
        fail(id);
        return;
    }
    VIEWS.with(|v| v.borrow_mut().last_bounds = None);
    update_surface();
    tracing::debug!(
        session = id.session,
        instance = id.instance,
        "Recording preview parent shown without activation"
    );
}
/// The native modal move/size loop reports physical geometry before Winit's
/// queued events. Install matching Slint geometry before accepting more input.
fn sync_native_geometry(w: &slint::Window, l: &WindowLifecycleCallbacks, width: f32, height: f32) {
    if width <= 0. || height <= 0. {
        return;
    }
    let p = w.position();
    let bounds = Rect {
        left: p.x,
        top: p.y,
        right: p.x + width as i32,
        bottom: p.y + height as i32,
    };
    let displays = (l.screenshot_displays)();
    let scale = displays
        .iter()
        .max_by_key(|(r, _)| {
            i64::from((r.right.min(bounds.right) - r.left.max(bounds.left)).max(0))
                * i64::from((r.bottom.min(bounds.bottom) - r.top.max(bounds.top)).max(0))
        })
        .map(|(_, s)| *s)
        .unwrap_or(w.scale_factor())
        .max(0.1);
    if (w.scale_factor() - scale).abs() > 0.001 {
        w.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
    }
    w.dispatch_event(slint::platform::WindowEvent::Resized {
        size: slint::LogicalSize::new(width / scale, height / scale),
    });
}
fn install() {
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.hints.clear();
            r.editor.clear();
            r.close_panel();
            if let Some(port) = &r.lifecycle.annotation_renderer {
                port.clear();
            }
            for c in &r.canvas_state.canvases {
                (r.lifecycle.cancel_window_interaction)(c.window.window());
                c.input().cancel_pending();
                let _ = c.window.window().hide();
            }
            if let Some(w) = r.toolbar_state.main.get() {
                let _ = w.hide();
            }
            r.toolbar_state.main.clear();
            r.canvas_state.canvases.clear();
        }
    });
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.control.clear();
        v.range.clear();
        v.timer.stop();
        v.document.take();
        v.geometry.clear();
        v.desktop.take();
    });
    tracing::debug!("Recording preview first frame presented; capture chrome retired");
}
fn act(action: i32, value: f32) {
    tracing::debug!(action, target = value, "Recording preview user input");
    let event = match action {
        0 => p::Input::PlayPause,
        1 => p::Input::Mute,
        2 => p::Input::BeginSeek,
        3 => {
            let Some(s) = state() else { return };
            p::Input::Seek(s.duration.mul_f64((f64::from(value) / 1000.).clamp(0., 1.)))
        }
        4 => p::Input::Save,
        5 => p::Input::Close,
        6 => p::Input::Retry,
        7 => p::Input::CancelSeek,
        _ => return,
    };
    input(event);
}
pub(super) fn sync(s: &lexift_core::domain::screenshot::recording::Session) {
    let Some(p) = &s.preview else { return };
    if p.phase == p::Phase::Failed && !p.confirming_discard() {
        VIEWS.with(|v| v.borrow_mut().confirmation.clear());
    }
    VIEWS.with(|v| {
        let v = v.borrow();
        let Some(w) = v.window.get() else { return };
        if v.session != p.identity.session || v.operation != p.identity.operation {
            return;
        }
        w.set_ready(p.presented && p.phase != p::Phase::Failed);
        w.set_playing(p.phase == p::Phase::Playing);
        w.set_has_audio(p.has_audio);
        w.set_duration_seconds(p.duration.as_secs_f32());
        w.set_muted(p.muted);
        w.set_busy(
            s.lifecycle.phase == Phase::Saving
                || matches!(p.phase, p::Phase::Preparing | p::Phase::Retiring),
        );
        w.set_error(
            p.error
                .as_ref()
                .map(|_| crate::i18n::tr("Recording preview unavailable"))
                .unwrap_or_default()
                .into(),
        );
        if p.phase != p::Phase::Seeking {
            w.set_progress(if p.duration.is_zero() {
                0.
            } else {
                (p.position.as_secs_f64() / p.duration.as_secs_f64() * 1000.) as f32
            });
        }
        let format = |d: Duration| format!("{:02}:{:02}", d.as_secs() / 60, d.as_secs() % 60);
        w.set_time_label(format!("{} / {}", format(p.position), format(p.duration)).into());
        if let Some(a) = &s.artifact {
            w.set_metadata(
                format!(
                    "{} × {} px · {}fps",
                    a.width,
                    a.height,
                    a.video.frame_rate.get()
                )
                .into(),
            );
        }
    });
    VIEWS.with(|v|{let mut v=v.borrow_mut();let status=(p.phase,s.lifecycle.phase,p.muted);if v.last_status!=Some(status){v.last_status=Some(status);tracing::debug!(session=p.identity.session,instance=p.identity.instance,request=p.identity.request,phase=?p.phase,recording_phase=?s.lifecycle.phase,muted=p.muted,"Recording preview state installed");}});
}
fn confirm(id: p::Identity) {
    if VIEWS.with(|v| v.borrow().confirmation.get().is_some()) {
        return;
    }
    let Some((l, _)) = services() else { return };
    let Ok(w) = crate::screen_interaction::create(&l, LongCaptureConfirmation::new) else {
        input(p::Input::DiscardConfirmed(false));
        return;
    };
    crate::theme::apply(&w);
    w.set_minimum_content_width(400.);
    w.set_heading(crate::i18n::tr("Recording preview").into());
    w.set_message(crate::i18n::tr("Discard this unsaved recording?").into());
    w.set_confirm_label(crate::i18n::tr("Discard").into());
    let revision = VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.confirmation.open(w);
        let revision = v.confirmation.revision();
        let w = v.confirmation.get().unwrap();
        let work = v.confirmation.input();
        w.on_cancel(move || work.post(move || confirm_done(id, revision, false)));
        let work = v.confirmation.input();
        w.on_confirm(move || work.post(move || confirm_done(id, revision, true)));
        let work = v.confirmation.input();
        w.window().on_close_requested(move || {
            work.post(move || confirm_done(id, revision, false));
            slint::CloseRequestResponse::KeepWindowShown
        });
        revision
    });
    prepare_confirmation(id, revision, 0);
}
fn confirm_done(id: p::Identity, revision: u64, accept: bool) {
    if valid(id) && VIEWS.with(|v| v.borrow().confirmation.revision() == revision) {
        VIEWS.with(|v| v.borrow_mut().confirmation.clear());
        emit_recording(Event::PreviewInput {
            identity: id,
            input: p::Input::DiscardConfirmed(accept),
        });
    }
}
/// A failed hidden preview has no window. Its retained completion control is
/// still the user-owned origin for discard confirmation and focus protection.
fn confirmation_origin(
    l: &WindowLifecycleCallbacks,
) -> Option<(
    slint::PhysicalPosition,
    slint::PhysicalSize,
    f32,
    Option<lexift_core::ports::magnifier::WindowToken>,
)> {
    let geometry = |w: &slint::Window| {
        (
            w.position(),
            w.size(),
            w.scale_factor(),
            (l.annotation_window_token)(w),
        )
    };
    VIEWS
        .with(|v| {
            v.borrow()
                .window
                .get()
                .filter(|w| w.window().is_visible())
                .map(|w| geometry(w.window()))
        })
        .or_else(|| VIEW.with(|v| v.borrow().control.get().map(|w| geometry(w.window()))))
}
fn prepare_confirmation(id: p::Identity, revision: u64, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            if !valid(id) || !VIEWS.with(|v| v.borrow().confirmation.revision() == revision) {
                return;
            }
            let Some((l, prepare)) = services() else {
                return;
            };
            let ready = VIEWS.with(|v| {
                let v = v.borrow();
                let w = v.confirmation.get()?;
                let (pos, size, d, _) = confirmation_origin(&l)?;
                let _ = w.window().take_snapshot();
                let confirmation_size = slint::PhysicalSize::new(
                    (w.get_content_width() * d) as u32,
                    (w.get_content_height() * d) as u32,
                );
                w.window().set_size(confirmation_size);
                let s = confirmation_size;
                w.window().set_position(slint::PhysicalPosition::new(
                    pos.x + (size.width as i32 - s.width as i32) / 2,
                    pos.y + (size.height as i32 - s.height as i32) / 2,
                ));
                match prepare(w.window()) {
                    PassiveWindowPreparation::Ready => Some(true),
                    PassiveWindowPreparation::Pending if attempt < 20 => None,
                    _ => Some(false),
                }
            });
            match ready {
                None if attempt < 20 => prepare_confirmation(id, revision, attempt + 1),
                Some(true) => {
                    let shown = VIEWS.with(|v| {
                        let v = v.borrow();
                        let w = v.confirmation.get().unwrap();
                        let origin = confirmation_origin(&l).and_then(|(_, _, _, token)| token);
                        if !(l.configure_menu_window)(w.window()) || w.show().is_err() {
                            return false;
                        }
                        let weak = w.as_weak();
                        let sink = v.confirmation.input().pointer_sink(move |input| {
                            if let Some(w) = weak.upgrade() {
                                crate::bridge::dispatch_pointer_input(w.window(), input);
                            }
                        });
                        if !(l.complete_passive_window_show)(w.window(), sink) {
                            return false;
                        }
                        if origin.is_some()
                            && l.annotation_foreground.as_ref().and_then(|q| q()) == origin
                        {
                            (l.activate_user_requested_window)(w.window());
                        }
                        true
                    });
                    if !shown {
                        confirm_done(id, revision, false);
                    }
                }
                _ => confirm_done(id, revision, false),
            }
        },
    );
}
