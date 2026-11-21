use super::{geometry::Point, pinned_image::visibility};
use crate::{AppCommand, AppState};
use std::collections::BTreeMap;

pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_PINS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Empty,
    TooLarge,
    Read,
    Limit,
    Window,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Creation {
    pub id: u64,
    pub anchor: Point,
    pub presenting: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pin {
    pub hidden: bool,
    pub copy: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub creation: Option<Creation>,
    pub pins: BTreeMap<u64, Pin>,
    serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Requested(Point),
    Read {
        id: u64,
        result: Result<(), Failure>,
    },
    Presented {
        id: u64,
        success: bool,
    },
    Copy {
        id: u64,
        selection: Option<(i32, i32)>,
    },
    Copied {
        id: u64,
        request: u64,
        success: bool,
    },
    Closed(u64),
    VisibilityApplied(visibility::Request, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Read(u64),
    Present(Creation),
    Copy {
        id: u64,
        request: u64,
        selection: Option<(i32, i32)>,
    },
    Drop(u64),
    Visibility(visibility::Request),
    Feedback {
        anchor: Option<Point>,
        failure: Failure,
    },
}

impl State {
    fn next(&mut self) -> u64 {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("text pin identity exhausted");
        self.serial
    }
}

/// Validates the frozen clipboard snapshot without altering its bytes.
pub fn validate(text: Option<&str>) -> Result<(), Failure> {
    let text = text.ok_or(Failure::Empty)?;
    if text.len() > MAX_BYTES {
        return Err(Failure::TooLarge);
    }
    if text.trim().is_empty() {
        return Err(Failure::Empty);
    }
    Ok(())
}

pub fn selection(text: &str, range: Option<(i32, i32)>) -> Option<&str> {
    let Some((anchor, cursor)) = range else {
        return Some(text);
    };
    let start = usize::try_from(anchor.min(cursor)).ok()?;
    let end = usize::try_from(anchor.max(cursor)).ok()?;
    text.get(start..end).filter(|value| !value.is_empty())
}

pub(crate) fn reduce(s: &mut AppState, event: Event) -> Vec<AppCommand> {
    let commands = match event {
        Event::Requested(anchor) if visibility::idle(s) && s.pin_restore.is_none() => {
            if s.text_pins.pins.len() >= MAX_PINS {
                vec![Command::Feedback {
                    anchor: Some(anchor),
                    failure: Failure::Limit,
                }]
            } else {
                let id = s.text_pins.next();
                s.text_pins.creation = Some(Creation {
                    id,
                    anchor,
                    presenting: false,
                });
                vec![Command::Read(id)]
            }
        }
        Event::Read { id, result } => {
            let Some(q) = s
                .text_pins
                .creation
                .as_mut()
                .filter(|q| q.id == id && !q.presenting)
            else {
                return vec![];
            };
            if let Err(failure) = result {
                let anchor = q.anchor;
                s.text_pins.creation = None;
                vec![
                    Command::Drop(id),
                    Command::Feedback {
                        anchor: Some(anchor),
                        failure,
                    },
                ]
            } else if s.screenshot.active() {
                s.text_pins.creation = None;
                vec![Command::Drop(id)]
            } else {
                q.presenting = true;
                vec![Command::Present(*q)]
            }
        }
        Event::Presented { id, success } => {
            let Some(q) = s.text_pins.creation.filter(|q| q.id == id && q.presenting) else {
                return vec![];
            };
            s.text_pins.creation = None;
            if success && !s.screenshot.active() {
                s.text_pins.pins.insert(id, Pin::default());
                vec![]
            } else if success {
                vec![Command::Drop(id)]
            } else {
                vec![
                    Command::Drop(id),
                    Command::Feedback {
                        anchor: Some(q.anchor),
                        failure: Failure::Window,
                    },
                ]
            }
        }
        Event::Copy { id, selection }
            if !s.screenshot.active() && s.pin_visibility.pending.is_none() =>
        {
            if !s
                .text_pins
                .pins
                .get(&id)
                .is_some_and(|pin| !pin.hidden && pin.copy.is_none())
            {
                return vec![];
            }
            let request = s.text_pins.next();
            s.text_pins.pins.get_mut(&id).unwrap().copy = Some(request);
            vec![Command::Copy {
                id,
                request,
                selection,
            }]
        }
        Event::Copied {
            id,
            request,
            success,
        } => {
            let Some(pin) = s
                .text_pins
                .pins
                .get_mut(&id)
                .filter(|pin| pin.copy == Some(request))
            else {
                return vec![];
            };
            pin.copy = None;
            if success || pin.hidden || s.screenshot.active() {
                vec![]
            } else {
                vec![Command::Feedback {
                    anchor: None,
                    failure: Failure::Copy,
                }]
            }
        }
        Event::Closed(id) => {
            if s.text_pins.creation.is_some_and(|q| q.id == id) {
                s.text_pins.creation = None;
            } else if s.text_pins.pins.remove(&id).is_none() {
                return vec![];
            }
            visibility::text_closed(s, id);
            vec![Command::Drop(id)]
        }
        Event::VisibilityApplied(request, success) => {
            visibility::text_applied(s, request, success);
            vec![]
        }
        _ => vec![],
    };
    commands.into_iter().map(AppCommand::TextPin).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppEvent, domain::pinned_image::Event as ImageEvent};

    fn dispatch(s: &mut AppState, e: Event) -> Vec<AppCommand> {
        s.reduce(AppEvent::TextPin(e))
    }

    fn begin(s: &mut AppState) -> u64 {
        let commands = dispatch(s, Event::Requested(Point::default()));
        match commands.as_slice() {
            [AppCommand::TextPin(Command::Read(id))] => *id,
            _ => panic!("expected one read"),
        }
    }

    fn open(s: &mut AppState) -> u64 {
        let id = begin(s);
        assert_eq!(dispatch(s, Event::Read { id, result: Ok(()) }).len(), 1);
        assert!(dispatch(s, Event::Presented { id, success: true }).is_empty());
        id
    }

    #[test]
    fn size_and_selection_are_in_utf8_bytes_and_preserve_the_snapshot() {
        let text = "  字😀\r\nleft\n ";
        assert_eq!(validate(Some(text)), Ok(()));
        assert_eq!(selection(text, None), Some(text));
        assert_eq!(selection(text, Some((9, 2))), Some("字😀"));
        assert_eq!(selection(text, Some((2, 3))), None);
        assert_eq!(selection(text, Some((0, 0))), None);
        assert_eq!(validate(Some(" \r\n\t\u{3000}")), Err(Failure::Empty));
        assert_eq!(validate(None), Err(Failure::Empty));
        let boundary = "字".repeat(MAX_BYTES / 3) + "a";
        assert_eq!(boundary.len(), MAX_BYTES);
        assert_eq!(validate(Some(&boundary)), Ok(()));
        assert_eq!(validate(Some(&(boundary + "a"))), Err(Failure::TooLarge));
    }

    #[test]
    fn read_and_presentation_are_fenced_and_limit_never_evicts() {
        let mut s = AppState::default();
        let first = begin(&mut s);
        assert!(dispatch(&mut s, Event::Requested(Point::default())).is_empty());
        dispatch(&mut s, Event::Closed(first));
        let second = begin(&mut s);
        assert!(
            dispatch(
                &mut s,
                Event::Read {
                    id: first,
                    result: Ok(())
                }
            )
            .is_empty()
        );
        assert!(
            dispatch(
                &mut s,
                Event::Presented {
                    id: first,
                    success: true
                }
            )
            .is_empty()
        );
        assert_eq!(
            dispatch(
                &mut s,
                Event::Read {
                    id: second,
                    result: Err(Failure::Read)
                }
            )
            .len(),
            2
        );
        for _ in 0..MAX_PINS {
            open(&mut s);
        }
        let before = s.text_pins.clone();
        assert!(matches!(
            dispatch(&mut s, Event::Requested(Point::default())).as_slice(),
            [AppCommand::TextPin(Command::Feedback {
                failure: Failure::Limit,
                ..
            })]
        ));
        assert_eq!(s.text_pins, before);
    }

    #[test]
    fn mixed_visibility_waits_for_both_kinds_and_isolates_closed_pin_copies() {
        let mut s = AppState::default();
        let text = open(&mut s);
        s.pinned_images.insert(1, Default::default());
        let requests = s.reduce(AppEvent::PinnedImage(ImageEvent::WindowVisibility(
            visibility::Event::Requested {
                hidden: true,
                anchor: Point::default(),
            },
        )));
        let [
            AppCommand::PinnedImage(super::super::pinned_image::Command::ApplyVisibility(image)),
            AppCommand::TextPin(Command::Visibility(window)),
        ] = requests.as_slice()
        else {
            panic!("expected both targets")
        };
        s.reduce(AppEvent::PinnedImage(ImageEvent::WindowVisibility(
            visibility::Event::Applied {
                request: *image,
                result: Ok(()),
            },
        )));
        assert!(s.pin_visibility.pending.is_some());
        dispatch(&mut s, Event::VisibilityApplied(*window, false));
        assert!(s.pin_visibility.pending.is_none());
        assert_eq!(s.pin_visibility.notice.unwrap().failed, 1);
        assert!(!s.text_pins.pins[&text].hidden);

        let commands = dispatch(
            &mut s,
            Event::Copy {
                id: text,
                selection: None,
            },
        );
        let [AppCommand::TextPin(Command::Copy { request, .. })] = commands.as_slice() else {
            panic!("expected one write");
        };
        assert!(
            dispatch(
                &mut s,
                Event::Copy {
                    id: text,
                    selection: None
                }
            )
            .is_empty()
        );
        dispatch(&mut s, Event::Closed(text));
        let next = open(&mut s);
        assert_ne!(text, next);
        assert!(
            dispatch(
                &mut s,
                Event::Copied {
                    id: text,
                    request: *request,
                    success: false
                }
            )
            .is_empty()
        );
        assert!(s.text_pins.pins[&next].copy.is_none());
        assert!(s.image_history.cache.entries().is_empty());
    }
}
