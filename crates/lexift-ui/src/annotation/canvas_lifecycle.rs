//! Annotation canvas lifecycle; scoped to its owning session.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BootstrapAction {
    Create(usize),
    Advance(usize),
    ShowToolbar,
}

pub(super) fn bootstrap_action(
    created: usize,
    total: usize,
    states: impl Iterator<Item = CanvasBootstrap>,
    cursor: usize,
) -> BootstrapAction {
    if created < total {
        return BootstrapAction::Create(created);
    }
    let cursor = if created == 0 { 0 } else { cursor % created };
    let mut before_cursor = None;
    for (index, state) in states.enumerate() {
        if !matches!(state, CanvasBootstrap::Ready | CanvasBootstrap::Prepared) {
            if index >= cursor {
                return BootstrapAction::Advance(index);
            }
            before_cursor.get_or_insert(index);
        }
    }
    before_cursor.map_or(BootstrapAction::ShowToolbar, BootstrapAction::Advance)
}

/// Advance one screen stage per event-loop turn; a pending native window does
/// not hold up preparation of the other monitors.
pub(super) fn bootstrap_tick(generation: u64, revision: u64) {
    let callback_start = Instant::now();
    REGISTRY.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.toolbar_state.main.revision() == revision)
        else {
            return;
        };
        let action = bootstrap_action(
            r.canvas_state.canvases.len(),
            r.canvas_state.pending_displays.len(),
            r.canvas_state
                .canvases
                .iter()
                .map(|canvas| canvas.bootstrap),
            r.canvas_state.bootstrap_cursor,
        );
        match action {
            BootstrapAction::Create(index) => create_canvas(r, index),
            BootstrapAction::Advance(index) => {
                r.canvas_state.bootstrap_cursor = (index + 1) % r.canvas_state.canvases.len();
                match r.canvas_state.canvases[index].bootstrap {
                    CanvasBootstrap::Prepare(attempt) => show_canvas(r, index, attempt),
                    CanvasBootstrap::Present => render_initial_canvas(r, index),
                    CanvasBootstrap::Ready | CanvasBootstrap::Prepared => unreachable!(),
                }
            }
            BootstrapAction::ShowToolbar => {
                if r.canvas_state
                    .canvases
                    .iter()
                    .any(|layer| layer.bootstrap == CanvasBootstrap::Prepared)
                {
                    for index in 0..r.canvas_state.canvases.len() {
                        if r.canvas_state.canvases[index].bootstrap == CanvasBootstrap::Prepared {
                            show_prepared_canvas(r, index);
                            if r.generation != generation {
                                return;
                            }
                        }
                    }
                } else {
                    let revision = r.toolbar_state.main.revision();
                    r.toolbar_state
                        .main
                        .prepare_after(Duration::from_millis(16), move || {
                            show_main(generation, revision, 0)
                        });
                    return;
                }
            }
        }
        if r.generation == generation {
            if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
                timing.largest_callback = timing.largest_callback.max(callback_start.elapsed());
            }
            r.toolbar_state
                .main
                .prepare_after(Duration::from_millis(16), move || {
                    bootstrap_tick(generation, revision)
                });
        }
    });
}

pub(super) fn create_canvas(r: &mut Registry, index: usize) {
    let bounds = r.canvas_state.pending_displays[index];
    let start = Instant::now();
    let Ok(window) = crate::screen_interaction::create(&r.lifecycle, AnnotationCanvas::new) else {
        r.close();
        return;
    };
    window.set_frame(slint::Image::default());
    let layer = CanvasLayer::new(window, bounds);
    let generation = r.generation;
    let revision = layer.revision();
    let window = &layer.window;
    window.on_pointer(move |kind, x, y| {
        if canvas_current(generation, index, revision) {
            canvas_pointer(index, kind, x, y);
        }
    });
    window.on_brush_size_scrolled(move |delta| {
        if canvas_current(generation, index, revision) {
            brush_size_scrolled(index, delta);
        }
    });
    let input = layer.input();
    let lifecycle = r.lifecycle.clone();
    window.on_escape_requested(move || {
        let origin = focus_origin(&lifecycle);
        later_canvas(&input, generation, index, revision, move || {
            escape_from(origin)
        });
    });
    let input = layer.input();
    window.on_undo_requested(move || later_canvas(&input, generation, index, revision, undo));
    let input = layer.input();
    window.on_delete_requested(move || {
        later_canvas(&input, generation, index, revision, delete_selected)
    });
    let input = layer.input();
    window.on_finish_path_requested(move || {
        later_canvas(&input, generation, index, revision, finish_polyline)
    });
    let input = layer.input();
    window.on_backspace_requested(move || {
        later_canvas(&input, generation, index, revision, backspace_polyline)
    });
    let input = layer.input();
    window.window().on_close_requested(move || {
        later_canvas(&input, generation, index, revision, close);
        slint::CloseRequestResponse::KeepWindowShown
    });
    r.canvas_state.canvases.push(layer);
    if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
        let elapsed = start.elapsed();
        timing.canvas_create += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen.push(CanvasColdTiming {
            create: elapsed,
            ..Default::default()
        });
    }
}

pub(super) fn show_canvas(r: &mut Registry, index: usize, attempt: u8) {
    let layer = &r.canvas_state.canvases[index];
    let stage_start = Instant::now();
    let rect = layer.bounds;
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    layer
        .window
        .window()
        .set_size(slint::PhysicalSize::new(width, height));
    layer
        .window
        .window()
        .set_position(slint::PhysicalPosition::new(rect.left, rect.top));
    let preparation = match layer.prepare(&r.lifecycle, None) {
        PassiveWindowPreparation::Ready => (r.prepare)(layer.window.window()),
        status => status,
    };
    if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
        let elapsed = stage_start.elapsed();
        timing.canvas_prepare += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].prepare += elapsed;
    }
    match preparation {
        PassiveWindowPreparation::Ready => {
            r.canvas_state.canvases[index].bootstrap = CanvasBootstrap::Prepared;
        }
        PassiveWindowPreparation::Pending if attempt < 20 => {
            let retry_start = Instant::now();
            if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
                let elapsed = retry_start.elapsed();
                timing.canvas_retry += elapsed;
                timing.largest_stage = timing.largest_stage.max(elapsed);
                timing.per_screen[index].retry += elapsed;
                timing.per_screen[index].retries += 1;
            }
            r.canvas_state.canvases[index].bootstrap = CanvasBootstrap::Prepare(attempt + 1);
        }
        _ => r.close(),
    }
}

/// Reveal only after every screen passed hidden preparation.
pub(super) fn show_prepared_canvas(r: &mut Registry, index: usize) {
    let layer = &r.canvas_state.canvases[index];

    let show_start = Instant::now();
    if layer.host.show().is_err() {
        r.close();
        return;
    }
    if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
        let elapsed = show_start.elapsed();
        timing.canvas_show += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].show += elapsed;
    }
    let native_start = Instant::now();
    if !(r.lifecycle.complete_passive_window_show)(
        layer.host.window.window(),
        pointer_canvas(
            layer.window.as_weak(),
            r.generation,
            index,
            layer.revision(),
            layer.input(),
        ),
    ) {
        r.close();
        return;
    }
    if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
        let elapsed = native_start.elapsed();
        timing.canvas_native_complete += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].native_complete += elapsed;
    }
    (r.lifecycle.annotation_click_through)(layer.window.window(), r.mode.is_mouse());
    settle_canvas(r, index, layer.revision(), 2);
    r.canvas_state.canvases[index].bootstrap = CanvasBootstrap::Present;
}

/// Present one complete blank monitor frame before its canvas accepts drawing.
/// The native layered bitmap is retained here until Windows hit testing of an
/// unpresented Slint canvas has been verified on supported DPI layouts.
pub(super) fn render_initial_canvas(r: &mut Registry, index: usize) {
    let layer = &mut r.canvas_state.canvases[index];
    let raster_start = Instant::now();
    let Some(frame) = crate::annotation_render::render(
        &r.session,
        layer.bounds,
        layer.window.window().scale_factor(),
        None,
        None,
    ) else {
        r.close();
        return;
    };
    if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
        let elapsed = raster_start.elapsed();
        timing.raster += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].raster += elapsed;
    }
    let present_start = Instant::now();
    let presented = layer.presented.submit(frame, |frame| {
        (r.lifecycle.annotation_frame_presenter)(
            layer.host.window.window(),
            frame.data(),
            frame.width(),
            frame.height(),
        )
    });
    if let Some(timing) = r.canvas_state.cold_open_timing.as_mut() {
        let elapsed = present_start.elapsed();
        timing.native_present += elapsed;
        timing.largest_stage = timing.largest_stage.max(elapsed);
        timing.per_screen[index].present += elapsed;
    }
    if !presented {
        r.close();
        return;
    }
    r.canvas_state.canvases[index].signature = Some(render_signature(
        &r.session,
        r.canvas_state.canvases[index].bounds,
        r.canvas_state.canvases[index]
            .window
            .window()
            .scale_factor(),
        None,
        None,
    ));
    r.canvas_state.canvases[index].host.presented();
    r.canvas_state.canvases[index].bootstrap = CanvasBootstrap::Ready;
}

pub(super) fn settle_canvas(r: &Registry, index: usize, revision: u64, remaining: u8) {
    let generation = r.generation;
    r.canvas_state.canvases[index].position_after(Duration::from_millis(32), move || {
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let Some(r) = slot.as_ref().filter(|r| r.generation == generation) else {
                return;
            };
            let Some(layer) = r
                .canvas_state
                .canvases
                .get(index)
                .filter(|layer| layer.revision() == revision)
            else {
                return;
            };
            let bounds = layer.bounds;
            layer
                .window
                .window()
                .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
            layer.window.window().set_size(slint::PhysicalSize::new(
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
            ));
            if remaining > 0 {
                settle_canvas(r, index, revision, remaining - 1);
            }
        });
    });
}

pub(super) fn canvas_current(generation: u64, index: usize, revision: u64) -> bool {
    REGISTRY.with(|slot| {
        slot.borrow().as_ref().is_some_and(|r| {
            r.generation == generation
                && r.canvas_state
                    .canvases
                    .get(index)
                    .is_some_and(|layer| layer.revision() == revision)
        })
    })
}

pub(super) fn later_canvas(
    input: &Rc<window::InputWork>,
    generation: u64,
    index: usize,
    revision: u64,
    action: impl FnOnce() + 'static,
) {
    input.post(move || {
        if canvas_current(generation, index, revision) {
            action();
        }
    });
}

pub(super) fn pointer_canvas(
    weak: slint::Weak<AnnotationCanvas>,
    generation: u64,
    index: usize,
    revision: u64,
    input_work: Rc<window::InputWork>,
) -> crate::bridge::PopupPointerSink {
    registry_pointer(
        input_work,
        Rc::new(move |input| {
            if canvas_current(generation, index, revision)
                && let Some(window) = weak.upgrade()
            {
                if let PopupPointerInput::LeftCancelled { x, y } = input {
                    let scale = window.window().scale_factor().max(0.1);
                    window.invoke_pointer(3, x / scale, y / scale);
                }
                dispatch(window.window(), input);
            }
        }),
    )
}
