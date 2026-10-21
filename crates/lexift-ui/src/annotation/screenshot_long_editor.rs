//! Independent image editing with a shared physical-pixel viewport and source-resolution output.
use super::*;
struct Pending {
    session: u64,
    operation: u64,
    window: AnnotationCanvas,
    image: Arc<CapturedRegion>,
    timer: slint::Timer,
    area: Rect,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
}
thread_local! { static PENDING: RefCell<Option<Pending>> = const {RefCell::new(None)}; }
pub(super) fn clear() {
    image_navigation::clear();
    PENDING.with(|p| {
        if let Some(p) = p.borrow_mut().take() {
            p.timer.stop();
            let _ = p.window.hide();
        }
    });
}
pub(super) fn open(session: u64, operation: u64, image: Arc<CapturedRegion>) {
    clear();
    let pending = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let area = long::source_area()?;
        let window = crate::screen_interaction::create(&r.lifecycle, AnnotationCanvas::new).ok()?;
        window.set_image_view(true);
        crate::theme::apply(&window);
        window.set_frame(slint::Image::from_rgba8_premultiplied(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &image.pixels,
                image.bounds.right as u32,
                image.bounds.bottom as u32,
            ),
        ));
        Some(Pending {
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
        long::editor_unavailable(session, operation);
        return;
    };
    PENDING.with(|p| *p.borrow_mut() = Some(pending));
    prepare(0);
}
fn cancel() {
    super::cancel();
}
fn prepare(attempt: u8) {
    let result = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        PENDING.with(|p| {
            let p = p.borrow();
            let p = p.as_ref()?;
            if !FLOW.with(|f| {
                let f = f.borrow();
                f.state.session == p.session
                    && f.state
                        .long
                        .as_ref()
                        .is_some_and(|l| l.operation == p.operation)
            }) {
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
        Some((PassiveWindowPreparation::Ready, true)) if attempt > 0 => install(),
        Some((status, _)) if status != PassiveWindowPreparation::Failed && attempt < 20 => PENDING
            .with(|p| {
                if let Some(p) = p.borrow().as_ref() {
                    p.timer.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(16),
                        move || prepare(attempt + 1),
                    );
                }
            }),
        _ => {
            let identity = PENDING.with(|p| p.borrow().as_ref().map(|p| (p.session, p.operation)));
            clear();
            if let Some((session, operation)) = identity {
                long::editor_unavailable(session, operation);
            }
        }
    }
}
fn install() {
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
                canvas_pointer(0, event, x / dpi, y / dpi);
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
        r.document.session = Session::new();
        r.document.session.displays = vec![p.image.bounds];
        r.document.mode = InteractionMode::Mouse;
        r.sync_edit_scope();
        let e = r.screenshot.as_mut()?;
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
        w.invoke_fit_image();
        long::set_editor(&w);
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
        long::editor_unavailable(p.session, p.operation);
    }
}

pub(in crate::annotation) fn outputting() -> bool {
    FLOW.with(|f| f.borrow().state.phase == ScreenshotPhase::Output)
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
    long::editor().map(|w| w.get_image_zoom() * w.window().scale_factor())
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
            let a = image_navigation::map(&w, [w.get_content_left(), w.get_content_top()], false)
                .unwrap_or([0., 0.]);
            let b =
                image_navigation::map(&w, [w.get_content_right(), w.get_content_bottom()], false)
                    .unwrap_or([s.width as f32, s.height as f32]);
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
