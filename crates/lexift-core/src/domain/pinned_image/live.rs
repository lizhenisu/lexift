//! Ownership of the single live window stream and its ordinary pin handoff.
use super::{Command as PinCommand, State as Pin};
use crate::{
    AppCommand, AppState,
    domain::{
        geometry::Rect,
        screenshot::{ScreenshotCommand, ScreenshotPhase},
    },
    ports::screen_capture::{CapturedRegion, WindowPinPause},
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub source: (u64, u64),
    pub id: Option<u64>,
    pub retired: bool,
    pub finished: bool,
    pub pause: Option<WindowPinPause>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Start {
        session: u64,
    },
    Pause {
        source: (u64, u64),
        reason: Option<WindowPinPause>,
    },
    Opened {
        source: (u64, u64),
        id: u64,
    },
    Failed {
        source: (u64, u64),
        reason: Failure,
    },
    Retired {
        source: (u64, u64),
    },
    Freeze {
        id: u64,
        image: Arc<CapturedRegion>,
    },
    Installed {
        id: u64,
        bounds: Rect,
    },
    Closed {
        id: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Failure {
    Selection,
    Changed,
    BufferLimit,
    #[default]
    Unavailable,
}

impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Selection => "Select an area entirely inside one window’s content",
            Self::Changed => "The source window changed. Take another screenshot",
            Self::BufferLimit => "The window is too large to capture live",
            Self::Unavailable => "Window capture is unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Start { source: (u64, u64), selection: Rect },
    Stop { source: (u64, u64) },
    Install { id: u64, image: Arc<CapturedRegion> },
}

pub fn available(state: &AppState) -> bool {
    let screenshot = &state.screenshot;
    state.live_pin.is_none()
        && state.pin_visibility.pending.is_none()
        && screenshot.editable()
        && screenshot.long.is_none()
        && screenshot.recording.is_none()
        && screenshot.refresh.is_none()
        && screenshot.bounds.is_some()
}

pub(super) fn reduce(state: &mut AppState, event: Event) -> Vec<AppCommand> {
    let wrap = |command| AppCommand::PinnedImage(PinCommand::Live(command));
    if let Event::Start { session } = event {
        if !available(state) || state.screenshot.session != session {
            return vec![];
        }
        state.screenshot.operation = state
            .screenshot
            .operation
            .checked_add(1)
            .expect("screenshot operation exhausted");
        state.screenshot.error = None;
        let source = (session, state.screenshot.operation);
        state.screenshot.phase = ScreenshotPhase::Output;
        state.live_pin = Some(State {
            source,
            id: None,
            retired: false,
            finished: false,
            pause: None,
        });
        return vec![wrap(Command::Start {
            source,
            selection: state.screenshot.bounds.unwrap(),
        })];
    }

    let Some(live) = state.live_pin.as_mut() else {
        return match event {
            Event::Opened { id, .. } if !state.pinned_images.contains_key(&id) => {
                vec![AppCommand::PinnedImage(PinCommand::Drop { id })]
            }
            _ => vec![],
        };
    };
    let mut commands = vec![];
    match event {
        Event::Opened { source, id }
            if live.source == source
                && live.id.is_none()
                && !live.finished
                && id > state.last_pin_id
                && state.screenshot.session == source.0
                && state.screenshot.operation == source.1
                && state.screenshot.phase == ScreenshotPhase::Output =>
        {
            live.id = Some(id);
            state.last_pin_id = id;
            state.pinned_images.insert(
                id,
                Pin {
                    live: true,
                    ..Default::default()
                },
            );
            state.screenshot.phase = ScreenshotPhase::Idle;
            commands.push(AppCommand::Screenshot(ScreenshotCommand::Restore {
                session: source.0,
            }));
        }
        Event::Opened { id, .. } if !state.pinned_images.contains_key(&id) => {
            commands.push(AppCommand::PinnedImage(PinCommand::Drop { id }))
        }
        Event::Failed { source, reason }
            if live.source == source && live.id.is_none() && !live.finished =>
        {
            live.finished = true;
            if state.screenshot.session == source.0
                && state.screenshot.operation == source.1
                && state.screenshot.phase == ScreenshotPhase::Output
            {
                state.screenshot.phase = ScreenshotPhase::Editing;
                state.screenshot.error = Some(reason.message().into());
            }
            commands.push(wrap(Command::Stop { source }));
        }
        Event::Retired { source } if live.source == source => live.retired = true,
        Event::Pause { source, reason }
            if live.source == source && !live.finished && !live.retired =>
        {
            live.pause = reason;
        }
        Event::Freeze { id, image } if live.id == Some(id) && !live.finished => {
            live.finished = true;
            commands.extend([
                wrap(Command::Stop {
                    source: live.source,
                }),
                wrap(Command::Install { id, image }),
            ]);
        }
        Event::Installed { id, bounds } if live.id == Some(id) && live.finished => {
            if let Some(pin) = state.pinned_images.get_mut(&id) {
                pin.source = Some(bounds);
                pin.live = false;
            }
            live.id = None;
        }
        Event::Closed { id } if live.id == Some(id) => {
            live.id = None;
            live.finished = true;
            commands.push(wrap(Command::Stop {
                source: live.source,
            }));
        }
        _ => {}
    }
    if live.retired && live.finished && live.id.is_none() {
        state.live_pin = None;
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::screenshot::{LastRegion, ScreenshotState};

    fn editing() -> AppState {
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 2,
            bottom: 1,
        };
        let mut state = AppState::default();
        state.screenshot = ScreenshotState {
            session: 10,
            phase: ScreenshotPhase::Editing,
            bounds: Some(bounds),
            last_region: Some(LastRegion {
                bounds,
                layout: vec![bounds],
            }),
            ..Default::default()
        };
        state
    }

    #[test]
    fn first_presentation_and_freeze_do_not_record_a_screenshot() {
        let mut state = editing();
        let last = state.screenshot.last_region.clone();
        assert_eq!(reduce(&mut state, Event::Start { session: 10 }).len(), 1);
        let source = state.live_pin.unwrap().source;
        assert!(reduce(&mut state, Event::Start { session: 10 }).is_empty());
        assert_eq!(state.screenshot.phase, ScreenshotPhase::Output);
        assert_eq!(reduce(&mut state, Event::Opened { source, id: 3 }).len(), 1);
        assert_eq!(state.screenshot.phase, ScreenshotPhase::Idle);
        assert!(state.pinned_images[&3].live);
        let bounds = state.screenshot.bounds.unwrap();
        let image = Arc::new(CapturedRegion {
            bounds,
            pixels: vec![8; 8],
        });
        let commands = reduce(
            &mut state,
            Event::Freeze {
                id: 3,
                image: image.clone(),
            },
        );
        assert!(
            matches!(&commands[1], AppCommand::PinnedImage(PinCommand::Live(Command::Install { id: 3, image: saved })) if Arc::ptr_eq(saved, &image))
        );
        reduce(&mut state, Event::Installed { id: 3, bounds });
        assert!(!state.pinned_images[&3].live);
        assert!(state.live_pin.is_some());
        reduce(&mut state, Event::Retired { source });
        assert!(state.live_pin.is_none());
        assert_eq!(state.screenshot.last_region, last);
        assert!(state.image_history.cache.entries().is_empty());
    }

    #[test]
    fn failed_first_frame_restores_editing_and_old_callbacks_cannot_open_a_new_pin() {
        let mut state = editing();
        reduce(&mut state, Event::Start { session: 10 });
        let old = state.live_pin.unwrap().source;
        reduce(
            &mut state,
            Event::Failed {
                source: old,
                reason: Failure::Changed,
            },
        );
        assert_eq!(state.screenshot.phase, ScreenshotPhase::Editing);
        assert!(!available(&state));
        reduce(&mut state, Event::Retired { source: old });
        assert!(available(&state));
        reduce(&mut state, Event::Start { session: 10 });
        let new = state.live_pin.unwrap().source;
        assert_ne!(old, new);
        let commands = reduce(&mut state, Event::Opened { source: old, id: 3 });
        assert_eq!(
            commands,
            vec![AppCommand::PinnedImage(PinCommand::Drop { id: 3 })]
        );
        assert!(state.pinned_images.is_empty());
        reduce(&mut state, Event::Retired { source: old });
        assert!(!state.live_pin.unwrap().retired);
    }
}
