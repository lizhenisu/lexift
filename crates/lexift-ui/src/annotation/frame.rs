//! Retains the last successful canvas frame for exact unchanged-frame suppression.
use tiny_skia::Pixmap;

#[derive(Default)]
pub(super) struct PresentedFrame {
    last: Option<Pixmap>,
}

impl PresentedFrame {
    /// A failed native presentation is never cached, so the same pixels can retry.
    pub fn submit(&mut self, frame: Pixmap, present: impl FnOnce(&Pixmap) -> bool) -> bool {
        if self.last.as_ref().is_some_and(|last| {
            last.width() == frame.width()
                && last.height() == frame.height()
                && last.data() == frame.data()
        }) {
            return true;
        }
        if !present(&frame) {
            return false;
        }
        self.last = Some(frame);
        true
    }
}

/// Session-owned scheduling for canvas work and cold lens preparation.
/// Callbacks carry identities only; timers never retain windows or their owner.
#[derive(Default)]
pub(super) struct Tasks {
    render: slint::Timer,
    lens: slint::Timer,
}

impl Tasks {
    pub fn queue_render(&self, generation: u64, callback: fn(u64)) {
        if !self.render.running() {
            self.render.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(16),
                move || callback(generation),
            );
        }
    }

    pub fn defer_lens(&self, generation: u64, revision: u64, callback: fn(u64, u64)) {
        self.lens.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(16),
            move || callback(generation, revision),
        );
    }

    #[cfg(test)]
    pub fn render_pending(&self) -> bool {
        self.render.running()
    }

    #[cfg(test)]
    pub fn lens_pending(&self) -> bool {
        self.lens.running()
    }

    pub fn cancel_render(&self) {
        self.render.stop();
    }
    pub fn cancel_lens(&self) {
        self.lens.stop();
    }
    pub fn clear(&self) {
        self.cancel_render();
        self.cancel_lens();
    }
}

impl Drop for Tasks {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        time::Duration,
    };
    thread_local! { static CALLS: RefCell<Vec<(u64, Option<u64>)>> = const { RefCell::new(Vec::new()) }; }
    fn render(generation: u64) {
        CALLS.with(|calls| calls.borrow_mut().push((generation, None)));
    }
    fn lens(generation: u64, revision: u64) {
        CALLS.with(|calls| calls.borrow_mut().push((generation, Some(revision))));
    }

    #[test]
    fn owned_tasks_coalesce_replace_and_cancel_before_owner_retirement() {
        struct Clock(Rc<Cell<Duration>>);
        impl slint::platform::Platform for Clock {
            fn create_window_adapter(
                &self,
            ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
                unreachable!("scheduling does not create windows")
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
        };
        let tasks = Tasks::default();
        tasks.queue_render(1, render);
        tasks.queue_render(1, render);
        tasks.defer_lens(1, 2, lens);
        tasks.defer_lens(1, 3, lens);
        advance();
        assert!(!tasks.render_pending());
        CALLS.with(|calls| {
            let mut observed = calls.borrow().clone();
            // Independent timers do not promise execution order at the same deadline.
            observed.sort();
            assert_eq!(observed, vec![(1, None), (1, Some(3))]);
        });
        tasks.queue_render(1, render);
        tasks.defer_lens(1, 4, lens);
        tasks.clear();
        advance();
        CALLS.with(|calls| assert_eq!(calls.borrow().len(), 2));
        tasks.queue_render(2, render);
        tasks.defer_lens(2, 1, lens);
        drop(tasks);
        advance();
        CALLS.with(|calls| assert_eq!(calls.borrow().len(), 2));
        let reopened = Tasks::default();
        reopened.queue_render(3, render);
        reopened.defer_lens(3, 1, lens);
        reopened.cancel_lens();
        advance();
        CALLS.with(|calls| assert_eq!(calls.borrow().last(), Some(&(3, None))));
    }
}
