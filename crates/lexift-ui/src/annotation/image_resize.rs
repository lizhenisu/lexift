//! Instance-owned proportional size drafts; only a prepared candidate commits the document.
use super::*;
use crate::{
    ScreenshotSizeInput,
    bridge::{PopupPointerInput, dispatch_pointer_input},
};
use lexift_core::domain::image_resize::ImageScale;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use std::cell::Cell;

struct Panel {
    token: u64,
    generation: u64,
    owner: (u64, u64),
    window: ScreenshotSizeInput,
    original: [u32; 2],
    source: Rect,
    bounds: Rect,
    orientation: lexift_core::domain::image_transform::Orientation,
    positioned: Cell<bool>,
    scale: Cell<ImageScale>,
    submitted: Cell<bool>,
    input: Rc<window::InputWork>,
    timer: slint::Timer,
    lifecycle: WindowLifecycleCallbacks,
    origin: Option<lexift_core::ports::magnifier::WindowToken>,
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.input.retire();
        self.timer.stop();
        (self.lifecycle.set_popup_dismissal)(self.window.window(), false);
        (self.lifecycle.cancel_window_interaction)(self.window.window());
        let _ = self.window.hide();
    }
}
thread_local! { static PANEL:RefCell<Option<Panel>>=const { RefCell::new(None) }; static SUPPRESS:Cell<bool>=const{Cell::new(false)}; }
pub(super) fn busy() -> bool {
    PANEL.with(|p| p.borrow().is_some())
}
pub(super) fn consume_pointer(kind: i32) -> bool {
    SUPPRESS.with(|s| {
        let active = s.get();
        if kind == 2 || kind == 3 {
            s.set(false);
        }
        active
    })
}
pub(super) fn clear() {
    SUPPRESS.with(|s| s.set(false));
    PANEL.with(|p| p.borrow_mut().take());
}
fn current(r: &Registry, p: &Panel) -> bool {
    r.generation == p.generation
        && r.screenshot.as_ref().is_some_and(|e| e.id == p.owner.0)
        && r.canvas_state
            .canvases
            .first()
            .is_some_and(|c| c.revision() == p.owner.1)
}
pub(super) fn begin() {
    if busy() || !image_transform::available() {
        return;
    }
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.gesture().is_some()
            || !r.pending_drawing.is_none()
            || screenshot::long::editor().is_none_or(|w| w.get_image_panning())
        {
            return;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        r.close_panel();
        r.hints.clear();
        let Some(source) = r.document.session.image_source else {
            return;
        };
        let bounds = r.document.session.image_crop.unwrap_or(source);
        let Ok(original) =
            ImageScale::default().bounds_size(bounds, r.document.session.image_orientation)
        else {
            return;
        };
        let Some(owner) = r
            .screenshot
            .as_ref()
            .zip(r.canvas_state.canvases.first())
            .map(|(e, c)| (e.id, c.revision()))
        else {
            return;
        };
        let Ok(w) = crate::screen_interaction::create(&r.lifecycle, ScreenshotSizeInput::new)
        else {
            return;
        };
        crate::theme::apply(&w);
        w.set_image_resize(true);
        w.set_locked(true);
        let scale = r.document.session.image_scale;
        let Ok(size) = scale.dimensions(original[0], original[1]) else {
            return;
        };
        w.set_width_text(size[0].to_string().into());
        w.set_height_text(size[1].to_string().into());
        let token = window::next_revision();
        let input = window::InputWork::new();
        let work = input.clone();
        w.on_dimension_edited(move |axis| work.post(move || edit(token, axis)));
        let work = input.clone();
        w.on_reset_requested(move || work.post(move || reset(token)));
        let work = input.clone();
        w.on_apply(move |_, _| work.post(move || apply(token)));
        let work = input.clone();
        w.on_cancel(move || work.post(move || cancel(token)));
        let work = input.clone();
        w.on_escape_pressed(move || work.post(move || cancel(token)));
        let work = input.clone();
        w.window().on_close_requested(move || {
            work.post(move || cancel(token));
            slint::CloseRequestResponse::KeepWindowShown
        });
        let work = input.clone();
        let focused = Cell::new(false);
        w.window().on_winit_window_event(move |_, event| {
            if matches!(event, winit::event::WindowEvent::ScaleFactorChanged { .. }) {
                work.post(move || {
                    PANEL.with(|s| {
                        if let Some(p) = s.borrow().as_ref().filter(|p| p.token == token) {
                            resize_panel(p);
                        }
                    })
                });
            }
            if let winit::event::WindowEvent::Focused(active) = event
                && focused.replace(*active)
                && !active
            {
                work.post(move || cancel(token));
            }
            EventResult::Propagate
        });
        let work = input.clone();
        let weak = w.as_weak();
        let drag = r.lifecycle.begin_window_drag;
        w.on_header_drag_requested(move |_, _| {
            let weak = weak.clone();
            work.post(move || {
                if let Some(w) = weak.upgrade() {
                    drag(w.window());
                }
            });
        });
        let origin = r.lifecycle.annotation_foreground.as_ref().and_then(|f| f());
        PANEL.with(|p| {
            *p.borrow_mut() = Some(Panel {
                token,
                generation: r.generation,
                owner,
                window: w,
                original,
                source,
                bounds,
                orientation: r.document.session.image_orientation,
                positioned: Cell::new(false),
                scale: Cell::new(scale),
                submitted: Cell::new(false),
                input,
                timer: Default::default(),
                lifecycle: r.lifecycle.clone(),
                origin,
            })
        });
        r.queue_render();
    });
    let token = PANEL.with(|p| p.borrow().as_ref().map(|p| p.token));
    if let Some(token) = token {
        show(token, 0);
    }
}
fn show(token: u64, attempt: u8) {
    let mut abandon = None;
    REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let Some(r) = slot.as_ref() else { return };
        PANEL.with(|p| {
            let p = p.borrow();
            let Some(p) = p.as_ref().filter(|p| p.token == token && current(r, p)) else {
                return;
            };
            let w = &p.window;
            let host = &r.canvas_state.canvases[0].window;
            if (r.prepare)(w.window()) != crate::bridge::PassiveWindowPreparation::Ready
                || !(r.lifecycle.configure_menu_window)(w.window())
                || !(r.lifecycle.attach_tool_window)(w.window(), host.window())
            {
                if attempt < 8 {
                    p.timer.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(16),
                        move || show(token, attempt + 1),
                    );
                } else {
                    abandon = Some(p.token);
                }
                return;
            }
            let dpi = host.window().scale_factor();
            let pos = host.window().position();
            let host_size = host.window().size();
            let area = (r.lifecycle.popup_work_area)(Point {
                x: pos.x + host_size.width as i32 / 2,
                y: pos.y + host_size.height as i32 / 2,
            });
            let width = (w.get_content_width() * dpi).round() as u32;
            let height = (w.get_content_height() * dpi).round() as u32;
            w.window().set_size(slint::PhysicalSize::new(width, height));
            let x = pos.x + (host_size.width as i32 - width as i32) / 2;
            let y = pos.y + (host_size.height as i32 - height as i32) / 2;
            if let Some(area) = area {
                w.window().set_position(slint::PhysicalPosition::new(
                    x.clamp(area.left, (area.right - width as i32).max(area.left)),
                    y.clamp(area.top, (area.bottom - height as i32).max(area.top)),
                ));
            } else {
                w.window().set_position(slint::PhysicalPosition::new(x, y));
            }
            if !p.positioned.replace(true) {
                p.timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(16),
                    move || show(token, attempt + 1),
                );
                return;
            }
            let token = p.token;
            let work = p.input.clone();
            let weak = w.as_weak();
            let sink = registry_pointer(
                p.input.clone(),
                Rc::new(move |event| {
                    if let PopupPointerInput::DismissRequested { cause } = event {
                        if cause == crate::bridge::PopupDismissCause::PointerOutside {
                            SUPPRESS.with(|s| s.set(true));
                        }
                        work.post(move || cancel(token));
                    } else if matches!(event, PopupPointerInput::NativeMoveFinished) {
                        work.post(move || {
                            PANEL.with(|s| {
                                if let Some(p) = s.borrow().as_ref().filter(|p| p.token == token) {
                                    resize_panel(p);
                                }
                            })
                        });
                    } else if let Some(w) = weak.upgrade() {
                        dispatch_pointer_input(w.window(), event);
                    }
                }),
            );
            if w.show().is_err()
                || !(r.lifecycle.complete_passive_window_show)(w.window(), sink)
                || !(r.lifecycle.set_popup_dismissal)(w.window(), true)
            {
                abandon = Some(p.token);
                return;
            }
            let foreground = r.lifecycle.annotation_foreground.as_ref().and_then(|f| f());
            if foreground == p.origin {
                (r.lifecycle.activate_user_requested_window)(w.window());
                w.invoke_focus_width();
            }
        });
    });
    if let Some(token) = abandon {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut() {
                mosaic::notify(r, "Operation failed. Try again.");
            }
        });
        cancel(token);
    }
}
fn edit(token: u64, axis: i32) {
    PANEL.with(|s| {
        let s = s.borrow();
        let Some(p) = s
            .as_ref()
            .filter(|p| p.token == token && !p.submitted.get())
        else {
            return;
        };
        let value = if axis == 0 {
            p.window.get_width_text()
        } else {
            p.window.get_height_text()
        };
        let axis = usize::from(axis != 0);
        let ratio = value
            .parse::<u32>()
            .ok()
            .filter(|_| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| ImageScale::new(v, p.original[axis]).ok());
        if let Some(size) =
            ratio.and_then(|scale| scale.draft_dimensions(p.original[0], p.original[1]).ok())
        {
            if axis == 0 {
                p.window.set_height_text(size[1].to_string().into());
            } else {
                p.window.set_width_text(size[0].to_string().into());
            }
        }
        let result = ratio.and_then(|scale| {
            scale
                .validate(p.source, p.bounds, p.orientation)
                .ok()
                .map(|size| (scale, size))
        });
        if let Some((scale, size)) = result {
            p.scale.set(scale);
            if axis == 0 {
                p.window.set_height_text(size[1].to_string().into());
            } else {
                p.window.set_width_text(size[0].to_string().into());
            }
            p.window.set_valid(true);
            p.window.set_feedback("".into());
            resize_panel(p);
        } else {
            p.window.set_valid(false);
            let memory_limit =
                ratio.is_some_and(|scale| scale.bounds_size(p.bounds, p.orientation).is_ok());
            p.window.set_feedback(
                crate::i18n::tr(if memory_limit {
                    "Image dimensions exceed the memory limit"
                } else {
                    "Enter valid image dimensions (maximum 40 MP)"
                })
                .into(),
            );
            resize_panel(p);
        }
    });
}
fn reset(token: u64) {
    PANEL.with(|s| {
        if let Some(p) = s
            .borrow()
            .as_ref()
            .filter(|p| p.token == token && !p.submitted.get())
        {
            p.scale.set(Default::default());
            p.window.set_width_text(p.original[0].to_string().into());
            p.window.set_height_text(p.original[1].to_string().into());
            p.window.set_valid(true);
            p.window.set_feedback("".into());
            resize_panel(p);
        }
    });
}
fn apply(token: u64) {
    let mut done = false;
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let Some(r) = s.as_mut() else { return };
        PANEL.with(|p| {
            if let Some(p) = p.borrow().as_ref().filter(|p| {
                p.token == token && current(r, p) && !p.submitted.get() && p.window.get_valid()
            }) {
                let mut candidate = r.document.session.clone();
                match candidate.resize_image(p.scale.get()) {
                    Ok(true) => {
                        p.submitted.set(true);
                        p.window.set_valid(false);
                        image_transform::submit(r, candidate);
                    }
                    Ok(false) => done = true,
                    Err(_) => p
                        .window
                        .set_feedback(crate::i18n::tr("Operation failed. Try again.").into()),
                }
            }
        });
    });
    if done {
        cancel(token);
    }
}
fn cancel(token: u64) {
    let panel = PANEL.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().is_some_and(|p| p.token == token) {
            p.take()
        } else {
            None
        }
    });
    if let Some(p) = panel {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut().filter(|r| current(r, &p)) {
                if p.submitted.get() {
                    image_transform::cancel_scale(r);
                }
                r.queue_render();
            }
        });
        drop(p);
    }
}
pub(super) fn completed() {
    clear();
}
pub(super) fn failed() {
    PANEL.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.submitted.set(false);
            p.window.set_valid(true);
            p.window
                .set_feedback(crate::i18n::tr("Operation failed. Try again.").into());
            resize_panel(p);
        }
    });
}

pub(super) fn style_to_source(mut style: Style, factor: f32) -> Style {
    style.width /= factor;
    style.rounding /= factor;
    style
}
pub(super) fn text_lengths(text: &mut TextAnnotation, factor: f32) {
    text.font_size *= factor;
    text.layout_scale *= factor;
    if let Some(outline) = text.outline.as_mut() {
        outline.width *= factor;
    }
    if let Some(background) = text.background.as_mut() {
        background.padding *= factor;
        background.rounding *= factor;
    }
}
pub(super) fn projected_object(
    object: &Object,
    factor: f32,
    style_factor: f32,
    pixel_units: bool,
) -> Object {
    if !pixel_units {
        return object.clone();
    }
    let text_factor = factor * crate::annotation_text::object_scale(object);
    let mut object = object.clone();
    object.style.width *= style_factor;
    object.style.rounding *= style_factor;
    match &mut object.extra {
        Extra::Text(text) => text_lengths(text, text_factor),
        Extra::Sequence(sequence) => {
            sequence.size *= factor * sequence.scale;
            text_lengths(&mut sequence.caption, factor);
        }
        Extra::Mosaic(value) => {
            value.diameter *= factor;
            value.creation_scale = 1.;
        }
        Extra::Sticker(value) => {
            value.texture_size *= factor;
            value.diameter *= factor;
            value.creation_scale = 1.;
        }
        _ => {}
    }
    object
}

fn resize_panel(p: &Panel) {
    let w = p.window.window();
    let dpi = w.scale_factor();
    w.set_size(slint::PhysicalSize::new(
        (p.window.get_content_width() * dpi).round() as u32,
        (p.window.get_content_height() * dpi).round() as u32,
    ));
    let pos = w.position();
    if let Some(area) = (p.lifecycle.popup_work_area)(Point { x: pos.x, y: pos.y }) {
        let size = w.size();
        w.set_position(slint::PhysicalPosition::new(
            pos.x
                .clamp(area.left, (area.right - size.width as i32).max(area.left)),
            pos.y
                .clamp(area.top, (area.bottom - size.height as i32).max(area.top)),
        ));
    }
}

pub(super) fn pixel_units(r: &Registry) -> bool {
    r.document.session.image_pixel_parameters
        || screenshot::long::image_editing()
        || (pin_editor::active() && r.document.session.image_scale != Default::default())
}
pub(super) fn style_factor(r: &Registry, point: Option<(f32, f32)>) -> f32 {
    let scale = r.document.session.image_scale.factor() as f32;
    if !pixel_units(r) {
        return scale;
    }
    let point = point
        .or_else(|| {
            r.document
                .session
                .selected
                .and_then(|i| r.document.session.objects.get(i))
                .map(|o| o.bounds.center())
        })
        .unwrap_or_else(|| {
            r.document
                .session
                .image_source
                .map(|b| {
                    (
                        (b.left as f32 + b.right as f32) * 0.5,
                        (b.top as f32 + b.bottom as f32) * 0.5,
                    )
                })
                .unwrap_or((0., 0.))
        });
    scale
        * history_editor::source_dpi(point)
            .or_else(|| pin_editor::source_dpi(point))
            .unwrap_or(1.)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestPlatform;
    impl slint::platform::Platform for TestPlatform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            Ok(
                slint::platform::software_renderer::MinimalSoftwareWindow::new(
                    slint::platform::software_renderer::RepaintBufferType::NewBuffer,
                ),
            )
        }
    }
    #[test]
    fn resize_panel_renders_all_languages_themes_and_dpis() {
        use lexift_core::domain::{settings::ThemePreference, ui_language::UiLanguage};
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        for language in UiLanguage::ALL {
            crate::i18n::select(language);
            for dark in [false, true] {
                crate::theme::set_current(if dark {
                    ThemePreference::Dark
                } else {
                    ThemePreference::Light
                });
                for dpi in [1., 1.25] {
                    for invalid in [false, true] {
                        let w = ScreenshotSizeInput::new().unwrap();
                        crate::theme::apply(&w);
                        w.set_image_resize(true);
                        w.set_locked(true);
                        w.set_width_text("40000000".into());
                        w.set_height_text("1".into());
                        w.set_valid(!invalid);
                        if invalid {
                            w.set_feedback(
                                crate::i18n::tr("Enter valid image dimensions (maximum 40 MP)")
                                    .into(),
                            );
                        }
                        w.window().dispatch_event(
                            slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: dpi },
                        );
                        w.window().set_size(slint::PhysicalSize::new(
                            (w.get_content_width() * dpi).round() as u32,
                            (w.get_content_height() * dpi).round() as u32,
                        ));
                        let image = w.window().take_snapshot().unwrap();
                        assert!(image.width() > 280);
                        assert!(image.height() < 260);
                        if let Ok(dir) = std::env::var("LEXIFT_RESIZE_RENDER") {
                            crate::i18n::write_test_bitmap(
                                &dir,
                                &format!("{}-{dark}-{dpi}-{invalid}", language.code()),
                                image.width(),
                                image.height(),
                                image.as_slice().iter().map(|p| [p.r, p.g, p.b, p.a]),
                            );
                        }
                    }
                }
            }
        }
        crate::i18n::select(UiLanguage::EnglishUs);
        crate::theme::set_current(ThemePreference::Light);
    }
}
