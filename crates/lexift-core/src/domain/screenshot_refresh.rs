//! A transactional frame replacement inside the existing screenshot session.
use crate::domain::{annotation_export::ExportRequest, screenshot::*};
use crate::{AppCommand, Result};

/// Enumeration order is irrelevant; physical geometry and effective DPI must agree.
pub fn layout_matches(
    a: &[(crate::domain::geometry::Rect, f32)],
    b: &[(crate::domain::geometry::Rect, f32)],
) -> bool {
    let normalize = |v: &[(crate::domain::geometry::Rect, f32)]| {
        let mut v = v
            .iter()
            .map(|(r, s)| (r.left, r.top, r.right, r.bottom, s.to_bits()))
            .collect::<Vec<_>>();
        v.sort_unstable();
        v
    };
    !a.is_empty() && normalize(a) == normalize(b)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Suspending,
    Capturing,
    Presenting,
    Committing,
    RollingBack,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub operation: u64,
    pub version: u64,
    pub stage: Stage,
    pub request: ExportRequest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Requested(ExportRequest),
    Suspended {
        session: u64,
        operation: u64,
        result: Result<()>,
    },
    Captured {
        session: u64,
        operation: u64,
        result: Result<()>,
    },
    Presented {
        session: u64,
        operation: u64,
        result: Result<()>,
    },
    Committed {
        session: u64,
        operation: u64,
        result: Result<()>,
    },
    RolledBack {
        session: u64,
        operation: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Suspend {
        session: u64,
        operation: u64,
    },
    Capture {
        session: u64,
        operation: u64,
        request: ExportRequest,
    },
    Install {
        session: u64,
        operation: u64,
    },
    Commit {
        session: u64,
        operation: u64,
    },
    Rollback {
        session: u64,
        operation: u64,
    },
    Finish {
        session: u64,
        operation: u64,
        success: bool,
    },
}
pub(super) fn reduce(s: &mut ScreenshotState, event: Event) -> Vec<AppCommand> {
    let wrap = |c| vec![AppCommand::Screenshot(ScreenshotCommand::Refresh(c))];
    if let Event::Requested(request) = event {
        if s.phase != ScreenshotPhase::Editing
            || s.long.is_some()
            || request.session != s.session
            || request.request <= s.operation
            || Some(request.snapshot.monitor) != s.bounds
        {
            return vec![];
        }
        s.operation = request.request;
        s.refresh = Some(State {
            operation: s.operation,
            version: s.frame_version + 1,
            stage: Stage::Suspending,
            request,
        });
        s.phase = ScreenshotPhase::Refreshing;
        s.color_pending = None;
        s.color_result = None;
        s.error = None;
        return wrap(Command::Suspend {
            session: s.session,
            operation: s.operation,
        });
    }
    let (session, operation, stage, result) = match event {
        Event::Suspended {
            session,
            operation,
            result,
        } => (session, operation, Stage::Suspending, result),
        Event::Captured {
            session,
            operation,
            result,
        } => (session, operation, Stage::Capturing, result),
        Event::Presented {
            session,
            operation,
            result,
        } => (session, operation, Stage::Presenting, result),
        Event::Committed {
            session,
            operation,
            result,
        } => (session, operation, Stage::Committing, result),
        Event::RolledBack { session, operation } => {
            (session, operation, Stage::RollingBack, Ok(()))
        }
        Event::Requested(_) => unreachable!(),
    };
    let Some(refresh) = s.refresh.as_mut().filter(|r| {
        s.phase == ScreenshotPhase::Refreshing
            && session == s.session
            && operation == r.operation
            && stage == r.stage
    }) else {
        return vec![];
    };
    if let Err(error) = result {
        s.error = Some(error.to_string());
        refresh.stage = Stage::RollingBack;
        return wrap(Command::Rollback { session, operation });
    }
    match stage {
        Stage::Suspending => {
            refresh.stage = Stage::Capturing;
            wrap(Command::Capture {
                session,
                operation,
                request: refresh.request.clone(),
            })
        }
        Stage::Capturing => {
            refresh.stage = Stage::Presenting;
            wrap(Command::Install { session, operation })
        }
        Stage::Presenting => {
            refresh.stage = Stage::Committing;
            wrap(Command::Commit { session, operation })
        }
        Stage::Committing | Stage::RollingBack => {
            let success = stage == Stage::Committing;
            if success {
                s.frame_version = refresh.version;
            }
            s.refresh = None;
            s.phase = ScreenshotPhase::Editing;
            wrap(Command::Finish {
                session,
                operation,
                success,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppEvent, AppState,
        domain::{annotation::Session, annotation_export::ExportSnapshot, geometry::Rect},
    };
    use std::sync::Arc;
    fn fixture() -> (AppState, ExportRequest) {
        let mut state = AppState::default();
        let b = Rect {
            left: -100,
            top: 0,
            right: 200,
            bottom: 200,
        };
        state.screenshot.session = 7;
        state.screenshot.phase = ScreenshotPhase::Editing;
        state.screenshot.bounds = Some(b);
        let request = ExportRequest {
            session: 7,
            request: 1,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: b,
                monitors: vec![(b, 1.25)],
                scale: 1.25,
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        };
        (state, request)
    }
    fn emit(s: &mut AppState, e: Event) -> Vec<AppCommand> {
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Refresh(e)))
    }
    #[test]
    fn refresh_requires_both_presentation_and_runtime_commit_and_never_changes_region_history() {
        let (mut s, r) = fixture();
        let last = s.screenshot.last_region.clone();
        assert!(!emit(&mut s, Event::Requested(r.clone())).is_empty());
        assert!(emit(&mut s, Event::Requested(r)).is_empty());
        assert!(
            emit(
                &mut s,
                Event::Presented {
                    session: 7,
                    operation: 1,
                    result: Ok(())
                }
            )
            .is_empty()
        );
        for e in [
            Event::Suspended {
                session: 7,
                operation: 1,
                result: Ok(()),
            },
            Event::Captured {
                session: 7,
                operation: 1,
                result: Ok(()),
            },
            Event::Presented {
                session: 7,
                operation: 1,
                result: Ok(()),
            },
        ] {
            assert!(!emit(&mut s, e).is_empty());
            assert!(!s.screenshot.editable());
            assert_eq!(s.screenshot.frame_version, 0);
        }
        emit(
            &mut s,
            Event::Committed {
                session: 7,
                operation: 1,
                result: Ok(()),
            },
        );
        assert!(s.screenshot.editable());
        assert_eq!(s.screenshot.frame_version, 1);
        assert_eq!(s.screenshot.last_region, last);
        assert!(
            emit(
                &mut s,
                Event::Committed {
                    session: 7,
                    operation: 1,
                    result: Ok(())
                }
            )
            .is_empty()
        );
    }
    #[test]
    fn each_failure_keeps_old_version_until_owned_rollback_and_late_results_are_ignored() {
        for stage in 0..4 {
            let (mut s, r) = fixture();
            emit(&mut s, Event::Requested(r));
            let events = [
                Event::Suspended {
                    session: 7,
                    operation: 1,
                    result: Ok(()),
                },
                Event::Captured {
                    session: 7,
                    operation: 1,
                    result: Ok(()),
                },
                Event::Presented {
                    session: 7,
                    operation: 1,
                    result: Ok(()),
                },
            ];
            for e in events.into_iter().take(stage) {
                emit(&mut s, e);
            }
            let result = Err(crate::Error::new("fixture failure"));
            let failure = match stage {
                0 => Event::Suspended {
                    session: 7,
                    operation: 1,
                    result,
                },
                1 => Event::Captured {
                    session: 7,
                    operation: 1,
                    result,
                },
                2 => Event::Presented {
                    session: 7,
                    operation: 1,
                    result,
                },
                _ => Event::Committed {
                    session: 7,
                    operation: 1,
                    result,
                },
            };
            assert!(!emit(&mut s, failure).is_empty());
            assert!(!s.screenshot.editable());
            assert!(
                emit(
                    &mut s,
                    Event::RolledBack {
                        session: 7,
                        operation: 9
                    }
                )
                .is_empty()
            );
            emit(
                &mut s,
                Event::RolledBack {
                    session: 7,
                    operation: 1,
                },
            );
            assert!(s.screenshot.editable());
            assert_eq!(s.screenshot.frame_version, 0);
            let (_, mut r) = fixture();
            r.request = 2;
            emit(&mut s, Event::Requested(r));
            assert!(
                emit(
                    &mut s,
                    Event::Captured {
                        session: 7,
                        operation: 1,
                        result: Ok(())
                    }
                )
                .is_empty()
            );
            assert_eq!(s.screenshot.operation, 2);
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
                session: 7,
            }));
            assert!(s.screenshot.refresh.is_none());
            assert!(!s.screenshot.active());
            assert!(
                emit(
                    &mut s,
                    Event::Captured {
                        session: 7,
                        operation: 2,
                        result: Ok(())
                    }
                )
                .is_empty()
            );
        }
    }
    #[test]
    fn refresh_admission_and_layout_reject_stale_or_unusable_sources() {
        for phase in [
            ScreenshotPhase::Idle,
            ScreenshotPhase::Preparing,
            ScreenshotPhase::Selecting,
            ScreenshotPhase::Output,
            ScreenshotPhase::Recognizing,
            ScreenshotPhase::CountingDown,
        ] {
            let (mut s, r) = fixture();
            s.screenshot.phase = phase;
            assert!(emit(&mut s, Event::Requested(r)).is_empty());
        }
        let (s, r) = fixture();
        let a = &r.snapshot.monitors;
        assert!(layout_matches(a, a));
        let mut changed = a.clone();
        changed[0].1 = 1.;
        assert!(!layout_matches(a, &changed));
        assert!(!layout_matches(a, &[]));
        let mut s = s;
        let mut r = r;
        r.session = 9;
        assert!(emit(&mut s, Event::Requested(r)).is_empty());
    }
}
