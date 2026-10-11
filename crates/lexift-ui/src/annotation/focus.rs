//! Cancellable, latest-only focus restoration after an annotation control interaction.

use std::{cell::Cell, rc::Rc};

/// The returned input belongs to an editor instance, not whichever editor is currently open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Text { editor: u64, panel: u64 },
    Watermark { editor: u64 },
}

/// Internal Slint focus can be restored without raising a native window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Activation {
    Local,
    UserRequested,
}

/// Captured at the user callback, before queued UI work can observe a different foreground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Origin {
    Unobserved,
    Foreground(Option<lexift_core::ports::magnifier::WindowToken>),
}

impl Origin {
    /// Popup retirement may return focus to this annotation's windows, never any same-process window.
    pub fn permits_activation(
        self,
        current: Option<lexift_core::ports::magnifier::WindowToken>,
        owners: &[lexift_core::ports::magnifier::WindowToken],
    ) -> bool {
        match self {
            Self::Unobserved => true,
            Self::Foreground(original) => current
                .is_some_and(|current| Some(current) == original || owners.contains(&current)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub session: u64,
    pub target: Target,
    pub menu: u64,
    pub activation: Activation,
}

/// A borrowed live-state summary is sufficient to decide whether restoration remains valid.
pub(super) struct Context {
    pub session: u64,
    pub panel: u64,
    pub menu: u64,
    pub text_editor: Option<u64>,
    pub watermark_editor: Option<u64>,
    pub popup_open: bool,
    pub controls_busy: bool,
}

impl Request {
    pub fn matches(self, live: &Context) -> bool {
        self.session == live.session
            && self.menu == live.menu
            && !live.popup_open
            && !live.controls_busy
            && match self.target {
                Target::Text { editor, panel } => {
                    // A newer watermark input owns the keyboard while this draft survives.
                    live.text_editor == Some(editor)
                        && live.panel == panel
                        && live.watermark_editor.is_none()
                }
                // The watermark popup belongs to its input window, independently of the toolbar.
                Target::Watermark { editor } => live.watermark_editor == Some(editor),
            }
    }
}

#[derive(Default)]
pub(super) struct Coordinator {
    timer: slint::Timer,
    pending: Rc<Cell<Option<Request>>>,
}

impl Coordinator {
    /// The caller validates the request against live state before restoring the input.
    pub fn restore(&self, request: Request, apply: impl FnOnce(Request) + 'static) {
        self.pending.set(Some(request));
        let pending = self.pending.clone();
        let mut apply = Some(apply);
        self.timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(16),
            move || {
                if pending.take() == Some(request)
                    && let Some(apply) = apply.take()
                {
                    apply(request);
                }
            },
        );
    }

    pub fn cancel(&self) {
        self.timer.stop();
        self.pending.set(None);
    }

    /// Retiring one input must not cancel a restoration destined for another input.
    pub fn cancel_for(&self, target: Target) {
        if self
            .pending
            .get()
            .is_some_and(|request| request.target == target)
        {
            self.cancel();
        }
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

    #[test]
    fn origin_allows_owner_return_but_rejects_unrelated_foreground() {
        use lexift_core::ports::magnifier::WindowToken as Token;
        let origin = Origin::Foreground(Some(Token(10)));
        assert!(origin.permits_activation(Some(Token(10)), &[Token(20)]));
        assert!(origin.permits_activation(Some(Token(20)), &[Token(20)]));
        assert!(!origin.permits_activation(Some(Token(30)), &[Token(20)]));
        assert!(!origin.permits_activation(None, &[Token(20)]));
        assert!(!Origin::Foreground(None).permits_activation(Some(Token(30)), &[Token(20)]));
        assert!(Origin::Foreground(None).permits_activation(Some(Token(20)), &[Token(20)]));
        assert!(Origin::Unobserved.permits_activation(None, &[]));
    }

    #[test]
    fn restore_keeps_only_the_latest_request_and_teardown_cancels_it() {
        use slint::platform::{Platform, WindowAdapter};
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                unreachable!("focus scheduling does not create windows")
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(20));
            slint::platform::update_timers_and_animations();
        };
        let coordinator = Coordinator::default();
        let applied = Rc::new(RefCell::new(Vec::new()));
        let request = Request {
            session: 1,
            target: Target::Text {
                editor: 2,
                panel: 3,
            },
            menu: 4,
            activation: Activation::UserRequested,
        };
        for editor in [2, 5] {
            let applied = applied.clone();
            coordinator.restore(
                Request {
                    target: Target::Text { editor, panel: 3 },
                    ..request
                },
                move |r| applied.borrow_mut().push(r),
            );
        }
        advance();
        assert_eq!(
            *applied.borrow(),
            vec![Request {
                target: Target::Text {
                    editor: 5,
                    panel: 3
                },
                ..request
            }]
        );
        let result = applied.clone();
        coordinator.restore(request, move |r| result.borrow_mut().push(r));
        coordinator.cancel();
        advance();
        assert_eq!(applied.borrow().len(), 1);
        let watermark = Request {
            target: Target::Watermark { editor: 7 },
            ..request
        };
        let result = applied.clone();
        coordinator.restore(watermark, move |r| result.borrow_mut().push(r));
        coordinator.cancel_for(request.target);
        advance();
        assert_eq!(applied.borrow().last(), Some(&watermark));
        let result = applied.clone();
        coordinator.restore(watermark, move |r| result.borrow_mut().push(r));
        coordinator.cancel_for(watermark.target);
        advance();
        assert_eq!(applied.borrow().len(), 2);
        let result = applied.clone();
        coordinator.restore(request, move |r| result.borrow_mut().push(r));
        drop(coordinator);
        advance();
        assert_eq!(applied.borrow().len(), 2);
    }
}
