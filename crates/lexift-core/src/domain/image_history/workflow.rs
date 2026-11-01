//! Correlate successful image outputs with asynchronously prepared history assets.
use super::*;
use crate::domain::{
    pinned_image as pin,
    screenshot::{ScreenshotCommand, ScreenshotEvent, ScreenshotPhase},
};
use crate::ports::annotation_save::SaveOutcome;
use crate::{AppCommand, AppEvent, AppState};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutputKey {
    Import {
        operation: u64,
    },
    Editor {
        window: u64,
        editor: u64,
        operation: u64,
    },
    History {
        window: u64,
        operation: u64,
    },
    Screenshot {
        session: u64,
        operation: u64,
    },
    Pin {
        id: u64,
        operation: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Import(u64),
    Screenshot(u64),
    LongImage(u64, u64),
    Pin(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    pin: bool,
    ticket: Ticket,
    kind: Kind,
    success: bool,
    prepared: Option<Entry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workflow {
    pub cache: State,
    sources: BTreeMap<Source, (Origin, Kind)>,
    pending: BTreeMap<OutputKey, Pending>,
    pub skipped: Option<Ticket>,
    pub window: Option<Window>,
    next_window: u64,
    pub editor: Option<Editor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub ready: bool,
    pub failed: bool,
    pub hidden: bool,
    pub window: u64,
    pub id: u64,
    pub origin: Origin,
    pub import_anchor: Option<super::super::geometry::Point>,
    pub kind: Kind,
    pub operation: u64,
    pub busy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub anchor: super::super::geometry::Point,
    pub selected: Option<Origin>,
    pub operation: u64,
    pub busy: bool,
    pub error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Edit,
    Copy,
    Save,
    Pin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    EditorOutput {
        window: u64,
        editor: u64,
        operation: u64,
        kind: crate::domain::screenshot::ScreenshotOutput,
        document: crate::domain::annotation_export::ExportRequest,
    },
    Output {
        window: u64,
        operation: u64,
        ticket: Ticket,
        action: Action,
        owner: Option<crate::ports::magnifier::WindowToken>,
        title: String,
    },
    Cancel {
        window: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    EditorVisibility {
        window: u64,
        editor: u64,
        hidden: bool,
    },
    EditorClosed {
        window: u64,
        editor: u64,
    },
    EditorOutput {
        window: u64,
        editor: u64,
        operation: u64,
        kind: crate::domain::screenshot::ScreenshotOutput,
        document: crate::domain::annotation_export::ExportRequest,
    },
    EditorCompleted {
        window: u64,
        editor: u64,
        operation: u64,
        result: crate::Result<SaveOutcome>,
    },
    Open {
        anchor: super::super::geometry::Point,
    },
    Close {
        window: u64,
    },
    Select {
        window: u64,
        ticket: Ticket,
    },
    Action {
        window: u64,
        ticket: Ticket,
        action: Action,
        owner: Option<crate::ports::magnifier::WindowToken>,
        title: String,
    },
    Completed {
        window: u64,
        operation: u64,
        result: crate::Result<SaveOutcome>,
    },
    Prepared {
        key: OutputKey,
        entry: Entry,
    },
    Unavailable {
        key: OutputKey,
        ticket: Ticket,
    },
    Delete {
        ticket: Ticket,
    },
    Clear,
}

impl Workflow {
    pub(crate) fn editor_owner(&mut self) -> u64 {
        self.next_window = self
            .next_window
            .checked_add(1)
            .expect("image owner exhausted");
        self.next_window
    }
    pub(crate) fn start_import(&mut self, operation: u64) {
        self.start(
            OutputKey::Import { operation },
            Source::Import(operation),
            Kind::Imported,
            true,
        );
    }

    pub fn pin_ticket(&self, key: OutputKey) -> Option<Ticket> {
        self.pending.get(&key).filter(|p| p.pin).map(|p| p.ticket)
    }
    pub fn ticket(&self, key: OutputKey) -> Option<(Ticket, Kind)> {
        self.pending.get(&key).map(|p| (p.ticket, p.kind))
    }

    /// Assets in flight remain live until both preparation and output succeed.
    pub fn retains(&self, ticket: Ticket) -> bool {
        self.cache
            .entry(ticket.origin)
            .is_some_and(|e| e.ticket == ticket)
            || self.pending.values().any(|p| p.ticket == ticket)
    }

    fn start(&mut self, key: OutputKey, source: Source, kind: Kind, pin: bool) {
        let origin = if let Some((origin, _)) = self.sources.get(&source) {
            *origin
        } else {
            let Some(origin) = self.cache.origin() else {
                return;
            };
            self.sources.insert(source, (origin, kind));
            origin
        };
        if let Some(ticket) = self.cache.begin(origin) {
            self.pending.insert(
                key,
                Pending {
                    pin,
                    ticket,
                    kind,
                    success: false,
                    prepared: None,
                },
            );
        }
    }

    fn finish(&mut self, key: OutputKey) {
        let ready = self
            .pending
            .get(&key)
            .is_some_and(|p| p.success && p.prepared.is_some());
        if ready {
            let pending = self.pending.remove(&key).unwrap();
            if self.cache.complete(pending.prepared.unwrap()) == Admission::TooLarge {
                self.skipped = Some(pending.ticket);
            }
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Prepared { key, entry } => {
                if let Some(pending) = self
                    .pending
                    .get_mut(&key)
                    .filter(|p| p.ticket == entry.ticket && p.kind == entry.kind)
                {
                    pending.prepared = Some(entry);
                    self.finish(key);
                }
            }
            Event::Unavailable { key, ticket } => {
                if self.pending.get(&key).is_some_and(|p| p.ticket == ticket) {
                    self.pending.remove(&key);
                    self.cache.cancel(ticket);
                }
            }
            Event::Delete { ticket } => {
                if self
                    .cache
                    .entry(ticket.origin)
                    .is_some_and(|e| e.ticket == ticket)
                {
                    self.cache.delete(ticket.origin);
                    self.pending.retain(|_, p| p.ticket.origin != ticket.origin);
                }
            }
            Event::Clear => {
                self.cache.clear();
                self.pending.clear();
                self.skipped = None;
            }
            _ => {}
        }
    }
}

pub fn blocked(state: &AppState) -> bool {
    state.pin_visibility.pending.is_some()
        || state.image_history.editor.is_some()
        || state.screenshot.active()
        || state
            .pinned_images
            .values()
            .any(|p| p.editing.is_some() || p.busy)
}

fn unavailable(state: &AppState) -> bool {
    use crate::domain::{ocr::OcrPhase, screenshot::ScreenshotPhase};
    matches!(
        state.screenshot.phase,
        ScreenshotPhase::CountingDown
            | ScreenshotPhase::Preparing
            | ScreenshotPhase::Selecting
            | ScreenshotPhase::Recognizing
            | ScreenshotPhase::Refreshing
            | ScreenshotPhase::Recording
            | ScreenshotPhase::Output
    ) || state
        .screenshot
        .long
        .as_ref()
        .is_some_and(|l| l.phase != crate::domain::long_capture::workflow::Phase::Editing)
        || matches!(
            state.ocr.phase,
            OcrPhase::Checking | OcrPhase::Capturing | OcrPhase::Recognizing
        )
        || state.annotation_export.request.is_some()
        || state.pinned_images.values().any(|pin| pin.busy)
}

pub(crate) fn reduce(state: &mut AppState, event: Event) -> Vec<AppCommand> {
    let blocked = blocked(state);
    let unavailable = unavailable(state);
    let h = &mut state.image_history;
    match event {
        Event::EditorVisibility {
            window,
            editor,
            hidden,
        } => {
            if let Some(e) = h
                .editor
                .as_mut()
                .filter(|e| e.window == window && e.id == editor && !e.busy)
            {
                e.hidden = hidden;
            }
        }
        Event::EditorClosed { window, editor }
            if h.editor
                .as_ref()
                .is_some_and(|e| e.window == window && e.id == editor) =>
        {
            h.editor = None;
        }
        Event::EditorOutput {
            window,
            editor,
            operation,
            kind,
            document,
        } if h.editor.as_ref().is_some_and(|e| {
            e.window == window && e.id == editor && !e.busy && operation > e.operation
        }) && document.session == editor
            && document.request == operation =>
        {
            let e = h.editor.as_mut().unwrap();
            e.operation = operation;
            e.busy = true;
            let next = h
                .cache
                .begin(e.origin)
                .expect("history output identity exhausted");
            h.pending.insert(
                OutputKey::Editor {
                    window,
                    editor,
                    operation,
                },
                Pending {
                    pin: kind == crate::domain::screenshot::ScreenshotOutput::Pin,
                    ticket: next,
                    kind: e.kind,
                    success: false,
                    prepared: None,
                },
            );
            return vec![AppCommand::ImageHistory(Command::EditorOutput {
                window,
                editor,
                operation,
                kind,
                document,
            })];
        }
        Event::EditorCompleted {
            window,
            editor,
            operation,
            result,
        } => {
            if let Some(e) = h.editor.as_mut().filter(|e| {
                e.window == window && e.id == editor && e.operation == operation && e.busy
            }) {
                e.busy = false;
                e.failed = result.is_err();
            }
        }
        Event::Open { anchor } if !unavailable => {
            if h.window.is_none() {
                h.next_window = h
                    .next_window
                    .checked_add(1)
                    .expect("history window identity exhausted");
                h.window = Some(Window {
                    id: h.next_window,
                    anchor,
                    selected: h.cache.entries().first().map(|e| e.ticket.origin),
                    operation: 0,
                    busy: false,
                    error: false,
                });
            }
        }
        Event::Close { window } if h.window.as_ref().is_some_and(|w| w.id == window) => {
            h.window = None;
            if h.editor
                .as_ref()
                .is_some_and(|e| e.window == window && !e.ready)
            {
                h.editor = None;
            }
            return vec![AppCommand::ImageHistory(Command::Cancel { window })];
        }
        Event::Select { window, ticket }
            if h.cache
                .entry(ticket.origin)
                .is_some_and(|e| e.ticket == ticket) =>
        {
            if let Some(w) = h.window.as_mut().filter(|w| w.id == window && !w.busy) {
                w.selected = Some(ticket.origin);
            }
        }
        Event::Action {
            window,
            ticket,
            action,
            owner,
            title,
        } if !blocked
            && !unavailable
            && h.cache
                .entry(ticket.origin)
                .is_some_and(|e| e.ticket == ticket) =>
        {
            if let Some(w) = h
                .window
                .as_mut()
                .filter(|w| w.id == window && !w.busy && w.selected == Some(ticket.origin))
            {
                w.operation = w
                    .operation
                    .checked_add(1)
                    .expect("history output identity exhausted");
                w.busy = true;
                w.error = false;
                if action == Action::Edit {
                    h.editor = Some(Editor {
                        ready: false,
                        failed: false,
                        hidden: h.cache.entry(ticket.origin).unwrap().hidden,
                        window,
                        id: w.operation,
                        origin: ticket.origin,
                        import_anchor: None,
                        kind: h.cache.entry(ticket.origin).unwrap().kind,
                        operation: 0,
                        busy: false,
                    });
                }
                if action != Action::Edit {
                    let next = h
                        .cache
                        .begin(ticket.origin)
                        .expect("history output identity exhausted");
                    let kind = h.cache.entry(ticket.origin).unwrap().kind;
                    h.pending.insert(
                        OutputKey::History {
                            window,
                            operation: w.operation,
                        },
                        Pending {
                            pin: action == Action::Pin,
                            ticket: next,
                            kind,
                            success: false,
                            prepared: None,
                        },
                    );
                }
                return vec![AppCommand::ImageHistory(Command::Output {
                    window,
                    operation: w.operation,
                    ticket,
                    action,
                    owner,
                    title,
                })];
            }
        }
        Event::Completed {
            window,
            operation,
            result,
        } => {
            if let Some(editor) = h
                .editor
                .as_mut()
                .filter(|e| e.window == window && e.id == operation && !e.ready)
            {
                editor.ready = result.is_ok();
            }
            if let Some(w) = h
                .window
                .as_mut()
                .filter(|w| w.id == window && w.operation == operation && w.busy)
            {
                w.busy = false;
                w.error = result.is_err();
            }
        }
        Event::Delete { .. } | Event::Clear
            if h.window.as_ref().is_none_or(|w| !w.busy)
                && h.editor.as_ref().is_none_or(|e| !e.busy) =>
        {
            h.apply(event)
        }
        Event::Prepared { .. } | Event::Unavailable { .. } => h.apply(event),
        _ => {}
    }
    if let Some(w) = h.window.as_mut()
        && w.selected.is_some_and(|id| h.cache.entry(id).is_none())
    {
        w.selected = h.cache.entries().first().map(|e| e.ticket.origin);
    }
    vec![]
}

/// Observe only commands accepted by their owning workflow. Invalid UI requests
/// cannot allocate origins, prepare snapshots or update a history entry.
pub(crate) fn after(
    state: &mut AppState,
    quitting: bool,
    commands: &[AppCommand],
    success: Option<OutputKey>,
) {
    if unavailable(state) {
        state.image_history.window = None;
    }
    let h = &mut state.image_history;
    if let Some(key) = success {
        if let Some(pending) = h.pending.get_mut(&key) {
            pending.success = true;
        }
        h.finish(key);
    }
    for command in commands {
        match command {
            AppCommand::Screenshot(ScreenshotCommand::Output {
                session,
                operation,
                kind: target,
                ..
            }) => {
                let (source, kind) = state
                    .screenshot
                    .long
                    .as_ref()
                    .map_or((Source::Screenshot(*session), Kind::Screenshot), |long| {
                        (Source::LongImage(*session, long.operation), Kind::LongImage)
                    });
                h.start(
                    OutputKey::Screenshot {
                        session: *session,
                        operation: *operation,
                    },
                    source,
                    kind,
                    *target == crate::domain::screenshot::ScreenshotOutput::Pin,
                );
            }
            AppCommand::PinnedImage(pin::Command::Output { id, operation, .. }) => {
                // Pins inherit their origin when their native preparation succeeds.
                // Legacy pins with no associated screenshot are intentionally excluded.
                if let Some((_, kind)) = h.sources.get(&Source::Pin(*id)).copied() {
                    h.start(
                        OutputKey::Pin {
                            id: *id,
                            operation: *operation,
                        },
                        Source::Pin(*id),
                        kind,
                        false,
                    );
                }
            }
            AppCommand::PinnedImage(pin::Command::Retain { id, source }) => {
                if let Some(pending) = h.pending.get(&OutputKey::Screenshot {
                    session: source.0,
                    operation: source.1,
                }) {
                    h.sources
                        .insert(Source::Pin(*id), (pending.ticket.origin, pending.kind));
                }
            }
            AppCommand::PinnedImage(pin::Command::RetainHistory { id, ticket }) => {
                if let Some(pending) = h.pending.values().find(|p| p.ticket == *ticket) {
                    h.sources
                        .insert(Source::Pin(*id), (ticket.origin, pending.kind));
                }
            }
            _ => {}
        }
    }
    h.pending.retain(|key, pending| {
        let keep = pending.success
            || match *key {
                OutputKey::Import { operation } => {
                    state.image_import.request.as_ref().is_some_and(|r| {
                        r.id == operation && r.phase == super::super::image_import::Phase::Preparing
                    })
                }
                OutputKey::Editor {
                    window,
                    editor,
                    operation,
                } => h.editor.as_ref().is_some_and(|e| {
                    e.window == window && e.id == editor && e.operation == operation && e.busy
                }),
                OutputKey::History { window, operation } => h
                    .window
                    .as_ref()
                    .is_some_and(|w| w.id == window && w.operation == operation && w.busy),
                OutputKey::Screenshot { session, operation } => {
                    state.screenshot.session == session
                        && state.screenshot.operation == operation
                        && state.screenshot.phase == ScreenshotPhase::Output
                }
                OutputKey::Pin { id, operation } => state
                    .pinned_images
                    .get(&id)
                    .is_some_and(|p| p.busy && p.operation == operation && p.recognition.is_none()),
            };
        if !keep {
            h.cache.cancel(pending.ticket);
        }
        keep
    });
    h.sources.retain(|source, _| match source {
        Source::Screenshot(session) | Source::LongImage(session, _) => {
            *session == state.screenshot.session
        }
        Source::Pin(id) => state.pinned_images.contains_key(id),
        Source::Import(id) => state
            .image_import
            .request
            .as_ref()
            .is_some_and(|r| r.id == *id),
    });
    if let Some(w) = h.window.as_mut()
        && w.selected
            .is_none_or(|origin| h.cache.entry(origin).is_none())
    {
        w.selected = h.cache.entries().first().map(|e| e.ticket.origin);
    }
    if quitting {
        *h = Workflow::default();
    }
}

pub(crate) fn successful_output(state: &AppState, event: &AppEvent) -> Option<OutputKey> {
    match event {
        AppEvent::ImageImport(super::super::image_import::Event::Completed {
            id,
            result: Ok(SaveOutcome::Saved),
        }) if state.image_import.request.as_ref().is_some_and(|r| {
            r.id == *id && r.target == super::super::image_import::Target::ClipboardPin
        }) =>
        {
            Some(OutputKey::Import { operation: *id })
        }
        AppEvent::ImageHistory(Event::EditorCompleted {
            window,
            editor,
            operation,
            result: Ok(SaveOutcome::Saved),
        }) if state.image_history.editor.as_ref().is_some_and(|e| {
            e.window == *window && e.id == *editor && e.operation == *operation && e.busy
        }) =>
        {
            Some(OutputKey::Editor {
                window: *window,
                editor: *editor,
                operation: *operation,
            })
        }
        AppEvent::ImageHistory(Event::Completed {
            window,
            operation,
            result: Ok(SaveOutcome::Saved),
        }) if state
            .image_history
            .window
            .as_ref()
            .is_some_and(|w| w.id == *window && w.operation == *operation && w.busy) =>
        {
            Some(OutputKey::History {
                window: *window,
                operation: *operation,
            })
        }
        AppEvent::Screenshot(ScreenshotEvent::Completed {
            session,
            operation,
            result: Ok(SaveOutcome::Saved),
        }) if state.screenshot.session == *session
            && state.screenshot.operation == *operation
            && state.screenshot.phase == ScreenshotPhase::Output =>
        {
            Some(OutputKey::Screenshot {
                session: *session,
                operation: *operation,
            })
        }
        AppEvent::PinnedImage(pin::Event::Completed {
            id,
            operation,
            result: Ok(SaveOutcome::Saved),
        }) if state
            .pinned_images
            .get(id)
            .is_some_and(|p| p.busy && p.recognition.is_none() && p.operation == *operation) =>
        {
            Some(OutputKey::Pin {
                id: *id,
                operation: *operation,
            })
        }
        _ => None,
    }
}
