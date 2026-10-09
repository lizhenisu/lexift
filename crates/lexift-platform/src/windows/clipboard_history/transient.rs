//! Buffer updates during Alt+X; discard only clipboard sequences claimed by that transaction.
use lexift_core::{
    Result,
    domain::clipboard_history::{HistoryEntry, MAX_TOTAL_BYTES},
    ports::clipboard_history::HistoryHandler,
};
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
};
#[derive(Default)]
struct State {
    active: usize,
    excluded: VecDeque<u32>,
    pending: Vec<(u32, Result<Option<HistoryEntry>>, HistoryHandler)>,
    bytes: usize,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}
pub(crate) struct Transient;
impl Transient {
    pub(crate) fn begin() -> Self {
        state().lock().unwrap_or_else(|p| p.into_inner()).active += 1;
        Self
    }
    pub(crate) fn exclude(sequence: u32) {
        let mut s = state().lock().unwrap_or_else(|p| p.into_inner());
        s.excluded.push_back(sequence);
        if s.excluded.len() > 128 {
            s.excluded.pop_front();
        }
    }
}
impl Drop for Transient {
    fn drop(&mut self) {
        let pending = {
            let mut s = state().lock().unwrap_or_else(|p| p.into_inner());
            s.active = s.active.saturating_sub(1);
            if s.active != 0 {
                return;
            }
            s.bytes = 0;
            let excluded = s.excluded.clone();
            std::mem::take(&mut s.pending)
                .into_iter()
                .filter(|(seq, _, _)| !excluded.contains(seq))
                .collect::<Vec<_>>()
        };
        for (_, entry, handler) in pending {
            handler(entry);
        }
    }
}
pub(super) fn deliver(sequence: u32, entry: Result<Option<HistoryEntry>>, handler: HistoryHandler) {
    let mut s = state().lock().unwrap_or_else(|p| p.into_inner());
    if s.excluded.contains(&sequence) {
        return;
    }
    if s.active > 0 {
        let bytes = entry
            .as_ref()
            .ok()
            .and_then(|e| e.as_ref())
            .map_or(0, |e| e.text.len());
        if s.bytes + bytes <= MAX_TOTAL_BYTES && s.pending.len() < 200 {
            s.bytes += bytes;
            s.pending.push((sequence, entry, handler));
        }
        return;
    }
    drop(s);
    handler(entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn transaction_excludes_owned_sequences_but_keeps_concurrent_user_copy() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let output = received.clone();
        let h: HistoryHandler = Arc::new(move |e| {
            output
                .lock()
                .unwrap()
                .push(e.unwrap().unwrap().text.to_string())
        });
        let transaction = Transient::begin();
        deliver(
            901,
            Ok(Some(HistoryEntry {
                id: 0,
                text: Arc::from("temporary"),
            })),
            h.clone(),
        );
        Transient::exclude(901);
        deliver(
            902,
            Ok(Some(HistoryEntry {
                id: 0,
                text: Arc::from("user-copy"),
            })),
            h.clone(),
        );
        assert!(received.lock().unwrap().is_empty());
        drop(transaction);
        deliver(
            901,
            Ok(Some(HistoryEntry {
                id: 0,
                text: Arc::from("late-temporary"),
            })),
            h,
        );
        assert_eq!(*received.lock().unwrap(), vec!["user-copy"]);
    }
}
