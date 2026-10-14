//! Output lifetime of independent pinned images; pixels remain owned by App.
use super::annotation_export::ExportRequest;
use crate::ports::{annotation_save::SaveOutcome, magnifier::WindowToken};
use crate::{AppCommand, AppState, Result};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Copy,
    Save,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub operation: u64,
    pub busy: bool,
    pub error: Option<String>,
    pub hidden: bool,
    pub editing: Option<u64>,
    pub revision: u64,
    pub last_edit: u64,
    pub document: Option<ExportRequest>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
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
pub(crate) fn reduce(state: &mut AppState, event: Event) -> Vec<AppCommand> {
    let command = |c| vec![AppCommand::PinnedImage(c)];
    match event {
        Event::EditFailed { id, edit, error } => {
            if let Some(p) = state
                .pinned_images
                .get_mut(&id)
                .filter(|p| p.editing == Some(edit))
            {
                p.editing = None;
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
            }
            vec![]
        }
        Event::EditRequested { id } => {
            if state.screenshot.active()
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
            let Some(pin) = state.pinned_images.get_mut(&id).filter(|p| !p.busy) else {
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
                .filter(|p| p.busy && p.operation == operation)
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
