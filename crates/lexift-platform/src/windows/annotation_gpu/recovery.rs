//! A device rebuild budget belongs to the session, never to an input revision.
#[derive(Default)]
pub(super) struct Recovery {
    scope: Option<(u64, u64)>,
    attempted: bool,
}

impl Recovery {
    pub fn enter(&mut self, generation: u64, session: u64) {
        if self.scope != Some((generation, session)) {
            self.scope = Some((generation, session));
            self.attempted = false;
        }
    }

    /// A bad window/source is not a lost device. Rebuild at most once, including
    /// a failed initial device creation; ordinary successful frames do not reset it.
    pub fn request(&mut self, recoverable: bool) -> bool {
        if !recoverable || self.attempted {
            return false;
        }
        self.attempted = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebuild_is_bounded_per_session_and_survives_input_revisions() {
        let mut recovery = Recovery::default();
        recovery.enter(1, 7);
        assert!(!recovery.request(false));
        assert!(recovery.request(true));
        recovery.enter(1, 7);
        assert!(!recovery.request(true));
        recovery.enter(1, 8);
        assert!(recovery.request(true));
        recovery.enter(2, 8);
        assert!(recovery.request(true));
        assert!(!recovery.request(true));
    }
}
