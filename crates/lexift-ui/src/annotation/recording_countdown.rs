//! UI-only countdown ownership. The domain and App own transitions and deadlines.
use super::*;
use crate::ScreenshotCountdownWindow;
#[derive(Default)]
struct Countdown {
    sequence: u64,
    identity: Option<Identity>,
    window: Option<ScreenshotCountdownWindow>,
    timer: slint::Timer,
    deadline: Option<Instant>,
}
thread_local! {static COUNTDOWN:RefCell<Countdown>=RefCell::new(Countdown::default());}
pub(super) fn available() -> bool {
    current().is_some_and(|s| s.lifecycle.phase == Phase::Ready && s.lifecycle.audio_ready)
        && REGISTRY.with(|r| {
            r.borrow().as_ref().is_some_and(|r| {
                r.gesture.is_none()
                    && r.toolbar_state
                        .panel
                        .get()
                        .is_none_or(|p| !p.get_parameter_input_active())
            })
        })
}
pub(in crate::annotation) fn choose(id: Identity, revision: u64, index: i32) {
    if !available() || audio_selection().is_none_or(|(i, v, _)| i != id || v != revision) {
        return;
    }
    let Some(seconds) = [0, 3, 5, 10].get(index as usize).copied() else {
        return;
    };
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            finish_text_edit_registry(r, true);
            r.finish_style_edit();
            r.close_panel();
            r.hints.clear();
        }
    });
    input(if seconds == 0 {
        Input::Start
    } else {
        Input::StartAfter(seconds)
    });
}
pub(super) fn clear() {
    let window = COUNTDOWN.with(|s| {
        let mut s = s.borrow_mut();
        s.sequence += 1;
        s.timer.stop();
        s.identity = None;
        s.deadline = None;
        s.window.take()
    });
    if let Some(w) = window {
        let _ = w.hide();
    }
}
fn valid(sequence: u64) -> bool {
    COUNTDOWN.with(|s| {
        let s = s.borrow();
        s.sequence == sequence
            && s.identity.is_some_and(|id| {
                current().is_some_and(|c| {
                    c.lifecycle.identity.session == id.session
                        && c.lifecycle.identity.operation == id.operation
                        && matches!(
                            c.lifecycle.phase,
                            Phase::PreparingCountdown
                                | Phase::CountingDown
                                | Phase::RetiringCountdown
                        )
                })
            })
    })
}
fn fail(id: Identity) {
    emit_recording(Event::CountdownFailed {
        identity: id,
        error: Error::new("Countdown unavailable"),
    });
}
pub(super) fn begin(id: Identity, seconds: u8) {
    if current().is_none_or(|s| {
        s.lifecycle.identity != id || s.lifecycle.phase != Phase::PreparingCountdown
    }) {
        return;
    }
    clear();
    let prepared = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        if r.gesture.is_some() {
            return None;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        r.editor.clear();
        r.close_watermark_input();
        r.close_panel();
        r.hints.clear();
        let tokens = r
            .canvas_state
            .canvases
            .iter()
            .filter_map(|c| (r.lifecycle.annotation_window_token)(c.window.window()))
            .chain(
                r.toolbar_state
                    .main
                    .get()
                    .and_then(|w| (r.lifecycle.annotation_window_token)(w.window())),
            )
            .chain(VIEW.with(|v| {
                v.borrow()
                    .control
                    .get()
                    .and_then(|w| (r.lifecycle.annotation_window_token)(w.window()))
            }))
            .collect::<Vec<_>>();
        if !(r.lifecycle.countdown_handoff)(&tokens, true) {
            return None;
        }
        for c in &r.canvas_state.canvases {
            (r.lifecycle.cancel_window_interaction)(c.window.window());
            c.input().cancel_pending();
            let _ = c.window.window().hide();
        }
        if let Some(w) = r.toolbar_state.main.get() {
            let _ = w.hide();
        }
        VIEW.with(|v| {
            if let Some(w) = v.borrow().control.get() {
                let _ = w.hide();
            }
        });
        if !(r.lifecycle.countdown_handoff)(&tokens, false) {
            return None;
        }
        let state = current()?;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: state.monitor.left,
            y: state.monitor.top,
        })?;
        let w =
            crate::screen_interaction::create(&r.lifecycle, ScreenshotCountdownWindow::new).ok()?;
        Some((w, area))
    });
    let Some((w, area)) = prepared else {
        fail(id);
        return;
    };
    crate::theme::apply(&w);
    w.set_recording(true);
    w.set_seconds(i32::from(seconds));
    w.window()
        .set_position(slint::PhysicalPosition::new(area.left, area.top));
    let sequence = COUNTDOWN.with(|s| {
        let mut s = s.borrow_mut();
        s.identity = Some(id);
        s.window = Some(w.clone_strong());
        s.sequence
    });
    w.on_cancel(move || {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if valid(sequence) {
                input(Input::CountdownCancelled);
            }
        });
    });
    prepare(sequence, id, seconds, area, 0);
}
fn prepare(sequence: u64, id: Identity, seconds: u8, area: Rect, attempt: u8) {
    slint::Timer::single_shot(
        Duration::from_millis(if attempt == 0 { 0 } else { 16 }),
        move || {
            if !valid(sequence) {
                return;
            }
            let w = COUNTDOWN.with(|s| {
                s.borrow()
                    .window
                    .as_ref()
                    .map(ComponentHandle::clone_strong)
            });
            let Some(w) = w else { return };
            let services = REGISTRY.with(|r| {
                r.borrow()
                    .as_ref()
                    .map(|r| (r.lifecycle.clone(), r.prepare))
            });
            let Some((l, prepare)) = services else {
                fail(id);
                return;
            };
            match prepare(w.window()) {
                PassiveWindowPreparation::Pending if attempt < 20 => {
                    self::prepare(sequence, id, seconds, area, attempt + 1);
                    return;
                }
                PassiveWindowPreparation::Ready => {}
                _ => {
                    fail(id);
                    return;
                }
            }
            let dpi = w.window().scale_factor();
            w.window().set_size(slint::LogicalSize::new(
                w.get_content_width()
                    .min((area.right - area.left) as f32 / dpi),
                48.,
            ));
            let _ = w.window().take_snapshot();
            let width = (w
                .get_content_width()
                .min((area.right - area.left) as f32 / dpi)
                * dpi)
                .ceil() as u32;
            let height = (48. * dpi).ceil() as u32;
            w.window().set_size(slint::PhysicalSize::new(width, height));
            w.window().set_position(slint::PhysicalPosition::new(
                area.left + (area.right - area.left - width as i32) / 2,
                (area.bottom - height as i32 - (16. * dpi) as i32).max(area.top),
            ));
            let weak = w.as_weak();
            let sink = Rc::new(move |e| {
                if let Some(w) = weak.upgrade() {
                    crate::bridge::dispatch_pointer_input(w.window(), e);
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
            COUNTDOWN.with(|s| {
                let mut s = s.borrow_mut();
                s.deadline = Some(deadline);
                s.timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(50),
                    move || {
                        if !valid(sequence) {
                            return;
                        }
                        super::check_layout();
                        COUNTDOWN.with(|s| {
                            let s = s.borrow();
                            if let (Some(w), Some(end)) = (&s.window, s.deadline) {
                                w.set_seconds(
                                    end.saturating_duration_since(Instant::now())
                                        .as_secs_f64()
                                        .ceil() as i32,
                                );
                            }
                        });
                    },
                );
            });
            tracing::debug!(
                session = id.session,
                request = id.request,
                seconds,
                "Recording countdown ready"
            );
            emit_recording(Event::Input {
                identity: id,
                event: Input::CountdownPrepared(deadline),
            });
        },
    );
}
pub(super) fn retire(id: Identity, restore: bool) {
    if current().is_none_or(|s| {
        s.lifecycle.identity != id
            || s.lifecycle.phase
                != if restore {
                    Phase::RestoringReady
                } else {
                    Phase::RetiringCountdown
                }
    }) {
        return;
    }

    let window = COUNTDOWN.with(|s| {
        let mut s = s.borrow_mut();
        s.sequence += 1;
        s.timer.stop();
        s.window.take()
    });
    let okay = REGISTRY.with(|r| {
        let r = r.borrow();
        let Some(r) = r.as_ref() else { return false };
        if let Some(w) = &window {
            let Some(token) = (r.lifecycle.annotation_window_token)(w.window()) else {
                return false;
            };
            if !(r.lifecycle.countdown_handoff)(&[token], true) {
                return false;
            }
            if w.hide().is_err() || !(r.lifecycle.countdown_handoff)(&[token], false) {
                return false;
            }
        }
        true
    });
    drop(window);
    if !okay {
        if restore {
            failed(id, Error::new("Countdown unavailable"));
        } else {
            fail(id);
        }
        return;
    }
    let shown = REGISTRY.with(|r| {
        let r = r.borrow();
        let Some(r) = r.as_ref() else { return false };
        let mut okay = true;
        for c in &r.canvas_state.canvases {
            okay &= c.window.window().show().is_ok();
            okay &= r
                .lifecycle
                .recording_window
                .as_ref()
                .is_some_and(|f| f(c.window.window(), crate::RecordingWindowRole::Interaction));
        }
        if let Some(w) = r.toolbar_state.main.get() {
            okay &= w.show().is_ok();
            okay &= r
                .lifecycle
                .recording_window
                .as_ref()
                .is_some_and(|f| f(w.window(), crate::RecordingWindowRole::Control));
        }
        VIEW.with(|v| {
            if let Some(w) = v.borrow().control.get() {
                okay &= w.show().is_ok();
                okay &= r
                    .lifecycle
                    .recording_window
                    .as_ref()
                    .is_some_and(|f| f(w.window(), crate::RecordingWindowRole::Control));
            }
        });
        okay
    });
    if !shown {
        failed(id, Error::new("Recording preparation failed"));
        return;
    }
    tracing::debug!(
        session = id.session,
        request = id.request,
        restore,
        "Recording countdown feedback retired"
    );
    emit_recording(Event::Input {
        identity: id,
        event: if restore {
            Input::ReadyRestored
        } else {
            Input::CountdownRetired
        },
    });
}
