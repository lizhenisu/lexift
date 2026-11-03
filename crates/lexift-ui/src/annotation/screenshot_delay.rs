//! Session-owned countdown and the screenshot button's transient duration menu.
use super::*;
use crate::bridge::{PopupPointerInput, dispatch_pointer_input};
use crate::{AnnotationChoiceWindow, ScreenshotCountdownWindow};
type Ready = Box<dyn FnOnce(lexift_core::Result<std::time::Instant>) + Send>;
#[derive(Default)]
struct Views {
    recording: Option<(lexift_core::domain::screen_recording::Identity, u64)>,
    menu_kind: u8,
    sequence: u64,
    menu: Option<AnnotationChoiceWindow>,
    countdown: Option<ScreenshotCountdownWindow>,
    session: u64,
    deadline: Option<Instant>,
    timer: slint::Timer,
    ready: Option<Ready>,
}
thread_local! { static VIEWS: RefCell<Views> = RefCell::new(Views::default()); }
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
fn current(id: u64) -> bool {
    FLOW.with(|s| {
        let s = s.borrow();
        s.state.session == id && s.state.phase == ScreenshotPhase::CountingDown
    })
}
pub(crate) fn sync(state: &ScreenshotState) {
    let stale = VIEWS.with(|s| {
        s.borrow().countdown.is_some()
            && (s.borrow().session != state.session
                || !matches!(
                    state.phase,
                    ScreenshotPhase::CountingDown | ScreenshotPhase::Preparing
                ))
    });
    let menu_stale = VIEWS.with(|s| {
        let s = s.borrow();
        if matches!(s.menu_kind, 3..=5) {
            s.menu.is_some()
                && super::recording::audio_selection().map(|(id, revision, _)| (id, revision))
                    != s.recording
        } else {
            s.menu.is_some() && s.menu_kind != 2 && state.active()
        }
    });
    if stale || menu_stale {
        clear();
    }
}
pub(crate) fn clear() {
    let old = VIEWS.with(|slot| {
        let mut s = slot.borrow_mut();
        s.sequence = s.sequence.wrapping_add(1);
        s.timer.stop();
        s.deadline = None;
        s.ready.take();
        (s.menu.take(), s.countdown.take())
    });
    if let Some((l, _)) = services()
        && let Some(w) = old.0
    {
        (l.set_popup_dismissal)(w.window(), false);
        let _ = w.hide();
    }
    if let Some(w) = old.1 {
        let _ = w.hide();
    }
}
fn choose(sequence: u64, index: Option<i32>) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        if !VIEWS.with(|s| s.borrow().sequence == sequence && s.borrow().menu.is_some()) {
            return;
        }
        let kind = VIEWS.with(|s| s.borrow().menu_kind);
        let recording = VIEWS.with(|s| s.borrow().recording);
        clear();
        if kind == 5 {
            if let (Some((identity, revision)), Some(index)) = (recording, index) {
                super::recording::countdown::choose(identity, revision, index);
            }
            return;
        }
        if kind == 4 {
            if let (Some((identity, revision)), Some(index)) = (recording, index) {
                super::recording::configure_video(identity, revision, index);
            }
            return;
        }
        if kind == 3 {
            if let (Some((identity, revision)), Some(index)) = (recording, index) {
                super::recording::configure_audio(identity, revision, index);
            }
            return;
        }
        if kind == 2 {
            if let Some(index) = index {
                super::super::image_transform::apply(index);
            }
            return;
        }
        if kind == 1 {
            if let Some(target) = index.and_then(|i| {
                [
                    lexift_core::domain::image_import::Target::FileEditor,
                    lexift_core::domain::image_import::Target::ClipboardPin,
                ]
                .get(i as usize)
                .copied()
            }) {
                super::super::image_import::request(target);
            }
            return;
        }
        if let Some(seconds) = index.and_then(|i| [0, 3, 5, 10].get(i as usize).copied()) {
            super::request_after(seconds);
        }
    });
}
pub(crate) fn menu() {
    menu_kind(0);
}
pub(crate) fn import_menu() {
    menu_kind(1);
}
pub(in crate::annotation) fn transform_menu() {
    menu_kind(2);
}
pub(super) fn recording_delay_menu() {
    menu_kind(5);
}
pub(super) fn video_menu() {
    menu_kind(4);
}
pub(super) fn audio_menu() {
    menu_kind(3);
}
fn menu_kind(kind: u8) {
    if matches!(kind, 3..=5) {
        if super::recording::audio_selection().is_none() {
            return;
        }
    } else if kind == 2 {
        if !super::super::image_transform::available() {
            return;
        }
    } else if !(if kind == 1 {
        super::super::image_import::available()
    } else {
        available()
    }) || super::active()
        || crate::ocr::screen_busy()
        || super::history_editor::active()
        || pin_editor::active()
    {
        return;
    }
    clear();
    crate::bridge::cancel_idle_memory_trim();
    let Some((l, _)) = services() else { return };
    let Ok(w) = crate::screen_interaction::create(&l, AnnotationChoiceWindow::new) else {
        return;
    };
    crate::theme::apply(&w);
    let labels: &[&str] = if kind == 5 {
        &[
            "Start now",
            "Start after 3 seconds",
            "Start after 5 seconds",
            "Start after 10 seconds",
        ]
    } else if kind == 4 {
        &["30fps", "60fps", "Standard", "High quality"]
    } else if kind == 3 {
        &[
            "No audio",
            "System audio",
            "Microphone",
            "System audio and microphone",
        ]
    } else if kind == 2 {
        &[
            "Rotate left 90°",
            "Rotate right 90°",
            "Flip horizontally",
            "Flip vertically",
        ]
    } else if kind == 1 {
        &["Open image", "Pin image from clipboard"]
    } else {
        &[
            "Screenshot now",
            "Screenshot after 3 seconds",
            "Screenshot after 5 seconds",
            "Screenshot after 10 seconds",
        ]
    };
    w.set_options(slint::ModelRc::new(slint::VecModel::from(
        labels
            .iter()
            .map(|s| crate::i18n::tr(s).into())
            .collect::<Vec<_>>(),
    )));
    let selection = matches!(kind, 3..=5)
        .then(super::recording::audio_selection)
        .flatten();
    if kind == 4 {
        let Some((_, _, video)) = super::recording::video_selection() else {
            return;
        };
        w.set_two_choice_groups(true);
        w.set_first_group_label(crate::i18n::tr("Frame rate").into());
        w.set_second_group_label(crate::i18n::tr("Video quality").into());
        w.set_checked_first(
            if video.frame_rate == lexift_core::domain::screen_recording::FrameRate::Fps30 {
                0
            } else {
                1
            },
        );
        w.set_checked_second(
            if video.quality == lexift_core::domain::screen_recording::Quality::Standard {
                2
            } else {
                3
            },
        );
    }
    w.set_selected(if kind == 5 {
        0
    } else if kind == 4 {
        w.get_checked_first()
    } else {
        selection.map(|(_, _, index)| index as i32).unwrap_or(0)
    });
    w.set_option_measurement(
        labels
            .iter()
            .map(|s| crate::i18n::tr(s))
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
    );
    let seq = VIEWS.with(|s| {
        let mut s = s.borrow_mut();
        s.sequence += 1;
        s.menu_kind = kind;
        s.recording = selection.map(|(identity, revision, _)| (identity, revision));
        s.menu = Some(w.clone_strong());
        s.sequence
    });
    w.on_chosen(move |i| choose(seq, Some(i)));
    w.on_cancel(move || choose(seq, None));
    prepare_menu(seq, 0);
}
fn prepare_menu(seq: u64, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            let Some((l, prepare)) = services() else {
                return;
            };
            let window = VIEWS.with(|s| {
                let s = s.borrow();
                (s.sequence == seq)
                    .then(|| s.menu.as_ref().map(ComponentHandle::clone_strong))
                    .flatten()
            });
            let Some(w) = window else { return };
            match prepare(w.window()) {
                PassiveWindowPreparation::Pending if attempt < 20 => {
                    prepare_menu(seq, attempt + 1);
                    return;
                }
                PassiveWindowPreparation::Ready => {}
                _ => {
                    choose(seq, None);
                    return;
                }
            }
            let anchor = (l.toolbar_cursor_position)().unwrap_or_default();
            let Some(area) = (l.popup_work_area)(anchor) else {
                choose(seq, None);
                return;
            };
            let dpi = w.window().scale_factor();
            let width =
                (w.get_options_width().max(240.) * dpi).min((area.right - area.left) as f32) as u32;
            let height = (w.get_menu_height() * dpi) as u32;
            w.set_content_width(width as f32 / dpi);
            w.window().set_size(slint::PhysicalSize::new(width, height));
            w.window().set_position(slint::PhysicalPosition::new(
                anchor
                    .x
                    .clamp(area.left, (area.right - width as i32).max(area.left)),
                (anchor.y + 12).clamp(area.top, (area.bottom - height as i32).max(area.top)),
            ));
            let weak = w.as_weak();
            let sink = Rc::new(move |e| {
                if matches!(e, PopupPointerInput::DismissRequested { .. }) {
                    choose(seq, None)
                } else if let Some(w) = weak.upgrade() {
                    dispatch_pointer_input(w.window(), e);
                }
            });
            if !(l.configure_menu_window)(w.window())
                || w.show().is_err()
                || !(l.complete_passive_window_show)(w.window(), sink)
                || !(l.set_popup_dismissal)(w.window(), true)
            {
                choose(seq, None);
                return;
            }
            (l.activate_user_requested_window)(w.window());
        },
    );
}
pub(crate) fn begin(id: u64, seconds: u8, ready: Ready) {
    clear();
    crate::bridge::cancel_idle_memory_trim();
    if !current(id) || !FLOW.with(|s| s.borrow().recovery.is_some()) {
        ready(Err(lexift_core::Error::new(
            "Screenshot suspension expired",
        )));
        return;
    }
    let Some((l, _)) = services() else {
        ready(Err(lexift_core::Error::new("Countdown unavailable")));
        return;
    };
    let Ok(w) = crate::screen_interaction::create(&l, ScreenshotCountdownWindow::new) else {
        ready(Err(lexift_core::Error::new("Countdown unavailable")));
        return;
    };
    crate::theme::apply(&w);
    w.set_seconds(i32::from(seconds));
    let anchor = (l.toolbar_cursor_position)().unwrap_or_default();
    let Some(area) = (l.popup_work_area)(anchor) else {
        ready(Err(lexift_core::Error::new(
            "Countdown work area unavailable",
        )));
        return;
    };
    w.window()
        .set_position(slint::PhysicalPosition::new(area.left, area.top));
    VIEWS.with(|s| {
        let mut s = s.borrow_mut();
        s.session = id;
        s.countdown = Some(w.clone_strong());
        s.ready = Some(ready);
    });
    w.on_cancel(move || {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if current(id) {
                emit(ScreenshotEvent::Cancelled { session: id });
            }
        });
    });
    prepare_countdown(id, seconds, area, 0);
}
fn prepare_countdown(id: u64, seconds: u8, area: Rect, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            if !current(id) {
                return;
            }
            let Some((l, prepare)) = services() else {
                return;
            };
            let Some(w) = VIEWS.with(|s| {
                s.borrow()
                    .countdown
                    .as_ref()
                    .map(ComponentHandle::clone_strong)
            }) else {
                return;
            };
            match prepare(w.window()) {
                PassiveWindowPreparation::Pending if attempt < 20 => {
                    prepare_countdown(id, seconds, area, attempt + 1);
                    return;
                }
                PassiveWindowPreparation::Ready => {}
                _ => {
                    fail(id);
                    return;
                }
            }
            let dpi = w.window().scale_factor();
            for _ in 0..2 {
                w.window().set_size(slint::LogicalSize::new(
                    w.get_content_width()
                        .min((area.right - area.left) as f32 / dpi),
                    48.,
                ));
                let _ = w.window().take_snapshot();
            }
            let size = slint::PhysicalSize::new(
                (w.get_content_width()
                    .min((area.right - area.left) as f32 / dpi)
                    * dpi)
                    .ceil() as u32,
                (48. * dpi).ceil() as u32,
            );
            w.window().set_size(size);
            w.window().set_position(slint::PhysicalPosition::new(
                area.left + (area.right - area.left - size.width as i32) / 2,
                (area.bottom - size.height as i32 - (16. * dpi) as i32).max(area.top),
            ));
            let weak = w.as_weak();
            let sink = Rc::new(move |e| {
                if let Some(w) = weak.upgrade() {
                    dispatch_pointer_input(w.window(), e);
                }
            });
            if !(l.configure_countdown_window)(w.window())
                || w.show().is_err()
                || !(l.complete_passive_window_show)(w.window(), sink)
                || !(l.configure_countdown_window)(w.window())
            {
                fail(id);
                return;
            }
            let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
            tracing::debug!(
                session = id,
                seconds,
                "Screenshot countdown started after suspension"
            );
            let ready = VIEWS.with(|s| {
                let mut s = s.borrow_mut();
                s.deadline = Some(deadline);
                s.timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(50),
                    move || tick(id),
                );
                s.ready.take()
            });
            if let Some(ready) = ready {
                ready(Ok(deadline));
            }
        },
    );
}
fn tick(id: u64) {
    if !current(id) {
        let handing_off = FLOW.with(|s| {
            let s = s.borrow();
            s.state.session == id && s.state.phase == ScreenshotPhase::Preparing
        });
        if handing_off {
            VIEWS.with(|s| s.borrow().timer.stop());
        } else {
            clear();
        }
        return;
    }
    VIEWS.with(|s| {
        let s = s.borrow();
        if let (Some(w), Some(end)) = (&s.countdown, s.deadline) {
            w.set_seconds(remaining_seconds(end, Instant::now()));
        }
    });
}
fn fail(id: u64) {
    clear();
    emit(ScreenshotEvent::Failed {
        session: id,
        error: crate::i18n::tr("Screenshot unavailable"),
    });
}
/// Rebase activation only at the explicit capture deadline, then retire feedback.
pub(crate) fn finish(id: u64) {
    let owned = VIEWS.with(|s| s.borrow().session == id && s.borrow().countdown.is_some());
    clear();
    if owned {
        tracing::debug!(session = id, "Screenshot countdown retired before capture");
        if let Some((l, _)) = services() {
            FLOW.with(|s| {
                s.borrow_mut().activation = l.annotation_foreground.as_ref().map(|q| q())
            });
        }
    }
}

/// Project a monotonic deadline; delayed UI ticks never extend the capture wait.
fn remaining_seconds(deadline: Instant, now: Instant) -> i32 {
    deadline.saturating_duration_since(now).as_secs_f64().ceil() as i32
}
#[cfg(test)]
#[path = "screenshot_delay_tests.rs"]
mod tests;

pub(crate) fn active() -> bool {
    VIEWS.with(|s| {
        let s = s.borrow();
        s.menu.is_some() || s.countdown.is_some()
    })
}
/// Called under the annotation registry borrow; never reenter Registry here.
pub(crate) fn retire_menu(l: &WindowLifecycleCallbacks) {
    let menu = VIEWS.with(|s| {
        let mut s = s.borrow_mut();
        s.sequence += 1;
        s.menu.take()
    });
    if let Some(w) = menu {
        (l.set_popup_dismissal)(w.window(), false);
        let _ = w.hide();
    }
}
thread_local! { static AVAILABLE: std::cell::Cell<bool> = const {std::cell::Cell::new(true)}; }
pub(super) fn available() -> bool {
    AVAILABLE.with(|v| v.get())
}
pub(crate) fn availability(state: &lexift_core::AppState) {
    let allowed = lexift_core::domain::screenshot::delay_available(state);
    AVAILABLE.with(|v| v.set(allowed));
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow().as_ref()
            && let Some(w) = r.toolbar_state.main.get()
        {
            w.set_delay_available(allowed);
        }
    });
    if !allowed && VIEWS.with(|s| s.borrow().menu.is_some()) {
        clear();
    }
}
