//! Process-local image history. Adapters own immutable resources; Core accounts for
//! their identities and accepts only the output versions it previously issued.
use super::geometry::Rect;
use std::collections::{BTreeMap, BTreeSet};
pub mod thumbnail;
pub mod workflow;

pub const MAX_ENTRIES: usize = 20;
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Immutable editable content transferred to an image host; no live view state.
#[derive(Clone)]
pub struct ImageSnapshot {
    pub base: std::sync::Arc<crate::ports::screen_capture::CapturedRegion>,
    pub samples: std::sync::Arc<crate::ports::screen_capture::ScreenCaptureFrame>,
    pub document: super::annotation_export::ExportRequest,
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(pub u64);

/// Charge each shared allocation once, including thumbnails and document storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub id: ResourceId,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Imported,
    Screenshot,
    LongImage,
}

/// Issued before an output starts. A newer success supersedes older successes;
/// merely starting a new output or failing it does not discard the old entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket {
    pub origin: Origin,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub ticket: Ticket,
    pub kind: Kind,
    pub hidden: bool,
    pub bounds: Rect,
    pub updated_ms: u64,
    pub resources: Vec<Resource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Accepted { removed: Vec<Ticket> },
    TooLarge,
    Invalid,
    Stale,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    revision: u64,
    entries: Vec<Entry>,
    next_origin: u64,
    next_version: u64,
    pending: BTreeSet<(Origin, u64)>,
    accepted: BTreeMap<Origin, u64>,
}

impl State {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn origin(&mut self) -> Option<Origin> {
        self.next_origin = self.next_origin.checked_add(1)?;
        Some(Origin(self.next_origin))
    }

    pub fn begin(&mut self, origin: Origin) -> Option<Ticket> {
        if origin.0 == 0 || origin.0 > self.next_origin {
            return None;
        }
        self.next_version = self.next_version.checked_add(1)?;
        let ticket = Ticket {
            origin,
            version: self.next_version,
        };
        self.pending.insert((origin, ticket.version));
        Some(ticket)
    }

    pub fn cancel(&mut self, ticket: Ticket) {
        self.pending.remove(&(ticket.origin, ticket.version));
    }

    pub fn entry(&self, origin: Origin) -> Option<&Entry> {
        self.entries.iter().find(|e| e.ticket.origin == origin)
    }

    pub fn retained_bytes(&self) -> u64 {
        resource_bytes(&self.entries).expect("admitted resource charges are consistent")
    }

    /// Admit only a successful output with ready immutable assets and thumbnail.
    /// Rejected replacements leave the previous entry and its ordering intact.
    pub fn complete(&mut self, entry: Entry) -> Admission {
        let ticket = entry.ticket;
        if !self.pending.remove(&(ticket.origin, ticket.version))
            || self
                .accepted
                .get(&ticket.origin)
                .is_some_and(|v| *v >= ticket.version)
        {
            return Admission::Stale;
        }
        if entry.bounds.right <= entry.bounds.left
            || entry.bounds.bottom <= entry.bounds.top
            || entry.resources.is_empty()
        {
            return Admission::Invalid;
        }
        let Some(bytes) = resource_bytes(std::slice::from_ref(&entry)) else {
            return Admission::Invalid;
        };
        if bytes > MAX_BYTES {
            return Admission::TooLarge;
        }
        let mut next = vec![entry];
        next.extend(
            self.entries
                .iter()
                .filter(|e| e.ticket.origin != ticket.origin)
                .cloned(),
        );
        if resource_bytes(&next).is_none() {
            return Admission::Invalid;
        }
        let mut removed: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.ticket.origin == ticket.origin)
            .map(|e| e.ticket)
            .collect();
        while next.len() > MAX_ENTRIES || resource_bytes(&next).unwrap() > MAX_BYTES {
            removed.push(next.pop().unwrap().ticket);
        }
        // Eviction also invalidates outstanding work for the evicted origins.
        for old in &removed {
            if old.origin != ticket.origin {
                self.pending.retain(|(origin, _)| *origin != old.origin);
            }
        }
        self.entries = next;
        self.revision = self
            .revision
            .checked_add(1)
            .expect("history revision exhausted");
        self.accepted.insert(ticket.origin, ticket.version);
        self.accepted.retain(|origin, _| {
            self.entries.iter().any(|e| e.ticket.origin == *origin)
                || self.pending.iter().any(|(id, _)| id == origin)
        });
        Admission::Accepted { removed }
    }

    pub fn delete(&mut self, origin: Origin) -> Option<Ticket> {
        self.pending.retain(|(id, _)| *id != origin);
        self.accepted.remove(&origin);
        let index = self
            .entries
            .iter()
            .position(|e| e.ticket.origin == origin)?;
        self.revision = self
            .revision
            .checked_add(1)
            .expect("history revision exhausted");
        Some(self.entries.remove(index).ticket)
    }

    pub fn clear(&mut self) -> Vec<Ticket> {
        if !self.entries.is_empty() {
            self.revision = self
                .revision
                .checked_add(1)
                .expect("history revision exhausted");
        }
        self.pending.clear();
        self.accepted.clear();
        self.entries.drain(..).map(|e| e.ticket).collect()
    }
}

fn resource_bytes(entries: &[Entry]) -> Option<u64> {
    let mut resources = BTreeMap::new();
    for resource in entries.iter().flat_map(|e| &e.resources) {
        if let Some(old) = resources.insert(resource.id, resource.bytes)
            && old != resource.bytes
        {
            return None;
        }
    }
    resources
        .values()
        .try_fold(0u64, |sum, bytes| sum.checked_add(*bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(ticket: Ticket, bytes: u64) -> Entry {
        Entry {
            ticket,
            kind: Kind::Screenshot,
            hidden: false,
            bounds: Rect {
                left: -40,
                top: -30,
                right: 60,
                bottom: 70,
            },
            updated_ms: ticket.version,
            resources: vec![Resource {
                id: ResourceId(ticket.version),
                bytes,
            }],
        }
    }
    fn accept(s: &mut State, bytes: u64) -> Ticket {
        let origin = s.origin().unwrap();
        let ticket = s.begin(origin).unwrap();
        assert!(matches!(
            s.complete(entry(ticket, bytes)),
            Admission::Accepted { .. }
        ));
        ticket
    }
    #[test]
    fn same_origin_success_moves_front_and_old_completion_cannot_replace_it() {
        let mut s = State::default();
        let a = accept(&mut s, 30);
        let b = accept(&mut s, 40);
        let old = s.begin(a.origin).unwrap();
        let new = s.begin(a.origin).unwrap();
        assert!(matches!(
            s.complete(entry(new, 50)),
            Admission::Accepted { .. }
        ));
        assert_eq!(s.complete(entry(old, 60)), Admission::Stale);
        assert_eq!(
            s.entries.iter().map(|e| e.ticket).collect::<Vec<_>>(),
            [new, b]
        );
        assert_eq!(s.retained_bytes(), 90);
    }
    #[test]
    fn cancellation_and_oversized_replacement_preserve_old_record() {
        let mut s = State::default();
        let a = accept(&mut s, 30);
        let cancelled = s.begin(a.origin).unwrap();
        s.cancel(cancelled);
        assert_eq!(s.complete(entry(cancelled, 40)), Admission::Stale);
        let huge = s.begin(a.origin).unwrap();
        assert_eq!(s.complete(entry(huge, MAX_BYTES + 1)), Admission::TooLarge);
        assert_eq!(s.entries[0].ticket, a);
    }
    #[test]
    fn count_and_unique_allocation_budget_evict_oldest() {
        let mut s = State::default();
        let old = accept(&mut s, 1);
        for _ in 0..MAX_ENTRIES {
            accept(&mut s, 1);
        }
        assert_eq!(s.entries.len(), MAX_ENTRIES);
        assert!(s.entry(old.origin).is_none());
        s.clear();
        let a = accept(&mut s, MAX_BYTES / 2);
        let b_origin = s.origin().unwrap();
        let b = s.begin(b_origin).unwrap();
        let mut shared = entry(b, MAX_BYTES / 2);
        shared.resources = s.entry(a.origin).unwrap().resources.clone();
        assert!(matches!(s.complete(shared), Admission::Accepted { .. }));
        assert_eq!(s.retained_bytes(), MAX_BYTES / 2);
        accept(&mut s, MAX_BYTES / 2 + 1);
        assert!(s.entry(a.origin).is_none());
        assert!(s.entry(b.origin).is_none());
        assert!(s.retained_bytes() <= MAX_BYTES);
    }
    #[test]
    fn delete_and_clear_invalidate_late_results_but_allow_fresh_outputs() {
        let mut s = State::default();
        let a = accept(&mut s, 1);
        let late = s.begin(a.origin).unwrap();
        assert_eq!(s.delete(a.origin), Some(a));
        assert_eq!(s.complete(entry(late, 2)), Admission::Stale);
        let fresh = s.begin(a.origin).unwrap();
        assert!(matches!(
            s.complete(entry(fresh, 2)),
            Admission::Accepted { .. }
        ));
        let late = s.begin(a.origin).unwrap();
        s.clear();
        assert_eq!(s.complete(entry(late, 3)), Admission::Stale);
    }
    #[test]
    fn malformed_or_inconsistent_charges_do_not_change_retained_entries() {
        let mut s = State::default();
        let a = accept(&mut s, 10);
        let origin = s.origin().unwrap();
        let ticket = s.begin(origin).unwrap();
        let mut malformed = entry(ticket, 11);
        malformed.resources[0].id = ResourceId(a.version);
        assert_eq!(s.complete(malformed), Admission::Invalid);
        assert_eq!(s.retained_bytes(), 10);
    }
}
