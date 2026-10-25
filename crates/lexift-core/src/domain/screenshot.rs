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
    CountingDown,
    Preparing,
    Selecting,
    Editing,
    Output,
    Recognizing,
}
pub use super::ocr::OcrOutcome;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotOutput {
    Copy,
    Save,
    Pin,
}
/// Geometry only: no pixels, document, or persisted screenshot history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastRegion {
    pub bounds: Rect,
    pub layout: Vec<Rect>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenshotState {
    pub long: Option<super::long_capture::workflow::State>,
    pub session: u64,
    pub source_session: u64,
    pub phase: ScreenshotPhase,
    pub operation: u64,
    pub bounds: Option<Rect>,
    pub layout: Vec<Rect>,
    pub last_region: Option<LastRegion>,
    pub reuse_sequence: u64,
    pub error: Option<String>,
    pub ocr_result: Option<OcrOutcome>,
    pub element_sequence: u64,
    pub element_pending: Option<super::element_candidate::ElementQuery>,
    pub color_sequence: u64,
    pub color_pending: Option<ColorRequest>,
    pub color_result: Option<ColorResult>,
}
impl ScreenshotState {
    pub fn editable(&self) -> bool {
        self.phase == ScreenshotPhase::Editing
            && self
                .long
                .as_ref()
                .is_none_or(|l| l.phase == super::long_capture::workflow::Phase::Editing)
    }
    /// Physical layout equality ignores enumeration order and logical DPI.
    pub fn reusable_region(&self) -> Option<Rect> {
        let last = self.last_region.as_ref()?;
        if self.phase != ScreenshotPhase::Selecting || self.layout.is_empty() {
            return None;
        }
        let normalize = |layout: &[Rect]| {
            let mut layout = layout.to_vec();
            layout.sort_by_key(|r| (r.left, r.top, r.right, r.bottom));
            layout.dedup();
            layout
        };
        (normalize(&last.layout) == normalize(&self.layout)).then_some(last.bounds)
    }
    pub fn active(&self) -> bool {
        self.phase != ScreenshotPhase::Idle
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenshotEvent {
    Delayed {
        request: ExportRequest,
        seconds: u8,
    },
    DelayElapsed {
        session: u64,
        request: ExportRequest,
    },
    Long(super::long_capture::workflow::Event),
    ReuseRegion {
        session: u64,
        host: u64,
        request: u64,
    },
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
    QueryElements(super::element_candidate::ElementQuery),
    ElementsPrepared(super::element_candidate::ElementQueryResult),
    CancelElements {
        session: u64,
        request: u64,
    },
    Requested(ExportRequest),
    Prepared {
        session: u64,
        result: Result<Vec<Rect>>,
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
    Countdown {
        session: u64,
        seconds: u8,
        request: ExportRequest,
    },
    Long(super::long_capture::workflow::Command),
    ReuseRegion {
        session: u64,
        host: u64,
        request: u64,
        bounds: Option<Rect>,
    },
    QueryElements(super::element_candidate::ElementQuery),
    UpdateElements(super::element_candidate::ElementQueryResult),
    CancelElements {
        session: u64,
    },
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
/// Shared eligibility for the domain request and its local menu affordance.
pub fn delay_available(state: &AppState) -> bool {
    state.pin_visibility.pending.is_none()
        && !state.screenshot.active()
        && state.image_import.request.is_none()
        && state.image_history.window.as_ref().is_none_or(|w| !w.busy)
        && state.image_history.editor.is_none()
        && state.annotation_export.request.is_none()
        && !state
            .pinned_images
            .values()
            .any(|p| p.editing.is_some() || p.busy)
        && !matches!(
            state.ocr.phase,
            super::ocr::OcrPhase::Checking
                | super::ocr::OcrPhase::Capturing
                | super::ocr::OcrPhase::Recognizing
                | super::ocr::OcrPhase::Translating
        )
}
pub(crate) fn reduce(state: &mut AppState, event: ScreenshotEvent) -> Vec<AppCommand> {
    let command = |command| vec![AppCommand::Screenshot(command)];
    let delay_allowed = delay_available(state);
    let s = &mut state.screenshot;
    match event {
        ScreenshotEvent::Delayed { request, seconds }
            if matches!(seconds, 3 | 5 | 10)
                && delay_allowed
                && state.annotation_export.session == Some(request.session) =>
        {
            s.session = s
                .session
                .checked_add(1)
                .expect("screenshot identity exhausted");
            s.source_session = request.session;
            s.phase = ScreenshotPhase::CountingDown;
            s.reuse_sequence = 0;
            s.element_sequence = 0;
            s.error = None;
            s.ocr_result = None;
            s.bounds = None;
            s.layout.clear();
            s.long = None;
            s.element_pending = None;
            s.color_pending = None;
            s.color_result = None;
            s.operation = 0;
            state.toolbar_selection = None;
            command(ScreenshotCommand::Countdown {
                session: s.session,
                seconds,
                request,
            })
        }
        ScreenshotEvent::DelayElapsed { session, request }
            if s.session == session
                && s.phase == ScreenshotPhase::CountingDown
                && s.source_session == request.session =>
        {
            s.phase = ScreenshotPhase::Preparing;
            command(ScreenshotCommand::Prepare { session, request })
        }
        ScreenshotEvent::Long(event) => super::long_capture::workflow::reduce(s, event)
            .into_iter()
            .map(AppCommand::Screenshot)
            .collect(),
        ScreenshotEvent::ReuseRegion {
            session,
            host,
            request,
        } if s.session == session
            && s.phase == ScreenshotPhase::Selecting
            && request > s.reuse_sequence =>
        {
            s.reuse_sequence = request;
            command(ScreenshotCommand::ReuseRegion {
                session,
                host,
                request,
                bounds: s.reusable_region(),
            })
        }
        ScreenshotEvent::Recognize { session, bounds }
            if s.session == session && s.editable() && s.bounds == Some(bounds) =>
        {
            let strategy = super::ocr::copy_strategy(&state.settings);
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
            s.long = None;
            command(ScreenshotCommand::Restore { session })
        }
        ScreenshotEvent::CopyColor {
            session,
            point,
            color,
        } if s.session == session
            && s.long.is_none()
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
            && s.long.is_none()
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
            s.element_sequence = 0;
            s.element_pending = None;
            s.operation = 0;
            s.long = None;
            s.layout.clear();
            s.reuse_sequence = 0;
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
        ScreenshotEvent::QueryElements(query)
            if query.session == s.session
                && s.phase == ScreenshotPhase::Selecting
                && query.request > s.element_sequence =>
        {
            s.element_sequence = query.request;
            s.element_pending = Some(query.clone());
            command(ScreenshotCommand::QueryElements(query))
        }
        ScreenshotEvent::CancelElements { session, request }
            if session == s.session && request >= s.element_sequence =>
        {
            s.element_sequence = request;
            s.element_pending = None;
            command(ScreenshotCommand::CancelElements { session })
        }
        ScreenshotEvent::ElementsPrepared(result)
            if s.phase == ScreenshotPhase::Selecting
                && s.element_pending.as_ref() == Some(&result.query) =>
        {
            s.element_pending = None;
            command(ScreenshotCommand::UpdateElements(result))
        }
        ScreenshotEvent::Prepared { session, result }
            if s.session == session && s.phase == ScreenshotPhase::Preparing =>
        {
            match result {
                Ok(layout) => {
                    s.layout = layout;
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
                && s.long.is_none()
                && matches!(
                    s.phase,
                    ScreenshotPhase::Selecting | ScreenshotPhase::Editing
                )
                && bounds.right > bounds.left
                && bounds.bottom > bounds.top =>
        {
            s.bounds = Some(bounds);
            s.phase = ScreenshotPhase::Editing;
            s.element_pending = None;
            vec![]
        }
        ScreenshotEvent::Output {
            session,
            operation,
            kind,
            request,
        } if s.session == session
            && (s.editable()
                || s.phase == ScreenshotPhase::Editing
                    && s.long.as_ref().is_some_and(|l| {
                        l.phase == super::long_capture::workflow::Phase::Frozen
                    }))
            && operation > s.operation
            && request.session == session
            && request.request == operation
            && (s.bounds == Some(request.snapshot.monitor)
                || s.long
                    .as_ref()
                    .is_some_and(|l| l.phase == super::long_capture::workflow::Phase::Editing)
                    && s.bounds.is_some_and(|full| {
                        let b = request.snapshot.monitor;
                        request.snapshot.session.image_crop == Some(b)
                            && b.left >= full.left
                            && b.top >= full.top
                            && b.right <= full.right
                            && b.bottom <= full.bottom
                            && b.right > b.left
                            && b.bottom > b.top
                    })) =>
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
                    if let Some(bounds) = s
                        .bounds
                        .filter(|_| !s.layout.is_empty() && s.long.is_none())
                    {
                        s.last_region = Some(LastRegion {
                            bounds,
                            layout: s.layout.clone(),
                        });
                    }
                    s.phase = ScreenshotPhase::Idle;
                    s.long = None;
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
            s.element_pending = None;
            s.long = None;
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
    fn delay_starts_without_capture_and_only_current_deadline_can_prepare() {
        for seconds in [3, 5, 10] {
            let mut s = begin();
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
                session: 1,
            }));
            let r = request(42, 1, bounds());
            let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
                request: r.clone(),
                seconds,
            }));
            assert!(
                matches!(&commands[..],[AppCommand::Screenshot(ScreenshotCommand::Countdown { session:2, seconds:n,.. })] if *n==seconds)
            );
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Requested(r.clone())))
                    .is_empty()
            );
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
                    request: r.clone(),
                    seconds: 5
                }))
                .is_empty()
            );
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::DelayElapsed {
                    session: 1,
                    request: r.clone()
                }))
                .is_empty()
            );
            assert_eq!(s.screenshot.phase, ScreenshotPhase::CountingDown);
            let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::DelayElapsed {
                session: 2,
                request: r.clone(),
            }));
            assert!(matches!(
                &commands[..],
                [AppCommand::Screenshot(ScreenshotCommand::Prepare {
                    session: 2,
                    ..
                })]
            ));
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::DelayElapsed {
                    session: 2,
                    request: r
                }))
                .is_empty()
            );
        }
    }
    #[test]
    fn cancelled_delay_rejects_late_deadline_and_preserves_region_and_image_history() {
        let mut s = begin();
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
            session: 1,
        }));
        s.screenshot.last_region = Some(LastRegion {
            bounds: bounds(),
            layout: vec![bounds()],
        });
        let before = s.image_history.clone();
        let last = s.screenshot.last_region.clone();
        let r = request(42, 1, bounds());
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
            request: r.clone(),
            seconds: 3,
        }));
        assert!(matches!(
            &s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
                session: 2
            }))[..],
            [AppCommand::Screenshot(ScreenshotCommand::Restore {
                session: 2
            })]
        ));
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::DelayElapsed {
                session: 2,
                request: r
            }))
            .is_empty()
        );
        assert_eq!(s.image_history, before);
        assert_eq!(s.screenshot.last_region, last);
    }
    #[test]
    fn delay_rejects_invalid_durations_and_busy_workflows() {
        let mut s = begin();
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
            session: 1,
        }));
        for seconds in [0, 1, 2, 4, 255] {
            assert!(
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
                    request: request(42, 1, bounds()),
                    seconds
                }))
                .is_empty()
            );
        }
        s.image_history.window = Some(super::super::image_history::workflow::Window {
            id: 1,
            anchor: super::super::geometry::Point { x: 0, y: 0 },
            selected: None,
            operation: 1,
            busy: true,
            error: false,
        });
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
                request: request(42, 1, bounds()),
                seconds: 3
            }))
            .is_empty()
        );
        s.image_history.window = None;
        s.ocr.phase = super::super::ocr::OcrPhase::Recognizing;
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
                request: request(42, 1, bounds()),
                seconds: 3
            }))
            .is_empty()
        );
        s.ocr.phase = super::super::ocr::OcrPhase::Error;
        assert!(
            !s.reduce(AppEvent::Screenshot(ScreenshotEvent::Delayed {
                request: request(42, 1, bounds()),
                seconds: 3
            }))
            .is_empty()
        );
    }
    #[test]
    fn cropped_long_output_requires_valid_image_metadata_and_ignores_stale_results() {
        use super::super::long_capture::{
            Feedback,
            workflow::{Phase, State},
        };
        let full = Rect {
            left: 0,
            top: 0,
            right: 600,
            bottom: 2000,
        };
        let crop = Rect {
            left: 80,
            top: 200,
            right: 520,
            bottom: 1800,
        };
        for phase in [Phase::Editing, Phase::Capturing] {
            for metadata in [false, true] {
                let mut s = AppState::default();
                s.screenshot.session = 10;
                s.screenshot.phase = ScreenshotPhase::Editing;
                s.screenshot.bounds = Some(full);
                s.screenshot.long = Some(State {
                    operation: 1,
                    source: full,
                    phase,
                    sequence: 1,
                    width: 600,
                    height: 2000,
                    feedback: Feedback::Accepted,
                    target: None,
                });
                let mut req = request(10, 2, crop);
                if metadata {
                    Arc::make_mut(&mut Arc::make_mut(&mut req.snapshot).session)
                        .set_image_crop(crop, full);
                }
                let commands = s.reduce(AppEvent::Screenshot(ScreenshotEvent::Output {
                    session: 10,
                    operation: 2,
                    kind: ScreenshotOutput::Copy,
                    request: req,
                }));
                assert_eq!(!commands.is_empty(), phase == Phase::Editing && metadata);
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Completed {
                    session: 9,
                    operation: 2,
                    result: Ok(SaveOutcome::Saved),
                }));
                assert_eq!(s.screenshot.session, 10);
            }
        }
    }
    #[test]
    fn last_region_records_only_successful_image_outputs_and_uses_final_bounds() {
        for kind in [
            ScreenshotOutput::Copy,
            ScreenshotOutput::Save,
            ScreenshotOutput::Pin,
        ] {
            let mut s = begin();
            let previous = LastRegion {
                bounds: bounds(),
                layout: vec![bounds()],
            };
            s.screenshot.last_region = Some(previous.clone());
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
                session: 1,
                result: Ok(vec![bounds()]),
            }));
            let adjusted = Rect {
                left: -50,
                top: -20,
                right: 90,
                bottom: 80,
            };
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
                session: 1,
                bounds: adjusted,
            }));
            for (operation, result) in [
                (1, Err(crate::Error::new("fixture"))),
                (2, Ok(SaveOutcome::Cancelled)),
                (3, Ok(SaveOutcome::Saved)),
            ] {
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Output {
                    session: 1,
                    operation,
                    kind,
                    request: request(1, operation, adjusted),
                }));
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Completed {
                    session: 99,
                    operation,
                    result: Ok(SaveOutcome::Saved),
                }));
                assert_eq!(s.screenshot.last_region, Some(previous.clone()));
                // Geometry cannot change after the output snapshot was frozen.
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
                    session: 1,
                    bounds: bounds(),
                }));
                s.reduce(AppEvent::Screenshot(ScreenshotEvent::Completed {
                    session: 1,
                    operation,
                    result,
                }));
                assert_eq!(
                    s.screenshot.last_region.as_ref().unwrap().bounds,
                    if operation == 3 { adjusted } else { bounds() }
                );
            }
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Requested(request(
                42,
                1,
                bounds(),
            ))));
            assert_eq!(s.screenshot.session, 2);
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
                session: 2,
                result: Ok(vec![bounds()]),
            }));
            assert_eq!(s.screenshot.reusable_region(), Some(adjusted));
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::Cancelled {
                session: 2,
            }));
            assert_eq!(s.screenshot.last_region.unwrap().bounds, adjusted);
        }
    }
    #[test]
    fn reuse_layout_is_physical_order_independent_and_requests_are_scoped() {
        let mut s = begin();
        let left = Rect {
            left: -300,
            top: -50,
            right: -100,
            bottom: 100,
        };
        let saved = LastRegion {
            bounds: bounds(),
            layout: vec![left, bounds()],
        };
        s.screenshot.last_region = Some(saved.clone());
        assert_eq!(s.screenshot.reusable_region(), None);
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Ok(vec![bounds(), left]),
        }));
        assert_eq!(s.screenshot.reusable_region(), Some(bounds()));
        let event = ScreenshotEvent::ReuseRegion {
            session: 1,
            host: 12,
            request: 1,
        };
        assert!(
            matches!(s.reduce(AppEvent::Screenshot(event.clone())).as_slice(),
            [AppCommand::Screenshot(ScreenshotCommand::ReuseRegion { session: 1, host: 12, request: 1, bounds: Some(b) })] if *b == bounds())
        );
        assert!(s.reduce(AppEvent::Screenshot(event)).is_empty());
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::ReuseRegion {
                session: 0,
                host: 12,
                request: 2
            }))
            .is_empty()
        );
        s.screenshot.layout = vec![bounds()];
        assert_eq!(s.screenshot.reusable_region(), None);
        assert!(matches!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::ReuseRegion {
                session: 1,
                host: 12,
                request: 2
            }))
            .as_slice(),
            [AppCommand::Screenshot(ScreenshotCommand::ReuseRegion {
                bounds: None,
                ..
            })]
        ));
        s.screenshot.layout = vec![left, bounds()];
        assert_eq!(s.screenshot.reusable_region(), Some(bounds()));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
            session: 1,
            bounds: bounds(),
        }));
        assert!(
            s.reduce(AppEvent::Screenshot(ScreenshotEvent::ReuseRegion {
                session: 1,
                host: 12,
                request: 3
            }))
            .is_empty()
        );
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::CopyColor {
            session: 1,
            point: Point { x: 0, y: 0 },
            color: Rgb(1, 2, 3),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::ColorCopied {
            session: 1,
            request: 1,
            result: Ok(()),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognize {
            session: 1,
            bounds: bounds(),
        }));
        s.reduce(AppEvent::Screenshot(ScreenshotEvent::Recognized {
            session: 1,
            operation: s.screenshot.operation,
            outcome: OcrOutcome::Copied,
        }));
        assert_eq!(s.screenshot.last_region, Some(saved));
        assert_eq!(ScreenshotState::default().last_region, None);
    }
    #[test]
    fn point_queries_replace_pending_and_ignore_cancelled_or_late_results() {
        use super::super::{
            element_candidate::{ElementQuery, ElementQueryResult},
            window_candidate::WindowCandidate,
        };
        use crate::ports::magnifier::WindowToken;
        let mut state = begin();
        let query = |request| ElementQuery {
            session: 1,
            request,
            host: 9,
            point: Point { x: 0, y: 0 },
            window: WindowCandidate {
                process_id: 0,
                window: WindowToken(7),
                bounds: bounds(),
                order: 0,
            },
        };
        let result = |request| {
            AppEvent::Screenshot(ScreenshotEvent::ElementsPrepared(ElementQueryResult {
                query: query(request),
                elements: vec![],
                leaf: false,
            }))
        };
        assert!(
            state
                .reduce(AppEvent::Screenshot(ScreenshotEvent::QueryElements(query(
                    1
                ))))
                .is_empty()
        );
        state.reduce(AppEvent::Screenshot(ScreenshotEvent::Prepared {
            session: 1,
            result: Ok(vec![bounds()]),
        }));
        assert!(
            !state
                .reduce(AppEvent::Screenshot(ScreenshotEvent::QueryElements(query(
                    1
                ))))
                .is_empty()
        );
        state.reduce(AppEvent::Screenshot(ScreenshotEvent::QueryElements(query(
            2,
        ))));
        assert!(state.reduce(result(1)).is_empty());
        assert!(matches!(
            &state.reduce(result(2))[0],
            AppCommand::Screenshot(ScreenshotCommand::UpdateElements(_))
        ));
        assert!(state.reduce(result(2)).is_empty());
        state.reduce(AppEvent::Screenshot(ScreenshotEvent::QueryElements(query(
            3,
        ))));
        state.reduce(AppEvent::Screenshot(ScreenshotEvent::CancelElements {
            session: 1,
            request: 4,
        }));
        assert!(state.reduce(result(3)).is_empty());
        state.reduce(AppEvent::Screenshot(ScreenshotEvent::QueryElements(query(
            5,
        ))));
        state.reduce(AppEvent::Screenshot(ScreenshotEvent::Selected {
            session: 1,
            bounds: bounds(),
        }));
        assert!(state.reduce(result(5)).is_empty());
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
                result: Ok(vec![bounds()]),
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
            result: Ok(vec![bounds()]),
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
            result: Ok(vec![bounds()]),
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
                result: Ok(vec![bounds()])
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
            result: Ok(vec![bounds()]),
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
            result: Ok(vec![bounds()]),
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
