use super::*;
use slint::platform::{
    Platform, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};

struct Backend(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
impl Platform for Backend {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(window.clone());
        Ok(window)
    }
}
fn backend() -> Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>> {
    let windows = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(Backend(windows.clone()))).unwrap();
    windows
}
fn setup() {
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
}
fn rectangle() -> Object {
    Object {
        id: Default::default(),
        kind: Kind::Rectangle,
        bounds: Bounds::from_corners((10., 20.), (130., 100.)),
        points: vec![],
        erased: vec![],
        style: Style::default(),
        extra: Extra::None,
    }
}
fn key(w: &slint::Window, text: &str, down: bool) {
    w.dispatch_event(if down {
        WindowEvent::KeyPressed { text: text.into() }
    } else {
        WindowEvent::KeyReleased { text: text.into() }
    });
}
fn chord(w: &slint::Window, text: &str, shift: bool) {
    let control = char::from(slint::platform::Key::Control).to_string();
    let shifted = char::from(slint::platform::Key::Shift).to_string();
    key(w, &control, true);
    if shift {
        key(w, &shifted, true);
    }
    key(w, text, true);
    key(w, text, false);
    if shift {
        key(w, &shifted, false);
    }
    key(w, &control, false);
}

#[test]
fn redo_ui_restores_projection_and_button_state_without_new_history() {
    backend();
    setup();
    let id = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.document.session.add(rectangle());
        r.document.session.update_selected(|o| o.style.width = 11.);
        r.document.session.objects[0].id
    });
    undo();
    REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref().unwrap();
        assert!(r.toolbar_state.main.get().unwrap().get_can_redo());
        assert_ne!(r.document.session.objects[0].style.width, 11.);
    });
    redo();
    REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref().unwrap();
        assert_eq!(r.document.session.objects[0].id, id);
        assert_eq!(r.document.session.objects[0].style.width, 11.);
        assert!(!r.toolbar_state.main.get().unwrap().get_can_redo());
    });
    undo();
    undo();
    REGISTRY.with(|s| {
        assert!(
            s.borrow()
                .as_ref()
                .unwrap()
                .document
                .session
                .objects
                .is_empty()
        )
    });
    redo();
    redo();
    REGISTRY.with(|s| {
        assert_eq!(
            s.borrow().as_ref().unwrap().document.session.objects[0]
                .style
                .width,
            11.
        )
    });
    shutdown();
}

#[test]
fn redo_ui_shortcuts_are_shared_and_shift_z_never_undoes() {
    backend();
    let toolbar = AnnotationToolbar::new().unwrap();
    let canvas = AnnotationCanvas::new().unwrap();
    let panel = AnnotationPanel::new().unwrap();
    let pin = crate::PinnedScreenshotWindow::new().unwrap();
    panel.set_tool(13);
    pin.set_editing(true);
    let actions = Rc::new(RefCell::new(Vec::new()));
    for bind in [true, false] {
        let a = actions.clone();
        if bind {
            toolbar.on_history_key(move |i| a.borrow_mut().push(i));
        } else {
            canvas.on_history_key(move |i| a.borrow_mut().push(i));
        }
    }
    let a = actions.clone();
    panel.on_undo_requested(move || a.borrow_mut().push(1));
    let a = actions.clone();
    panel.on_redo_requested(move || a.borrow_mut().push(2));
    let a = actions.clone();
    pin.on_edit_key(move |i| a.borrow_mut().push(if i == 4 { 2 } else { 1 }));
    for w in [
        toolbar.window(),
        canvas.window(),
        panel.window(),
        pin.window(),
    ] {
        w.show().unwrap();
        actions.borrow_mut().clear();
        for (text, shift) in [
            ("z", false),
            ("Z", false),
            ("z", true),
            ("Z", true),
            ("y", false),
            ("Y", false),
        ] {
            chord(w, text, shift);
        }
        assert_eq!(*actions.borrow(), [1, 1, 2, 2, 2, 2]);
    }
    actions.borrow_mut().clear();
    toolbar.set_output_busy(true);
    chord(toolbar.window(), "Y", false);
    assert!(actions.borrow().is_empty());
    panel.set_parameter_input_active(true);
    chord(panel.window(), "z", true);
    assert!(actions.borrow().is_empty());
}

#[test]
fn redo_ui_input_busy_gesture_and_retired_callbacks_cannot_change_document() {
    backend();
    setup();
    let (old, old_canvas) = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        r.canvas_state.pending_displays = vec![Rect {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        }];
        create_canvas(r, 0);
        (
            r.canvas_state.canvases[0].window.clone_strong(),
            r.canvas_state.canvases[0].revision(),
        )
    });
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
        r.document.session.add(rectangle());
        r.document.session.undo();
        r.toolbar_state.main.get().unwrap().set_output_busy(true);
    });
    redo();
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.document.session.objects.is_empty());
        r.toolbar_state.main.get().unwrap().set_output_busy(false);
        let panel = AnnotationPanel::new().unwrap();
        panel.set_parameter_input_active(true);
        r.toolbar_state.panel.open(panel);
    });
    history_key(2);
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.document.session.objects.is_empty());
        r.close_panel();
        r.document.session.begin_drag();
    });
    redo();
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.document.session.objects.is_empty());
        r.document.session.cancel_drag();
        r.generation += 1;
        assert_eq!(r.canvas_state.canvases[0].revision(), old_canvas);
    });
    old.invoke_redo_requested();
    slint::platform::update_timers_and_animations();
    REGISTRY.with(|s| {
        assert!(
            s.borrow()
                .as_ref()
                .unwrap()
                .document
                .session
                .objects
                .is_empty()
        )
    });
    redo();
    REGISTRY.with(|s| {
        assert_eq!(
            s.borrow().as_ref().unwrap().document.session.objects.len(),
            1
        )
    });
    shutdown();
}

#[test]
fn redo_ui_toolbar_renders_all_languages_themes_dpis_and_states() {
    let windows = backend();
    let w = AnnotationToolbar::new().unwrap();
    let adapter = windows.borrow().last().unwrap().clone();
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for dark in [false, true] {
            w.global::<crate::Colors>().set_preference(i32::from(dark));
            for dpi in [1., 1.25] {
                adapter
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                for mode in 0..4 {
                    w.set_screenshot_mode(mode != 0);
                    w.set_pin_edit_mode(mode == 2);
                    w.set_image_edit_mode(mode == 3);
                    for state in 0..3 {
                        w.set_can_undo(state != 0);
                        w.set_can_redo(state == 1);
                        w.set_output_busy(state == 2);
                        let width = (w.get_toolbar_width() * dpi) as u32;
                        let height = (54. * dpi) as u32;
                        adapter.set_size(slint::PhysicalSize::new(width, height));
                        let mut pixels =
                            vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                        adapter.request_redraw();
                        adapter.draw_if_needed(|r| {
                            r.render(&mut pixels, width as usize);
                        });
                        assert!(pixels.windows(2).any(|p| p[0] != p[1]));
                        if let Ok(dir) = std::env::var("LEXIFT_REDO_RENDER_DIR") {
                            crate::i18n::write_test_bitmap(
                                &dir,
                                &format!("toolbar-{}-{dark}-{dpi}-{mode}-{state}", language.code()),
                                width,
                                height,
                                pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
}
