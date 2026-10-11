//! Watermark input presentation and owned UI events, independent of the document.
use super::window::InputWork;
use crate::AnnotationWatermarkInput;
use lexift_core::domain::geometry::{Point, Rect};
use slint::ComponentHandle;
use std::rc::Rc;

pub(super) struct Request {
    pub value: slint::SharedString,
    pub position: Point,
}

pub(super) enum Input {
    Presets {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    DismissPresets,
    Apply(String),
    Cancel,
}

/// Build from a presentation snapshot. Display preparation, ownership, template
/// parsing and document updates belong to the calling session.
pub(super) fn build_window(request: Request) -> Option<AnnotationWatermarkInput> {
    let editor = AnnotationWatermarkInput::new().ok()?;
    crate::theme::apply(&editor);
    editor.set_value(request.value);
    editor.window().set_position(slint::PhysicalPosition::new(
        request.position.x,
        request.position.y,
    ));
    Some(editor)
}

/// Use the actual physical window size once its monitor DPI is known.
pub(super) fn position(
    toolbar: Rect,
    size: slint::PhysicalSize,
    scale: f32,
    work_area: Option<Rect>,
) -> Point {
    // Preserve the existing 15 logical pixel separation from the toolbar.
    let gap = (15. * scale.max(0.1)).round() as i32;
    crate::placement::place_toolbar_popup(
        toolbar,
        size.width,
        size.height,
        work_area,
        gap,
        crate::placement::Side::Before,
    )
}

/// All callbacks use the installed instance's FIFO queue. A retained component
/// cannot apply or cancel a replacement once that queue has retired.
pub(super) fn bind_input(
    editor: &AnnotationWatermarkInput,
    input: Rc<InputWork>,
    receive: Rc<dyn Fn(Input)>,
) {
    let queue = input.receiver(receive);
    let presets = queue.clone();
    editor.on_presets_requested(move |x, y, width, height| {
        presets(Input::Presets {
            x,
            y,
            width,
            height,
        });
    });
    let dismiss = queue.clone();
    editor.on_dismiss_presets(move || dismiss(Input::DismissPresets));
    let apply = queue.clone();
    editor.on_apply(move |value| apply(Input::Apply(value.to_string())));
    let cancel = queue.clone();
    editor.on_cancel(move || cancel(Input::Cancel));
    editor.window().on_close_requested(move || {
        queue(Input::Cancel);
        slint::CloseRequestResponse::KeepWindowShown
    });
}

/// Keep native pointer dispatch synchronous while its instance gate and the
/// supplied routing check reject callbacks from replaced input windows.
pub(super) fn pointer_sink(
    weak: slint::Weak<AnnotationWatermarkInput>,
    current: impl Fn() -> bool + 'static,
    input: Rc<InputWork>,
    before_dispatch: impl Fn(crate::bridge::PopupPointerInput) + 'static,
) -> crate::bridge::PopupPointerSink {
    input.pointer_sink(move |event| {
        if current()
            && let Some(editor) = weak.upgrade()
        {
            before_dispatch(event);
            crate::bridge::dispatch_pointer_input(editor.window(), event);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::REGISTRY;
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn watermark_placement_uses_physical_size_and_handles_negative_and_small_work_areas() {
        let area = Rect {
            left: -1920,
            top: -1000,
            right: 0,
            bottom: 1000,
        };
        for (scale, size, gap) in [
            (1., slint::PhysicalSize::new(440, 270), 15),
            (1.25, slint::PhysicalSize::new(550, 338), 19),
        ] {
            let toolbar = Rect {
                left: -1000,
                top: 0,
                right: -100,
                bottom: 75,
            };
            let expected_y = -(size.height as i32) - gap;
            let above = position(toolbar, size, scale, Some(area));
            assert_eq!(
                above,
                Point {
                    x: -1000,
                    y: expected_y
                }
            );
            assert_eq!(above.y + size.height as i32 + gap, toolbar.top);
            let at_top = Rect {
                top: -990,
                bottom: -915,
                ..toolbar
            };
            let below = position(at_top, size, scale, Some(area));
            assert_eq!(below.y, at_top.bottom + gap);
            let at_right = Rect {
                left: -100,
                ..toolbar
            };
            let clamped = position(at_right, size, scale, Some(area));
            assert_eq!(clamped.x + size.width as i32, area.right);
            for p in [above, below, clamped] {
                assert!(p.x >= area.left && p.y >= area.top);
                assert!(p.x + size.width as i32 <= area.right);
                assert!(p.y + size.height as i32 <= area.bottom);
            }
            let small = Rect {
                left: -100,
                top: -50,
                right: 200,
                bottom: 200,
            };
            assert_eq!(
                position(toolbar, size, scale, Some(small)),
                Point { x: -100, y: -50 }
            );
            assert_eq!(
                position(toolbar, size, scale, None),
                Point {
                    x: toolbar.left,
                    y: expected_y
                }
            );
        }
    }

    #[test]
    fn watermark_snapshot_and_input_do_not_borrow_the_session_inline() {
        use slint::platform::{Platform, WindowAdapter, software_renderer::SoftwareRenderer};
        // MinimalSoftwareWindow ignores position requests; this adapter records
        // the builder's physical placement without claiming native screen coverage.
        struct PositionedWindow {
            window: slint::Window,
            renderer: SoftwareRenderer,
            position: Cell<slint::PhysicalPosition>,
        }
        impl WindowAdapter for PositionedWindow {
            fn window(&self) -> &slint::Window {
                &self.window
            }
            fn renderer(&self) -> &dyn slint::platform::Renderer {
                &self.renderer
            }
            fn size(&self) -> slint::PhysicalSize {
                slint::PhysicalSize::new(440, 270)
            }
            fn position(&self) -> Option<slint::PhysicalPosition> {
                Some(self.position.get())
            }
            fn set_position(&self, position: slint::WindowPosition) {
                self.position
                    .set(position.to_physical(self.window.scale_factor()));
            }
        }
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(Rc::<PositionedWindow>::new_cyclic(|weak| {
                    PositionedWindow {
                        window: slint::Window::new(weak.clone()),
                        renderer: SoftwareRenderer::new(),
                        position: Cell::new(slint::PhysicalPosition::new(0, 0)),
                    }
                }))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let editor = REGISTRY.with(|slot| {
            let _session = slot.borrow_mut();
            build_window(Request {
                value: "中文草稿\n$yyyy/MM/dd HH:mm:ss$".into(),
                position: Point { x: -490, y: -285 },
            })
            .unwrap()
        });
        assert_eq!(editor.get_value(), "中文草稿\n$yyyy/MM/dd HH:mm:ss$");
        assert_eq!(
            editor.window().position(),
            slint::PhysicalPosition::new(-490, -285)
        );
        assert_eq!(editor.get_selected_preset(), 1);
        let input = InputWork::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        let received = events.clone();
        bind_input(
            &editor,
            input.clone(),
            Rc::new(move |event| {
                REGISTRY.with(|slot| {
                    let _session = slot.borrow_mut();
                });
                let label = match event {
                    Input::Presets {
                        x,
                        y,
                        width,
                        height,
                    } => {
                        assert_eq!((x, y, width, height), (60., 217., 140., 34.));
                        "presets"
                    }
                    Input::DismissPresets => "dismiss",
                    Input::Apply(template) => {
                        assert_eq!(template, "新中文\n$yyyy$");
                        "apply"
                    }
                    Input::Cancel => "cancel",
                };
                received.borrow_mut().push(label);
            }),
        );
        REGISTRY.with(|slot| {
            let _session = slot.borrow_mut();
            editor.invoke_presets_requested(60., 217., 140., 34.);
            editor.invoke_dismiss_presets();
            editor.invoke_apply("新中文\n$yyyy$".into());
            editor.invoke_cancel();
            assert!(events.borrow().is_empty());
        });
        slint::platform::update_timers_and_animations();
        assert_eq!(*events.borrow(), ["presets", "dismiss", "apply", "cancel"]);
        editor.invoke_apply("new input must not leak".into());
        input.retire();
        editor.invoke_presets_requested(60., 217., 140., 34.);
        editor.invoke_cancel();
        slint::platform::update_timers_and_animations();
        assert_eq!(events.borrow().len(), 4);
    }
}
