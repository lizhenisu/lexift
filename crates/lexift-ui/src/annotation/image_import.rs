//! Import menu intents and availability. Native acquisition is dispatched through Core.
use super::*;
use lexift_core::{
    AppEvent,
    domain::image_import::{Event, Target},
};
thread_local! { static AVAILABLE: std::cell::Cell<bool> = const {std::cell::Cell::new(true)}; }
thread_local! { static REQUEST: RefCell<Option<lexift_core::domain::image_import::Request>> = const {RefCell::new(None)}; }
thread_local! { static PIN_MODE: RefCell<Option<(u64,InteractionMode)>> = const {RefCell::new(None)}; }
/// A new pin must be operable even if acquisition started from a desktop drawing tool.
pub(crate) fn prepare_pin(id: u64) -> bool {
    if anchor(id).is_none() {
        return false;
    }
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else {
            return false;
        };
        let old = r.document.mode;
        if !r.set_mode(InteractionMode::Mouse) {
            return false;
        }
        PIN_MODE.with(|s| *s.borrow_mut() = Some((id, old)));
        r.queue_render();
        true
    })
}
pub(crate) fn anchor(id: u64) -> Option<Point> {
    REQUEST.with(|r| r.borrow().as_ref().filter(|r| r.id == id).map(|r| r.anchor))
}
pub(crate) fn available() -> bool {
    AVAILABLE.with(|v| v.get())
}
pub(crate) fn emit(event: Event) {
    let handler = EXPORT_HANDLER.with(|h| h.borrow().clone());
    if let Some(handler) = handler {
        handler(AppEvent::ImageImport(event));
    }
}
pub(crate) fn request(target: Target) {
    let intent = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        let anchor = (r.lifecycle.toolbar_cursor_position)().unwrap_or_default();
        let owner = r
            .toolbar_state
            .main
            .get()
            .and_then(|w| (r.lifecycle.annotation_window_token)(w.window()));
        Some(Event::Requested {
            target,
            anchor,
            owner,
            title: crate::i18n::tr("Open image"),
        })
    });
    if let Some(event) = intent {
        emit(event);
    }
}
pub(crate) fn sync(state: &lexift_core::AppState) {
    REQUEST.with(|r| *r.borrow_mut() = state.image_import.request.clone());
    let retired = PIN_MODE.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|(id, _)| {
            state
                .image_import
                .request
                .as_ref()
                .is_none_or(|r| r.id != *id)
        }) {
            s.take()
        } else {
            None
        }
    });
    if let Some((id, mode)) = retired
        && state
            .image_import
            .notice
            .is_some_and(|(notice, _, feedback)| {
                notice == id && feedback == lexift_core::domain::image_import::Feedback::Failed
            })
    {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut()
                && r.document.mode.is_mouse()
            {
                r.set_mode(mode);
                r.queue_render();
            }
        });
    }
    let available = lexift_core::domain::image_import::available(state);
    AVAILABLE.with(|v| v.set(available));
    REGISTRY.with(|r| {
        if let Some(w) = r.borrow().as_ref().and_then(|r| r.toolbar_state.main.get()) {
            w.set_import_available(available);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(window.clone());
            Ok(window)
        }
    }
    #[test]
    fn clipboard_pin_leaves_drawing_and_restores_tool_only_on_failure() {
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last))).unwrap();
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| true,
                |_, _, _| true,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.document.mode = InteractionMode::Tool(0);
            r.document.session.add(Object {
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((1., 2.), (10., 20.)),
                points: vec![],
                erased: vec![],
                style: Style::default(),
                extra: Extra::None,
            });
        });
        let original = REGISTRY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .document
                .session
                .objects
                .clone()
        });
        let mut state = lexift_core::AppState::default();
        state.image_import.request = Some(lexift_core::domain::image_import::Request {
            id: 1,
            target: Target::ClipboardPin,
            anchor: Point::default(),
            phase: lexift_core::domain::image_import::Phase::Preparing,
            editor_owner: None,
        });
        sync(&state);
        assert!(prepare_pin(1));
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert!(r.document.mode.is_mouse());
            assert_eq!(r.document.session.objects, original);
        });
        state.image_import.request = None;
        state.image_import.notice = Some((
            1,
            Point::default(),
            lexift_core::domain::image_import::Feedback::Failed,
        ));
        sync(&state);
        REGISTRY.with(|s| {
            assert_eq!(
                s.borrow().as_ref().unwrap().document.mode,
                InteractionMode::Tool(0)
            )
        });
        state.image_import.request = Some(lexift_core::domain::image_import::Request {
            id: 2,
            target: Target::ClipboardPin,
            anchor: Point::default(),
            phase: lexift_core::domain::image_import::Phase::Preparing,
            editor_owner: None,
        });
        state.image_import.notice = None;
        sync(&state);
        assert!(prepare_pin(2));
        state.image_import.request = None;
        sync(&state);
        REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            assert!(r.document.mode.is_mouse());
            assert_eq!(r.document.session.objects, original);
        });
        assert!(!prepare_pin(1));
        REGISTRY.with(|s| s.borrow_mut().take());
    }
    #[test]
    fn import_menu_renders_every_locale_and_split_button_does_not_double_trigger() {
        let last = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(last.clone()))).unwrap();
        let menu = crate::AnnotationChoiceWindow::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        for language in lexift_core::domain::ui_language::UiLanguage::ALL {
            crate::i18n::select(language);
            let labels = [
                crate::i18n::tr("Open image"),
                crate::i18n::tr("Pin image from clipboard"),
            ];
            menu.set_option_measurement(labels.join("\n").into());
            menu.set_options(ModelRc::new(VecModel::from(
                labels.map(Into::into).to_vec(),
            )));
            for theme in [0, 1] {
                for dpi in [1., 1.25] {
                    menu.global::<crate::Colors>().set_preference(theme);
                    menu.window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                    let width = menu.get_options_width().max(240.);
                    menu.set_content_width(width);
                    menu.window()
                        .set_size(slint::LogicalSize::new(width, menu.get_menu_height()));
                    menu.show().unwrap();
                    adapter.request_redraw();
                    let size = adapter.size();
                    let mut pixels =
                        vec![slint::Rgb8Pixel::default(); (size.width * size.height) as usize];
                    assert!(adapter.draw_if_needed(|renderer| {
                        renderer.render(&mut pixels, size.width as usize);
                    }));
                    if let Ok(folder) = std::env::var("LEXIFT_I18N_SNAPSHOT_DIR") {
                        crate::i18n::write_test_bitmap(
                            &folder,
                            &format!("import-menu-{}-{theme}-{dpi}", language.code()),
                            size.width,
                            size.height,
                            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                }
            }
        }
        menu.hide().unwrap();
        let toolbar = AnnotationToolbar::new().unwrap();
        let adapter = last.borrow().as_ref().unwrap().clone();
        toolbar
            .window()
            .set_size(slint::LogicalSize::new(toolbar.get_toolbar_width(), 54.));
        toolbar.show().unwrap();
        let opened = Rc::new(std::cell::Cell::new(0));
        let menus = Rc::new(std::cell::Cell::new(0));
        let count = opened.clone();
        toolbar.on_image_open_requested(move || count.set(count.get() + 1));
        let count = menus.clone();
        toolbar.on_image_import_menu(move || count.set(count.get() + 1));
        for (x, expected) in [(515., (1, 0)), (551., (1, 1))] {
            for pressed in [true, false] {
                let position = slint::LogicalPosition::new(x, 27.);
                adapter.window().dispatch_event(if pressed {
                    WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    }
                } else {
                    WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    }
                });
            }
            assert_eq!((opened.get(), menus.get()), expected);
        }
        toolbar.set_import_available(false);
        for pressed in [true, false] {
            let position = slint::LogicalPosition::new(515., 27.);
            adapter.window().dispatch_event(if pressed {
                WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                }
            } else {
                WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                }
            });
        }
        assert_eq!(opened.get(), 1);
    }
}
