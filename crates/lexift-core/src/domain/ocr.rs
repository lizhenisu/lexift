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
    pub resources: ResourceState,
    pub required_resource: crate::ports::ocr::OcrResourceKind,
    pub strategy: OcrRecognitionStrategy,
    pub generation: u64,
    pub phase: OcrPhase,
    pub popup: Option<PopupSessionId>,
    pub resources_ready: bool,
    pub download_bytes: u64,
    pub downloaded_bytes: u64,
    pub error: String,
}

/// Resource operations have their own version and do not cancel capture or translation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceState {
    pub request_id: u64,
    pub busy: Option<ResourceOperation>,
    pub last_operation: Option<ResourceOperation>,
    pub info: Option<crate::ports::ocr::OcrResources>,
    pub error: String,
    pub received: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceOperation {
    Check,
    Download,
    Remove,
    DownloadKorean,
    RemoveKorean,
}
impl ResourceOperation {
    pub fn kind(self) -> crate::ports::ocr::OcrResourceKind {
        use crate::ports::ocr::OcrResourceKind;
        match self {
            Self::DownloadKorean | Self::RemoveKorean => OcrResourceKind::Korean,
            _ => OcrResourceKind::Base,
        }
    }
    pub fn downloading(self) -> bool {
        matches!(self, Self::Download | Self::DownloadKorean)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum OcrEvent {
    ManageResources(ResourceOperation),
    CancelResourceDownload,
    ResourceCompleted {
        request_id: u64,
        result: crate::Result<crate::ports::ocr::OcrResources>,
    },
    ResourceProgress {
        request_id: u64,
        received: u64,
    },
    ResourceFailed {
        request_id: u64,
        info: Option<crate::ports::ocr::OcrResources>,
        error: String,
    },
    Checked {
        generation: u64,
        resources: crate::ports::ocr::OcrResources,
    },
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
    Resource {
        request_id: u64,
        operation: ResourceOperation,
    },
    CancelResourceDownload,
    Check {
        generation: u64,
    },
    Download {
        generation: u64,
        kind: crate::ports::ocr::OcrResourceKind,
    },
    Capture {
        generation: u64,
    },
    Recognize {
        generation: u64,
        snapshot_id: u64,
        bounds: Rect,
        strategy: OcrRecognitionStrategy,
    },
    Cancel,
    ReleaseKorean,
}

pub const LANGUAGES: &[&str] = &[
    "zh-CN", "zh-TW", "en-US", "en-GB", "ja", "ko", "de", "fr", "es", "it", "pt-BR", "pt-PT",
];

/// Recognition policy is frozen at shortcut dispatch, independently of translation language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OcrRecognitionStrategy {
    #[default]
    Automatic,
    AutomaticKorean,
    Language(String),
}
impl OcrRecognitionStrategy {
    pub fn from_settings(value: Option<&str>, interface: &str) -> Self {
        match value {
            Some("auto") => Self::Automatic,
            Some(language) => Self::Language(language.into()),
            None => Self::Language(interface.into()),
        }
    }
    pub fn requires_korean(&self) -> bool {
        matches!(self, Self::AutomaticKorean)
            || matches!(self, Self::Language(language) if language == "ko")
    }
}

/// Stable UI mapping: auto, follow interface, then the existing twelve languages.
pub fn language_index(value: Option<&str>) -> i32 {
    match value {
        Some("auto") => 0,
        None => 1,
        Some(code) => code
            .parse::<super::ui_language::UiLanguage>()
            .map_or(1, |language| language.index() + 2),
    }
}
pub fn language_at_index(index: i32) -> Option<Option<String>> {
    match index {
        0 => Some(Some("auto".into())),
        1 => Some(None),
        2..=13 => Some(Some(
            super::ui_language::UiLanguage::from_index(index - 2)
                .code()
                .into(),
        )),
        _ => None,
    }
}

pub use super::screen_snapshot::crop_frames;

pub(crate) fn reduce(state: &mut AppState, event: OcrEvent) -> Vec<AppCommand> {
    use OcrEvent::*;
    let cmd = |c| vec![AppCommand::Ocr(c)];
    match event {
        ManageResources(operation) => {
            let active = matches!(
                state.ocr.phase,
                OcrPhase::Checking | OcrPhase::Capturing | OcrPhase::Recognizing
            );
            let pending =
                state.ocr.resources.info.as_ref().is_some_and(|r| {
                    r.status == crate::ports::ocr::OcrResourceStatus::PendingRemoval
                });
            if state.ocr.resources.busy.is_some()
                || (operation != ResourceOperation::Check && active)
                || (operation.downloading() && pending)
                || (operation == ResourceOperation::DownloadKorean
                    && !state.ocr.resources.info.as_ref().is_some_and(|r| r.ready))
            {
                return vec![];
            }
            let r = &mut state.ocr.resources;
            r.request_id = r.request_id.wrapping_add(1);
            r.busy = Some(operation);
            r.last_operation = Some(operation);
            r.error.clear();
            r.received = 0;
            cmd(OcrCommand::Resource {
                request_id: r.request_id,
                operation,
            })
        }
        CancelResourceDownload
            if state
                .ocr
                .resources
                .busy
                .is_some_and(ResourceOperation::downloading) =>
        {
            let r = &mut state.ocr.resources;
            r.request_id = r.request_id.wrapping_add(1);
            r.busy = Some(ResourceOperation::Check);
            vec![
                AppCommand::Ocr(OcrCommand::CancelResourceDownload),
                AppCommand::Ocr(OcrCommand::Resource {
                    request_id: r.request_id,
                    operation: ResourceOperation::Check,
                }),
            ]
        }
        ResourceCompleted { request_id, result }
            if request_id == state.ocr.resources.request_id
                && state.ocr.resources.busy.is_some() =>
        {
            let r = &mut state.ocr.resources;
            let operation = r.busy.take();
            match result {
                Ok(info) => {
                    state.ocr.resources_ready = info.ready;
                    state.ocr.download_bytes = info.download_bytes;
                    r.info = Some(info);
                    r.error.clear();
                    if operation.is_some_and(|operation| {
                        operation.downloading()
                            || matches!(
                                operation,
                                ResourceOperation::Remove | ResourceOperation::RemoveKorean
                            )
                    }) {
                        state.ocr.error.clear();
                    }
                }
                Err(error) => r.error = error.to_string(),
            }
            vec![]
        }
        ResourceFailed {
            request_id,
            info,
            error,
        } if request_id == state.ocr.resources.request_id && state.ocr.resources.busy.is_some() => {
            let r = &mut state.ocr.resources;
            r.busy = None;
            r.error = error;
            if let Some(info) = info {
                state.ocr.resources_ready = info.ready;
                r.info = Some(info);
            } else {
                state.ocr.resources_ready = false;
                r.info = None;
            }
            vec![]
        }
        ResourceProgress {
            request_id,
            received,
        } if request_id == state.ocr.resources.request_id
            && state
                .ocr
                .resources
                .busy
                .is_some_and(ResourceOperation::downloading) =>
        {
            state.ocr.resources.received = received;
            vec![]
        }
        Checked {
            generation,
            resources,
        } if generation == state.ocr.generation && state.ocr.phase == OcrPhase::Checking => {
            if state.ocr.resources.busy.is_none() {
                state.ocr.resources.info = Some(resources.clone());
            }
            if resources.status == crate::ports::ocr::OcrResourceStatus::PendingRemoval {
                return reduce(
                    state,
                    Failed {
                        generation,
                        error: "OCR resources will be removed after restart".into(),
                    },
                );
            }
            let required = if !resources.ready {
                &resources
            } else if state.ocr.strategy.requires_korean() {
                state.ocr.required_resource = crate::ports::ocr::OcrResourceKind::Korean;
                resources.korean.as_deref().unwrap_or(&resources)
            } else {
                &resources
            };
            let ready = required.ready
                && (!state.ocr.strategy.requires_korean()
                    || resources.korean.as_deref().is_some_and(|r| r.ready));
            reduce(
                state,
                Resources {
                    generation,
                    ready,
                    bytes: required.download_bytes,
                },
            )
        }
        Requested { .. }
            if state.ocr.phase != OcrPhase::Capturing && !state.settings.ocr_enabled =>
        {
            let id = state.prepare_active_popup_session(
                String::new(),
                state.settings.target_language.clone(),
                TranslationPhase::Error,
                None,
            );
            if let Some(p) = state.popup_session_mut(id) {
                p.error_message = "OCR provider is disabled".into();
            }
            vec![AppCommand::ShowPopup {
                session_id: id,
                anchor: None,
            }]
        }
        Requested { .. } if state.ocr.resources.busy.is_some() => {
            vec![AppCommand::ShowSettingsWindow]
        }
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
            reduce(state, ManageResources(ResourceOperation::Check))
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
            state.ocr.strategy = OcrRecognitionStrategy::from_settings(
                state.settings.ocr_language.as_deref(),
                state.settings.ui_language.code(),
            );
            if state.settings.ocr_korean_auto
                && state.ocr.strategy == OcrRecognitionStrategy::Automatic
            {
                state.ocr.strategy = OcrRecognitionStrategy::AutomaticKorean;
            }
            state.ocr.required_resource = crate::ports::ocr::OcrResourceKind::Base;
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
                kind: state.ocr.required_resource,
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
            state.ocr.error.clear();
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
                    strategy: state.ocr.strategy.clone(),
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
        assert!(s.ocr.error.is_empty());
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
    fn automatic_and_legacy_choices_have_stable_ui_mapping() {
        for index in 0..14 {
            let value = language_at_index(index).unwrap();
            assert_eq!(language_index(value.as_deref()), index);
        }
        assert!(language_at_index(14).is_none());
        assert_eq!(
            OcrRecognitionStrategy::from_settings(Some("auto"), "ja"),
            OcrRecognitionStrategy::Automatic
        );
        assert_eq!(
            OcrRecognitionStrategy::from_settings(None, "ja"),
            OcrRecognitionStrategy::Language("ja".into())
        );
    }
    #[test]
    fn settings_changes_during_selection_do_not_change_recognition_strategy() {
        let mut state = AppState::default();
        state.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        let generation = state.ocr.generation;
        state.reduce(AppEvent::Ocr(OcrEvent::Resources {
            generation,
            ready: true,
            bytes: 0,
        }));
        state.settings.ocr_language = Some("ko".into());
        let commands = state.reduce(AppEvent::Ocr(OcrEvent::Selected {
            generation,
            bounds: Rect {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
        }));
        assert!(commands.iter().any(|c| matches!(
            c,
            AppCommand::Ocr(OcrCommand::Recognize {
                strategy: OcrRecognitionStrategy::Automatic,
                ..
            })
        )));
    }
    #[test]
    fn recognized_text_is_not_debugged() {
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

#[cfg(test)]
mod provider_tests {
    use super::*;
    use crate::{
        domain::{service_provider::ServiceProviderId, settings::SettingsChange},
        ports::ocr::{OcrResourceStatus, OcrResources},
    };
    fn info(status: OcrResourceStatus) -> OcrResources {
        OcrResources {
            ready: status == OcrResourceStatus::Ready,
            download_bytes: 100,
            status,
            version: "v1".into(),
            installed_bytes: 50,
            korean: None,
            entries: vec![],
        }
    }
    #[test]
    fn disabled_provider_refuses_new_work_without_cancelling_frozen_task() {
        let mut state = AppState::default();
        state.settings.ocr_enabled = false;
        let commands = state.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        assert!(commands.iter().all(|c| !matches!(c, AppCommand::Ocr(_))));
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::ShowPopup { .. }]
        ));
        state.ocr.phase = OcrPhase::Capturing;
        let commands = state.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        assert_eq!(commands, vec![AppCommand::Ocr(OcrCommand::Cancel)]);
    }
    #[test]
    fn resource_versions_are_independent_and_reject_late_progress_and_results() {
        let mut state = AppState::default();
        let generation = state.ocr.generation;
        state.reduce(AppEvent::Ocr(OcrEvent::ManageResources(
            ResourceOperation::Download,
        )));
        let id = state.ocr.resources.request_id;
        assert_eq!(state.ocr.generation, generation);
        state.reduce(AppEvent::Ocr(OcrEvent::CancelResourceDownload));
        state.reduce(AppEvent::Ocr(OcrEvent::ManageResources(
            ResourceOperation::Check,
        )));
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceCompleted {
            request_id: id,
            result: Ok(info(OcrResourceStatus::Ready)),
        }));
        assert!(state.ocr.resources.info.is_none());
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceProgress {
            request_id: id,
            received: 100,
        }));
        assert_eq!(state.ocr.resources.received, 0);
        let id = state.ocr.resources.request_id;
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceCompleted {
            request_id: id,
            result: Ok(info(OcrResourceStatus::PendingRemoval)),
        }));
        assert!(
            state
                .reduce(AppEvent::Ocr(OcrEvent::ManageResources(
                    ResourceOperation::Download
                )))
                .is_empty()
        );
        state.ocr.phase = OcrPhase::Recognizing;
        assert!(
            state
                .reduce(AppEvent::Ocr(OcrEvent::ManageResources(
                    ResourceOperation::Remove
                )))
                .is_empty()
        );
    }
    #[test]
    fn startup_check_and_download_completion_use_resource_versions() {
        let mut state = AppState::default();
        let commands = state.reduce(AppEvent::Ocr(OcrEvent::RefreshResources));
        let id = state.ocr.resources.request_id;
        assert_eq!(
            commands,
            vec![AppCommand::Ocr(OcrCommand::Resource {
                request_id: id,
                operation: ResourceOperation::Check
            })]
        );
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceCompleted {
            request_id: id,
            result: Ok(info(OcrResourceStatus::NotInstalled)),
        }));
        state.reduce(AppEvent::Ocr(OcrEvent::ManageResources(
            ResourceOperation::Download,
        )));
        let id = state.ocr.resources.request_id;
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceCompleted {
            request_id: id,
            result: Ok(info(OcrResourceStatus::Ready)),
        }));
        assert!(state.ocr.resources_ready);
        assert!(state.ocr.error.is_empty());
        state.reduce(AppEvent::Ocr(OcrEvent::ManageResources(
            ResourceOperation::Remove,
        )));
        let id = state.ocr.resources.request_id;
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceFailed {
            request_id: id,
            info: None,
            error: "inspection denied".into(),
        }));
        assert!(!state.ocr.resources_ready);
        assert!(state.ocr.resources.info.is_none());
        state.reduce(AppEvent::Ocr(OcrEvent::Checked {
            generation: state.ocr.generation,
            resources: info(OcrResourceStatus::Ready),
        }));
        assert!(
            state.ocr.resources.info.is_none(),
            "retired capture check cannot restore deleted resource state"
        );
    }
    #[test]
    fn ocr_enablement_is_a_targeted_change_and_failure_rolls_back() {
        let mut state = AppState::default();
        let before = state.settings.clone();
        let change = SettingsChange::ProviderEnabled {
            request_id: 9,
            provider: ServiceProviderId::PaddleOcr,
            enabled: false,
        };
        state.reduce(AppEvent::SettingsChangeRequested {
            change: change.clone(),
        });
        assert!(!state.desired_settings.ocr_enabled);
        assert_eq!(state.desired_settings.llm, before.llm);
        state.reduce(AppEvent::SettingsSaveFailed {
            change,
            error: "fixture".into(),
        });
        assert!(state.desired_settings.ocr_enabled);
        assert_eq!(state.settings, before);
    }
}

#[cfg(test)]
mod extension_tests {
    use super::*;
    use crate::domain::settings::{SettingsChange, SettingsField};
    use crate::ports::ocr::{OcrResourceKind, OcrResourceStatus, OcrResources};
    fn ready() -> OcrResources {
        OcrResources {
            ready: true,
            status: OcrResourceStatus::Ready,
            version: "v1".into(),
            download_bytes: 10,
            installed_bytes: 10,
            entries: vec![],
            korean: None,
        }
    }
    fn request(state: &mut AppState) -> u64 {
        state.reduce(AppEvent::Ocr(OcrEvent::Requested {
            annotation_active: false,
        }));
        state.ocr.generation
    }
    #[test]
    fn default_automatic_capture_needs_no_extension() {
        let mut state = AppState::default();
        let generation = request(&mut state);
        let commands = state.reduce(AppEvent::Ocr(OcrEvent::Checked {
            generation,
            resources: ready(),
        }));
        assert_eq!(state.ocr.strategy, OcrRecognitionStrategy::Automatic);
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::Ocr(OcrCommand::Capture { .. })]
        ));
    }
    #[test]
    fn manual_and_automatic_korean_stop_before_capture_if_extension_missing() {
        for selection in 0..3 {
            let mut state = AppState::default();
            if selection == 0 {
                state.settings.ocr_language = Some("ko".into());
            } else if selection == 1 {
                state.settings.ocr_language = None;
                state.settings.ui_language = crate::domain::ui_language::UiLanguage::Korean;
            } else {
                state.settings.ocr_korean_auto = true;
            }
            let generation = request(&mut state);
            let commands = state.reduce(AppEvent::Ocr(OcrEvent::Checked {
                generation,
                resources: ready(),
            }));
            assert_eq!(state.ocr.phase, OcrPhase::DownloadRequired);
            assert_eq!(state.ocr.required_resource, OcrResourceKind::Korean);
            assert!(matches!(
                commands.as_slice(),
                [AppCommand::ShowSettingsWindow]
            ));
            state.settings.ocr_korean_auto = false;
            assert!(state.ocr.strategy.requires_korean());
        }
    }
    #[test]
    fn korean_enable_requires_verified_extension_and_failed_save_rolls_back() {
        let mut state = AppState::default();
        state.ocr.resources.info = Some(ready());
        let change = SettingsChange::OcrKoreanAuto(true);
        let commands = state.reduce(AppEvent::SettingsChangeRequested {
            change: change.clone(),
        });
        assert!(!state.desired_settings.ocr_korean_auto);
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::ShowSettingsFeedback { .. }]
        ));
        state.ocr.resources.info.as_mut().unwrap().korean = Some(Box::new(ready()));
        state.reduce(AppEvent::SettingsChangeRequested {
            change: change.clone(),
        });
        assert!(state.desired_settings.ocr_korean_auto);
        state.reduce(AppEvent::RuntimeConfigUpdateFailed {
            change,
            error: "fixture failure".into(),
        });
        assert!(!state.settings.ocr_korean_auto);
        assert!(!state.desired_settings.ocr_korean_auto);
        assert_eq!(
            state.settings_error_field,
            Some(SettingsField::OcrKoreanAuto)
        );
    }
    #[test]
    fn queued_enable_is_rejected_if_extension_was_removed() {
        let mut state = AppState::default();
        let mut resources = ready();
        resources.korean = Some(Box::new(ready()));
        state.ocr.resources.info = Some(resources);
        state.credential_busy = true;
        assert!(
            state
                .reduce(AppEvent::SettingsChangeRequested {
                    change: SettingsChange::OcrKoreanAuto(true),
                })
                .is_empty()
        );
        state.ocr.resources.info.as_mut().unwrap().korean = None;
        let commands = state.reduce(AppEvent::CredentialAccessFailed {
            error: "fixture".into(),
        });
        assert!(!state.desired_settings.ocr_korean_auto);
        assert!(!state.settings_saving);
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, AppCommand::PersistSettings { .. }))
        );
    }
    #[test]
    fn extension_operations_require_base_and_reject_stale_results() {
        let mut state = AppState::default();
        assert!(
            state
                .reduce(AppEvent::Ocr(OcrEvent::ManageResources(
                    ResourceOperation::DownloadKorean
                )))
                .is_empty()
        );
        state.ocr.resources.info = Some(ready());
        state.reduce(AppEvent::Ocr(OcrEvent::ManageResources(
            ResourceOperation::DownloadKorean,
        )));
        let request_id = state.ocr.resources.request_id;
        assert_eq!(
            state.ocr.resources.busy,
            Some(ResourceOperation::DownloadKorean)
        );
        state.reduce(AppEvent::Ocr(OcrEvent::CancelResourceDownload));
        state.reduce(AppEvent::Ocr(OcrEvent::ResourceCompleted {
            request_id,
            result: Ok(ready()),
        }));
        assert_eq!(state.ocr.resources.busy, Some(ResourceOperation::Check));
    }
}
