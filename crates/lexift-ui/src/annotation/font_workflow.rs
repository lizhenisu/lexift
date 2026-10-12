//! UI-thread subscriptions to the process font service; never retains a component.
use super::*;

pub(super) fn ensure(r: &mut Registry, retry: bool) -> bool {
    if r.fonts.is_loaded() {
        return true;
    }
    let status = font_service::request(retry);
    if r.fonts.ensure() {
        r.reset_font_defaults();
    }
    if !r.fonts.is_loaded() {
        let panel = r
            .toolbar_state
            .panel
            .get()
            .filter(|p| !p.get_menu() && matches!(p.get_tool(), 8..=10))
            .map(|_| r.toolbar_state.panel.revision());
        r.fonts.subscription = Some((status.request(), r.generation, panel));
    }
    r.fonts.is_loaded()
}

pub(super) fn sync_panel(r: &mut Registry) {
    let Some(panel) = r
        .toolbar_state
        .panel
        .get()
        .filter(|p| !p.get_menu() && matches!(p.get_tool(), 8..=10))
    else {
        return;
    };
    let status = if r.fonts.is_loaded() {
        1
    } else if matches!(
        font_service::service().status(),
        font_service::Status::Failed(_)
    ) {
        2
    } else {
        0
    };
    panel.set_font_status(status);
    if status != 1 {
        return;
    }
    panel.set_font_families(ModelRc::new(VecModel::from(
        r.fonts
            .names()
            .iter()
            .cloned()
            .map(Into::into)
            .collect::<Vec<_>>(),
    )));
    let tool = panel.get_tool() as usize;
    let object = r.parameter_object(tool);
    let name = object
        .and_then(|id| r.session.object_index(id))
        .and_then(|i| match &r.session.objects[i].extra {
            Extra::Text(text) => Some(text.font_family.clone()),
            Extra::Sequence(s) => Some(s.caption.font_family.clone()),
            _ => None,
        })
        .or_else(|| {
            (tool == 9)
                .then(|| r.session.watermark.as_ref().map(|w| w.font_family.clone()))
                .flatten()
        });
    let index = name
        .and_then(|name| r.fonts.names().iter().position(|f| f == &name))
        .map(|i| i as i32)
        .unwrap_or_else(|| r.values(tool).font);
    r.parameter_values.edit(tool, object).font = index;
    let panel = r.toolbar_state.panel.get().unwrap().clone_strong();
    let mut values = panel.get_values();
    values.font = index;
    panel.set_values(values);
}

pub(super) fn completed(request: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some((expected, generation, panel)) = r.fonts.subscription else {
            return;
        };
        if request != expected
            || generation != r.generation
            || panel.is_some_and(|revision| {
                revision != r.toolbar_state.panel.revision()
                    || r.toolbar_state.panel.get().is_none()
            })
        {
            return;
        }
        if font_service::service().status().request() != request {
            return;
        }
        r.fonts.subscription = None;
        if r.fonts.ensure() {
            r.reset_font_defaults();
        }
        sync_panel(r);
    });
}

pub(super) fn retry(generation: u64, revision: u64) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.toolbar_state.panel.revision() == revision
                && r.toolbar_state.panel.get().is_some()
        }) {
            ensure(r, true);
            sync_panel(r);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::{
        Model,
        platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        },
    };
    use std::sync::{Arc, Mutex, mpsc};

    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    fn render_states(panel: &AnnotationPanel, state: &str) {
        let Ok(directory) = std::env::var("LEXIFT_ANNOTATION_SNAPSHOT_DIR") else {
            return;
        };
        for language in lexift_core::domain::ui_language::UiLanguage::ALL {
            crate::i18n::select(language);
            for scale in [1., 1.25] {
                panel
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                        scale_factor: scale,
                    });
                panel.window().set_size(toolbar::physical_size(
                    panel.get_numeric_content_width(),
                    panel.get_numeric_content_height(),
                    scale,
                ));
                for dark in [false, true] {
                    panel
                        .global::<crate::Colors>()
                        .set_preference(i32::from(dark));
                    let pixels = panel.window().take_snapshot().unwrap();
                    crate::i18n::write_test_bitmap(
                        &directory,
                        &format!("font-{state}-{}-{dark}-{scale}", language.code()),
                        pixels.width(),
                        pixels.height(),
                        pixels.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                    );
                }
            }
        }
        crate::i18n::select(lexift_core::domain::ui_language::UiLanguage::EnglishUs);
    }
    #[test]
    fn failure_retry_updates_only_fonts_on_the_current_panel() {
        let fixture = font_service::snapshot().unwrap();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let service = font_service::Service::new(Arc::new(move || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err("unavailable");
            }
            wait.lock().unwrap().recv().unwrap();
            Ok((*fixture).clone())
        }));
        font_service::TEST_SERVICE.with(|s| *s.borrow_mut() = Some(service.clone()));
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|s| {
            s.borrow_mut()
                .as_mut()
                .unwrap()
                .toolbar_state
                .main
                .open(AnnotationToolbar::new().unwrap())
        });
        open_panel(3, false);
        let deadline = Instant::now() + Duration::from_secs(3);
        while matches!(service.status(), font_service::Status::Loading(_))
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        completed(service.status().request());
        let (generation, revision) = REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            let panel = r.toolbar_state.panel.get().unwrap().clone_strong();
            assert_eq!(panel.get_font_status(), 2);
            render_states(&panel, "failed");
            (r.generation, r.toolbar_state.panel.revision())
        });
        retry(generation, revision);
        let request = service.status().request();
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            let panel = r.toolbar_state.panel.get().unwrap().clone_strong();
            assert_eq!(panel.get_font_status(), 0);
            let mut values = panel.get_values();
            values.strength = 43.;
            values.size = 29;
            panel.set_values(values.clone());
            r.replace_values(9, values);
            render_states(&panel, "loading");
        });
        crate::i18n::select(lexift_core::domain::ui_language::UiLanguage::Japanese);
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while matches!(service.status(), font_service::Status::Loading(_))
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        completed(request);
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            let panel = r.toolbar_state.panel.get().unwrap().clone_strong();
            assert_eq!(panel.get_font_status(), 1);
            assert_eq!(panel.get_values().strength, 43.);
            assert_eq!(panel.get_values().size, 29);
            assert_eq!(chosen_font(r, 9), r.preferred_font(9));
            render_states(&panel, "ready");
        });
        shutdown();
        font_service::TEST_SERVICE.with(|s| *s.borrow_mut() = None);
    }
    #[test]
    fn pending_fonts_keep_panel_responsive_drop_creation_and_reject_retired_subscriptions() {
        let fixture = font_service::snapshot().unwrap();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let service = font_service::Service::new(Arc::new(move || {
            wait.lock().unwrap().recv().unwrap();
            Ok((*fixture).clone())
        }));
        font_service::TEST_SERVICE.with(|s| *s.borrow_mut() = Some(service.clone()));
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .toolbar_state
                .main
                .open(AnnotationToolbar::new().unwrap())
        });
        open_panel(3, false);
        let (generation, old, request) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let panel = r.toolbar_state.panel.get().unwrap().clone_strong();
            assert_eq!(panel.get_font_status(), 0);
            assert_eq!(panel.get_font_families().row_count(), 0);
            let mut values = panel.get_values();
            values.strength = 37.;
            panel.set_values(values.clone());
            r.replace_values(9, values);
            begin_sequence_placement(r, (50., 50.), 1.);
            assert!(r.session.objects.is_empty());
            assert!(!r.session.can_undo());
            (
                r.generation,
                r.toolbar_state.panel.revision(),
                r.fonts.subscription.unwrap().0,
            )
        });
        open_watermark_input_from(generation, current_focus_origin());
        REGISTRY.with(|slot| {
            assert!(
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .toolbar_state
                    .watermark
                    .get()
                    .is_none()
            )
        });
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().close_panel());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while matches!(service.status(), font_service::Status::Loading(_))
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        completed(request);
        REGISTRY.with(|slot| {
            assert!(
                !slot.borrow().as_ref().unwrap().fonts.is_loaded(),
                "closed subscription must not apply defaults"
            )
        });
        open_panel(3, false);
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_ne!(r.toolbar_state.panel.revision(), old);
            assert!(r.fonts.is_loaded());
            let panel = r.toolbar_state.panel.get().unwrap().clone_strong();
            assert_eq!(panel.get_font_status(), 1);
            assert!(!panel.get_font_families().row_count().eq(&0));
            assert_eq!(panel.get_values().strength, 37.);
        });
        completed(request);
        shutdown();
        font_service::TEST_SERVICE.with(|s| *s.borrow_mut() = None);
    }
}
