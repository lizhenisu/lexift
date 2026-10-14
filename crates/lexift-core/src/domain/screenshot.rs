//! Screenshot workflow identities; image ownership and native resources stay in adapters.
use super::{annotation_export::ExportRequest, geometry::Rect};
use crate::ports::annotation_save::SaveOutcome;
use crate::{AppCommand, AppState, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenshotPhase {
    #[default]
    Idle,
    Preparing,
    Selecting,
    Editing,
    Output,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotOutput {
    Copy,
    Save,
    Pin,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenshotState {
    pub session: u64,
    pub source_session: u64,
    pub phase: ScreenshotPhase,
    pub operation: u64,
    pub bounds: Option<Rect>,
    pub error: Option<String>,
}
impl ScreenshotState {
    pub fn active(&self) -> bool {
        self.phase != ScreenshotPhase::Idle
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenshotEvent {
    Requested(ExportRequest),
    Prepared {
        session: u64,
        result: Result<()>,
    },
    Selected {
        session: u64,
        bounds: Rect,
    },
    Output {
        session: u64,
        operation: u64,
        kind: ScreenshotOutput,
        request: ExportRequest,
    },
    Completed {
        session: u64,
        operation: u64,
        result: Result<SaveOutcome>,
    },
    Cancelled {
        session: u64,
    },
    Failed {
        session: u64,
        error: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenshotCommand {
    Prepare {
        session: u64,
        request: ExportRequest,
    },
    Show {
        session: u64,
    },
    Output {
        session: u64,
        operation: u64,
        kind: ScreenshotOutput,
        request: ExportRequest,
    },
    Restore {
        session: u64,
    },
}
pub(crate) fn reduce(state: &mut AppState, event: ScreenshotEvent) -> Vec<AppCommand> {
    let command = |command| vec![AppCommand::Screenshot(command)];
    let s = &mut state.screenshot;
    match event {
        ScreenshotEvent::Requested(request)
            if !s.active()
                && state.annotation_export.session == Some(request.session)
                && state.annotation_export.request.is_none() =>
        {
            s.session = s
                .session
                .checked_add(1)
                .expect("screenshot identity exhausted");
            s.source_session = request.session;
            s.phase = ScreenshotPhase::Preparing;
            s.operation = 0;
            s.bounds = None;
            s.error = None;
            state.toolbar_selection = None;
            command(ScreenshotCommand::Prepare {
                session: s.session,
                request,
            })
        }
        ScreenshotEvent::Prepared { session, result }
            if s.session == session && s.phase == ScreenshotPhase::Preparing =>
        {
            match result {
                Ok(()) => {
                    s.phase = ScreenshotPhase::Selecting;
                    command(ScreenshotCommand::Show { session })
                }
                Err(error) => {
                    s.phase = ScreenshotPhase::Idle;
                    s.error = Some(error.to_string());
                    command(ScreenshotCommand::Restore { session })
                }
            }
        }
        ScreenshotEvent::Selected { session, bounds }
            if s.session == session
                && matches!(
                    s.phase,
                    ScreenshotPhase::Selecting | ScreenshotPhase::Editing
                )
                && bounds.right > bounds.left
                && bounds.bottom > bounds.top =>
        {
            s.bounds = Some(bounds);
            s.phase = ScreenshotPhase::Editing;
            vec![]
        }
        ScreenshotEvent::Output {
            session,
            operation,
            kind,
            request,
        } if s.session == session
            && s.phase == ScreenshotPhase::Editing
            && operation > s.operation
            && request.session == session
            && request.request == operation
            && s.bounds == Some(request.snapshot.monitor) =>
        {
            s.operation = operation;
            s.phase = ScreenshotPhase::Output;
            s.error = None;
            command(ScreenshotCommand::Output {
                session,
                operation,
                kind,
                request,
            })
        }
        ScreenshotEvent::Completed {
            session,
            operation,
            result,
        } if s.session == session
            && s.operation == operation
            && s.phase == ScreenshotPhase::Output =>
        {
            match result {
                Ok(SaveOutcome::Saved) => {
                    s.phase = ScreenshotPhase::Idle;
                    command(ScreenshotCommand::Restore { session })
                }
                Ok(SaveOutcome::Cancelled) => {
                    s.phase = ScreenshotPhase::Editing;
                    vec![]
                }
                Err(error) => {
                    s.phase = ScreenshotPhase::Editing;
                    s.error = Some(error.to_string());
                    vec![]
                }
            }
        }
        ScreenshotEvent::Cancelled { session } | ScreenshotEvent::Failed { session, .. }
            if s.session == session && s.active() =>
        {
            if let ScreenshotEvent::Failed { error, .. } = event {
                s.error = Some(error);
            }
            s.phase = ScreenshotPhase::Idle;
            command(ScreenshotCommand::Restore { session })
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppEvent,
        domain::{
            annotation::Session,
            annotation_export::{ExportEvent, ExportSnapshot},
            settings::Settings,
        },
    };
    use std::sync::Arc;
    fn request(session: u64, operation: u64, bounds: Rect) -> ExportRequest {
        ExportRequest {
            session,
            request: operation,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: bounds,
                scale: 1.,
                monitors: vec![(bounds, 1.)],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        }
    }
    fn bounds() -> Rect {
        Rect {
            left: -100,
            top: -50,
            right: 100,
            bottom: 100,
        }
    }
    fn begin() -> AppState {
        let mut s = AppState::new(Settings::default());
        s.reduce(AppEvent::AnnotationExport(ExportEvent::Opened {
            session: 42,
        }));
        let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::Requested(request(
            42,
            1,
            bounds(),
        ))));
        assert!(matches!(
            &commands[0],
            AppCommand::Screenshot(ScreenshotCommand::Prepare { session: 1, .. })
        ));
        s
    }
    #[test]
    fn screenshot_rejects_duplicates_old_source_and_late_results() {
        let mut s = begin();
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Requested(request(
                42,
                1,
                bounds()
            ))))
            .is_empty()
        );
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
            session: 1,
        }));
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
                session: 1,
                result: Ok(())
            }))
            .is_empty()
        );
        s.reduce(AppEvent::AnnotationExport(ExportEvent::Opened {
            session: 44,
        }));
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Requested(request(
                42,
                1,
                bounds()
            ))))
            .is_empty()
        );
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Requested(request(
            44,
            1,
            bounds(),
        ))));
        assert_eq!(s.screenshot.session, 2);
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Failed {
                session: 1,
                error: "old".into()
            }))
            .is_empty()
        );
        assert!(s.screenshot.active());
    }
    #[test]
    fn output_failure_cancel_and_success_have_distinct_restoration() {
        let mut s = begin();
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Ok(()),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
            session: 1,
            bounds: bounds(),
        }));
        for (op, result, phase) in [
            (
                1,
                Err(crate::Error::new("fixture")),
                ScreenshotPhase::Editing,
            ),
            (2, Ok(SaveOutcome::Cancelled), ScreenshotPhase::Editing),
            (3, Ok(SaveOutcome::Saved), ScreenshotPhase::Idle),
        ] {
            let event = ScreenshotEvent::Output {
                session: 1,
                operation: op,
                kind: ScreenshotOutput::Copy,
                request: request(1, op, bounds()),
            };
            assert_eq!(s.reduce(AppEvent::Screenshot(event.clone())).len(), 1);
            assert!(s.reduce(AppEvent::Screenshot(event)).is_empty());
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Completed {
                    session: 1,
                    operation: op + 1,
                    result: Ok(SaveOutcome::Saved)
                }))
                .is_empty()
            );
            let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::Completed {
                session: 1,
                operation: op,
                result,
            }));
            assert_eq!(s.screenshot.phase, phase);
            assert_eq!(!commands.is_empty(), phase == ScreenshotPhase::Idle);
        }
    }
    #[test]
    fn prepare_failure_restores_and_screen_entrypoints_are_suppressed() {
        let mut s = begin();
        assert!(s.reduce(AppEvent::SelectionInteractionStarted).is_empty());
        assert!(
            s.reduce(AppEvent::Ocr(crate::domain::ocr::OcrEvent::Requested {
                annotation_active: false
            }))
            .is_empty()
        );
        let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Err(crate::Error::new("fixture")),
        }));
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Idle);
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::Screenshot(ScreenshotCommand::Restore {
                session: 1
            })]
        ));
    }
    #[test]
    fn settings_restores_before_opening_and_invalid_crop_output_is_rejected() {
        let mut s = begin();
        let commands = s.reduce(AppEvent::SettingsWindowRequested);
        assert!(matches!(
            commands.first(),
            Some(AppCommand::Screenshot(ScreenshotCommand::Restore {
                session: 1
            }))
        ));
        assert!(matches!(
            commands.get(1),
            Some(AppCommand::ShowSettingsWindow)
        ));
        let mut s = begin();
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Ok(()),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
            session: 1,
            bounds: bounds(),
        }));
        let mut bad = bounds();
        bad.right -= 10;
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Output {
                session: 1,
                operation: 1,
                kind: ScreenshotOutput::Save,
                request: request(1, 1, bad)
            }))
            .is_empty()
        );
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Editing);
    }
}
