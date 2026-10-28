//! Independent image editing with a shared physical-pixel viewport and source-resolution output.
use super::*;
struct Pending {
    token: u64,
    document_owner: Option<(u64, u64)>,
    session: u64,
    operation: u64,
    window: AnnotationCanvas,
    image: Arc<CapturedRegion>,
    timer: slint::Timer,
    area: Rect,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
}
thread_local! { static NEXT: std::cell::Cell<u64> = const {std::cell::Cell::new(0)}; }
thread_local! { static PENDING: RefCell<Option<Pending>> = const {RefCell::new(None)}; }
pub(in crate::annotation) fn clear() {
    super::frozen_editor::clear_image_timing();
    editor_ocr::clear();
    image_transform::clear();
    image_navigation::clear();
    PENDING.with(|p| {
        if let Some(p) = p.borrow_mut().take() {
            p.timer.stop();
            let _ = p.window.hide();
        }
    });
}
pub(super) fn open(session: u64, operation: u64, image: Arc<CapturedRegion>) {
    open_owned(session, operation, image, None);
}
pub(in crate::annotation) fn open_document(window: u64, editor: u64, image: Arc<CapturedRegion>) {
    open_owned(editor, editor, image, Some((window, editor)));
}
fn open_owned(
    session: u64,
    operation: u64,
    image: Arc<CapturedRegion>,
    document_owner: Option<(u64, u64)>,
) {
    clear();
    let token = NEXT.with(|n| {
        let next = n
            .get()
            .checked_add(1)
            .expect("image editor instance exhausted");
        n.set(next);
        next
    });
    let pending = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let area = long::source_area()?;
        let window = crate::screen_interaction::create(&r.lifecycle, AnnotationCanvas::new).ok()?;
        let generation = r.generation;
        let _ = window.window().set_rendering_notifier(move |state, _| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                super::frozen_editor::image_rendered(generation, session);
            }
        });
        window.set_image_view(true);
        crate::theme::apply(&window);
        window.set_frame(slint::Image::from_rgba8_premultiplied(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &image.pixels,
                (image.bounds.right - image.bounds.left) as u32,
                (image.bounds.bottom - image.bounds.top) as u32,
            ),
        ));
        window.set_source_left(image.bounds.left as f32);
        window.set_source_top(image.bounds.top as f32);
        Some(Pending {
            token,
            document_owner,
            session,
            operation,
            window,
            image,
            timer: Default::default(),
            area,
            origin: r.lifecycle.annotation_foreground.as_ref().map(|q| q()),
        })
    });
    let Some(pending) = pending else {
        unavailable(document_owner, session, operation);
        return;
    };
    PENDING.with(|p| *p.borrow_mut() = Some(pending));
    prepare(token, 0);
}
fn unavailable(owner: Option<(u64, u64)>, session: u64, operation: u64) {
    if let Some((window, editor)) = owner {
        history_editor::unavailable(window, editor);
    } else {
        long::editor_unavailable(session, operation);
    }
}
fn cancel() {
    if history_editor::active() {
        history_editor::close();
    } else {
        super::cancel();
    }
}
fn prepare(token: u64, attempt: u8) {
    if !PENDING.with(|p| p.borrow().as_ref().is_some_and(|p| p.token == token)) {
        return;
    }
    let result = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        PENDING.with(|p| {
            let p = p.borrow();
            let p = p.as_ref()?;
            if !p.document_owner.map_or_else(
                || {
                    FLOW.with(|f| {
                        let f = f.borrow();
                        f.state.session == p.session
                            && f.state
                                .long
                                .as_ref()
                                .is_some_and(|l| l.operation == p.operation)
                    })
                },
                |(window, editor)| history_editor::current(window, editor),
            ) {
                return None;
            }
            let a = p.area;
            let dpi = p.window.window().scale_factor();
            let width = (960. * dpi).min((a.right - a.left) as f32 * 0.9).max(1.) as u32;
            let height = (760. * dpi).min((a.bottom - a.top) as f32 * 0.9).max(1.) as u32;
            p.window.window().set_position(slint::PhysicalPosition::new(
                a.left + (a.right - a.left - width as i32) / 2,
                a.top + (a.bottom - a.top - height as i32) / 2,
            ));
            p.window
                .window()
                .set_size(slint::PhysicalSize::new(width, height));
            p.window.set_image_pixel_scale(dpi);
            Some((
                (r.prepare)(p.window.window()),
                (r.lifecycle.configure_menu_window)(p.window.window()),
            ))
        })
    });
    match result {
        Some((PassiveWindowPreparation::Ready, true)) if attempt > 0 => install(token),
        Some((status, _)) if status != PassiveWindowPreparation::Failed && attempt < 20 => PENDING
            .with(|p| {
                if let Some(p) = p.borrow().as_ref() {
                    p.timer.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(16),
                        move || prepare(token, attempt + 1),
                    );
                }
            }),
        _ => {
            let identity = PENDING.with(|p| {
                p.borrow()
                    .as_ref()
                    .map(|p| (p.document_owner, p.session, p.operation))
            });
            clear();
            if let Some((owner, session, operation)) = identity {
                unavailable(owner, session, operation);
            }
        }
    }
}
fn install(token: u64) {
    if !PENDING.with(|p| p.borrow().as_ref().is_some_and(|p| p.token == token)) {
        return;
    }
    let Some(p) = PENDING.with(|p| p.borrow_mut().take()) else {
        return;
    };
    let w = p.window;
    let success = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        if r.screenshot.as_ref().is_none_or(|e| e.id != p.session) {
            return None;
        }
        let generation = r.generation;
        let mut layer = CanvasLayer::new(w.clone_strong(), p.image.bounds);
        layer.image_view = true;
        layer.bootstrap = CanvasBootstrap::Ready;
        let revision = layer.revision();
        w.set_history_image(p.document_owner.is_some());
        w.set_imported_image(history_editor::imported());
        w.set_annotations_hidden(history_editor::hidden());
        w.on_visibility_requested(history_editor::visibility);
        w.on_image_dimensions_requested(move || {
            if canvas_current(generation, 0, revision) {
                image_resize::begin();
            }
        });
        w.on_prepare_image_menu(move || {
            canvas_current(generation, 0, revision) && image_transform::prepare_menu()
        });
        w.on_image_color_requested(move |kind, value| {
            use lexift_core::domain::image_color::{Action, Brightness, Contrast};
            if !canvas_current(generation, 0, revision) {
                return;
            }
            let action = match kind {
                0 => Some(Action::Grayscale),
                1 => Some(Action::Invert),
                2 => Brightness::new(value).map(Action::Brightness),
                3 => Contrast::new(value).map(Action::Contrast),
                4 => Some(Action::Reset),
                _ => None,
            };
            if let Some(action) = action {
                image_transform::color(action);
            }
        });
        image_navigation::bind(&w);
        let weak = w.as_weak();
        w.on_crop_action(move |apply| {
            if canvas_current(generation, 0, revision) && weak.upgrade().is_some() {
                image_crop::finish(apply);
            }
        });
        image_navigation::attach_cursor(r.lifecycle.image_pan_cursor.clone());
        hints::bind_image_editor(&w, generation, hints::Owner::ImageEditor(revision));
        w.on_region_key(keyboard::bind(
            w.window(),
            generation,
            keyboard::Owner::Canvas(0, revision),
        ));
        let weak = w.as_weak();
        w.on_image_pointer(move |event, x, y| {
            if canvas_current(generation, 0, revision)
                && let Some(w) = weak.upgrade()
            {
                let dpi = w.window().scale_factor();
                canvas_pointer(
                    0,
                    event,
                    (x - w.get_source_left()) / dpi,
                    (y - w.get_source_top()) / dpi,
                );
            }
        });
        w.on_escape_requested(move || {
            if canvas_current(generation, 0, revision) {
                escape_from(current_focus_origin());
            }
        });
        w.on_history_key(move |action| {
            if canvas_current(generation, 0, revision) {
                history_key(action);
            }
        });
        w.on_redo_requested(move || {
            if canvas_current(generation, 0, revision) {
                redo();
            }
        });
        w.on_undo_requested(move || {
            if canvas_current(generation, 0, revision) {
                undo();
            }
        });
        w.on_delete_requested(move || {
            if canvas_current(generation, 0, revision) {
                delete_selected();
            }
        });
        w.on_finish_path_requested(move || {
            if canvas_current(generation, 0, revision) {
                finish_polyline();
            }
        });
        w.on_backspace_requested(move || {
            if canvas_current(generation, 0, revision) {
                backspace_polyline();
            }
        });
        w.on_brush_size_scrolled(move |delta| {
            if canvas_current(generation, 0, revision) {
                brush_size_scrolled(0, delta);
            }
        });
        let weak = w.as_weak();
        let drag = r.lifecycle.begin_window_drag;
        w.on_header_drag_requested(move || {
            if image_resize::consume_pointer(-1) {
                return;
            }
            let weak = weak.clone();
            // Dispatch after the native down message establishes the bridge capture lease.
            slint::Timer::single_shot(Duration::ZERO, move || {
                if canvas_current(generation, 0, revision)
                    && let Some(w) = weak.upgrade()
                {
                    drag(w.window());
                }
            });
        });
        let weak = w.as_weak();
        let resize = r.lifecycle.begin_window_resize.clone();
        w.on_image_resize_requested(move || {
            let weak = weak.clone();
            let resize = resize.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if canvas_current(generation, 0, revision)
                    && let Some(w) = weak.upgrade()
                {
                    resize(
                        w.window(),
                        crate::bridge::PopupResizeEdge::BottomRight,
                        false,
                    );
                }
            });
        });
        w.on_image_close_requested(move || {
            if canvas_current(generation, 0, revision) {
                cancel();
            }
        });
        let weak = w.as_weak();
        let geometry = Rc::new(RefCell::new(None));
        let color = w.global::<crate::Colors>().get_surface();
        (r.lifecycle.configure_resize_background)(
            w.window(),
            [color.red(), color.green(), color.blue()],
        );
        (r.lifecycle.configure_window_paint_repair)(
            w.window(),
            Rc::new(move || {
                let weak = weak.clone();
                let geometry = geometry.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if !canvas_current(generation, 0, revision) {
                        return;
                    }
                    if let Some(w) = weak.upgrade() {
                        let now = (
                            w.window().position(),
                            w.window().size(),
                            w.window().scale_factor(),
                        );
                        if geometry.borrow().as_ref() == Some(&now) {
                            return;
                        }
                        *geometry.borrow_mut() = Some(now);
                        image_navigation::resize(&w);
                        REGISTRY.with(|s| {
                            if let Some(r) = s.borrow().as_ref() {
                                position_toolbar(r, &w);
                                if let (Some(window), Some(edit)) =
                                    (r.editor.window(), r.editor.edit())
                                {
                                    editor::place(
                                        window,
                                        edit.bounds,
                                        edit.scale,
                                        &edit.original,
                                        &r.document.session.displays,
                                    );
                                }
                            }
                        });
                    }
                });
            }),
        );
        w.window().on_close_requested(move || {
            if canvas_current(generation, 0, revision) {
                cancel();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        // Rebind ownership before dropping the old native canvas, which may own the toolbar.
        if let Some(main) = r.toolbar_state.main.get() {
            (r.lifecycle.attach_tool_window)(main.window(), w.window());
            main.set_image_edit_mode(true);
            main.set_long_capture_available(false);
        }
        // The retired desktop owner may refer to another monitor index. The replacement
        // has one canvas; refresh its identity before DPI callbacks can synchronize ownership.
        r.toolbar_state.toolbar_owner = None;
        let old = std::mem::replace(&mut r.canvas_state.canvases, vec![layer]);
        long::retain_canvases(old);
        sync_toolbar_owner(r);
        r.canvas_state.pending_displays = vec![];
        r.canvas_state.annotation_ready = true;
        if p.document_owner.is_none() {
            r.document.session = Session::new();
            r.document.session.image_source = Some(p.image.bounds);
            r.document.session.displays = vec![p.image.bounds];
        }
        r.document.session.image_pixel_parameters = true;
        r.document.mode = InteractionMode::Mouse;
        r.sync_edit_scope();
        let e = r.screenshot.as_mut()?;
        if p.document_owner.is_none() {
            e.frames = Arc::new(vec![(*p.image).clone()]);
            e.capture = Arc::new(ScreenCaptureFrame {
                revision: p.operation,
                serial: p.operation,
                regions: vec![(*p.image).clone()],
            });
            e.region = RegionEditor::new(p.image.bounds);
            e.preview = None;
            e.revision = e.revision.wrapping_add(1);
            e.presented = 0;
            FLOW.with(|f| f.borrow_mut().frames = Some(e.frames.clone()));
            r.lifecycle.annotation_screen_capture =
                Some(Arc::new(crate::FrozenCapture::new(e.frames.clone())));
        }
        let weak = w.as_weak();
        let input = r.canvas_state.canvases[0].input();
        let sink = registry_pointer(
            input,
            Rc::new(move |event| {
                if let Some(w) = weak.upgrade() {
                    if image_navigation::native(&w, event) {
                        return;
                    }
                    crate::bridge::dispatch_pointer_input(w.window(), event);
                }
            }),
        );
        if w.show().is_err() || !(r.lifecycle.complete_passive_window_show)(w.window(), sink) {
            return None;
        }
        long::set_editor(&w);
        image_crop::sync(r, true);
        long::editor_origin(p.origin);
        if let Some(main) = r.toolbar_state.main.get() {
            position_toolbar(r, &w);
            let _ = main.show();
        }
        r.queue_render();
        Some(())
    });
    if success.is_none() {
        let _ = w.hide();
        let old = long::take_retired_canvases();
        if !old.is_empty() {
            REGISTRY.with(|s| {
                if let Some(r) = s.borrow_mut().as_mut() {
                    if let (Some(main), Some(canvas)) = (r.toolbar_state.main.get(), old.first()) {
                        (r.lifecycle.attach_tool_window)(main.window(), canvas.window.window());
                    }
                    r.canvas_state.canvases = old;
                    r.toolbar_state.toolbar_owner = None;
                    sync_toolbar_owner(r);
                }
            });
        }
        unavailable(p.document_owner, p.session, p.operation);
    }
}

pub(in crate::annotation) fn outputting() -> bool {
    history_editor::busy() || FLOW.with(|f| f.borrow().state.phase == ScreenshotPhase::Output)
}
/// Map source pixels to/from physical screen coordinates for native text and cursor overlays.
pub(in crate::annotation) fn view_point(point: (f32, f32), reverse: bool) -> Option<(f32, f32)> {
    let w = long::editor()?;
    let p = w.window().position();
    let origin = [
        p.x as f32,
        p.y as f32 + w.get_image_header_height() * w.window().scale_factor(),
    ];
    let q = if reverse {
        image_navigation::map(&w, [point.0 - origin[0], point.1 - origin[1]], true)?
    } else {
        let q = image_navigation::map(&w, [point.0, point.1], false)?;
        [q[0] + origin[0], q[1] + origin[1]]
    };
    Some((q[0], q[1]))
}
pub(in crate::annotation) fn view_zoom() -> Option<f32> {
    long::editor().map(|w| {
        w.get_image_zoom() * w.window().scale_factor() * image_navigation::document_scale(&w)
    })
}
/// Native text surfaces must not escape the image viewport into the title or status bar.
pub(in crate::annotation) fn clip_text(mut bounds: Bounds) -> Bounds {
    if let Some(w) = long::editor() {
        let p = w.window().position();
        let s = w.window().size();
        let dpi = w.window().scale_factor();
        let left = p.x as f32;
        let top = p.y as f32 + w.get_image_header_height() * dpi;
        let right = left + s.width as f32;
        let bottom = p.y as f32 + s.height as f32 - w.get_image_footer_height() * dpi;
        let (left, top, right, bottom) = if !w.get_image_cropping() {
            let (a, b) = image_navigation::visible_screen_bounds(&w)
                .unwrap_or(([0., 0.], [s.width as f32, s.height as f32]));
            let crop_left = (p.x as f32 + a[0]).clamp(left, right - 1.);
            let crop_top = (top + a[1]).clamp(top, bottom - 1.);
            (
                crop_left,
                crop_top,
                (p.x as f32 + b[0]).clamp(crop_left + 1., right),
                (top + b[1]).clamp(crop_top + 1., bottom),
            )
        } else {
            (left, top, right, bottom)
        };
        bounds.left = bounds.left.clamp(left, right - 1.);
        bounds.top = bounds.top.clamp(top, bottom - 1.);
        bounds.right = bounds.right.clamp(bounds.left + 1., right);
        bounds.bottom = bounds.bottom.clamp(bounds.top + 1., bottom);
    }
    bounds
}

/// Keep the existing tool palette attached to the editor, including native moves and DPI changes.
fn position_toolbar(r: &Registry, w: &AnnotationCanvas) {
    let Some(main) = r.toolbar_state.main.get() else {
        return;
    };
    let p = w.window().position();
    let size = w.window().size();
    let area = (r.lifecycle.popup_work_area)(Point { x: p.x, y: p.y }).or_else(long::source_area);
    let Some(a) = area else { return };
    let ms = main.window().size();
    let gap = (8. * w.window().scale_factor()) as i32;
    let x = (p.x + size.width as i32 - ms.width as i32)
        .clamp(a.left, (a.right - ms.width as i32).max(a.left));
    let y = if p.y + size.height as i32 + gap + ms.height as i32 <= a.bottom {
        p.y + size.height as i32 + gap
    } else {
        (p.y - gap - ms.height as i32).max(a.top)
    };
    main.window()
        .set_position(slint::PhysicalPosition::new(x, y));
}
