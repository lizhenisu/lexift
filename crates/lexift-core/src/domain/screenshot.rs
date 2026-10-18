//! Screenshot workflow identities; image ownership and native resources stay in adapters.
use super::{annotation_export::ExportRequest, geometry::Rect};
use super::{geometry::Point, screen_snapshot::Rgb};
use crate::ports::annotation_save::SaveOutcome;
use crate::{AppCommand, AppState, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorRequest {
    pub id: u64,
    pub point: Point,
    pub color: Rgb,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorResult {
    pub request: ColorRequest,
    pub success: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenshotPhase {
    #[default]
    Idle,
    Preparing,
    Selecting,
    Editing,
    Output,
    Recognizing,
}
/// Terminal screenshot OCR feedback never retains recognized text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrOutcome {
    Copied,
    Empty,
    ResourcesUnavailable,
    RecognitionFailed,
    CopyFailed,
}
impl OcrOutcome {
    pub fn message(self) -> &'static str {
        match self {
            Self::Copied => "Recognized text copied",
            Self::Empty => "No text recognized",
            Self::ResourcesUnavailable => "OCR resources unavailable. Manage them in Settings.",
            Self::RecognitionFailed => "Text recognition failed",
            Self::CopyFailed => "Could not copy recognized text",
        }
    }
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
    pub ocr_result: Option<OcrOutcome>,
    pub color_sequence: u64,
    pub color_pending: Option<ColorRequest>,
    pub color_result: Option<ColorResult>,
}
impl ScreenshotState {
    pub fn active(&self) -> bool {
        self.phase != ScreenshotPhase::Idle
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenshotEvent {
    Recognize {
        session: u64,
        bounds: Rect,
    },
    Recognized {
        session: u64,
        operation: u64,
        outcome: OcrOutcome,
    },
    CopyColor {
        session: u64,
        point: Point,
        color: Rgb,
    },
    ColorCopied {
        session: u64,
        request: u64,
        result: Result<()>,
    },
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
    Recognize {
        session: u64,
        operation: u64,
        bounds: Rect,
        strategy: super::ocr::OcrRecognitionStrategy,
    },
    CopyColor {
        session: u64,
        request: ColorRequest,
    },
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
        ScreenshotEvent::Recognize { session, bounds }
            if s.session == session
                && s.phase == ScreenshotPhase::Editing
                && s.bounds == Some(bounds) =>
        {
            let mut strategy = super::ocr::OcrRecognitionStrategy::from_settings(
                state.settings.ocr_language.as_deref(),
                state.settings.ui_language.code(),
            );
            if state.settings.ocr_korean_auto
                && strategy == super::ocr::OcrRecognitionStrategy::Automatic
            {
                strategy = super::ocr::OcrRecognitionStrategy::AutomaticKorean;
            }
            s.operation = s
                .operation
                .checked_add(1)
                .expect("screenshot operation exhausted");
            s.phase = ScreenshotPhase::Recognizing;
            s.color_pending = None;
            s.color_result = None;
            s.error = None;
            s.ocr_result = None;
            command(ScreenshotCommand::Recognize {
                session,
                operation: s.operation,
                bounds,
                strategy,
            })
        }
        ScreenshotEvent::Recognized {
            session,
            operation,
            outcome,
        } if s.session == session
            && s.operation == operation
            && s.phase == ScreenshotPhase::Recognizing =>
        {
            s.phase = ScreenshotPhase::Idle;
            s.ocr_result = Some(outcome);
            command(ScreenshotCommand::Restore { session })
        }
        ScreenshotEvent::CopyColor {
            session,
            point,
            color,
        } if s.session == session
            && matches!(
                s.phase,
                ScreenshotPhase::Selecting | ScreenshotPhase::Editing
            )
            && s.color_pending.is_none() =>
        {
            s.color_sequence = s
                .color_sequence
                .checked_add(1)
                .expect("color identity exhausted");
            let request = ColorRequest {
                id: s.color_sequence,
                point,
                color,
            };
            s.color_pending = Some(request);
            s.color_result = None;
            command(ScreenshotCommand::CopyColor { session, request })
        }
        ScreenshotEvent::ColorCopied {
            session,
            request,
            result,
        } if s.session == session
            && matches!(
                s.phase,
                ScreenshotPhase::Selecting | ScreenshotPhase::Editing
            )
            && s.color_pending.is_some_and(|p| p.id == request) =>
        {
            s.color_result = s.color_pending.take().map(|request| ColorResult {
                request,
                success: result.is_ok(),
            });
            vec![]
        }
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
            s.ocr_result = None;
            s.bounds = None;
            s.error = None;
            s.color_pending = None;
            s.color_result = None;
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
            s.color_pending = None;
            s.color_result = None;
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
            s.color_pending = None;
            s.color_result = None;
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
    fn ocr_freezes_strategy_rejects_duplicates_and_closes_for_every_outcome() {
        for outcome in [
            OcrOutcome::Copied,
            OcrOutcome::Empty,
            OcrOutcome::ResourcesUnavailable,
            OcrOutcome::RecognitionFailed,
            OcrOutcome::CopyFailed,
        ] {
            let mut s = begin();
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
                session: 1,
                result: Ok(()),
            }));
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
                session: 1,
                bounds: bounds(),
            }));
            s.settings.ocr_language = Some("auto".into());
            s.settings.ocr_korean_auto = true;
            let event = ScreenshotEvent::Recognize {
                session: 1,
                bounds: bounds(),
            };
            let commands = s.reduce(AppEvent::Screenshot(event.clone()));
            assert!(matches!(
                &commands[..],
                [AppCommand::Screenshot(ScreenshotCommand::Recognize {
                    session: 1,
                    operation: 1,
                    strategy: super::super::ocr::OcrRecognitionStrategy::AutomaticKorean,
                    ..
                })]
            ));
            assert_eq!(s.screenshot.phase, ScreenshotPhase::Recognizing);
            assert!(s.reduce(AppEvent::Screenshot(event)).is_empty());
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
                    session: 1,
                    bounds: Rect {
                        right: 1,
                        ..bounds()
                    }
                }))
                .is_empty()
            );
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognized {
                    session: 1,
                    operation: 0,
                    outcome
                }))
                .is_empty()
            );
            let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognized {
                session: 1,
                operation: 1,
                outcome,
            }));
            assert!(matches!(
                &commands[..],
                [AppCommand::Screenshot(ScreenshotCommand::Restore {
                    session: 1
                })]
            ));
            assert_eq!(s.screenshot.phase, ScreenshotPhase::Idle);
            assert_eq!(s.screenshot.ocr_result, Some(outcome));
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognized {
                    session: 1,
                    operation: 1,
                    outcome
                }))
                .is_empty()
            );
        }
    }
    #[test]
    fn cancelled_ocr_cannot_finish_or_override_a_new_screenshot() {
        let mut s = begin();
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Ok(()),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
            session: 1,
            bounds: bounds(),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognize {
            session: 1,
            bounds: bounds(),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
            session: 1,
        }));
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognized {
                session: 1,
                operation: 1,
                outcome: OcrOutcome::Copied
            }))
            .is_empty()
        );
        assert_eq!(s.screenshot.ocr_result, None);
        s.screenshot.session = 2;
        s.screenshot.phase = ScreenshotPhase::Editing;
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognized {
                session: 1,
                operation: 1,
                outcome: OcrOutcome::Copied
            }))
            .is_empty()
        );
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Editing);
    }
    #[test]
    fn color_copy_has_independent_identity_and_never_finishes_a_screenshot() {
        let mut s = begin();
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Ok(()),
        }));
        let event = ScreenshotEvent::CopyColor {
            session: 1,
            point: Point { x: -10, y: 20 },
            color: Rgb(10, 20, 30),
        };
        assert!(matches!(
            s.reduce(AppEvent::Screenshot(event.clone())).as_slice(),
            [AppCommand::Screenshot(ScreenshotCommand::CopyColor {
                request: ColorRequest { id: 1, .. },
                ..
            })]
        ));
        assert_eq!(s.screenshot.operation, 0);
        assert!(s.reduce(AppEvent::Screenshot(event.clone())).is_empty());
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::ColorCopied {
            session: 1,
            request: 99,
            result: Ok(()),
        }));
        assert!(s.screenshot.color_pending.is_some());
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
            session: 1,
            bounds: bounds(),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::ColorCopied {
            session: 1,
            request: 1,
            result: Ok(()),
        }));
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Editing);
        assert!(s.screenshot.color_result.unwrap().success);
        assert_eq!(s.screenshot.bounds, Some(bounds()));
        s.reduce(AppEvent::Screenshot(event.clone()));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::ColorCopied {
            session: 1,
            request: 2,
            result: Err(crate::Error::new("fixture")),
        }));
        assert!(!s.screenshot.color_result.unwrap().success);
        assert!(s.screenshot.error.is_none());
        s.reduce(AppEvent::Screenshot(event));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Output {
            session: 1,
            operation: 1,
            kind: ScreenshotOutput::Copy,
            request: request(1, 1, bounds()),
        }));
        assert!(s.screenshot.color_pending.is_none());
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::ColorCopied {
            session: 1,
            request: 3,
            result: Ok(()),
        }));
        assert!(s.screenshot.color_result.is_none());
        assert_eq!(s.screenshot.phase, ScreenshotPhase::Output);
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
