use crate::{
    Result,
    domain::selection::{Selection, SelectionTarget},
};

pub trait SelectionPort: Send + Sync {
    /// Identifies the current input target without activation or input injection.
    fn passive_target(&self) -> Option<SelectionTarget> {
        None
    }

    /// Read only while the original foreground/focus identity remains current.
    fn selected_text_passive_for(&self, target: SelectionTarget) -> Result<Option<Selection>> {
        if self.passive_target() != Some(target) {
            return Ok(None);
        }
        let selection = self.selected_text_passive()?;
        Ok(selection.filter(|_| self.passive_target() == Some(target)))
    }

    fn selected_text(&self) -> Result<Option<Selection>>;

    /// Reads a selection without generating input or changing the clipboard.
    fn selected_text_passive(&self) -> Result<Option<Selection>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::geometry::Point, ports::magnifier::WindowToken};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    fn target() -> SelectionTarget {
        SelectionTarget {
            window: WindowToken(1),
            focus: Some(WindowToken(2)),
            process: 3,
            thread: 4,
        }
    }
    struct Port {
        current: AtomicBool,
        change_during_read: bool,
        reads: AtomicUsize,
    }
    impl SelectionPort for Port {
        fn selected_text(&self) -> Result<Option<Selection>> {
            panic!("passive reads must not use clipboard fallback")
        }
        fn passive_target(&self) -> Option<SelectionTarget> {
            self.current.load(Ordering::Relaxed).then(target)
        }
        fn selected_text_passive(&self) -> Result<Option<Selection>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.change_during_read {
                self.current.store(false, Ordering::Relaxed);
            }
            Ok(Some(Selection {
                text: "fixture".into(),
                anchor: Some(Point { x: -12, y: 7 }),
            }))
        }
    }
    #[test]
    fn passive_capture_rejects_changed_target_before_and_after_read() {
        for (current, change, reads, success) in [
            (false, false, 0, false),
            (true, true, 1, false),
            (true, false, 1, true),
        ] {
            let port = Port {
                current: AtomicBool::new(current),
                change_during_read: change,
                reads: AtomicUsize::new(0),
            };
            assert_eq!(
                port.selected_text_passive_for(target()).unwrap().is_some(),
                success
            );
            assert_eq!(port.reads.load(Ordering::Relaxed), reads);
        }
    }
    #[test]
    fn passive_capture_rejects_each_identity_component() {
        let port = Port {
            current: AtomicBool::new(true),
            change_during_read: false,
            reads: AtomicUsize::new(0),
        };
        for changed in [
            SelectionTarget {
                window: WindowToken(9),
                ..target()
            },
            SelectionTarget {
                focus: None,
                ..target()
            },
            SelectionTarget {
                process: 9,
                ..target()
            },
            SelectionTarget {
                thread: 9,
                ..target()
            },
        ] {
            assert!(port.selected_text_passive_for(changed).unwrap().is_none());
        }
        assert_eq!(port.reads.load(Ordering::Relaxed), 0);
    }
}
