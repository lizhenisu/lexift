//! Apply recording capture exclusion to every annotation popup before display.
//! Keep this scope separate from the view borrow: preparation can run while its
//! caller already holds both the annotation registry and recording view.
use super::*;

type Prepare = fn(&slint::Window) -> PassiveWindowPreparation;
type Policy = Rc<dyn Fn(&slint::Window, crate::RecordingWindowRole) -> bool>;
#[derive(Clone)]
struct Context {
    generation: u64,
    native: Prepare,
    policy: Policy,
}
thread_local! { static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) }; }

pub(super) fn install(r: &mut Registry) -> lexift_core::Result<()> {
    let policy = r
        .lifecycle
        .recording_window
        .clone()
        .ok_or_else(|| lexift_core::Error::new("Recording window policy unavailable"))?;
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return Err(lexift_core::Error::new(
                "Previous recording chrome has not retired",
            ));
        }
        *slot = Some(Context {
            generation: r.generation,
            native: r.prepare,
            policy,
        });
        r.prepare = prepare;
        Ok(())
    })
}

fn prepare(window: &slint::Window) -> PassiveWindowPreparation {
    let Some(context) = CONTEXT.with(|slot| slot.borrow().clone()) else {
        // A delayed popup from a retired scope must never become visible.
        return PassiveWindowPreparation::Failed;
    };
    match (context.native)(window) {
        PassiveWindowPreparation::Ready => {
            if (context.policy)(window, crate::RecordingWindowRole::Control) {
                PassiveWindowPreparation::Ready
            } else {
                PassiveWindowPreparation::Failed
            }
        }
        pending => pending,
    }
}

pub(super) fn clear() {
    let context = CONTEXT.with(|slot| slot.borrow_mut().take());
    if let Some(context) = context {
        REGISTRY.with(|slot| {
            if let Some(r) = slot
                .borrow_mut()
                .as_mut()
                .filter(|r| r.generation == context.generation)
            {
                r.prepare = context.native;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct Backend;
    impl Platform for Backend {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    #[test]
    fn popup_exclusion_precedes_display_and_retired_preparations_fail_closed() {
        slint::platform::set_platform(Box::new(Backend)).unwrap();
        super::super::init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| false,
                |_, _| true,
                || true,
            ),
        );
        let calls = Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.lifecycle.recording_window = Some(Rc::new(move |window, role| {
                assert!(!window.is_visible());
                assert_eq!(role, crate::RecordingWindowRole::Control);
                observed.set(observed.get() + 1);
                true
            }));
            install(r).unwrap();
        });
        let panel = crate::AnnotationPanel::new().unwrap();
        REGISTRY.with(|slot| {
            assert_eq!(
                (slot.borrow().as_ref().unwrap().prepare)(panel.window()),
                PassiveWindowPreparation::Ready
            );
        });
        assert_eq!(calls.get(), 1);
        assert!(!panel.window().is_visible());
        clear();
        assert_eq!(prepare(panel.window()), PassiveWindowPreparation::Failed);
        REGISTRY.with(|slot| {
            assert_eq!(
                (slot.borrow().as_ref().unwrap().prepare)(panel.window()),
                PassiveWindowPreparation::Ready
            );
        });
        assert_eq!(calls.get(), 1);
        super::super::shutdown();
    }
}
