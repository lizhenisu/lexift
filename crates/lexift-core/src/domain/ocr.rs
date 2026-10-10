//! OCR session ownership; screenshots remain owned by the capture adapter.
use super::{
    geometry::{Point, Rect},
    translation::PopupSessionId,
};
use crate::{AppCommand, AppEvent, AppState, TranslationPhase};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OcrPhase {
    #[default]
    Idle,
    Checking,
    DownloadRequired,
    Downloading,
    Capturing,
    Recognizing,
    Translating,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcrState {
    pub generation: u64,
    pub phase: OcrPhase,
    pub popup: Option<PopupSessionId>,
    pub resources_ready: bool,
    pub download_bytes: u64,
    pub downloaded_bytes: u64,
    pub error: String,
}

#[derive(Clone, PartialEq, Eq)]
pub enum OcrEvent {
    RefreshResources,
    SelectionSubmitted {
        selection: super::region_selection::RegionSelectionResult,
    },
    Requested {
        annotation_active: bool,
    },
    Resources {
        generation: u64,
        ready: bool,
        bytes: u64,
    },
    DownloadRequested,
    DownloadProgress {
        generation: u64,
        received: u64,
        total: u64,
    },
    DownloadFinished {
        generation: u64,
    },
    Selected {
        generation: u64,
        bounds: Rect,
    },
    Recognized {
        generation: u64,
        text: String,
    },
    Failed {
        generation: u64,
        error: String,
    },
    Cancelled,
}

impl std::fmt::Debug for OcrEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OcrEvent")
            .field("kind", &std::mem::discriminant(self))
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrCommand {
    Check {
        generation: u64,
    },
    Download {
        generation: u64,
    },
    Capture {
        generation: u64,
    },
    Recognize {
        generation: u64,
        snapshot_id: u64,
        bounds: Rect,
        language: String,
    },
    Cancel,
}

pub const LANGUAGES: &[&str] = &[
    "zh-CN", "zh-TW", "en-US", "en-GB", "ja", "ko", "de", "fr", "es", "it", "pt-BR", "pt-PT",
];

pub fn model_for(language: &str) -> &'static str {
    match language {
        "ko" => "korean",
        "de" | "fr" | "es" | "it" | "pt-BR" | "pt-PT" => "latin",
        _ => "common",
    }
}

pub use super::screen_snapshot::crop_frames;

pub(crate) fn reduce(state: &mut AppState, event: OcrEvent) -> Vec<AppCommand> {
    use OcrEvent::*;
    let cmd = |c| vec![AppCommand::Ocr(c)];
    match event {
        SelectionSubmitted { selection }
            if selection.context
                == super::region_selection::SelectionContext::standalone(state.ocr.generation) =>
        {
            reduce(
                state,
                Selected {
                    generation: selection.context.request_id,
                    bounds: selection.bounds,
                },
            )
        }
        RefreshResources if matches!(state.ocr.phase, OcrPhase::Idle | OcrPhase::Error) => {
            cmd(OcrCommand::Check {
                generation: state.ocr.generation,
            })
        }
        Requested {
            annotation_active: true,
        } => {
            if state.ocr.phase != OcrPhase::Downloading {
                state.ocr.generation = state.ocr.generation.wrapping_add(1);
                state.ocr.phase = OcrPhase::Error;
                state.ocr.popup = None;
            }
            state.ocr.error = "Exit annotation before starting OCR".into();
            let popup = state.prepare_active_popup_session(
                String::new(),
                state.settings.target_language.clone(),
                TranslationPhase::Error,
                None,
            );
            let error = state.ocr.error.clone();
            if let Some(p) = state.popup_session_mut(popup) {
                p.error_message = error;
            }
            vec![AppCommand::ShowPopup {
                session_id: popup,
                anchor: None,
            }]
        }
        Requested { .. } if state.ocr.phase == OcrPhase::Capturing => reduce(state, Cancelled),
        Requested { .. } if state.ocr.phase == OcrPhase::Downloading => {
            vec![AppCommand::ShowSettingsWindow]
        }
        Requested { .. } => {
            let mut commands = vec![];
            if matches!(
                state.ocr.phase,
                OcrPhase::Recognizing | OcrPhase::Translating
            ) && let Some(id) = state.ocr.popup
            {
                if state.popup_sessions.iter().any(|p| p.id == id && !p.pinned) {
                    commands.extend(state.close_popup_session(id));
                } else if state.ocr.phase == OcrPhase::Recognizing
                    && let Some(p) = state.popup_session_mut(id)
                {
                    p.phase = TranslationPhase::Idle;
                }
            }
            state.ocr.generation = state.ocr.generation.wrapping_add(1);
            state.ocr.phase = OcrPhase::Checking;
            state.ocr.popup = None;
            state.ocr.error.clear();
            state.toolbar_selection = None;
            state.toolbar_generation = state.toolbar_generation.wrapping_add(1);
            commands.extend([
                AppCommand::Ocr(OcrCommand::Cancel),
                AppCommand::Ocr(OcrCommand::Check {
                    generation: state.ocr.generation,
                }),
            ]);
            commands
        }
        Resources {
            generation,
            ready,
            bytes,
        } if generation == state.ocr.generation && state.ocr.phase == OcrPhase::Checking => {
            state.ocr.resources_ready = ready;
            state.ocr.download_bytes = bytes;
            state.ocr.phase = if ready {
                OcrPhase::Capturing
            } else {
                OcrPhase::DownloadRequired
            };
            if ready {
                cmd(OcrCommand::Capture { generation })
            } else {
                vec![AppCommand::ShowSettingsWindow]
            }
        }
        Resources {
            generation,
            ready,
            bytes,
        } if generation == state.ocr.generation
            && matches!(state.ocr.phase, OcrPhase::Idle | OcrPhase::Error) =>
        {
            state.ocr.resources_ready = ready;
            state.ocr.download_bytes = bytes;
            vec![]
        }
        DownloadRequested
            if matches!(
                state.ocr.phase,
                OcrPhase::Idle | OcrPhase::Error | OcrPhase::DownloadRequired
            ) =>
        {
            state.ocr.generation = state.ocr.generation.wrapping_add(1);
            state.ocr.popup = None;
            state.ocr.phase = OcrPhase::Downloading;
            state.ocr.downloaded_bytes = 0;
            state.ocr.error.clear();
            cmd(OcrCommand::Download {
                generation: state.ocr.generation,
            })
        }
        DownloadProgress {
            generation,
            received,
            total,
        } if generation == state.ocr.generation && state.ocr.phase == OcrPhase::Downloading => {
            state.ocr.downloaded_bytes = received;
            state.ocr.download_bytes = total;
            vec![]
        }
        DownloadFinished { generation }
            if generation == state.ocr.generation && state.ocr.phase == OcrPhase::Downloading =>
        {
            state.ocr.resources_ready = true;
            state.ocr.phase = OcrPhase::Idle;
            state.ocr.error = "OCR resources ready. Press the shortcut again".into();
            vec![]
        }
        Selected { generation, bounds }
            if generation == state.ocr.generation
                && state.ocr.phase == OcrPhase::Capturing
                && bounds.right > bounds.left
                && bounds.bottom > bounds.top =>
        {
            state.ocr.phase = OcrPhase::Recognizing;
            let popup = state.prepare_active_popup_session(
                String::new(),
                state.settings.target_language.clone(),
                TranslationPhase::Capturing,
                None,
            );
            state.ocr.popup = Some(popup);
            let language = state
                .settings
                .ocr_language
                .clone()
                .unwrap_or_else(|| state.settings.ui_language.code().into());
            vec![
                AppCommand::ShowPopup {
                    session_id: popup,
                    anchor: Some(Point {
                        x: bounds.right,
                        y: bounds.bottom,
                    }),
                },
                AppCommand::Ocr(OcrCommand::Recognize {
                    generation,
                    snapshot_id: generation,
                    bounds,
                    language,
                }),
            ]
        }
        Recognized { generation, text }
            if generation == state.ocr.generation && state.ocr.phase == OcrPhase::Recognizing =>
        {
            if text.trim().is_empty() {
                return reduce(
                    state,
                    Failed {
                        generation,
                        error: "No text recognized in the selected region".into(),
                    },
                );
            }
            let Some(session_id) = state.ocr.popup else {
                return vec![];
            };
            state.ocr.phase = OcrPhase::Translating;
            state.reduce(AppEvent::PopupTranslationRequested {
                session_id,
                text,
                source_language: None,
                target_language: state.settings.target_language.clone(),
            })
        }
        Failed { generation, error } if generation == state.ocr.generation => {
            state.ocr.phase = OcrPhase::Error;
            state.ocr.error = error.clone();
            if let Some(id) = state.ocr.popup {
                if let Some(p) = state.popup_session_mut(id) {
                    p.phase = TranslationPhase::Error;
                    p.error_message = error;
                }
                vec![]
            } else {
                vec![AppCommand::ShowSettingsWindow]
            }
        }
        Cancelled => {
            state.ocr.generation = state.ocr.generation.wrapping_add(1);
            state.ocr.phase = OcrPhase::Idle;
            state.ocr.popup = None;
            cmd(OcrCommand::Cancel)
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selecting() -> AppState {
        let mut s = AppState::new(Default::default());
        s.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        s.reduce(AppEvent::Ocr(OcrEvent::Resources {
            generation: s.ocr.generation,
            ready: true,
            bytes: 100,
        }));
        s
    }
    #[test]
    fn submitted_region_validates_source_request_and_snapshot_before_recognition() {
        use super::super::region_selection::{RegionSelectionResult, SelectionContext};
        let mut s = selecting();
        let generation = s.ocr.generation;
        let bounds = Rect {
            left: -10,
            top: 20,
            right: 90,
            bottom: 70,
        };
        for context in [
            SelectionContext {
                source_session: generation + 1,
                request_id: generation,
                snapshot_id: generation,
            },
            SelectionContext {
                source_session: generation,
                request_id: generation + 1,
                snapshot_id: generation,
            },
            SelectionContext {
                source_session: generation,
                request_id: generation,
                snapshot_id: generation + 1,
            },
        ] {
            assert!(
                s.reduce(AppEvent::Ocr(OcrEvent::SelectionSubmitted {
                    selection: RegionSelectionResult { context, bounds }
                }))
                .is_empty()
            );
            assert_eq!(s.ocr.phase, OcrPhase::Capturing);
        }
        let commands = s.reduce(AppEvent::Ocr(OcrEvent::SelectionSubmitted {
            selection: RegionSelectionResult {
                context: SelectionContext::standalone(generation),
                bounds,
            },
        }));
        assert!(commands.iter().any(|c| matches!(c,AppCommand::Ocr(OcrCommand::Recognize{snapshot_id,..}) if *snapshot_id == generation)));
    }
    fn select(s: &mut AppState) -> Vec<AppCommand> {
        s.reduce(AppEvent::Ocr(OcrEvent::Selected {
            generation: s.ocr.generation,
            bounds: Rect {
                left: -10,
                top: 20,
                right: 90,
                bottom: 70,
            },
        }))
    }
    #[test]
    fn cancellation_and_late_results_never_translate() {
        let mut s = selecting();
        select(&mut s);
        let old = s.ocr.generation;
        s.reduce(AppEvent::Ocr(OcrEvent::Cancelled));
        assert!(
            s.reduce(AppEvent::Ocr(OcrEvent::Recognized {
                generation: old,
                text: "private".into()
            }))
            .is_empty()
        );
        assert_eq!(s.ocr.phase, OcrPhase::Idle);
    }
    #[test]
    fn empty_ocr_retains_error_popup_without_translating() {
        let mut s = selecting();
        assert!(
            select(&mut s)
                .iter()
                .any(|c| matches!(c, AppCommand::Ocr(OcrCommand::Recognize { .. })))
        );
        assert!(
            s.reduce(AppEvent::Ocr(OcrEvent::Recognized {
                generation: s.ocr.generation,
                text: " \n".into()
            }))
            .is_empty()
        );
        assert_eq!(s.ocr.phase, OcrPhase::Error);
        assert!(s.popup_sessions[0].source_text.is_empty());
    }
    #[test]
    fn closing_popup_invalidates_its_ocr_task() {
        let mut s = selecting();
        select(&mut s);
        let id = s.ocr.popup.unwrap();
        let generation = s.ocr.generation;
        s.reduce(AppEvent::PopupClosed { session_id: id });
        assert!(
            s.reduce(AppEvent::Ocr(OcrEvent::Recognized {
                generation,
                text: "Hello".into()
            }))
            .is_empty()
        );
        assert!(s.popup_sessions.is_empty());
    }
    #[test]
    fn replacement_retires_unpinned_pending_popup_and_preserves_pinned_lifecycle() {
        for pinned in [false, true] {
            let mut s = selecting();
            select(&mut s);
            let id = s.ocr.popup.unwrap();
            let generation = s.ocr.generation;
            if pinned {
                s.reduce(AppEvent::PopupPinChanged {
                    session_id: id,
                    pinned: true,
                });
            }
            let commands = s.reduce(AppEvent::Ocr(OcrEvent::Requested {
                annotation_active: false,
            }));
            assert_eq!(s.ocr.phase, OcrPhase::Checking);
            if pinned {
                assert_eq!(s.popup_sessions[0].phase, TranslationPhase::Idle);
            } else {
                assert!(s.popup_sessions.is_empty());
                assert!(
                    commands
                        .iter()
                        .any(|c| matches!(c,AppCommand::HidePopup{session_id} if *session_id==id))
                );
            }
            assert!(
                s.reduce(AppEvent::Ocr(OcrEvent::Recognized {
                    generation,
                    text: "late".into()
                }))
                .is_empty()
            );
        }
    }
    #[test]
    fn annotation_and_repeat_shortcut_preserve_domain_boundaries() {
        let mut s = selecting();
        let old = s.ocr.generation;
        s.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        assert!(s.ocr.generation > old);
        assert_eq!(s.ocr.phase, OcrPhase::Idle);
        let commands = s.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: true,
        }));
        assert!(commands.iter().all(|c| !matches!(c, AppCommand::Ocr(_))));
    }
    #[test]
    fn resources_require_confirmation_and_completion_does_not_capture() {
        let mut s = AppState::new(Default::default());
        s.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        let commands = s.reduce(AppEvent::Ocr(OcrEvent::Resources {
            generation: s.ocr.generation,
            ready: false,
            bytes: 123,
        }));
        assert_eq!(commands, vec![AppCommand::ShowSettingsWindow]);
        assert_eq!(s.ocr.phase, OcrPhase::DownloadRequired);
        s.reduce(AppEvent::Ocr(OcrEvent::DownloadRequested));
        assert!(
            s.reduce(AppEvent::Ocr(OcrEvent::DownloadFinished {
                generation: s.ocr.generation
            }))
            .is_empty()
        );
        assert_eq!(s.ocr.phase, OcrPhase::Idle);
    }
    #[test]
    fn crop_combines_negative_monitor_coordinates_and_white_gaps() {
        use crate::ports::screen_capture::CapturedRegion;
        let frames = [
            CapturedRegion {
                bounds: Rect {
                    left: -2,
                    top: 0,
                    right: 0,
                    bottom: 1,
                },
                pixels: vec![10; 8],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 1,
                    top: 0,
                    right: 2,
                    bottom: 1,
                },
                pixels: vec![20; 4],
            },
        ];
        assert_eq!(
            crop_frames(
                &frames,
                Rect {
                    left: -1,
                    top: 0,
                    right: 2,
                    bottom: 1
                }
            )
            .unwrap()
            .pixels,
            [10, 10, 10, 10, 255, 255, 255, 255, 20, 20, 20, 20]
        );
        assert!(
            crop_frames(
                &frames,
                Rect {
                    left: i32::MIN,
                    top: 0,
                    right: i32::MAX,
                    bottom: 2
                }
            )
            .is_err()
        );
    }
    #[test]
    fn model_mapping_and_sensitive_debug() {
        assert_eq!(model_for("ja"), "common");
        assert_eq!(model_for("en-GB"), "common");
        assert_eq!(model_for("pt-PT"), "latin");
        assert_eq!(model_for("ko"), "korean");
        assert!(
            !format!(
                "{:?}",
                OcrEvent::Recognized {
                    generation: 1,
                    text: "secret".into()
                }
            )
            .contains("secret")
        );
    }
}
