//! Owns parameter gesture state and its one cancellable idle completion task.
use lexift_core::domain::annotation::{ObjectId, Session};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub panel: u64,
    pub tool: i32,
    pub object: Option<ObjectId>,
    pub editor: Option<u64>,
    pub watermark: u64,
    pub projection: u64,
}

#[derive(Clone, Copy)]
pub(super) struct Token(u64);

#[derive(Default)]
pub(super) struct Transaction {
    target: Option<Target>,
    active: bool,
    dragging: bool,
    revision: u64,
    idle: slint::Timer,
}

impl Transaction {
    #[cfg(test)]
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn dragging(&self) -> bool {
        self.dragging
    }
    pub fn set_dragging(&mut self, dragging: bool) {
        self.dragging = dragging;
    }

    /// A target change ends the prior operation before starting another Core
    /// snapshot. Repeated updates of one target share a single undo operation.
    pub fn touch(&mut self, session: &mut Session, target: Target) -> Token {
        if self.active && self.target != Some(target) {
            self.finish(session);
        }
        if !self.active {
            session.begin_drag();
            self.active = true;
            self.target = Some(target);
        }
        self.idle.stop();
        self.revision = super::window::next_revision();
        Token(self.revision)
    }
    pub fn current(&self, token: Token) -> bool {
        self.active && self.revision == token.0
    }
    pub fn ready_to_finish(&self, token: Token, menu_dragging: bool) -> bool {
        self.current(token) && !self.dragging && !menu_dragging
    }
    pub fn finish_after(&self, token: Token, finish: impl FnMut() + 'static) {
        if self.current(token) {
            self.idle.start(
                slint::TimerMode::SingleShot,
                Duration::from_millis(300),
                finish,
            );
        }
    }
    /// Returns whether a Core transaction was closed; the caller updates UI undo
    /// availability. Unchanged snapshots still produce no Core undo record.
    pub fn finish(&mut self, session: &mut Session) -> bool {
        self.idle.stop();
        if !self.active {
            return false;
        }
        session.finish_drag();
        self.active = false;
        self.target = None;
        self.revision = super::window::next_revision();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    fn target(editor: u64) -> Target {
        Target {
            panel: 1,
            tool: 8,
            object: None,
            editor: Some(editor),
            watermark: 0,
            projection: 0,
        }
    }

    #[test]
    fn continuous_updates_coalesce_and_target_changes_split_core_undo() {
        let mut session = Session::new();
        let mut edit = Transaction::default();
        let old = edit.touch(&mut session, target(1));
        session.set_spotlight_opacity(0.3);
        let latest = edit.touch(&mut session, target(1));
        session.set_spotlight_opacity(0.5);
        assert!(!edit.current(old));
        edit.set_dragging(true);
        assert!(!edit.ready_to_finish(latest, false));
        edit.set_dragging(false);
        assert!(!edit.ready_to_finish(latest, true));
        assert!(edit.ready_to_finish(latest, false));
        let next = edit.touch(&mut session, target(2));
        assert!(!edit.current(latest));
        assert!(edit.current(next));
        session.set_spotlight_opacity(0.7);
        assert!(edit.finish(&mut session));
        assert!(!edit.finish(&mut session));
        assert!(session.undo());
        assert_eq!(session.spotlight_opacity, 0.5);
        assert!(session.undo());
        assert_eq!(session.spotlight_opacity, 0.1);
        assert!(
            !session.undo(),
            "updates inside one target must not add extra history"
        );
        let unchanged = edit.touch(&mut session, target(2));
        assert!(edit.finish(&mut session));
        assert!(!edit.current(unchanged));
        assert!(
            !session.can_undo(),
            "unchanged parameter operation must remain history-free"
        );
    }

    #[test]
    fn idle_completion_is_latest_only_and_retired_owners_cannot_finish_replacements() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let mut session = Session::new();
        let mut edit = Transaction::default();
        let retired = edit.touch(&mut session, target(1));
        edit.finish_after(retired, || panic!("superseded idle task ran"));
        let current = edit.touch(&mut session, target(1));
        let completed = Rc::new(Cell::new(0));
        let calls = completed.clone();
        edit.finish_after(current, move || calls.set(calls.get() + 1));
        clock.set(Duration::from_millis(299));
        slint::platform::update_timers_and_animations();
        assert_eq!(completed.get(), 0);
        clock.set(Duration::from_millis(301));
        slint::platform::update_timers_and_animations();
        assert_eq!(completed.get(), 1);
        edit.finish(&mut session);
        edit.finish_after(current, || panic!("completed transaction rearmed"));
        let retired = edit.touch(&mut session, target(1));
        edit.finish_after(retired, || panic!("destroyed parameter owner ran"));
        drop(edit);
        session.cancel_drag();
        let mut replacement = Transaction::default();
        let current = replacement.touch(&mut session, target(1));
        assert!(!replacement.current(retired));
        assert!(replacement.ready_to_finish(current, false));
        clock.set(Duration::from_millis(1000));
        slint::platform::update_timers_and_animations();
        assert_eq!(completed.get(), 1);
        replacement.finish(&mut session);
    }
}
