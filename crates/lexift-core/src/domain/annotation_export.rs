//! Export ownership is independent of rendering and platform resources.
use super::{annotation::Session, geometry::Rect};
use crate::ports::annotation_save::SaveOutcome;
use crate::ports::magnifier::WindowToken;
use crate::{AppCommand, AppState, Result};
use std::sync::Arc;

/// Immutable visible document; excludes undo history and all UI components.
pub struct ExportSnapshot {
    pub session: Arc<Session>,
    pub monitor: Rect,
    pub scale: f32,
    pub monitors: Vec<(Rect, f32)>,
    pub excluded: Vec<WindowToken>,
    pub owner: Option<WindowToken>,
    pub name: String,
    pub title: String,
}
#[derive(Clone)]
pub struct ExportRequest {
    pub session: u64,
    pub request: u64,
    pub snapshot: Arc<ExportSnapshot>,
}
// Equality describes the immutable request identity, never document contents.
impl PartialEq for ExportRequest {
    fn eq(&self, other: &Self) -> bool {
        self.session == other.session
            && self.request == other.request
            && Arc::ptr_eq(&self.snapshot, &other.snapshot)
    }
}
impl Eq for ExportRequest {}
impl std::fmt::Debug for ExportRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportRequest")
            .field("session", &self.session)
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportEvent {
    Opened {
        session: u64,
    },
    Requested(ExportRequest),
    Closed {
        session: u64,
    },
    Completed {
        session: u64,
        request: u64,
        result: Result<SaveOutcome>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportCommand {
    Run(ExportRequest),
    Cancel { session: u64 },
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportState {
    pub last_session: u64,
    pub session: Option<u64>,
    pub request: Option<u64>,
    pub completed: Option<u64>,
    pub result: Option<Result<SaveOutcome>>,
}
pub(crate) fn reduce(state: &mut AppState, event: ExportEvent) -> Vec<AppCommand> {
    let s = &mut state.annotation_export;
    let command = |c| vec![AppCommand::AnnotationExport(c)];
    match event {
        ExportEvent::Opened { session } => {
            if session <= s.last_session {
                return vec![];
            }
            let previous = s.session;
            *s = ExportState {
                last_session: session,
                session: Some(session),
                ..Default::default()
            };
            previous.map_or_else(Vec::new, |session| {
                command(ExportCommand::Cancel { session })
            })
        }
        ExportEvent::Requested(request)
            if s.session == Some(request.session)
                && s.request.is_none()
                && s.completed.is_none_or(|id| request.request > id) =>
        {
            s.request = Some(request.request);
            s.result = None;
            command(ExportCommand::Run(request))
        }
        ExportEvent::Closed { session } if s.session == Some(session) => {
            *s = ExportState {
                last_session: s.last_session,
                ..Default::default()
            };
            command(ExportCommand::Cancel { session })
        }
        ExportEvent::Completed {
            session,
            request,
            result,
        } if s.session == Some(session) && s.request == Some(request) => {
            s.request = None;
            s.completed = Some(request);
            s.result = Some(result);
            vec![]
        }
        _ => vec![],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppEvent;
    use crate::domain::settings::Settings;
    fn request(session: u64, request: u64) -> ExportRequest {
        ExportRequest {
            session,
            request,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: Rect {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10,
                },
                scale: 1.,
                monitors: vec![],
                excluded: vec![],
                owner: None,
                name: String::new(),
                title: String::new(),
            }),
        }
    }
    #[test]
    fn duplicate_and_retired_exports_cannot_run_or_complete() {
        let mut state = AppState::new(Settings::default());
        let send = |s: &mut AppState, e| s.reduce(AppEvent::AnnotationExport(e));
        assert!(send(&mut state, ExportEvent::Requested(request(1, 1))).is_empty());
        send(&mut state, ExportEvent::Opened { session: 1 });
        assert_eq!(
            send(&mut state, ExportEvent::Requested(request(1, 1))).len(),
            1
        );
        assert!(send(&mut state, ExportEvent::Requested(request(1, 2))).is_empty());
        assert_eq!(
            send(&mut state, ExportEvent::Closed { session: 1 }).len(),
            1
        );
        send(&mut state, ExportEvent::Opened { session: 2 });
        send(
            &mut state,
            ExportEvent::Completed {
                session: 1,
                request: 1,
                result: Ok(SaveOutcome::Saved),
            },
        );
        assert!(state.annotation_export.result.is_none());
        assert!(send(&mut state, ExportEvent::Opened { session: 1 }).is_empty());
        assert_eq!(state.annotation_export.session, Some(2));
        send(&mut state, ExportEvent::Requested(request(2, 3)));
        send(
            &mut state,
            ExportEvent::Completed {
                session: 2,
                request: 3,
                result: Ok(SaveOutcome::Cancelled),
            },
        );
        assert_eq!(state.annotation_export.completed, Some(3));
        assert!(send(&mut state, ExportEvent::Requested(request(2, 3))).is_empty());
        assert_eq!(
            send(&mut state, ExportEvent::Requested(request(2, 4))).len(),
            1
        );
    }
    #[test]
    fn failed_export_can_retry_and_exit_retires_the_session() {
        let mut state = AppState::new(Settings::default());
        state.reduce(AppEvent::AnnotationExport(ExportEvent::Opened {
            session: 1,
        }));
        state.reduce(AppEvent::AnnotationExport(ExportEvent::Requested(request(
            1, 1,
        ))));
        state.reduce(AppEvent::AnnotationExport(ExportEvent::Completed {
            session: 1,
            request: 1,
            result: Err(crate::Error::new("save failed")),
        }));
        assert!(state.annotation_export.result.as_ref().unwrap().is_err());
        assert_eq!(
            state
                .reduce(AppEvent::AnnotationExport(ExportEvent::Requested(request(
                    1, 2
                ))))
                .len(),
            1
        );
        let commands = state.reduce(AppEvent::ExitRequested);
        assert!(matches!(
            commands.first(),
            Some(AppCommand::AnnotationExport(ExportCommand::Cancel {
                session: 1
            }))
        ));
        assert!(state.annotation_export.session.is_none());
        assert!(
            state
                .reduce(AppEvent::AnnotationExport(ExportEvent::Opened {
                    session: 1
                }))
                .is_empty()
        );
    }
}
