use super::*;
use slint::platform::{
    Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};

struct TestPlatform(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.0.borrow_mut().push(adapter.clone());
        Ok(adapter)
    }
}

#[test]
fn collapse_preserves_tool_and_document_retires_panel_and_startup_placement() {
    let adapters = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(TestPlatform(adapters))).unwrap();
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
    let placement_calls = Rc::new(Cell::new(0));
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        r.main.open(AnnotationToolbar::new().unwrap());
        r.panel.open(AnnotationPanel::new().unwrap());
        r.mode = InteractionMode::Tool(0);
        r.sync_mode_display();
        r.edit_values(0).size = 13;
        r.session.add(Object {
            id: Default::default(),
            kind: Kind::Rectangle,
            bounds: Bounds::from_corners((10., 10.), (90., 80.)),
            points: Vec::new(),
            erased: Vec::new(),
            style: Style::default(),
            extra: Extra::None,
        });
        r.panel.get().unwrap().set_values(r.values(0).clone());
        let output = placement_calls.clone();
        r.main
            .position_after(Duration::ZERO, move || output.set(output.get() + 1));
    });
    toggle_toolbar_collapse();
    slint::platform::update_timers_and_animations();
    assert_eq!(placement_calls.get(), 0);
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref().unwrap();
        assert!(r.main.get().unwrap().get_collapsed());
        assert_eq!(r.main.get().unwrap().window().size().width, 80);
        assert!(r.panel.get().is_none());
        assert_eq!(r.mode, InteractionMode::Tool(0));
        assert_eq!(r.values(0).size, 13);
        assert_eq!(r.session.objects.len(), 1);
        assert!(r.session.can_undo());
    });
    open_panel(0, false);
    REGISTRY.with(|slot| assert!(slot.borrow().as_ref().unwrap().panel.get().is_none()));
    toggle_toolbar_collapse();
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().unwrap();
        assert!(!r.main.get().unwrap().get_collapsed());
        assert_eq!(r.main.get().unwrap().window().size().width, 695);
        assert!(r.panel.get().is_none());
        r.close();
        r.main.open(AnnotationToolbar::new().unwrap());
        assert!(!r.main.get().unwrap().get_collapsed());
    });
    REGISTRY.with(|slot| slot.borrow_mut().take());
}

#[test]
fn collapsed_toolbar_has_only_grip_and_expand_hit_targets_and_renders_all_languages() {
    let adapters = Rc::new(RefCell::new(Vec::new()));
    slint::platform::set_platform(Box::new(TestPlatform(adapters.clone()))).unwrap();
    let main = AnnotationToolbar::new().unwrap();
    let adapter = adapters.borrow()[0].clone();
    let clicks = Rc::new(Cell::new(0));
    let output = clicks.clone();
    main.on_collapse_requested(move || output.set(output.get() + 1));
    let finishes = Rc::new(Cell::new(0));
    let output = finishes.clone();
    main.on_finish_requested(move || output.set(output.get() + 1));
    let drags = Rc::new(Cell::new(0));
    let output = drags.clone();
    main.on_drag_requested(move || output.set(output.get() + 1));
    let tools = Rc::new(Cell::new(0));
    let output = tools.clone();
    main.on_operate_requested(move || output.set(output.get() + 1));
    let click = |x, y| {
        for pressed in [true, false] {
            let position = slint::LogicalPosition::new(x, y);
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
    };
    for language in lexift_core::domain::ui_language::UiLanguage::ALL {
        crate::i18n::select(language);
        for dark in [false, true] {
            for scale in [1., 1.25] {
                for collapsed in [false, true] {
                    main.set_collapsed(collapsed);
                    main.global::<crate::Colors>()
                        .set_preference(i32::from(dark));
                    adapter
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        });
                    let width = (main.get_toolbar_width() * scale).round() as u32;
                    let height = (54. * scale).round() as u32;
                    adapter.set_size(slint::PhysicalSize::new(width, height));
                    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                    adapter.request_redraw();
                    adapter.draw_if_needed(|renderer| {
                        renderer.render(&mut pixels, width as usize);
                    });
                    if let Ok(directory) = std::env::var("LEXIFT_ANNOTATION_SNAPSHOT_DIR") {
                        crate::i18n::write_test_bitmap(
                            &directory,
                            &format!("toolbar-{}-{dark}-{scale}-{collapsed}", language.code()),
                            width,
                            height,
                            pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                    let before = clicks.get();
                    if !collapsed {
                        let before = finishes.get();
                        click(main.get_toolbar_width() - 71., 27.);
                        assert_eq!(
                            finishes.get(),
                            before + 1,
                            "finish must sit next to collapse with the standard gap"
                        );
                    }
                    click(main.get_toolbar_width() - 26., 27.);
                    assert_eq!(clicks.get(), before + 1);
                    if collapsed {
                        let before = drags.get();
                        click(16., 27.);
                        assert_eq!(drags.get(), before + 1);
                        assert_eq!(tools.get(), 0, "collapsed controls must not receive input");
                    }
                }
            }
        }
    }
}
