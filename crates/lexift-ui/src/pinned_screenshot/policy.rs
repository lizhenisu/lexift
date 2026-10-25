//! Native policy installation and recovery, owned by one pin window instance.
use super::*;
use lexift_core::domain::pinned_image::{
    Event, States,
    interaction::{Policy, State as PolicyState},
};

pub(crate) fn recoverable() -> bool {
    STATE.with(|s| {
        s.borrow().as_ref().is_some_and(|r| {
            r.windows.values().any(|p| {
                p.interaction.applied.passthrough
                    || p.interaction.pending.is_some_and(|v| v.policy.passthrough)
            })
        })
    })
}
pub(crate) fn locked(id: u64) -> bool {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .and_then(|r| r.windows.get(&id))
            .is_some_and(|p| {
                p.interaction.applied.locked
                    || p.interaction.pending.is_some()
                    || p.window.get().is_some_and(|w| w.get_policy_busy())
            })
    })
}
fn project(w: &PinnedScreenshotWindow, state: &PolicyState) {
    w.set_locked(state.applied.locked);
    w.set_mouse_passthrough(state.applied.passthrough);
    w.set_policy_busy(state.pending.is_some());
}
pub(super) fn sync(states: &States) {
    let ids = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else {
            return (vec![], vec![]);
        };
        let mut changed = vec![];
        let mut failed = vec![];
        for (&id, p) in &mut r.windows {
            if let Some(state) = states.get(&id) {
                if state.interaction.serial < p.interaction.serial
                    || (state.interaction.serial == p.interaction.serial
                        && state.interaction.settled < p.interaction.settled)
                {
                    continue;
                }
                if p.interaction.failed != state.interaction.failed
                    && state.interaction.failed.is_some()
                    && p.window.get().is_some_and(|w| w.window().is_visible())
                {
                    failed.push(id);
                }
                let update = p.interaction.applied != state.interaction.applied
                    || (p.interaction.pending.is_some() && state.interaction.pending.is_none());
                p.interaction = state.interaction.clone();
                if let Some(w) = p.window.get() {
                    project(w, &p.interaction);
                }
                if update {
                    changed.push(id);
                }
            }
        }
        (changed, failed)
    });
    for id in ids.1 {
        show_feedback_details(id, "Operation failed. Try again.", Some(true), 0, None);
    }
    for id in ids.0 {
        apply_policy(id);
    }
    crate::tray_menu::refresh_pin_recovery();
}
pub(super) fn request(id: u64, locked: bool, passthrough: bool) {
    let allowed = STATE.with(|s| {
        let s = s.borrow();
        s.as_ref().is_some_and(|r| {
            r.interactive
                && r.windows.get(&id).is_some_and(|p| {
                    p.phase == Phase::Visible
                        && !p.native_drag
                        && !p.gesture.pressed.get()
                        && p.window.get().is_some_and(|w| {
                            !w.get_output_busy()
                                && !w.get_recognizing()
                                && !w.get_cropping()
                                && (!passthrough || !w.get_editing())
                        })
                        && p.interaction.pending.is_none()
                })
        })
    });
    if allowed && crate::annotation::pin_editor::prepare_interaction(id) {
        STATE.with(|s| {
            if let Some(w) = s
                .borrow()
                .as_ref()
                .and_then(|r| r.windows.get(&id))
                .and_then(|p| p.window.get())
            {
                w.set_policy_busy(true);
            }
        });
        crate::annotation::pin_editor::policy_requested(id);
        emit_pin(Event::InteractionRequested {
            id,
            policy: Policy {
                locked,
                passthrough,
            },
        });
    }
}
fn frame(w: &PinnedScreenshotWindow, lifecycle: &WindowLifecycleCallbacks, enabled: bool) -> bool {
    if enabled {
        let Ok(pixels) = w.window().take_snapshot() else {
            return false;
        };
        (lifecycle.pinned_passthrough_frame)(
            w.window(),
            Some((pixels.width(), pixels.height(), pixels.as_bytes())),
        )
    } else {
        let result = (lifecycle.pinned_passthrough_frame)(w.window(), None);
        w.window().request_redraw();
        result
    }
}
/// Coalesce native size/DPI notifications; no stationary refresh timer is retained.
pub(super) fn schedule_frame(id: u64) {
    let job = STATE.with(|s| {
        let Ok(mut s) = s.try_borrow_mut() else {
            return None;
        };
        let r = s.as_mut()?;
        let p = r.windows.get_mut(&id)?;
        let w = p.window.get()?;
        if p.frame_queued || !w.get_mouse_passthrough() || !w.window().is_visible() {
            return None;
        }
        p.frame_queued = true;
        Some((p.window.input(), p.window.revision()))
    });
    if let Some((input, instance)) = job {
        input.post(move || {
            let current = STATE.with(|s| {
                let mut s = s.borrow_mut();
                let r = s.as_mut()?;
                let p = r
                    .windows
                    .get_mut(&id)
                    .filter(|p| p.window.revision() == instance)?;
                p.frame_queued = false;
                let w = p.window.get()?;
                (w.get_mouse_passthrough() && w.window().is_visible())
                    .then(|| (w.clone_strong(), r.lifecycle.clone()))
            });
            if let Some((w, lifecycle)) = current
                && !frame(&w, &lifecycle, true)
            {
                show_feedback_details(id, "Operation failed. Try again.", Some(true), 0, None);
            }
        });
    }
}
pub(crate) fn apply(id: u64, request: u64, policy: Policy) {
    let context = STATE.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let p = r.windows.get(&id)?;
        if p.phase == Phase::Closing
            || !p
                .interaction
                .pending
                .is_some_and(|v| v.request == request && v.policy == policy)
        {
            return None;
        }
        Some((
            p.window.get()?.clone_strong(),
            p.window.revision(),
            p.interaction.applied,
            p.gesture.clone(),
            r.lifecycle.clone(),
        ))
    });
    let Some((w, instance, before, gesture, lifecycle)) = context else {
        return;
    };
    gesture.cancel();
    w.invoke_close_menu();
    (lifecycle.cancel_window_interaction)(w.window());
    clear_feedback(id, None);
    w.set_locked(policy.locked);
    w.set_mouse_passthrough(policy.passthrough);
    let success = (policy.passthrough == before.passthrough
        || frame(&w, &lifecycle, policy.passthrough))
        && apply_policy(id);
    if !success {
        w.set_locked(before.locked);
        w.set_mouse_passthrough(before.passthrough);
        frame(&w, &lifecycle, before.passthrough);
        apply_policy(id);
    }
    let current = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(p) = s.as_mut().and_then(|r| r.windows.get_mut(&id)).filter(|p| {
            p.window.revision() == instance
                && p.interaction.pending.is_some_and(|v| v.request == request)
        }) else {
            return false;
        };
        p.interaction.settled = request;
        if success {
            p.interaction.applied = policy;
        }
        true
    });
    if !current {
        return;
    }
    emit_pin(Event::InteractionApplied {
        id,
        request,
        result: if success {
            Ok(())
        } else {
            Err(lexift_core::Error::new("Operation failed. Try again."))
        },
    });
    if success && policy.passthrough && !before.passthrough && w.window().is_visible() {
        show_feedback_details(
            id,
            "Mouse passthrough is on. Restore mouse input from the tray menu.",
            Some(false),
            0,
            None,
        );
    }
}
