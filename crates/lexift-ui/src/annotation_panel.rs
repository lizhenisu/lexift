//! Typed, cancellable control input for a secondary annotation toolbar.
//! Component callbacks capture user origin; session routing belongs to the owner.
use super::{focus::Origin, window::InputWork};
use crate::{AnnotationChoiceRequest, AnnotationPanel};
use slint::ComponentHandle;
use std::rc::Rc;

pub(super) enum Input {
    Finished {
        confirmed: bool,
        origin: Origin,
    },
    Watermark(Origin),
    Dragging {
        active: bool,
        origin: Origin,
    },
    Tool(i32),
    Choice {
        request: AnnotationChoiceRequest,
        origin: Origin,
    },
    StyleChoice {
        request: AnnotationChoiceRequest,
        origin: Origin,
    },
    Escape(Origin),
    Undo,
    ClearAll,
}

/// Bind controls without a session reference. Delivery is FIFO and ends with
/// the panel's input owner, even if the component is retained after retirement.
pub(super) fn bind_input(
    panel: &AnnotationPanel,
    input: Rc<InputWork>,
    origin: Rc<dyn Fn() -> Origin>,
    receive: Rc<dyn Fn(Input)>,
) {
    let queue = input.receiver(receive);
    let finished = queue.clone();
    let at = origin.clone();
    panel.on_parameter_input_finished(move |confirmed| {
        finished(Input::Finished {
            confirmed,
            origin: at(),
        });
    });
    let watermark = queue.clone();
    let at = origin.clone();
    panel.on_watermark_edit_requested(move || watermark(Input::Watermark(at())));
    let dragging = queue.clone();
    let at = origin.clone();
    panel.on_parameter_dragging(move |active| {
        dragging(Input::Dragging {
            active,
            origin: at(),
        });
    });
    let selected = queue.clone();
    panel.on_selected(move |tool| selected(Input::Tool(tool)));
    let choice = queue.clone();
    let at = origin.clone();
    panel.on_choice_requested(move |request| {
        choice(Input::Choice {
            request,
            origin: at(),
        });
    });
    let style = queue.clone();
    let at = origin.clone();
    panel.on_text_style_requested(move |request| {
        style(Input::StyleChoice {
            request,
            origin: at(),
        })
    });
    let escape = queue.clone();
    let at = origin.clone();
    panel.on_escape_requested(move || escape(Input::Escape(at())));
    let clear = queue.clone();
    panel.on_clear_all_requested(move || clear(Input::ClearAll));
    let undo = queue.clone();
    panel.on_undo_requested(move || undo(Input::Undo));
    panel.window().on_close_requested(move || {
        queue(Input::Escape(origin()));
        slint::CloseRequestResponse::KeepWindowShown
    });
}

#[cfg(test)]
mod tests {
    use super::super::REGISTRY;
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn dropdown_ink_is_centered_for_30_and_38_dip_controls_at_both_dpis() {
        use slint::platform::{
            Platform, PointerEventButton, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct RenderPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for RenderPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(RenderPlatform(adapter.clone()))).unwrap();
        let panel = AnnotationPanel::new().unwrap();
        panel.global::<crate::Colors>().set_preference(0);
        panel.set_tool(10);
        panel.set_font_families(slint::ModelRc::new(slint::VecModel::from(vec![
            "Segoe UI".into(),
        ])));
        let values = crate::AnnotationValues {
            sequence_expanded: true,
            text_size: 22,
            ..Default::default()
        };
        panel.set_values(values.clone());
        let anchors = Rc::new(RefCell::new(std::collections::HashMap::new()));
        let received = anchors.clone();
        panel.on_choice_requested(move |q| {
            if matches!(q.field, 12 | 16 | 20) {
                received.borrow_mut().insert(q.field, q);
            }
        });
        for scale in [1., 1.25] {
            adapter.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            let size = super::super::toolbar::physical_size(1600., 180., scale);
            panel.window().set_size(size);
            panel.show().unwrap();
            // Obtain the real component bounds through its existing menu anchor
            // contract, rather than duplicating Flexbox positions in the test.
            for y in (15..160).step_by(10) {
                for x in (15..1590).step_by(10) {
                    panel.set_values(values.clone());
                    let position = slint::LogicalPosition::new(x as f32, y as f32);
                    adapter.dispatch_event(WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    });
                    adapter.dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    });
                }
            }
            panel.set_values(values.clone());
            for field in [12, 16, 20] {
                let q = anchors
                    .borrow()
                    .get(&field)
                    .expect("dropdown anchor")
                    .clone();
                assert_eq!(q.height, if field == 16 { 30. } else { 38. });
                for state in 0..3 {
                    panel.set_active_choice(if state == 2 { field } else { -1 });
                    let position = if state == 0 {
                        slint::LogicalPosition::new(1599., 179.)
                    } else {
                        slint::LogicalPosition::new(q.x + q.width - 14., q.y + q.height / 2.)
                    };
                    adapter.dispatch_event(WindowEvent::PointerMoved { position });
                    adapter.request_redraw();
                    let mut pixels =
                        slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(size.width, size.height);
                    assert!(adapter.draw_if_needed(|renderer| {
                        renderer.render(pixels.make_mut_slice(), size.width as usize);
                    }));
                    let mut rows = Vec::new();
                    for y in ((q.y + q.height / 2. - 6.) * scale).floor() as u32
                        ..((q.y + q.height / 2. + 6.) * scale).ceil() as u32
                    {
                        for x in ((q.x + q.width - 19.) * scale).floor() as u32
                            ..((q.x + q.width - 9.) * scale).ceil() as u32
                        {
                            let p = pixels.as_slice()[(y * size.width + x) as usize];
                            if p.r < 160 && p.g < 160 && p.b < 160 {
                                rows.push(y);
                            }
                        }
                    }
                    let center = (*rows.iter().min().expect("visible arrow")
                        + *rows.iter().max().unwrap()
                        + 1) as f32
                        / 2.;
                    assert!(
                        (center - (q.y + q.height / 2.) * scale).abs() <= 1.,
                        "field={field}, state={state}, scale={scale}, center={center}"
                    );
                }
            }
        }
        panel.hide().unwrap();
    }

    #[test]
    fn first_final_size_frame_keeps_corners_and_bottom_for_all_tools() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct RenderPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for RenderPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(RenderPlatform(adapter.clone()))).unwrap();
        for scale in [1., 1.25] {
            adapter.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            for tool in 0..=13 {
                for available in [320., 1400.] {
                    let panel = AnnotationPanel::new().unwrap();
                    panel.global::<crate::Colors>().set_preference(0);
                    panel.set_tool(tool);
                    panel.set_numeric_measuring(true);
                    let width = panel.get_numeric_content_width().min(available);
                    panel.set_numeric_measuring(false);
                    adapter.set_size(super::super::toolbar::physical_size(width, 220., scale));
                    let height = panel.get_numeric_content_height();
                    let size = super::super::toolbar::physical_size(width, height, scale);
                    adapter.set_size(size);
                    panel.show().unwrap();
                    let mut pixels =
                        slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(size.width, size.height);
                    assert!(adapter.draw_if_needed(|renderer| {
                        renderer.render(pixels.make_mut_slice(), size.width as usize);
                    }));
                    let at = |x: u32, y: u32| pixels.as_slice()[(y * size.width + x) as usize];
                    for (x, y) in [
                        (0, 0),
                        (size.width - 1, 0),
                        (0, size.height - 1),
                        (size.width - 1, size.height - 1),
                    ] {
                        assert_eq!(
                            at(x, y),
                            slint::Rgb8Pixel::default(),
                            "opaque corner: tool={tool}, scale={scale}"
                        );
                    }
                    let bottom = at(size.width / 2, size.height - (3. * scale).ceil() as u32);
                    assert_ne!(
                        bottom,
                        slint::Rgb8Pixel::default(),
                        "missing bottom: tool={tool}, scale={scale}"
                    );
                    panel.hide().unwrap();
                }
            }
        }
    }

    #[test]
    fn mosaic_row_has_equal_vertical_margins_at_native_panel_sizes() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct RenderPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for RenderPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(RenderPlatform(adapter.clone()))).unwrap();
        let panel = AnnotationPanel::new().unwrap();
        panel.set_tool(11);
        panel.set_mosaic_available(true);
        panel.global::<crate::Colors>().set_preference(0);
        panel.show().unwrap();
        let selected = panel.global::<crate::Colors>().get_primary_container();
        for scale in [1.0, 1.25] {
            adapter.dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
            let width = (620.0 * scale) as u32;
            let height = (58.0 * scale).round() as u32;
            adapter.set_size(slint::PhysicalSize::new(width, height));
            adapter.request_redraw();
            let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
            assert!(adapter.draw_if_needed(|renderer| {
                renderer.render(pixels.make_mut_slice(), width as usize);
            }));
            let x = (25.0 * scale) as usize;
            let rows: Vec<_> = (0..height as usize)
                .filter(|y| {
                    let pixel = pixels.as_slice()[y * width as usize + x];
                    (pixel.r, pixel.g, pixel.b)
                        == (selected.red(), selected.green(), selected.blue())
                })
                .collect();
            let top = *rows.first().expect("selected area button must be rendered");
            let bottom = height as usize - 1 - rows.last().unwrap();
            assert!(
                top >= (8.0 * scale) as usize && bottom >= (8.0 * scale) as usize,
                "clipped row at scale {scale}: top={top}, bottom={bottom}"
            );
            assert!(
                top.abs_diff(bottom) <= 2,
                "row is not centered at scale {scale}: top={top}, bottom={bottom}"
            );
        }
    }

    #[test]
    fn panel_control_binding_is_fifo_captures_origin_and_retires_pending_input() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let panel = AnnotationPanel::new().unwrap();
        let input = InputWork::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        let received = events.clone();
        let origin = Rc::new(Cell::new(Origin::Foreground(None)));
        let observed = origin.clone();
        bind_input(
            &panel,
            input.clone(),
            Rc::new(move || observed.get()),
            Rc::new(move |event| {
                // A receiver may borrow the session; component callbacks must not execute it inline.
                REGISTRY.with(|slot| {
                    let _session = slot.borrow_mut();
                });
                let (label, at) = match event {
                    Input::Finished {
                        confirmed: true,
                        origin,
                    } => ("confirmed", Some(origin)),
                    Input::Finished {
                        confirmed: false,
                        origin,
                    } => ("ended", Some(origin)),
                    Input::Watermark(origin) => ("watermark", Some(origin)),
                    Input::Dragging {
                        active: true,
                        origin,
                    } => ("press", Some(origin)),
                    Input::Dragging {
                        active: false,
                        origin,
                    } => ("release", Some(origin)),
                    Input::Tool(10) => ("tool", None),
                    Input::Choice { request, origin } => {
                        assert_eq!((request.field, request.x, request.y), (23, 7., 9.));
                        ("choice", Some(origin))
                    }
                    Input::Escape(origin) => ("escape", Some(origin)),
                    Input::Undo => ("undo", None),
                    _ => panic!("unexpected panel event"),
                };
                if let Some(at) = at {
                    assert_eq!(at, Origin::Foreground(None));
                }
                received.borrow_mut().push(label);
            }),
        );
        REGISTRY.with(|slot| {
            let _session = slot.borrow_mut();
            panel.invoke_parameter_input_finished(true);
            panel.invoke_parameter_input_finished(false);
            panel.invoke_watermark_edit_requested();
            panel.invoke_parameter_dragging(true);
            panel.invoke_selected(10);
            panel.invoke_choice_requested(AnnotationChoiceRequest {
                field: 23,
                x: 7.,
                y: 9.,
                ..Default::default()
            });
            panel.invoke_parameter_dragging(false);
            panel.invoke_undo_requested();
            panel.invoke_escape_requested();
            assert!(events.borrow().is_empty());
        });
        origin.set(Origin::Unobserved);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            *events.borrow(),
            [
                "confirmed",
                "ended",
                "watermark",
                "press",
                "tool",
                "choice",
                "release",
                "undo",
                "escape"
            ]
        );
        panel.invoke_selected(10);
        input.retire();
        panel.invoke_parameter_dragging(false);
        panel.invoke_undo_requested();
        slint::platform::update_timers_and_animations();
        assert_eq!(
            events.borrow().len(),
            9,
            "queued and future input retire together"
        );
    }
}
