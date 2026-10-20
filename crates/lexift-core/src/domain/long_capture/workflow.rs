//! Session-scoped control of live capture, separate from desktop region editing.
use super::{Feedback, Preview};
use crate::domain::screenshot::ScreenshotOutput;
use crate::{
    Result,
    domain::{
        geometry::Rect,
        screenshot::{ScreenshotCommand, ScreenshotPhase, ScreenshotState},
    },
    ports::{magnifier::WindowToken, screen_capture::CapturedRegion},
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    AwaitingFrame,
    Capturing,
    Paused,
    Finishing,
    Frozen,
    Editing,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub operation: u64,
    pub source: Rect,
    pub phase: Phase,
    pub sequence: u64,
    pub width: usize,
    pub height: usize,
    pub feedback: Feedback,
    pub target: Option<ScreenshotOutput>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Start {
        session: u64,
        bounds: Rect,
    },
    Ready {
        session: u64,
        operation: u64,
        excluded: Vec<WindowToken>,
        preview_size: (usize, usize),
    },
    Progress {
        session: u64,
        operation: u64,
        sequence: u64,
        width: usize,
        height: usize,
        feedback: Feedback,
        preview: Preview,
    },
    Pause {
        session: u64,
        operation: u64,
        paused: bool,
    },
    Finish {
        session: u64,
        operation: u64,
        target: Option<ScreenshotOutput>,
    },
    Finished {
        session: u64,
        operation: u64,
        result: Result<Arc<CapturedRegion>>,
    },
    EditorUnavailable {
        session: u64,
        operation: u64,
    },
    OpenEditor {
        session: u64,
        operation: u64,
        image: Arc<CapturedRegion>,
    },
    Cancel {
        session: u64,
        operation: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Prepare {
        session: u64,
        operation: u64,
        bounds: Rect,
    },
    Capture {
        session: u64,
        operation: u64,
        bounds: Rect,
        excluded: Vec<WindowToken>,
        preview_size: (usize, usize),
    },
    Present {
        session: u64,
        operation: u64,
        sequence: u64,
        first: bool,
        preview: Preview,
    },
    Pause {
        session: u64,
        operation: u64,
        paused: bool,
    },
    Finish {
        session: u64,
        operation: u64,
    },
    Edit {
        session: u64,
        operation: u64,
        image: Arc<CapturedRegion>,
        target: Option<ScreenshotOutput>,
    },
    Restore {
        session: u64,
        operation: u64,
    },
}
pub(crate) fn reduce(s: &mut ScreenshotState, event: Event) -> Vec<ScreenshotCommand> {
    let wrap = |c| vec![ScreenshotCommand::Long(c)];
    if let Event::Start { session, bounds } = event {
        if session != s.session
            || s.phase != ScreenshotPhase::Editing
            || s.long.is_some()
            || s.bounds != Some(bounds)
            || !s.layout.iter().any(|b| {
                bounds.left >= b.left
                    && bounds.top >= b.top
                    && bounds.right <= b.right
                    && bounds.bottom <= b.bottom
            })
        {
            return vec![];
        }
        s.operation += 1;
        s.color_pending = None;
        s.color_result = None;
        s.long = Some(State {
            operation: s.operation,
            source: bounds,
            phase: Phase::Preparing,
            sequence: 0,
            width: 0,
            height: 0,
            feedback: Feedback::Unchanged,
            target: None,
        });
        return wrap(Command::Prepare {
            session,
            operation: s.operation,
            bounds,
        });
    }
    let (session, operation) = match &event {
        Event::Ready {
            session, operation, ..
        }
        | Event::Progress {
            session, operation, ..
        }
        | Event::Pause {
            session, operation, ..
        }
        | Event::Finish {
            session, operation, ..
        }
        | Event::OpenEditor {
            session, operation, ..
        }
        | Event::EditorUnavailable { session, operation }
        | Event::Finished {
            session, operation, ..
        }
        | Event::Cancel { session, operation } => (*session, *operation),
        Event::Start { .. } => unreachable!(),
    };
    if session != s.session || s.phase != ScreenshotPhase::Editing {
        return vec![];
    }
    let Some(l) = s.long.as_mut().filter(|l| l.operation == operation) else {
        return vec![];
    };
    match event {
        Event::Ready {
            excluded,
            preview_size,
            ..
        } if l.phase == Phase::Preparing => {
            l.phase = Phase::AwaitingFrame;
            wrap(Command::Capture {
                session,
                operation,
                bounds: l.source,
                excluded,
                preview_size,
            })
        }
        Event::Progress {
            sequence,
            width,
            height,
            feedback,
            preview,
            ..
        } if sequence > l.sequence
            && matches!(
                l.phase,
                Phase::Preparing
                    | Phase::AwaitingFrame
                    | Phase::Capturing
                    | Phase::Paused
                    | Phase::Finishing
            ) =>
        {
            let first = l.sequence == 0;
            l.sequence = sequence;
            l.width = width;
            l.height = height;
            l.feedback = feedback;
            if matches!(l.phase, Phase::Preparing | Phase::AwaitingFrame) {
                l.phase = Phase::Capturing;
            }
            if matches!(feedback, Feedback::Limit | Feedback::CaptureLost)
                && l.phase != Phase::Finishing
            {
                l.phase = Phase::Paused;
            }
            wrap(Command::Present {
                session,
                operation,
                sequence,
                first,
                preview,
            })
        }
        Event::Pause { paused, .. }
            if matches!(l.phase, Phase::Capturing | Phase::Paused)
                && (paused || l.feedback != Feedback::Limit) =>
        {
            l.phase = if paused {
                Phase::Paused
            } else {
                Phase::Capturing
            };
            wrap(Command::Pause {
                session,
                operation,
                paused,
            })
        }
        Event::Finish { target, .. } if matches!(l.phase, Phase::Capturing | Phase::Paused) => {
            l.target = target;
            l.phase = Phase::Finishing;
            wrap(Command::Finish { session, operation })
        }
        Event::Finished {
            result: Ok(image), ..
        } if l.phase == Phase::Finishing => {
            l.phase = Phase::Frozen;
            s.bounds = Some(image.bounds);
            wrap(Command::Edit {
                session,
                operation,
                image,
                target: l.target,
            })
        }
        Event::EditorUnavailable { .. } if l.phase == Phase::Editing => {
            l.phase = Phase::Frozen;
            s.error = Some("Long capture unavailable. Select a smaller scrolling area.".into());
            vec![]
        }
        Event::OpenEditor { image, .. } if l.phase == Phase::Frozen => {
            l.phase = Phase::Editing;
            wrap(Command::Edit {
                session,
                operation,
                image,
                target: None,
            })
        }
        Event::Finished {
            result: Err(error), ..
        } if !matches!(l.phase, Phase::Editing | Phase::Frozen) => {
            s.error = Some(error.to_string());
            s.long = None;
            wrap(Command::Restore { session, operation })
        }
        Event::Cancel { .. } if l.phase != Phase::Editing => {
            s.bounds = Some(l.source);
            s.long = None;
            wrap(Command::Restore { session, operation })
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screenshot() -> ScreenshotState {
        let bounds = Rect {
            left: -600,
            top: 20,
            right: -200,
            bottom: 420,
        };
        ScreenshotState {
            session: 9,
            phase: ScreenshotPhase::Editing,
            bounds: Some(bounds),
            layout: vec![Rect {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080,
            }],
            ..Default::default()
        }
    }
    fn start(s: &mut ScreenshotState) -> u64 {
        let b = s.bounds.unwrap();
        assert_eq!(
            reduce(
                s,
                Event::Start {
                    session: 9,
                    bounds: b
                }
            )
            .len(),
            1
        );
        s.long.as_ref().unwrap().operation
    }
    fn thumbnail() -> Arc<CapturedRegion> {
        Arc::new(CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            pixels: vec![255; 4],
        })
    }
    fn preview() -> Preview {
        let image = thumbnail();
        Preview {
            extent: image.bounds,
            visible: image.bounds,
            current: image.bounds,
            image,
        }
    }
    #[test]
    fn frozen_output_cancel_failure_retry_and_editor_failure_keep_the_owned_result() {
        use crate::domain::{
            annotation::Session,
            annotation_export::{ExportRequest, ExportSnapshot},
        };
        use crate::ports::annotation_save::SaveOutcome;
        use crate::{AppEvent, AppState};
        for target in [
            ScreenshotOutput::Copy,
            ScreenshotOutput::Save,
            ScreenshotOutput::Pin,
        ] {
            let mut screenshot = screenshot();
            let original = screenshot.bounds;
            let op = start(&mut screenshot);
            screenshot.long.as_mut().unwrap().phase = Phase::Capturing;
            reduce(
                &mut screenshot,
                Event::Finish {
                    session: 9,
                    operation: op,
                    target: Some(target),
                },
            );
            let result = thumbnail();
            let commands = reduce(
                &mut screenshot,
                Event::Finished {
                    session: 9,
                    operation: op,
                    result: Ok(result.clone()),
                },
            );
            assert!(
                matches!(&commands[0],ScreenshotCommand::Long(Command::Edit{target:Some(k),..}) if *k==target)
            );
            let mut state = AppState::default();
            state.screenshot = screenshot;
            for outcome in [
                Err(crate::Error::new("fixture output failure")),
                Ok(SaveOutcome::Cancelled),
            ] {
                let operation = state.screenshot.operation + 1;
                let request = ExportRequest {
                    session: 9,
                    request: operation,
                    snapshot: Arc::new(ExportSnapshot {
                        session: Arc::new(Session::new()),
                        monitor: result.bounds,
                        scale: 1.,
                        monitors: vec![(result.bounds, 1.)],
                        excluded: vec![],
                        owner: None,
                        name: String::new(),
                        title: String::new(),
                    }),
                };
                assert!(
                    !state
                        .reduce(AppEvent::Screenshot(
                            crate::domain::screenshot::ScreenshotEvent::Output {
                                session: 9,
                                operation,
                                kind: target,
                                request
                            }
                        ))
                        .is_empty()
                );
                state.reduce(AppEvent::Screenshot(
                    crate::domain::screenshot::ScreenshotEvent::Completed {
                        session: 9,
                        operation,
                        result: outcome,
                    },
                ));
                assert_eq!(state.screenshot.phase, ScreenshotPhase::Editing);
                assert_eq!(state.screenshot.long.as_ref().unwrap().phase, Phase::Frozen);
                assert_eq!(state.screenshot.bounds, Some(result.bounds));
            }
            reduce(
                &mut state.screenshot,
                Event::OpenEditor {
                    session: 9,
                    operation: op,
                    image: result,
                },
            );
            reduce(
                &mut state.screenshot,
                Event::EditorUnavailable {
                    session: 9,
                    operation: op,
                },
            );
            assert_eq!(state.screenshot.long.as_ref().unwrap().phase, Phase::Frozen);
            reduce(
                &mut state.screenshot,
                Event::Cancel {
                    session: 9,
                    operation: op,
                },
            );
            assert_eq!(state.screenshot.bounds, original);
            assert!(state.screenshot.long.is_none());
        }
    }
    #[test]
    fn identity_start_guards_and_cancelled_late_frames() {
        let mut s = screenshot();
        let b = s.bounds.unwrap();
        assert!(
            reduce(
                &mut s,
                Event::Start {
                    session: 8,
                    bounds: b
                }
            )
            .is_empty()
        );
        let op = start(&mut s);
        assert!(
            reduce(
                &mut s,
                Event::Start {
                    session: 9,
                    bounds: b
                }
            )
            .is_empty()
        );
        assert!(!s.editable());
        assert_eq!(
            reduce(
                &mut s,
                Event::Cancel {
                    session: 9,
                    operation: op
                }
            )
            .len(),
            1
        );
        assert!(s.editable());
        assert_eq!(s.bounds, Some(b));
        assert!(
            reduce(
                &mut s,
                Event::Progress {
                    session: 9,
                    operation: op,
                    sequence: 1,
                    width: 400,
                    height: 400,
                    feedback: Feedback::Accepted,
                    preview: preview()
                }
            )
            .is_empty()
        );
    }
    #[test]
    fn progress_pause_finish_and_stale_sequences() {
        let mut s = screenshot();
        let op = start(&mut s);
        assert_eq!(
            reduce(
                &mut s,
                Event::Ready {
                    session: 9,
                    operation: op,
                    excluded: vec![],
                    preview_size: (200, 800),
                }
            )
            .len(),
            1
        );
        let progress = |seq| Event::Progress {
            session: 9,
            operation: op,
            sequence: seq,
            width: 400,
            height: 600,
            feedback: Feedback::Accepted,
            preview: preview(),
        };
        assert_eq!(reduce(&mut s, progress(2)).len(), 1);
        assert!(reduce(&mut s, progress(1)).is_empty());
        reduce(
            &mut s,
            Event::Pause {
                session: 9,
                operation: op,
                paused: true,
            },
        );
        assert_eq!(s.long.as_ref().unwrap().phase, Phase::Paused);
        reduce(
            &mut s,
            Event::Finish {
                target: None,
                session: 9,
                operation: op,
            },
        );
        assert_eq!(s.long.as_ref().unwrap().phase, Phase::Finishing);
        assert!(
            reduce(
                &mut s,
                Event::Finish {
                    target: None,
                    session: 9,
                    operation: op
                }
            )
            .is_empty()
        );
        let commands = reduce(
            &mut s,
            Event::Finished {
                session: 9,
                operation: op,
                result: Ok(thumbnail()),
            },
        );
        assert_eq!(commands.len(), 1);
        assert!(!s.editable());
        assert_eq!(s.bounds, Some(thumbnail().bounds));
        reduce(
            &mut s,
            Event::OpenEditor {
                session: 9,
                operation: op,
                image: thumbnail(),
            },
        );
        assert!(s.editable());
    }
    #[test]
    fn single_screen_and_failure_preserve_original_region() {
        let mut s = screenshot();
        s.bounds.as_mut().unwrap().right = 10;
        let b = s.bounds.unwrap();
        assert!(
            reduce(
                &mut s,
                Event::Start {
                    session: 9,
                    bounds: b
                }
            )
            .is_empty()
        );
        s = screenshot();
        let before = s.bounds;
        let op = start(&mut s);
        assert!(
            reduce(
                &mut s,
                Event::Finished {
                    session: 9,
                    operation: op + 1,
                    result: Err(crate::Error::new("stale"))
                }
            )
            .is_empty()
        );
        assert_eq!(
            reduce(
                &mut s,
                Event::Finished {
                    session: 9,
                    operation: op,
                    result: Err(crate::Error::new("capture"))
                }
            )
            .len(),
            1
        );
        assert_eq!(s.bounds, before);
        assert!(s.long.is_none());
    }
    #[test]
    fn successful_long_output_preserves_previous_desktop_history_and_retires_state() {
        use crate::domain::screenshot::{LastRegion, ScreenshotEvent};
        use crate::{AppEvent, AppState};
        let mut s = screenshot();
        let original = s.bounds.unwrap();
        let layout = s.layout.clone();
        let last = LastRegion {
            bounds: original,
            layout: layout.clone(),
        };
        s.last_region = Some(last.clone());
        let operation = start(&mut s);
        s.phase = ScreenshotPhase::Output;
        s.operation = operation + 1;
        s.long.as_mut().unwrap().phase = Phase::Editing;
        s.bounds = Some(Rect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 2000,
        });
        let mut state = AppState::default();
        state.screenshot = s;
        let commands = state.reduce(AppEvent::Screenshot(ScreenshotEvent::Completed {
            session: 9,
            operation: operation + 1,
            result: Ok(crate::ports::annotation_save::SaveOutcome::Saved),
        }));
        assert_eq!(commands.len(), 1);
        assert_eq!(state.screenshot.last_region, Some(last));
        assert!(state.screenshot.long.is_none());
        assert_eq!(state.screenshot.phase, ScreenshotPhase::Idle);
    }
}
