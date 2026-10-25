//! Output lifetime of independent pinned images; pixels remain owned by App.
use super::annotation_export::ExportRequest;
use crate::ports::{annotation_save::SaveOutcome, magnifier::WindowToken};
use crate::{AppCommand, AppState, Result};
use std::collections::BTreeMap;
pub mod interaction;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Copy,
    Save,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub interaction: interaction::State,
    pub operation: u64,
    pub busy: bool,
    pub recognition: Option<Recognition>,
    pub ocr_result: Option<(u64, super::ocr::OcrOutcome)>,
    pub error: Option<String>,
    pub hidden: bool,
    pub editing: Option<u64>,
    pub revision: u64,
    pub last_edit: u64,
    pub document: Option<ExportRequest>,
    pub source: Option<super::geometry::Rect>,
    pub proposed: Option<ExportRequest>,
    pub proposal_ready: bool,
}
/// Immutable input identity; geometry changes of the pin window do not change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recognition {
    pub orientation: super::image_transform::Orientation,
    pub operation: u64,
    pub revision: u64,
    pub bounds: super::geometry::Rect,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    InteractionRequested {
        id: u64,
        policy: interaction::Policy,
    },
    RestoreMouseInput,
    InteractionApplied {
        id: u64,
        request: u64,
        result: Result<()>,
    },
    HistoryOpened {
        id: u64,
        hidden: bool,
        key: super::image_history::workflow::OutputKey,
    },
    Recognize {
        id: u64,
    },
    CancelRecognition {
        id: u64,
    },
    Recognized {
        id: u64,
        operation: u64,
        revision: u64,
        outcome: super::ocr::OcrOutcome,
    },
    SourceReady {
        id: u64,
        bounds: super::geometry::Rect,
    },
    DocumentProposed {
        id: u64,
        edit: u64,
        document: ExportRequest,
    },
    ProposalCancelled {
        id: u64,
        edit: u64,
        request: u64,
    },
    Rendered {
        id: u64,
        revision: u64,
    },
    ProposalAccepted {
        id: u64,
        edit: u64,
        request: u64,
        revision: u64,
    },
    EditRequested {
        id: u64,
    },
    EditFinished {
        id: u64,
        edit: u64,
    },
    EditFailed {
        id: u64,
        edit: u64,
        error: String,
    },
    PreviewFailed {
        id: u64,
        revision: u64,
        error: String,
    },
    DocumentChanged {
        id: u64,
        document: ExportRequest,
    },
    VisibilityChanged {
        id: u64,
        hidden: bool,
    },
    Opened {
        id: u64,
        source: (u64, u64),
    },
    Requested {
        id: u64,
        kind: Output,
        owner: Option<WindowToken>,
        title: String,
    },
    Completed {
        id: u64,
        operation: u64,
        result: Result<SaveOutcome>,
    },
    Closed {
        id: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    ApplyInteraction {
        id: u64,
        request: u64,
        policy: interaction::Policy,
    },
    RetainHistory {
        id: u64,
        ticket: super::image_history::Ticket,
    },
    Recognize {
        id: u64,
        request: Recognition,
        strategy: super::ocr::OcrRecognitionStrategy,
    },
    BeginEdit {
        id: u64,
        edit: u64,
    },
    EndEdit {
        id: u64,
        edit: u64,
    },
    Render {
        id: u64,
        revision: u64,
        document: ExportRequest,
        hidden: bool,
    },
    Retain {
        id: u64,
        source: (u64, u64),
    },
    Output {
        id: u64,
        operation: u64,
        kind: Output,
        owner: Option<WindowToken>,
        title: String,
        document: Option<ExportRequest>,
        hidden: bool,
    },
    Drop {
        id: u64,
    },
}
pub type States = BTreeMap<u64, State>;
fn cancel_one(p: &mut State) {
    if p.recognition.take().is_some() {
        p.busy = false;
    }
    p.ocr_result = None;
}
pub(crate) fn cancel_recognition(pins: &mut States) {
    for p in pins.values_mut() {
        cancel_one(p);
    }
}
/// Cancel copy-only work when its source or owning interaction is retired.
pub(crate) fn before_event(pins: &mut States, event: &crate::AppEvent) {
    use crate::AppEvent;
    match event {
        AppEvent::ExitRequested
        | AppEvent::Screenshot(super::screenshot::ScreenshotEvent::Requested(_))
        | AppEvent::Ocr(super::ocr::OcrEvent::Requested { .. }) => cancel_recognition(pins),
        AppEvent::PinnedImage(Event::EditRequested { id } | Event::Requested { id, .. })
            if pins
                .get(id)
                .is_some_and(|p| !p.busy && p.proposed.is_none()) =>
        {
            cancel_recognition(pins)
        }
        AppEvent::PinnedImage(
            Event::Closed { id }
            | Event::EditFinished { id, .. }
            | Event::EditFailed { id, .. }
            | Event::VisibilityChanged { id, .. },
        ) => {
            if let Some(p) = pins.get_mut(id) {
                cancel_one(p);
            }
        }
        _ => {}
    }
}
pub(crate) fn reduce(state: &mut AppState, event: Event) -> Vec<AppCommand> {
    let input_id = match &event {
        Event::Recognize { id }
        | Event::Requested { id, .. }
        | Event::EditRequested { id }
        | Event::VisibilityChanged { id, .. }
        | Event::DocumentProposed { id, .. } => Some(*id),
        _ => None,
    };
    if input_id
        .and_then(|id| state.pinned_images.get(&id))
        .is_some_and(|p| p.interaction.applied.passthrough || p.interaction.pending.is_some())
    {
        return vec![];
    }
    let command = |c| vec![AppCommand::PinnedImage(c)];
    match event {
        Event::InteractionRequested { id, policy } => interaction::request(state, id, policy),
        Event::RestoreMouseInput => interaction::restore(state),
        Event::InteractionApplied {
            id,
            request,
            result,
        } => {
            interaction::complete(state, id, request, result);
            vec![]
        }
        Event::Recognize { id } => {
            if state.screenshot.active()
                || matches!(
                    state.ocr.phase,
                    super::ocr::OcrPhase::Checking
                        | super::ocr::OcrPhase::Capturing
                        | super::ocr::OcrPhase::Recognizing
                )
                || !state
                    .pinned_images
                    .get(&id)
                    .is_some_and(|p| !p.busy && p.proposed.is_none() && p.source.is_some())
            {
                return vec![];
            }
            cancel_recognition(&mut state.pinned_images);
            let p = state.pinned_images.get_mut(&id).unwrap();
            p.operation = p.operation.checked_add(1).expect("pin operation exhausted");
            let bounds = p
                .document
                .as_ref()
                .map_or(p.source.unwrap(), |d| d.snapshot.monitor);
            let request = Recognition {
                orientation: p
                    .document
                    .as_ref()
                    .map_or(Default::default(), |d| d.snapshot.session.image_orientation),
                operation: p.operation,
                revision: p.revision,
                bounds,
            };
            p.recognition = Some(request);
            p.busy = true;
            p.ocr_result = None;
            p.error = None;
            command(Command::Recognize {
                id,
                request,
                strategy: super::ocr::copy_strategy(&state.settings),
            })
        }
        Event::CancelRecognition { id } => {
            if let Some(p) = state.pinned_images.get_mut(&id) {
                cancel_one(p);
            }
            vec![]
        }
        Event::Recognized {
            id,
            operation,
            revision,
            outcome,
        } => {
            if let Some(p) = state.pinned_images.get_mut(&id).filter(|p| {
                p.recognition
                    .is_some_and(|r| r.operation == operation && r.revision == revision)
                    && p.revision == revision
            }) {
                p.recognition = None;
                p.busy = false;
                p.ocr_result = Some((operation, outcome));
            }
            vec![]
        }
        Event::SourceReady { id, bounds } => {
            if let Some(p) = state.pinned_images.get_mut(&id) {
                p.source = Some(bounds);
            }
            vec![]
        }
        Event::DocumentProposed { id, edit, document } => {
            let Some(p) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| !p.busy && p.editing == Some(edit) && p.proposed.is_none())
            else {
                return vec![];
            };
            if p.interaction.applied.locked
                && p.document.as_ref().is_some_and(|old| {
                    interaction::display_size(old) != interaction::display_size(&document)
                })
            {
                return vec![];
            }
            let b = document.snapshot.monitor;
            if document.session != edit
                || p.document
                    .as_ref()
                    .is_some_and(|old| document.request <= old.request)
                || !p.source.is_some_and(|f| {
                    b.left >= f.left
                        && b.top >= f.top
                        && b.right <= f.right
                        && b.bottom <= f.bottom
                        && b.right > b.left
                        && b.bottom > b.top
                })
            {
                return vec![];
            }
            p.proposed = Some(document.clone());
            p.proposal_ready = false;
            p.revision += 1;
            p.error = None;
            command(Command::Render {
                id,
                revision: p.revision,
                document,
                hidden: false,
            })
        }
        Event::ProposalCancelled { id, edit, request } => {
            if let Some(p) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.editing == Some(edit))
                && p.proposed.as_ref().is_some_and(|d| d.request == request)
            {
                p.proposed = None;
                p.proposal_ready = false;
                p.revision += 1;
            }
            vec![]
        }
        Event::Rendered { id, revision } => {
            if let Some(p) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.revision == revision)
            {
                p.proposal_ready = p.proposed.is_some();
            }
            vec![]
        }
        Event::ProposalAccepted {
            id,
            edit,
            request,
            revision,
        } => {
            if let Some(p) = state.pinned_images.get_mut(&id).filter(|p| {
                p.editing == Some(edit)
                    && p.revision == revision
                    && p.proposal_ready
                    && p.proposed.as_ref().is_some_and(|d| d.request == request)
            }) {
                p.document = p.proposed.take();
                p.proposal_ready = false;
            }
            vec![]
        }
        Event::EditFailed { id, edit, error } => {
            if let Some(p) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.editing == Some(edit))
            {
                p.editing = None;
                if p.proposed.take().is_some() {
                    p.revision += 1;
                }
                p.proposal_ready = false;
                p.error = Some(error);
                command(Command::EndEdit { id, edit })
            } else {
                vec![]
            }
        }
        Event::PreviewFailed {
            id,
            revision,
            error,
        } => {
            if let Some(p) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.revision == revision)
            {
                p.error = Some(error);
                p.proposed = None;
                p.proposal_ready = false;
            }
            vec![]
        }
        Event::EditRequested { id } => {
            if state.image_history.editor.is_some()
                || state.screenshot.active()
                || matches!(
                    state.ocr.phase,
                    super::ocr::OcrPhase::Checking | super::ocr::OcrPhase::Capturing
                )
                || !state
                    .pinned_images
                    .get(&id)
                    .is_some_and(|p| !p.busy && p.editing.is_none() && p.document.is_some())
            {
                return vec![];
            }
            let mut commands = Vec::new();
            for (&other, pin) in &mut state.pinned_images {
                if let Some(edit) = pin.editing.take() {
                    if pin.proposed.take().is_some() {
                        pin.revision += 1;
                    }
                    pin.proposal_ready = false;
                    commands.push(AppCommand::PinnedImage(Command::EndEdit {
                        id: other,
                        edit,
                    }));
                }
            }
            let pin = state.pinned_images.get_mut(&id).unwrap();
            pin.operation += 1;
            let edit = pin.operation;
            pin.editing = Some(edit);
            pin.last_edit = edit;
            pin.hidden = false;
            pin.error = None;
            commands.push(AppCommand::PinnedImage(Command::BeginEdit { id, edit }));
            commands
        }
        Event::EditFinished { id, edit } => {
            if let Some(pin) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.editing == Some(edit))
            {
                pin.editing = None;
                pin.proposal_ready = false;
                if pin.proposed.take().is_some() {
                    pin.revision += 1;
                }
                command(Command::EndEdit { id, edit })
            } else {
                vec![]
            }
        }
        Event::DocumentChanged { id, document } => {
            let Some(pin) = state.pinned_images.get_mut(&id) else {
                return vec![];
            };
            if pin.document.as_ref().is_some_and(|old| {
                document.request <= old.request || document.session != pin.last_edit
            }) {
                return vec![];
            }
            if pin.proposed.is_some() || pin.recognition.is_some() {
                return vec![];
            }
            pin.document = Some(document.clone());
            pin.error = None;
            pin.revision += 1;
            command(Command::Render {
                id,
                revision: pin.revision,
                document,
                hidden: pin.hidden,
            })
        }
        Event::VisibilityChanged { id, hidden } => {
            let Some(pin) = state.pinned_images.get_mut(&id) else {
                return vec![];
            };
            if pin.hidden == hidden {
                return vec![];
            }
            pin.proposed = None;
            pin.proposal_ready = false;
            pin.hidden = hidden;
            let mut commands = Vec::new();
            if let Some(edit) = pin.editing.take() {
                commands.push(AppCommand::PinnedImage(Command::EndEdit { id, edit }));
            }
            if let Some(document) = pin.document.clone() {
                pin.revision += 1;
                commands.push(AppCommand::PinnedImage(Command::Render {
                    id,
                    revision: pin.revision,
                    document,
                    hidden,
                }));
            }
            commands
        }
        Event::HistoryOpened { id, key, hidden }
            if id > state.last_pin_id
                && !state.pinned_images.contains_key(&id)
                && state.image_history.pin_ticket(key).is_some() =>
        {
            let (ticket, _) = state.image_history.ticket(key).unwrap();
            state.last_pin_id = id;
            state.pinned_images.insert(
                id,
                State {
                    hidden,
                    ..Default::default()
                },
            );
            command(Command::RetainHistory { id, ticket })
        }
        Event::HistoryOpened { id, .. } if !state.pinned_images.contains_key(&id) => {
            command(Command::Drop { id })
        }
        Event::Opened { id, source }
            if id > state.last_pin_id
                && !state.pinned_images.contains_key(&id)
                && source == (state.screenshot.session, state.screenshot.operation)
                && state.screenshot.phase == super::screenshot::ScreenshotPhase::Output =>
        {
            state.last_pin_id = id;
            state.pinned_images.insert(id, State::default());
            command(Command::Retain { id, source })
        }
        Event::Opened { id, .. } if !state.pinned_images.contains_key(&id) => {
            command(Command::Drop { id })
        }
        Event::Requested {
            id,
            kind,
            owner,
            title,
        } => {
            let Some(pin) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| !p.busy && p.proposed.is_none())
            else {
                return vec![];
            };
            pin.operation = pin
                .operation
                .checked_add(1)
                .expect("pin operation exhausted");
            pin.busy = true;
            pin.error = None;
            command(Command::Output {
                id,
                operation: pin.operation,
                kind,
                owner,
                title,
                document: pin.document.clone(),
                hidden: pin.hidden,
            })
        }
        Event::Completed {
            id,
            operation,
            result,
        } => {
            if let Some(pin) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.busy && p.recognition.is_none() && p.operation == operation)
            {
                pin.busy = false;
                pin.error = result.err().map(|e| e.to_string());
            }
            vec![]
        }
        Event::Closed { id } => {
            let Some(pin) = state.pinned_images.remove(&id) else {
                return vec![];
            };
            let mut commands = Vec::new();
            if let Some(edit) = pin.editing {
                commands.push(AppCommand::PinnedImage(Command::EndEdit { id, edit }));
            }
            commands.push(AppCommand::PinnedImage(Command::Drop { id }));
            commands
        }
        _ => vec![],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn document(request: u64) -> ExportRequest {
        use crate::domain::{
            annotation::Session, annotation_export::ExportSnapshot, geometry::Rect,
        };
        use std::sync::Arc;
        ExportRequest {
            session: 1,
            request,
            snapshot: Arc::new(ExportSnapshot {
                session: Arc::new(Session::new()),
                monitor: Rect {
                    left: 0,
                    top: 0,
                    right: 20,
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
    fn ocr_state() -> AppState {
        let mut s = AppState::default();
        for id in [1, 2] {
            s.pinned_images.insert(
                id,
                State {
                    source: Some(super::super::geometry::Rect {
                        left: -10,
                        top: -20,
                        right: 20,
                        bottom: 10,
                    }),
                    document: Some(document(1)),
                    ..Default::default()
                },
            );
        }
        s
    }
    #[test]
    fn recognition_freezes_confirmed_crop_and_replaces_only_other_request() {
        let mut s = ocr_state();
        let commands = reduce(&mut s, Event::Recognize { id: 1 });
        let old = s.pinned_images[&1].recognition.unwrap();
        assert_eq!(
            old.bounds,
            s.pinned_images[&1]
                .document
                .as_ref()
                .unwrap()
                .snapshot
                .monitor
        );
        assert!(matches!(
            commands.as_slice(),
            [AppCommand::PinnedImage(Command::Recognize { .. })]
        ));
        assert!(reduce(&mut s, Event::Recognize { id: 1 }).is_empty());
        assert_eq!(s.pinned_images[&1].recognition, Some(old));
        reduce(&mut s, Event::Recognize { id: 2 });
        assert!(!s.pinned_images[&1].busy);
        reduce(
            &mut s,
            Event::Recognized {
                id: 1,
                operation: old.operation,
                revision: old.revision,
                outcome: super::super::ocr::OcrOutcome::Copied,
            },
        );
        assert!(s.pinned_images[&1].ocr_result.is_none());
        assert!(s.pinned_images[&2].busy);
    }
    #[test]
    fn outcomes_keep_pin_and_editor_and_cancel_rejects_late_result() {
        for outcome in [
            super::super::ocr::OcrOutcome::Copied,
            super::super::ocr::OcrOutcome::Empty,
            super::super::ocr::OcrOutcome::ResourcesUnavailable,
            super::super::ocr::OcrOutcome::RecognitionFailed,
            super::super::ocr::OcrOutcome::CopyFailed,
        ] {
            let mut s = ocr_state();
            s.pinned_images.get_mut(&1).unwrap().editing = Some(7);
            reduce(&mut s, Event::Recognize { id: 1 });
            let r = s.pinned_images[&1].recognition.unwrap();
            assert!(
                reduce(
                    &mut s,
                    Event::Recognized {
                        id: 1,
                        operation: r.operation,
                        revision: r.revision + 1,
                        outcome
                    }
                )
                .is_empty()
            );
            assert!(s.pinned_images[&1].busy);
            reduce(
                &mut s,
                Event::Recognized {
                    id: 1,
                    operation: r.operation,
                    revision: r.revision,
                    outcome,
                },
            );
            assert_eq!(s.pinned_images[&1].editing, Some(7));
            assert_eq!(s.pinned_images[&1].ocr_result, Some((r.operation, outcome)));
            reduce(&mut s, Event::Recognize { id: 1 });
            let r = s.pinned_images[&1].recognition.unwrap();
            reduce(&mut s, Event::CancelRecognition { id: 1 });
            reduce(
                &mut s,
                Event::Recognized {
                    id: 1,
                    operation: r.operation,
                    revision: r.revision,
                    outcome,
                },
            );
            assert!(!s.pinned_images[&1].busy);
            assert!(s.pinned_images[&1].ocr_result.is_none());
        }
    }
    #[test]
    fn output_of_same_pin_is_blocked_but_other_output_cancels_recognition() {
        let mut s = ocr_state();
        s.reduce(crate::AppEvent::PinnedImage(Event::Recognize { id: 1 }));
        let output = |id| {
            crate::AppEvent::PinnedImage(Event::Requested {
                id,
                kind: Output::Copy,
                owner: None,
                title: String::new(),
            })
        };
        assert!(s.reduce(output(1)).is_empty());
        assert!(s.pinned_images[&1].recognition.is_some());
        assert!(!s.reduce(output(2)).is_empty());
        assert!(s.pinned_images[&1].recognition.is_none());
        s.reduce(crate::AppEvent::PinnedImage(Event::Recognize { id: 1 }));
        s.reduce(crate::AppEvent::ExitRequested);
        assert!(s.pinned_images.values().all(|p| p.recognition.is_none()));
    }
    #[test]
    fn crop_candidate_commits_only_after_matching_presentation_and_cancel_retires_it() {
        let mut s = AppState::default();
        for id in [1, 2] {
            s.pinned_images.insert(
                id,
                State {
                    editing: Some(1),
                    last_edit: 1,
                    document: Some(document(1)),
                    source: Some(document(1).snapshot.monitor),
                    ..Default::default()
                },
            );
        }
        let propose = || Event::DocumentProposed {
            id: 1,
            edit: 1,
            document: document(2),
        };
        assert_eq!(reduce(&mut s, propose()).len(), 1);
        let rev = s.pinned_images[&1].revision;
        assert_eq!(
            s.pinned_images[&1].document.as_ref().map(|d| d.request),
            Some(1)
        );
        assert!(reduce(&mut s, propose()).is_empty());
        assert!(
            reduce(
                &mut s,
                Event::Requested {
                    id: 1,
                    kind: Output::Copy,
                    owner: None,
                    title: String::new()
                }
            )
            .is_empty()
        );
        reduce(
            &mut s,
            Event::ProposalAccepted {
                id: 1,
                edit: 1,
                request: 2,
                revision: rev,
            },
        );
        assert_eq!(
            s.pinned_images[&1].document.as_ref().map(|d| d.request),
            Some(1),
            "unrendered candidate is not committed"
        );
        reduce(
            &mut s,
            Event::Rendered {
                id: 1,
                revision: rev,
            },
        );
        reduce(
            &mut s,
            Event::ProposalCancelled {
                id: 1,
                edit: 1,
                request: 2,
            },
        );
        reduce(
            &mut s,
            Event::ProposalAccepted {
                id: 1,
                edit: 1,
                request: 2,
                revision: rev,
            },
        );
        assert_eq!(
            s.pinned_images[&1].document.as_ref().map(|d| d.request),
            Some(1),
            "cancel wins over queued presentation"
        );
        reduce(&mut s, propose());
        let rev = s.pinned_images[&1].revision;
        reduce(
            &mut s,
            Event::Rendered {
                id: 1,
                revision: rev,
            },
        );
        reduce(
            &mut s,
            Event::ProposalAccepted {
                id: 1,
                edit: 99,
                request: 2,
                revision: rev,
            },
        );
        assert!(s.pinned_images[&1].proposed.is_some());
        reduce(
            &mut s,
            Event::ProposalAccepted {
                id: 1,
                edit: 1,
                request: 2,
                revision: rev,
            },
        );
        assert_eq!(
            s.pinned_images[&1].document.as_ref().map(|d| d.request),
            Some(2)
        );
        assert_eq!(
            s.pinned_images[&2].document.as_ref().map(|d| d.request),
            Some(1)
        );
    }
    #[test]
    fn failed_or_retired_crop_never_replaces_confirmed_document() {
        let mut s = AppState::default();
        s.pinned_images.insert(
            1,
            State {
                editing: Some(1),
                last_edit: 1,
                document: Some(document(1)),
                source: Some(document(1).snapshot.monitor),
                ..Default::default()
            },
        );
        let mut invalid = document(2);
        std::sync::Arc::make_mut(&mut invalid.snapshot)
            .monitor
            .right = 21;
        assert!(
            reduce(
                &mut s,
                Event::DocumentProposed {
                    id: 1,
                    edit: 1,
                    document: invalid
                }
            )
            .is_empty()
        );
        for end in [false, true] {
            reduce(
                &mut s,
                Event::DocumentProposed {
                    id: 1,
                    edit: 1,
                    document: document(2),
                },
            );
            let revision = s.pinned_images[&1].revision;
            reduce(
                &mut s,
                if end {
                    Event::EditFinished { id: 1, edit: 1 }
                } else {
                    Event::PreviewFailed {
                        id: 1,
                        revision,
                        error: "fixture".into(),
                    }
                },
            );
            reduce(&mut s, Event::Rendered { id: 1, revision });
            reduce(
                &mut s,
                Event::ProposalAccepted {
                    id: 1,
                    edit: 1,
                    request: 2,
                    revision,
                },
            );
            assert_eq!(
                s.pinned_images[&1].document.as_ref().map(|d| d.request),
                Some(1)
            );
            assert!(s.pinned_images[&1].proposed.is_none());
        }
    }
    #[test]
    fn editing_visibility_and_output_are_independent_per_pin() {
        let mut s = AppState::default();
        for id in [1, 2] {
            s.pinned_images.insert(id, State::default());
            reduce(
                &mut s,
                Event::DocumentChanged {
                    id,
                    document: document(1),
                },
            );
        }
        let frozen = s.pinned_images[&1].document.clone().unwrap();
        reduce(
            &mut s,
            Event::VisibilityChanged {
                id: 1,
                hidden: true,
            },
        );
        assert!(s.pinned_images[&1].hidden && !s.pinned_images[&2].hidden);
        let commands = reduce(
            &mut s,
            Event::Requested {
                id: 1,
                kind: Output::Copy,
                owner: None,
                title: String::new(),
            },
        );
        assert!(
            matches!(&commands[0],AppCommand::PinnedImage(Command::Output{hidden:true,document:Some(d),..}) if d==&frozen)
        );
        assert!(
            reduce(&mut s, Event::EditRequested { id: 1 }).is_empty(),
            "busy output blocks entry"
        );
        reduce(
            &mut s,
            Event::Completed {
                id: 1,
                operation: 1,
                result: Ok(SaveOutcome::Saved),
            },
        );
        reduce(&mut s, Event::EditRequested { id: 1 });
        let edit = s.pinned_images[&1].editing.unwrap();
        assert!(!s.pinned_images[&1].hidden);
        let commands = reduce(&mut s, Event::EditRequested { id: 2 });
        assert!(
            matches!(commands[0],AppCommand::PinnedImage(Command::EndEdit{id:1,edit:e}) if e==edit)
        );
        assert!(s.pinned_images[&1].editing.is_none());
        reduce(&mut s, Event::EditFinished { id: 2, edit: 99 });
        assert!(
            s.pinned_images[&2].editing.is_some(),
            "stale finish cannot close new editor"
        );
        reduce(
            &mut s,
            Event::VisibilityChanged {
                id: 2,
                hidden: true,
            },
        );
        assert!(s.pinned_images[&2].editing.is_none());
        reduce(&mut s, Event::EditRequested { id: 1 });
        let mut stale = document(99);
        stale.session = edit;
        assert!(
            reduce(
                &mut s,
                Event::DocumentChanged {
                    id: 1,
                    document: stale
                }
            )
            .is_empty(),
            "old editing sessions cannot replace a reopened document"
        );
        let revision = s.pinned_images[&1].revision;
        assert!(
            reduce(
                &mut s,
                Event::DocumentChanged {
                    id: 1,
                    document: document(1)
                }
            )
            .is_empty()
        );
        assert_eq!(s.pinned_images[&1].revision, revision);
        reduce(&mut s, Event::Closed { id: 1 });
        assert!(
            reduce(
                &mut s,
                Event::DocumentChanged {
                    id: 1,
                    document: document(50)
                }
            )
            .is_empty()
        );
    }
    #[test]
    fn output_is_owned_by_pin_and_late_results_are_ignored() {
        let mut s = AppState::default();
        s.screenshot.session = 1;
        s.screenshot.operation = 2;
        s.screenshot.phase = super::super::screenshot::ScreenshotPhase::Output;
        assert_eq!(
            reduce(
                &mut s,
                Event::Opened {
                    id: 7,
                    source: (1, 2)
                }
            )
            .len(),
            1
        );
        let request = Event::Requested {
            id: 7,
            kind: Output::Copy,
            owner: None,
            title: String::new(),
        };
        s.screenshot.phase = super::super::screenshot::ScreenshotPhase::Idle;
        assert_eq!(reduce(&mut s, request.clone()).len(), 1);
        assert!(reduce(&mut s, request).is_empty());
        reduce(
            &mut s,
            Event::Completed {
                id: 7,
                operation: 99,
                result: Ok(SaveOutcome::Saved),
            },
        );
        assert!(s.pinned_images[&7].busy);
        assert_eq!(reduce(&mut s, Event::Closed { id: 7 }).len(), 1);
        reduce(
            &mut s,
            Event::Completed {
                id: 7,
                operation: 1,
                result: Ok(SaveOutcome::Saved),
            },
        );
        assert!(s.pinned_images.is_empty());
        s.screenshot.phase = super::super::screenshot::ScreenshotPhase::Output;
        assert_eq!(
            reduce(
                &mut s,
                Event::Opened {
                    id: 7,
                    source: (1, 2)
                }
            ),
            vec![AppCommand::PinnedImage(Command::Drop { id: 7 })],
            "retired IDs must dispose only their own late view"
        );
        assert_eq!(
            reduce(
                &mut s,
                Event::Opened {
                    id: 8,
                    source: (1, 2)
                }
            )
            .len(),
            1
        );
        reduce(
            &mut s,
            Event::Completed {
                id: 7,
                operation: 1,
                result: Err(crate::Error::new("late")),
            },
        );
        assert_eq!(s.pinned_images[&8], State::default());
    }
}
