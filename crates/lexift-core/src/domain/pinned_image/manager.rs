//! One auxiliary manager over the existing image and text pin lifetimes.
use super::{interaction, visibility};
use crate::{AppCommand, AppState, domain::geometry::Point};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinId {
    Image(u64),
    Text(u64),
}

impl PinId {
    pub fn hidden(self, s: &AppState) -> Option<bool> {
        match self {
            Self::Image(id) => s.pinned_images.get(&id).map(|p| p.user_hidden),
            Self::Text(id) => s.text_pins.pins.get(&id).map(|p| p.hidden),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub anchor: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Visibility(bool),
    Locate,
    RestoreMouse,
    Close { confirmed_live: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub window: u64,
    pub operation: u64,
    pub pin: PinId,
    pub action: Action,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub window: Option<Window>,
    pub order: Vec<PinId>,
    pub pending: Option<Request>,
    serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Open(Point),
    Closed(u64),
    Requested {
        window: u64,
        pin: PinId,
        action: Action,
    },
    Finished {
        request: Request,
        success: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Retire(u64),
    Execute(Request),
    Failed { window: u64, anchor: Point },
}

fn next(s: &mut State) -> u64 {
    s.serial = s
        .serial
        .checked_add(1)
        .expect("pin manager identity exhausted");
    s.serial
}

fn desktop_capture_active(s: &AppState) -> bool {
    use crate::domain::{ocr::OcrPhase, screenshot::ScreenshotPhase};
    s.screenshot.phase != ScreenshotPhase::Idle
        || s.annotation_export.request.is_some()
        || matches!(
            s.ocr.phase,
            OcrPhase::Checking
                | OcrPhase::Capturing
                | OcrPhase::Recognizing
                | OcrPhase::Translating
        )
}

fn desktop_busy(s: &AppState) -> bool {
    desktop_capture_active(s) || s.screenshot.shortcut_pending.is_some()
}

pub fn eligible(s: &AppState, pin: PinId, action: Action) -> bool {
    if desktop_busy(s)
        || s.pin_visibility.pending.is_some()
        || s.pin_restore.is_some()
        || s.pin_manager.pending.is_some()
    {
        return false;
    }
    let Some(hidden) = pin.hidden(s) else {
        return false;
    };
    let idle = match pin {
        PinId::Image(id) => {
            let p = &s.pinned_images[&id];
            !p.busy
                && p.editing.is_none()
                && p.recognition.is_none()
                && p.proposed.is_none()
                && p.interaction.pending.is_none()
                && !p.transforming
        }
        PinId::Text(id) => s.text_pins.pins[&id].copy.is_none(),
    };
    idle && match action {
        Action::Visibility(value) => hidden != value && visibility::idle(s),
        Action::Locate => !hidden,
        Action::RestoreMouse => {
            !hidden
                && matches!(pin, PinId::Image(id) if s.pinned_images[&id].interaction.applied.passthrough)
        }
        Action::Close { confirmed_live } => {
            matches!(pin, PinId::Image(id) if s.pinned_images[&id].live) == confirmed_live
        }
    }
}

fn retire(s: &mut State) -> Vec<AppCommand> {
    s.pending = None;
    s.window
        .take()
        .map(|w| AppCommand::PinManager(Command::Retire(w.id)))
        .into_iter()
        .collect()
}

pub(crate) fn reduce(s: &mut AppState, event: Event) -> Vec<AppCommand> {
    match event {
        Event::Open(anchor) if !desktop_busy(s) && s.pin_manager.window.is_none() => {
            s.pin_manager.window = Some(Window {
                id: next(&mut s.pin_manager),
                anchor,
            });
            vec![]
        }
        Event::Closed(id) if s.pin_manager.window.is_some_and(|w| w.id == id) => {
            retire(&mut s.pin_manager)
        }
        Event::Requested {
            window,
            pin,
            action,
        } => {
            let Some(view) = s
                .pin_manager
                .window
                .filter(|w| w.id == window && eligible(s, pin, action))
            else {
                return vec![];
            };
            match action {
                Action::Visibility(hidden) => visibility::reduce(
                    s,
                    visibility::Event::Target {
                        pin,
                        hidden,
                        anchor: view.anchor,
                    },
                ),
                Action::RestoreMouse => {
                    let PinId::Image(id) = pin else { return vec![] };
                    let policy = interaction::Policy {
                        passthrough: false,
                        ..s.pinned_images[&id].interaction.applied
                    };
                    interaction::request(s, id, policy)
                }
                _ => {
                    let request = Request {
                        window,
                        pin,
                        action,
                        operation: next(&mut s.pin_manager),
                    };
                    s.pin_manager.pending = Some(request);
                    vec![AppCommand::PinManager(Command::Execute(request))]
                }
            }
        }
        Event::Finished { request, success } if s.pin_manager.pending == Some(request) => {
            s.pin_manager.pending = None;
            if !success
                && request.pin.hidden(s).is_some()
                && let Some(w) = s.pin_manager.window.filter(|w| w.id == request.window)
            {
                vec![AppCommand::PinManager(Command::Failed {
                    window: w.id,
                    anchor: w.anchor,
                })]
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

pub(crate) fn after(s: &mut AppState, quitting: bool, membership_changed: bool) -> Vec<AppCommand> {
    if membership_changed {
        let manager = &mut s.pin_manager;
        manager.order.retain(|key| match *key {
            PinId::Image(id) => s.pinned_images.contains_key(&id),
            PinId::Text(id) => s.text_pins.pins.contains_key(&id),
        });
        if manager.order.len() != s.pinned_images.len() + s.text_pins.pins.len() {
            for id in s
                .pinned_images
                .keys()
                .copied()
                .map(PinId::Image)
                .chain(s.text_pins.pins.keys().copied().map(PinId::Text))
            {
                if !manager.order.contains(&id) {
                    manager.order.push(id);
                }
            }
        }
    }
    // The shortcut snapshot must record the foreground before the manager retires.
    if quitting || desktop_capture_active(s) {
        return retire(&mut s.pin_manager);
    }
    if s.pin_manager
        .pending
        .is_some_and(|q| {
            q.pin.hidden(s).is_none()
                || matches!(q.action, Action::Close { confirmed_live: true })
                    && !matches!(q.pin, PinId::Image(id) if s.pinned_images.get(&id).is_some_and(|p| p.live))
        })
    {
        s.pin_manager.pending = None;
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppEvent,
        domain::{pinned_image as image, screenshot::ScreenshotPhase, text_pin},
    };

    fn open(s: &mut AppState) -> u64 {
        s.reduce(AppEvent::PinManager(Event::Open(Point { x: -500, y: 100 })));
        s.pin_manager.window.unwrap().id
    }

    fn request(s: &mut AppState, window: u64, pin: PinId, action: Action) -> Vec<AppCommand> {
        s.reduce(AppEvent::PinManager(Event::Requested {
            window,
            pin,
            action,
        }))
    }

    #[test]
    fn mixed_identity_targets_one_pin_and_serializes_with_bulk() {
        let mut s = AppState::default();
        s.text_pins.pins.insert(2, text_pin::Pin::default());
        let window = open(&mut s);
        s.pinned_images.insert(2, image::State::default());
        assert_eq!(open(&mut s), window);
        assert_eq!(s.pin_manager.order, [PinId::Text(2), PinId::Image(2)]);
        let output = request(&mut s, window, PinId::Text(2), Action::Visibility(true));
        let [AppCommand::TextPin(text_pin::Command::Visibility(q))] = output.as_slice() else {
            panic!("only the text pin should receive the show/hide request");
        };
        assert!(
            s.reduce(AppEvent::PinnedImage(image::Event::WindowVisibility(
                visibility::Event::Requested {
                    hidden: true,
                    anchor: Point::default()
                },
            )))
            .is_empty()
        );
        s.reduce(AppEvent::TextPin(text_pin::Event::VisibilityApplied(
            *q, false,
        )));
        assert!(!s.text_pins.pins[&2].hidden);
        let output = request(&mut s, window, PinId::Text(2), Action::Visibility(true));
        let [AppCommand::TextPin(text_pin::Command::Visibility(new))] = output.as_slice() else {
            panic!("failed target can retry");
        };
        s.reduce(AppEvent::TextPin(text_pin::Event::VisibilityApplied(
            *q, true,
        )));
        assert!(!s.text_pins.pins[&2].hidden);
        s.reduce(AppEvent::TextPin(text_pin::Event::VisibilityApplied(
            *new, true,
        )));
        assert!(s.text_pins.pins[&2].hidden);
        assert!(!s.pinned_images[&2].user_hidden);
    }

    #[test]
    fn live_freeze_keeps_row_and_close_acknowledgement_is_scoped() {
        let mut s = AppState::default();
        let id = PinId::Image(3);
        s.pinned_images.insert(
            3,
            image::State {
                live: true,
                ..Default::default()
            },
        );
        let window = open(&mut s);
        assert!(
            request(
                &mut s,
                window,
                id,
                Action::Close {
                    confirmed_live: false
                }
            )
            .is_empty()
        );
        let commands = request(
            &mut s,
            window,
            id,
            Action::Close {
                confirmed_live: true,
            },
        );
        let [AppCommand::PinManager(Command::Execute(old))] = commands.as_slice() else {
            panic!("confirmed live close must go through the native owner");
        };
        s.reduce(AppEvent::PinManager(Event::Closed(window)));
        let next = open(&mut s);
        assert_ne!(next, window);
        s.pinned_images.get_mut(&3).unwrap().live = false;
        assert!(
            s.reduce(AppEvent::PinManager(Event::Finished {
                request: *old,
                success: false
            }))
            .is_empty()
        );
        assert_eq!(s.pin_manager.order, [id]);
        assert!(
            request(
                &mut s,
                next,
                id,
                Action::Close {
                    confirmed_live: false
                }
            )
            .len()
                == 1
        );
    }

    #[test]
    fn mouse_recovery_preserves_other_policy_and_capture_retires_manager() {
        let mut s = AppState::default();
        s.pinned_images.insert(1, image::State::default());
        let pin = s.pinned_images.get_mut(&1).unwrap();
        pin.interaction.applied = interaction::Policy {
            passthrough: true,
            locked: true,
            opacity: interaction::Opacity::Sixty,
        };
        let window = open(&mut s);
        let commands = request(&mut s, window, PinId::Image(1), Action::RestoreMouse);
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::PinnedImage(image::Command::ApplyInteraction {
                policy: interaction::Policy {
                    passthrough: false,
                    locked: true,
                    opacity: interaction::Opacity::Sixty
                },
                ..
            })]
        ));
        s.screenshot.shortcut_pending = Some(7);
        assert!(request(&mut s, window, PinId::Image(1), Action::Locate).is_empty());
        assert_eq!(s.pin_manager.window.unwrap().id, window);
        s.screenshot.shortcut_pending = None;
        s.screenshot.phase = ScreenshotPhase::Preparing;
        let commands = s.reduce(AppEvent::PinManager(Event::Open(Point::default())));
        assert_eq!(commands, [AppCommand::PinManager(Command::Retire(window))]);
        assert!(s.pin_manager.window.is_none());
    }
}
