//! Per-window interaction policy, separate from documents and temporary UI suspension.
use super::{Command, State as Pin};
use crate::{AppCommand, AppState, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub locked: bool,
    pub passthrough: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub request: u64,
    pub policy: Policy,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub applied: Policy,
    pub serial: u64,
    pub settled: u64,
    pub pending: Option<Pending>,
    pub failed: Option<u64>,
}
fn queue(id: u64, pin: &mut Pin, policy: Policy) -> Option<AppCommand> {
    if pin.interaction.pending.is_some_and(|p| p.policy == policy)
        || (pin.interaction.pending.is_none() && pin.interaction.applied == policy)
    {
        return None;
    }
    pin.interaction.serial = pin
        .interaction
        .serial
        .checked_add(1)
        .expect("pin policy exhausted");
    let request = pin.interaction.serial;
    pin.interaction.pending = Some(Pending { request, policy });
    pin.interaction.failed = None;
    Some(AppCommand::PinnedImage(Command::ApplyInteraction {
        id,
        request,
        policy,
    }))
}
pub(super) fn request(state: &mut AppState, id: u64, policy: Policy) -> Vec<AppCommand> {
    if state.screenshot.active() {
        return vec![];
    }
    let Some(pin) = state.pinned_images.get_mut(&id).filter(|p| {
        !p.busy
            && p.proposed.is_none()
            && p.interaction.pending.is_none()
            && (!policy.passthrough || p.editing.is_none())
    }) else {
        return vec![];
    };
    queue(id, pin, policy).into_iter().collect()
}
/// Recovery may supersede a queued enable and may run while capture owns the screen.
pub(super) fn restore(state: &mut AppState) -> Vec<AppCommand> {
    state
        .pinned_images
        .iter_mut()
        .filter_map(|(&id, pin)| {
            let desired = pin
                .interaction
                .pending
                .map_or(pin.interaction.applied, |p| p.policy);
            if !desired.passthrough && !pin.interaction.applied.passthrough {
                return None;
            }
            queue(
                id,
                pin,
                Policy {
                    passthrough: false,
                    ..desired
                },
            )
        })
        .collect()
}
pub(super) fn complete(state: &mut AppState, id: u64, request: u64, result: Result<()>) {
    let Some(pin) = state.pinned_images.get_mut(&id) else {
        return;
    };
    if let Some(pending) = pin.interaction.pending.filter(|p| p.request == request) {
        pin.interaction.pending = None;
        pin.interaction.settled = request;
        if result.is_ok() {
            pin.interaction.applied = pending.policy;
        } else {
            pin.interaction.failed = Some(request);
        }
    }
}

/// Window geometry changes cannot consume a history entry while a pin is locked.
pub fn display_size(document: &super::ExportRequest) -> (i32, i32) {
    let b = document.snapshot.monitor;
    let b = document
        .snapshot
        .session
        .image_orientation
        .rect(b, b)
        .unwrap_or(b);
    (b.right - b.left, b.bottom - b.top)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEvent, domain::pinned_image::Event};
    fn state() -> AppState {
        let mut s = AppState::default();
        s.pinned_images.insert(1, Pin::default());
        s.pinned_images.insert(2, Pin::default());
        s
    }
    fn send(s: &mut AppState, id: u64, policy: Policy) -> u64 {
        assert!(
            !s.reduce(AppEvent::PinnedImage(Event::InteractionRequested {
                id,
                policy
            }))
            .is_empty()
        );
        s.pinned_images[&id].interaction.pending.unwrap().request
    }
    fn finish(s: &mut AppState, id: u64, request: u64, success: bool) {
        s.reduce(AppEvent::PinnedImage(Event::InteractionApplied {
            id,
            request,
            result: if success {
                Ok(())
            } else {
                Err(crate::Error::new("fixture"))
            },
        }));
    }
    #[test]
    fn geometry_history_check_is_non_consuming_and_retains_annotation_steps() {
        use crate::domain::{
            annotation::{Bounds, Extra, Kind, Object, Session},
            geometry::Rect,
            image_transform::{Action, Orientation},
        };
        let full = Rect {
            left: -500,
            top: -300,
            right: 100,
            bottom: 900,
        };
        let mut session = Session::new();
        session.image_source = Some(full);
        session.set_image_crop(
            Rect {
                left: -400,
                top: -200,
                right: 0,
                bottom: 500,
            },
            full,
        );
        assert!(session.history_changes_image_extent(false, full));
        session.add(Object {
            id: Default::default(),
            image_basis: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((-350., -150.), (-100., 300.)),
            style: Default::default(),
            points: vec![],
            extra: Extra::None,
            erased: vec![],
        });
        assert!(!session.history_changes_image_extent(false, full));
        assert!(session.undo());
        assert!(session.history_changes_image_extent(false, full));
        assert!(session.can_undo());
        assert!(session.redo());
        assert_eq!(session.objects.len(), 1);
        session.orient_image(Orientation::identity().action(Action::Right));
        assert!(session.history_changes_image_extent(false, full));
    }
    #[test]
    fn policy_is_per_pin_acknowledged_and_failure_preserves_previous_flags() {
        for locked in [false, true] {
            for passthrough in [false, true] {
                if !locked && !passthrough {
                    continue;
                }
                let mut s = state();
                let original_history = s.image_history.clone();
                let policy = Policy {
                    locked,
                    passthrough,
                };
                let request = send(&mut s, 1, policy);
                assert_eq!(s.pinned_images[&1].interaction.applied, Policy::default());
                assert!(
                    s.reduce(AppEvent::PinnedImage(Event::InteractionRequested {
                        id: 1,
                        policy
                    }))
                    .is_empty()
                );
                finish(&mut s, 1, request, true);
                assert_eq!(s.pinned_images[&1].interaction.applied, policy);
                assert_eq!(s.pinned_images[&2].interaction, State::default());
                assert_eq!(s.image_history, original_history);
                let next = send(&mut s, 1, Policy::default());
                finish(&mut s, 1, next, false);
                assert_eq!(s.pinned_images[&1].interaction.applied, policy);
                assert_eq!(s.pinned_images[&1].interaction.failed, Some(next));
                finish(&mut s, 1, request, true);
                assert_eq!(s.pinned_images[&1].interaction.failed, Some(next));
            }
        }
    }
    #[test]
    fn recovery_supersedes_queued_enable_and_accepts_partial_failure() {
        let mut s = state();
        let policy = Policy {
            locked: true,
            passthrough: true,
        };
        let old = send(&mut s, 1, policy);
        let other = send(&mut s, 2, policy);
        finish(&mut s, 2, other, true);
        let commands = s.reduce(AppEvent::PinnedImage(Event::RestoreMouseInput));
        assert_eq!(commands.len(), 2);
        let first = s.pinned_images[&1].interaction.pending.unwrap().request;
        let second = s.pinned_images[&2].interaction.pending.unwrap().request;
        finish(&mut s, 1, old, true);
        assert_eq!(
            s.pinned_images[&1].interaction.pending.unwrap().request,
            first
        );
        finish(&mut s, 1, first, true);
        finish(&mut s, 2, second, false);
        assert_eq!(
            s.pinned_images[&1].interaction.applied,
            Policy {
                locked: true,
                passthrough: false
            }
        );
        assert_eq!(s.pinned_images[&2].interaction.applied, policy);
        assert_eq!(
            s.reduce(AppEvent::PinnedImage(Event::RestoreMouseInput))
                .len(),
            1
        );
        let latest = s.pinned_images[&2].interaction.pending.unwrap().request;
        s.reduce(AppEvent::PinnedImage(Event::Closed { id: 2 }));
        finish(&mut s, 2, latest, true);
        assert!(!s.pinned_images.contains_key(&2));
    }
    #[test]
    fn editing_allows_lock_but_blocks_passthrough_and_busy_changes() {
        let mut s = state();
        s.pinned_images.get_mut(&1).unwrap().editing = Some(8);
        let policy = Policy {
            locked: true,
            passthrough: true,
        };
        assert!(
            s.reduce(AppEvent::PinnedImage(Event::InteractionRequested {
                id: 1,
                policy
            }))
            .is_empty()
        );
        let n = send(
            &mut s,
            1,
            Policy {
                locked: true,
                passthrough: false,
            },
        );
        finish(&mut s, 1, n, true);
        s.pinned_images.get_mut(&1).unwrap().busy = true;
        assert!(
            s.reduce(AppEvent::PinnedImage(Event::InteractionRequested {
                id: 1,
                policy: Policy::default()
            }))
            .is_empty()
        );
        s.pinned_images.get_mut(&2).unwrap().interaction.applied = policy;
        s.screenshot.phase = crate::domain::screenshot::ScreenshotPhase::Preparing;
        assert_eq!(
            s.reduce(AppEvent::PinnedImage(Event::RestoreMouseInput))
                .len(),
            1
        );
    }
}
