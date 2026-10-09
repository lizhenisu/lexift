//! Bounded plain-text history and independent paste sessions. Debug never exposes content.
use super::{geometry::Point, selection::SelectionTarget};
use crate::{AppCommand, AppState};
use std::sync::Arc;
pub const MAX_ENTRY_BYTES: usize = 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
pub const UNSUPPORTED_TARGET_MESSAGE: &str =
    "The original selection is not a supported editable input.";
#[derive(Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: u64,
    pub text: Arc<str>,
}
impl std::fmt::Debug for HistoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryEntry")
            .field("id", &self.id)
            .field("bytes", &self.text.len())
            .finish()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasteTarget {
    pub selection: SelectionTarget,
    pub runtime_id: Vec<i32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteTargetState {
    Pending,
    Ready(PasteTarget),
    Unsupported,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySession {
    pub copy: std::collections::BTreeMap<u64, super::copy_feedback::CopyStatus>,
    pub id: u64,
    pub target: SelectionTarget,
    pub anchor: Point,
    pub paste_target: PasteTargetState,
    pub unsupported_attempt: u64,
    pub busy: bool,
    pub request: u64,
    pub error: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryState {
    pub entries: Vec<HistoryEntry>,
    pub session: Option<HistorySession>,
    pub loaded: bool,
    pub recording: bool,
    pub storage_failed: bool,
    pub revision: u64,
    pub error: String,
    next_entry: u64,
    next_session: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEvent {
    Open {
        generation: u64,
        anchor: Point,
    },
    Close {
        session: u64,
    },
    Loaded(crate::Result<Vec<HistoryEntry>>),
    Captured(HistoryEntry),
    CaptureSkipped,
    Delete {
        session: u64,
        entry: u64,
    },
    Enable {
        session: u64,
    },
    OpenSettings {
        session: u64,
    },
    Clear,
    Copy {
        session: u64,
        entry: u64,
    },
    CopyFinished {
        session: u64,
        entry: u64,
        request: u64,
        success: bool,
    },
    CopyFeedbackCleared {
        session: u64,
        entry: u64,
        request: u64,
    },
    MonitoringFailed,
    MonitoringReady(bool),
    TargetReady {
        session: u64,
        target: crate::Result<PasteTarget>,
    },
    Paste {
        session: u64,
        entry: u64,
    },
    PasteFinished {
        session: u64,
        request: u64,
        result: crate::Result<()>,
    },
    Stored {
        revision: u64,
        success: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryCommand {
    Copy {
        session: u64,
        request: u64,
        entry: HistoryEntry,
    },
    Save {
        revision: u64,
        entries: Vec<HistoryEntry>,
    },
    CaptureTarget {
        session: u64,
        target: SelectionTarget,
    },
    Paste {
        session: u64,
        request: u64,
        target: PasteTarget,
        entry: HistoryEntry,
    },
}
impl HistoryState {
    pub fn trim(&mut self, limit: u16) {
        self.entries.truncate(usize::from(limit.clamp(10, 200)));
        let mut bytes = 0;
        self.entries.retain(|e| {
            bytes += e.text.len();
            bytes <= MAX_TOTAL_BYTES
        });
        if let Some(session) = self.session.as_mut() {
            session
                .copy
                .retain(|id, _| self.entries.iter().any(|e| e.id == *id));
        }
    }
    fn save(&mut self) -> Vec<AppCommand> {
        self.revision = self.revision.wrapping_add(1);
        if self.storage_failed {
            return vec![];
        }
        vec![AppCommand::ClipboardHistory(HistoryCommand::Save {
            revision: self.revision,
            entries: self.entries.clone(),
        })]
    }
    pub fn reconcile_limit(&mut self, limit: u16) -> Vec<AppCommand> {
        let before = self.entries.len();
        self.trim(limit);
        if before != self.entries.len() {
            self.save()
        } else {
            vec![]
        }
    }
}
pub fn reduce(state: &mut AppState, event: HistoryEvent) -> Vec<AppCommand> {
    let limit = state.settings.clipboard_history_limit;
    let h = &mut state.clipboard_history;
    match event {
        HistoryEvent::Open { generation, anchor } => {
            if generation != state.toolbar_generation || state.toolbar_selection.is_none() {
                return vec![];
            }
            let Some(target) = state.toolbar_target.take() else {
                return vec![];
            };
            state.toolbar_selection = None;
            state.toolbar_generation = state.toolbar_generation.wrapping_add(1);
            h.next_session = h.next_session.wrapping_add(1);
            h.session = Some(HistorySession {
                copy: Default::default(),
                id: h.next_session,
                target,
                anchor,
                paste_target: PasteTargetState::Pending,
                unsupported_attempt: 0,
                busy: false,
                request: 0,
                error: String::new(),
            });
            vec![AppCommand::ClipboardHistory(
                HistoryCommand::CaptureTarget {
                    session: h.next_session,
                    target,
                },
            )]
        }
        HistoryEvent::Close { session } => {
            if h.session.as_ref().is_some_and(|s| s.id == session) {
                h.session = None;
            }
            vec![]
        }
        HistoryEvent::Loaded(result) => {
            if h.loaded {
                return vec![];
            }
            h.loaded = true;
            match result {
                Ok(entries) => {
                    for e in entries {
                        if !e.text.is_empty()
                            && e.text.len() <= MAX_ENTRY_BYTES
                            && !h.entries.iter().any(|old| old.text == e.text)
                        {
                            h.next_entry = h.next_entry.max(e.id);
                            h.entries.push(e);
                        }
                    }
                    h.reconcile_limit(limit)
                }
                Err(_) => {
                    h.storage_failed = true;
                    h.error =
                        "Clipboard history could not be loaded. Clear history to start again."
                            .into();
                    vec![]
                }
            }
        }
        HistoryEvent::Captured(mut entry)
            if h.loaded && state.settings.clipboard_history_enabled =>
        {
            if entry.text.is_empty() {
                return vec![];
            }
            if entry.text.len() > MAX_ENTRY_BYTES {
                h.error = "This clipboard text is too large to record.".into();
                return vec![];
            }
            // Stable IDs keep in-flight actions attached when an identical copy moves to the top.
            if let Some(old) = h.entries.iter().find(|old| old.text == entry.text) {
                entry.id = old.id;
            } else {
                h.next_entry = h.next_entry.wrapping_add(1);
                entry.id = h.next_entry;
            }
            h.entries.retain(|old| old.text != entry.text);
            h.entries.insert(0, entry);
            h.trim(limit);
            h.save()
        }
        HistoryEvent::MonitoringReady(enabled) => {
            h.recording = enabled;
            if enabled && h.error == "Clipboard history monitoring is unavailable" {
                h.error.clear();
            }
            vec![]
        }
        HistoryEvent::MonitoringFailed => {
            h.recording = false;
            h.error = "Clipboard history monitoring is unavailable".into();
            vec![]
        }
        HistoryEvent::CaptureSkipped => {
            h.error = "This clipboard text is too large to record.".into();
            vec![]
        }
        HistoryEvent::Captured(_) => vec![],
        HistoryEvent::Delete { session, entry } => {
            if !h
                .session
                .as_ref()
                .is_some_and(|s| s.id == session && !s.busy)
            {
                return vec![];
            }
            h.entries.retain(|e| e.id != entry);
            if let Some(s) = h.session.as_mut() {
                s.copy.remove(&entry);
            }
            h.save()
        }
        HistoryEvent::Enable { session } => {
            if !h.session.as_ref().is_some_and(|s| s.id == session) {
                return vec![];
            }
            state.reduce(crate::AppEvent::SettingsChangeRequested {
                change: super::settings::SettingsChange::ClipboardHistoryEnabled(true),
            })
        }
        HistoryEvent::OpenSettings { session } => {
            if !h.session.as_ref().is_some_and(|s| s.id == session) {
                return vec![];
            }
            h.session = None;
            state.reduce(crate::AppEvent::SettingsWindowRequested)
        }
        HistoryEvent::Clear => {
            h.entries.clear();
            if let Some(session) = h.session.as_mut() {
                session.copy.clear();
            }
            h.storage_failed = false;
            h.error.clear();
            h.save()
        }
        HistoryEvent::TargetReady { session, target } => {
            if let Some(s) = h
                .session
                .as_mut()
                .filter(|s| s.id == session && s.paste_target == PasteTargetState::Pending)
            {
                s.paste_target = match target {
                    Ok(t) => PasteTargetState::Ready(t),
                    Err(_) => PasteTargetState::Unsupported,
                };
            }
            vec![]
        }
        HistoryEvent::Copy { session, entry } => {
            let Some(s) = h.session.as_mut().filter(|s| s.id == session && !s.busy) else {
                return vec![];
            };
            let Some(item) = h.entries.iter().find(|e| e.id == entry).cloned() else {
                return vec![];
            };
            let Some(request) = s.copy.entry(entry).or_default().begin() else {
                return vec![];
            };
            s.error.clear();
            vec![AppCommand::ClipboardHistory(HistoryCommand::Copy {
                session,
                request,
                entry: item,
            })]
        }
        HistoryEvent::CopyFinished {
            session,
            entry,
            request,
            success,
        } => {
            if h.entries.iter().any(|e| e.id == entry)
                && let Some(s) = h.session.as_mut().filter(|s| s.id == session)
                && s.copy
                    .get_mut(&entry)
                    .is_some_and(|c| c.finish(request, success))
                && !success
            {
                s.error = "Could not copy clipboard entry.".into();
            }
            vec![]
        }
        HistoryEvent::CopyFeedbackCleared {
            session,
            entry,
            request,
        } => {
            if let Some(s) = h.session.as_mut().filter(|s| s.id == session)
                && let Some(c) = s.copy.get_mut(&entry)
            {
                c.clear(request);
            }
            vec![]
        }
        HistoryEvent::Paste { session, entry } => {
            let Some(s) = h
                .session
                .as_mut()
                .filter(|s| s.id == session && !s.busy && !s.copy.values().any(|c| c.pending))
            else {
                return vec![];
            };
            let Some(entry) = h.entries.iter().find(|e| e.id == entry).cloned() else {
                return vec![];
            };
            let target = match &s.paste_target {
                PasteTargetState::Pending => return vec![],
                PasteTargetState::Unsupported => {
                    s.unsupported_attempt = s.unsupported_attempt.wrapping_add(1);
                    return vec![];
                }
                PasteTargetState::Ready(target) => target.clone(),
            };
            s.busy = true;
            s.request = s.request.wrapping_add(1);
            s.error.clear();
            vec![AppCommand::ClipboardHistory(HistoryCommand::Paste {
                session,
                request: s.request,
                target,
                entry,
            })]
        }
        HistoryEvent::PasteFinished {
            session,
            request,
            result,
        } => {
            if let Some(s) = h
                .session
                .as_mut()
                .filter(|s| s.id == session && s.request == request && s.busy)
            {
                match result {
                    Ok(()) => h.session = None,
                    Err(e) => {
                        s.busy = false;
                        s.error = e.to_string();
                    }
                }
            }
            vec![]
        }
        HistoryEvent::Stored { revision, success } => {
            if revision == h.revision {
                h.error = if success {
                    String::new()
                } else {
                    "Clipboard history could not be saved.".into()
                };
            }
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppEvent;
    use crate::{
        domain::{selection::Selection, settings::Settings},
        ports::magnifier::WindowToken,
    };
    fn state() -> AppState {
        let mut s = AppState::new(Settings::default());
        s.settings.clipboard_history_enabled = true;
        s.clipboard_history.loaded = true;
        s
    }
    fn capture(s: &mut AppState, text: &str) {
        reduce(
            s,
            HistoryEvent::Captured(HistoryEntry {
                id: 0,
                text: Arc::from(text),
            }),
        );
    }
    fn open(s: &mut AppState) -> u64 {
        s.toolbar_selection = Some(Selection {
            text: "selected".into(),
            bounds: vec![],
            anchor: None,
            gesture_start: None,
        });
        s.toolbar_target = Some(SelectionTarget {
            window: WindowToken(1),
            focus: Some(WindowToken(2)),
            process: 3,
            thread: 4,
        });
        let generation = s.toolbar_generation;
        reduce(
            s,
            HistoryEvent::Open {
                generation,
                anchor: Point { x: -200, y: 100 },
            },
        );
        s.clipboard_history.session.as_ref().unwrap().id
    }
    #[test]
    fn unsupported_target_is_silent_until_a_valid_entry_is_clicked_and_never_injects() {
        let mut s = state();
        capture(&mut s, "clipboard text");
        let id = open(&mut s);
        let entry = s.clipboard_history.entries[0].id;
        assert!(reduce(&mut s, HistoryEvent::Paste { session: id, entry }).is_empty());
        assert_eq!(
            s.clipboard_history
                .session
                .as_ref()
                .unwrap()
                .unsupported_attempt,
            0
        );
        assert!(
            reduce(
                &mut s,
                HistoryEvent::TargetReady {
                    session: id,
                    target: Err(crate::Error::new("private native error")),
                }
            )
            .is_empty()
        );
        let session = s.clipboard_history.session.as_ref().unwrap();
        assert_eq!(session.paste_target, PasteTargetState::Unsupported);
        assert!(session.error.is_empty());
        assert_eq!(session.unsupported_attempt, 0);
        assert!(
            reduce(
                &mut s,
                HistoryEvent::Paste {
                    session: id,
                    entry: u64::MAX
                }
            )
            .is_empty()
        );
        for attempt in 1..=2 {
            // No commands means no clipboard write, Ctrl+V, persistence or activation.
            assert!(reduce(&mut s, HistoryEvent::Paste { session: id, entry }).is_empty());
            let session = s.clipboard_history.session.as_ref().unwrap();
            assert_eq!(session.unsupported_attempt, attempt);
            assert!(session.error.is_empty());
            assert!(!session.busy);
        }
        assert!(matches!(
            reduce(&mut s, HistoryEvent::Copy { session: id, entry }).as_slice(),
            [AppCommand::ClipboardHistory(HistoryCommand::Copy { .. })]
        ));
        assert!(matches!(
            reduce(&mut s, HistoryEvent::Delete { session: id, entry }).as_slice(),
            [AppCommand::ClipboardHistory(HistoryCommand::Save { .. })]
        ));
        reduce(&mut s, HistoryEvent::Close { session: id });
        let next = open(&mut s);
        assert!(reduce(&mut s, HistoryEvent::Paste { session: id, entry }).is_empty());
        reduce(
            &mut s,
            HistoryEvent::TargetReady {
                session: id,
                target: Err(crate::Error::new("late")),
            },
        );
        let session = s.clipboard_history.session.as_ref().unwrap();
        assert_eq!(session.id, next);
        assert_eq!(session.paste_target, PasteTargetState::Pending);
        assert_eq!(session.unsupported_attempt, 0);
    }
    #[test]
    fn copying_keeps_full_text_and_survives_dedup_reordering() {
        let mut s = state();
        capture(&mut s, "  first\n第二行\n");
        let entry = s.clipboard_history.entries[0].id;
        capture(&mut s, "other");
        let session = open(&mut s);
        let commands = reduce(&mut s, HistoryEvent::Copy { session, entry });
        let [
            AppCommand::ClipboardHistory(HistoryCommand::Copy {
                request,
                entry: copied,
                ..
            }),
        ] = commands.as_slice()
        else {
            panic!("missing copy command");
        };
        let request = *request;
        assert_eq!(&*copied.text, "  first\n第二行\n");
        assert!(reduce(&mut s, HistoryEvent::Copy { session, entry }).is_empty());
        capture(&mut s, "  first\n第二行\n");
        assert_eq!(s.clipboard_history.entries[0].id, entry);
        reduce(
            &mut s,
            HistoryEvent::CopyFinished {
                session,
                entry,
                request,
                success: true,
            },
        );
        assert_eq!(
            s.clipboard_history.session.as_ref().unwrap().copy[&entry].completed,
            request
        );
        assert!(
            s.clipboard_history
                .session
                .as_ref()
                .unwrap()
                .error
                .is_empty()
        );
        reduce(&mut s, HistoryEvent::Copy { session, entry });
        let next = s.clipboard_history.session.as_ref().unwrap().copy[&entry].request;
        reduce(
            &mut s,
            HistoryEvent::CopyFinished {
                session,
                entry,
                request: next,
                success: true,
            },
        );
        reduce(
            &mut s,
            HistoryEvent::CopyFeedbackCleared {
                session,
                entry,
                request,
            },
        );
        assert_eq!(
            s.clipboard_history.session.as_ref().unwrap().copy[&entry].completed,
            next
        );
        reduce(
            &mut s,
            HistoryEvent::CopyFeedbackCleared {
                session,
                entry,
                request: next,
            },
        );
        assert_eq!(
            s.clipboard_history.session.as_ref().unwrap().copy[&entry].completed,
            0
        );
    }
    #[test]
    fn failed_deleted_and_closed_copy_requests_do_not_report_success() {
        let mut s = state();
        capture(&mut s, "one");
        let entry = s.clipboard_history.entries[0].id;
        let session = open(&mut s);
        reduce(&mut s, HistoryEvent::Copy { session, entry });
        reduce(
            &mut s,
            HistoryEvent::CopyFinished {
                session,
                entry,
                request: 1,
                success: false,
            },
        );
        let active = s.clipboard_history.session.as_ref().unwrap();
        assert!(!active.copy[&entry].pending);
        assert_eq!(active.copy[&entry].completed, 0);
        assert!(!active.error.is_empty());
        reduce(&mut s, HistoryEvent::Copy { session, entry });
        reduce(&mut s, HistoryEvent::Delete { session, entry });
        reduce(
            &mut s,
            HistoryEvent::CopyFinished {
                session,
                entry,
                request: 2,
                success: true,
            },
        );
        assert!(
            !s.clipboard_history
                .session
                .as_ref()
                .unwrap()
                .copy
                .contains_key(&entry)
        );
        reduce(&mut s, HistoryEvent::Close { session });
        reduce(
            &mut s,
            HistoryEvent::CopyFinished {
                session,
                entry,
                request: 2,
                success: true,
            },
        );
        assert!(s.clipboard_history.session.is_none());
    }
    #[test]
    fn bounded_exact_dedup_preserves_whitespace_and_rejects_large_text() {
        let mut s = state();
        s.settings.clipboard_history_limit = 10;
        for i in 0..12 {
            capture(&mut s, &format!(" {i}\n"));
        }
        assert_eq!(s.clipboard_history.entries.len(), 10);
        capture(&mut s, " 4\n");
        assert_eq!(s.clipboard_history.entries.len(), 10);
        assert_eq!(&*s.clipboard_history.entries[0].text, " 4\n");
        capture(&mut s, "4");
        assert_ne!(
            s.clipboard_history.entries[0].text,
            s.clipboard_history.entries[1].text
        );
        let entries = s.clipboard_history.entries.clone();
        capture(&mut s, &"x".repeat(MAX_ENTRY_BYTES + 1));
        assert_eq!(s.clipboard_history.entries, entries);
        s.settings.clipboard_history_limit = 200;
        for i in 0..20 {
            capture(&mut s, &format!("{i}{}", "x".repeat(MAX_ENTRY_BYTES - 2)));
        }
        assert!(
            s.clipboard_history
                .entries
                .iter()
                .map(|e| e.text.len())
                .sum::<usize>()
                <= MAX_TOTAL_BYTES
        );
    }
    #[test]
    fn toolbar_retirement_does_not_cancel_history_and_stale_results_are_inert() {
        let mut s = state();
        capture(&mut s, "full\ntext");
        let id = open(&mut s);
        assert!(s.toolbar_selection.is_none());
        s.reduce(AppEvent::SelectionToolbarDismissRequested);
        assert_eq!(s.clipboard_history.session.as_ref().unwrap().id, id);
        let target = PasteTarget {
            selection: s.clipboard_history.session.as_ref().unwrap().target,
            runtime_id: vec![1, 2],
        };
        reduce(
            &mut s,
            HistoryEvent::TargetReady {
                session: id,
                target: Ok(target),
            },
        );
        let entry = s.clipboard_history.entries[0].id;
        let commands = reduce(&mut s, HistoryEvent::Paste { session: id, entry });
        assert_eq!(commands.len(), 1);
        assert!(reduce(&mut s, HistoryEvent::Paste { session: id, entry }).is_empty());
        reduce(&mut s, HistoryEvent::Close { session: id });
        let next = open(&mut s);
        reduce(
            &mut s,
            HistoryEvent::PasteFinished {
                session: id,
                request: 1,
                result: Ok(()),
            },
        );
        assert_eq!(s.clipboard_history.session.as_ref().unwrap().id, next);
        s.reduce(AppEvent::SelectionInteractionStarted);
        assert!(s.clipboard_history.session.is_none());
    }
    #[test]
    fn corrupt_load_never_overwrites_until_explicit_clear_and_stale_write_cannot_report_success() {
        let mut s = AppState::new(Settings::default());
        s.settings.clipboard_history_enabled = true;
        reduce(
            &mut s,
            HistoryEvent::Loaded(Err(crate::Error::new("corrupt"))),
        );
        assert!(s.clipboard_history.storage_failed);
        assert!(
            reduce(
                &mut s,
                HistoryEvent::Captured(HistoryEntry {
                    id: 0,
                    text: Arc::from("new")
                })
            )
            .is_empty()
        );
        let commands = reduce(&mut s, HistoryEvent::Clear);
        assert_eq!(commands.len(), 1);
        assert!(!s.clipboard_history.storage_failed);
        let revision = s.clipboard_history.revision;
        reduce(
            &mut s,
            HistoryEvent::Stored {
                revision,
                success: false,
            },
        );
        let error = s.clipboard_history.error.clone();
        reduce(
            &mut s,
            HistoryEvent::Stored {
                revision: revision - 1,
                success: true,
            },
        );
        assert_eq!(s.clipboard_history.error, error);
    }
    #[test]
    fn disabled_recording_retains_history_and_debug_is_redacted() {
        let mut s = state();
        capture(&mut s, "secret-test-body");
        s.settings.clipboard_history_enabled = false;
        capture(&mut s, "ignored");
        assert_eq!(s.clipboard_history.entries.len(), 1);
        assert!(!format!("{:?}", s.clipboard_history).contains("secret-test-body"));
    }
    #[test]
    fn closed_session_cannot_delete_enable_or_open_settings() {
        let mut s = state();
        capture(&mut s, "kept");
        let id = open(&mut s);
        let entry = s.clipboard_history.entries[0].id;
        reduce(&mut s, HistoryEvent::Close { session: id });
        assert!(reduce(&mut s, HistoryEvent::Delete { session: id, entry }).is_empty());
        assert_eq!(s.clipboard_history.entries.len(), 1);
        assert!(reduce(&mut s, HistoryEvent::Enable { session: id }).is_empty());
        assert!(reduce(&mut s, HistoryEvent::OpenSettings { session: id }).is_empty());
    }
}
