#[path = "screenshot_long_feedback.rs"]
mod feedback;
// Session-owned live range, bounded preview and controls. Capture and matching stay outside UI.
use super::*;
use crate::{LongCaptureConfirmation, LongCaptureControl, LongCapturePreview, LongCaptureRange};
use lexift_core::domain::long_capture::{
    Feedback, Preview,
    workflow::{Command, Event, Phase},
};

#[derive(Default)]
struct View {
    session: u64,
    operation: u64,
    control: window::Slot<LongCaptureControl>,
    confirmation: window::Slot<LongCaptureConfirmation>,
    preview: window::Slot<LongCapturePreview>,
    range: window::Slot<LongCaptureRange>,
    source: Rect,
    screen: Rect,
    area: Rect,
    presented: u64,
    committed: bool,
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
    editor: Option<slint::Weak<AnnotationCanvas>>,
    frozen: Option<Arc<CapturedRegion>>,
    original: Option<(Arc<Vec<CapturedRegion>>, Arc<ScreenCaptureFrame>)>,
    drag: Option<toolbar::Drag>,
    retired_canvases: Vec<CanvasLayer>,
    preview_size: (usize, usize),
    started: Option<std::time::Instant>,
    finish_started: Option<std::time::Instant>,
    notification: Option<(String, bool)>,
    notification_revision: u64,
}
thread_local! { static VIEW: RefCell<View> = RefCell::new(View::default()); }
fn send(event: Event) {
    emit(ScreenshotEvent::Long(event));
}
pub(crate) fn clear() {
    dismiss_notifications();
    super::long_editor::clear();
    VIEW.with(|v| *v.borrow_mut() = View::default());
}
/// Resolve hints only for the current visible control, never a retired capture owner.
pub(in crate::annotation) fn with_hint_window<T>(
    session: u64,
    operation: u64,
    revision: u64,
    f: impl FnOnce(&slint::Window) -> T,
) -> Option<T> {
    if !current(session, operation, revision)
        || FLOW.with(|f| {
            f.borrow()
                .state
                .long
                .as_ref()
                .is_none_or(|l| matches!(l.phase, Phase::Finishing | Phase::Editing))
        })
    {
        return None;
    }
    VIEW.with(|v| {
        let v = v.borrow();
        if v.drag.is_some() {
            return None;
        }
        v.control.get().map(|w| f(w.window()))
    })
}
pub(super) fn attach_notification(
    r: &Registry,
    window: &slint::Window,
    session: u64,
    operation: u64,
) -> bool {
    VIEW.with(|v| {
        let v = v.borrow();
        v.session == session
            && v.operation == operation
            && v.control
                .get()
                .is_some_and(|w| (r.lifecycle.attach_tool_window)(window, w.window()))
    })
}
fn dismiss_notifications() {
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            hints::clear_long(r);
        }
    });
    super::clear_long_feedback();
}
/// Only meaningful state transitions notify; progress frames never restart a toast timer.
fn notification(state: &ScreenshotState) -> Option<(String, bool)> {
    let l = state.long.as_ref()?;
    if state.phase != ScreenshotPhase::Editing || l.phase == Phase::Editing {
        return None;
    }
    if let Some(error) = state.error.as_ref() {
        return Some((error.clone(), true));
    }
    let (message, error) = if l.phase == Phase::Paused {
        ("Capture paused", false)
    } else if matches!(l.phase, Phase::Capturing | Phase::Frozen) {
        match l.feedback {
            Feedback::Uncertain => (
                "Cannot match this frame. Scroll back to the last captured area.",
                true,
            ),
            Feedback::Limit => ("Image limit reached. Finish the current capture.", true),
            _ => return None,
        }
    } else {
        return None;
    };
    Some((message.into(), error))
}
pub(in crate::annotation) fn image_editing() -> bool {
    FLOW.with(|f| {
        f.borrow().state.active()
            && f.borrow()
                .state
                .long
                .as_ref()
                .is_some_and(|l| matches!(l.phase, Phase::Editing | Phase::Frozen))
    })
}
pub(in crate::annotation) fn busy() -> bool {
    VIEW.with(|v| v.borrow().confirmation.get().is_some())
        || FLOW.with(|f| {
            let f = f.borrow();
            f.state.active()
                && f.state
                    .long
                    .as_ref()
                    .is_some_and(|l| l.phase != Phase::Editing)
        })
}
pub(in crate::annotation) fn available(r: &Registry) -> bool {
    r.document.mode.is_mouse()
        && !busy()
        && !image_editing()
        && r.screenshot.as_ref().is_some_and(|e| {
            e.pin.is_none()
                && !e.region.dragging()
                && e.region.bounds().is_some_and(|b| {
                    e.frames.iter().any(|f| {
                        b.left >= f.bounds.left
                            && b.top >= f.bounds.top
                            && b.right <= f.bounds.right
                            && b.bottom <= f.bounds.bottom
                    })
                })
        })
}
pub(in crate::annotation) fn request() {
    keyboard::commit_current();
    let start = REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut()?;
        if !available(r) {
            return None;
        }
        size_input::close(r);
        let e = r.screenshot.as_ref()?;
        Some((e.id, e.region.bounds()?, r.document.session.has_content()))
    });
    if let Some((session, bounds, confirm)) = start {
        if confirm {
            open_confirmation(session, bounds);
        } else {
            send(Event::Start { session, bounds });
        }
    }
}
fn valid(session: u64, operation: u64) -> bool {
    FLOW.with(|f| {
        let f = f.borrow();
        f.state.session == session
            && f.state.phase == ScreenshotPhase::Editing
            && (operation == 0 && f.state.long.is_none()
                || f.state
                    .long
                    .as_ref()
                    .is_some_and(|l| l.operation == operation))
    })
}
fn current(session: u64, operation: u64, revision: u64) -> bool {
    valid(session, operation)
        && VIEW.with(|v| {
            let v = v.borrow();
            v.session == session
                && v.operation == operation
                && if operation == 0 {
                    v.confirmation.revision() == revision
                } else {
                    v.control.revision() == revision
                }
        })
}
fn cancel(session: u64, operation: u64, revision: u64) {
    if current(session, operation, revision) {
        if operation == 0 {
            clear();
        } else {
            send(Event::Cancel { session, operation });
        }
    }
}
fn fail(session: u64, operation: u64) {
    if operation == 0 {
        clear();
    } else {
        send(Event::Finished {
            session,
            operation,
            result: Err(lexift_core::Error::new(crate::i18n::tr(
                "Long capture unavailable. Select a smaller scrolling area.",
            ))),
        });
    }
}
fn image(frame: &CapturedRegion) -> slint::Image {
    slint::Image::from_rgba8_premultiplied(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &frame.pixels,
            (frame.bounds.right - frame.bounds.left) as u32,
            (frame.bounds.bottom - frame.bounds.top) as u32,
        ),
    )
}
pub(crate) fn execute(command: Command) {
    match command {
        Command::Prepare {
            session,
            operation,
            bounds,
        } if valid(session, operation) => open_control(session, operation, bounds),
        Command::Present {
            session,
            operation,
            sequence,
            preview,
            ..
        } if valid(session, operation) && !image_editing() => {
            let first = VIEW.with(|v| {
                let mut v = v.borrow_mut();
                if v.session != session || v.operation != operation || sequence <= v.presented {
                    return None;
                }
                v.presented = sequence;
                let first = !v.committed;
                v.committed = true;
                Some(first)
            });
            let Some(first) = first else { return };
            if first {
                REGISTRY.with(|s| {
                    if let Some(r) = s.borrow_mut().as_mut() {
                        r.document.session = Session::new();
                        r.sync_edit_scope();
                    }
                });
            }
            present_preview(preview);
            if first {
                VIEW.with(|v| {
                    let mut v = v.borrow_mut();
                    if let Some(start) = v.started.take() {
                        tracing::debug!(
                            session,
                            operation,
                            elapsed_ms = start.elapsed().as_secs_f64() * 1000.,
                            "Long capture first feedback presented"
                        );
                    }
                });
            }
            sync();
        }
        Command::Restore { session, operation }
            if FLOW.with(|f| {
                let f = f.borrow();
                f.state.session == session
                    && f.state.phase == ScreenshotPhase::Editing
                    && f.state.long.is_none()
            }) =>
        {
            if !VIEW.with(|v| {
                let v = v.borrow();
                v.session == session && v.operation == operation
            }) {
                return;
            }
            let (original, source, retired) = VIEW.with(|v| {
                let mut v = v.borrow_mut();
                (
                    v.original.take(),
                    v.source,
                    std::mem::take(&mut v.retired_canvases),
                )
            });
            clear();
            REGISTRY.with(|s| {
                if let Some(r) = s.borrow_mut().as_mut() {
                    if let (Some((frames, capture)), Some(e)) = (original, r.screenshot.as_mut()) {
                        e.frames = frames;
                        e.capture = capture;
                        e.region = RegionEditor::new(source);
                        e.preview = None;
                        e.revision = e.revision.wrapping_add(1);
                        FLOW.with(|f| f.borrow_mut().frames = Some(e.frames.clone()));
                        r.document.session.displays = e.frames.iter().map(|f| f.bounds).collect();
                        r.lifecycle.annotation_screen_capture =
                            Some(Arc::new(crate::FrozenCapture::new(e.frames.clone())));
                    }
                    if !retired.is_empty() {
                        r.canvas_state.canvases = retired;
                    }
                    if let Some(w) = r.toolbar_state.main.get() {
                        w.set_image_edit_mode(false);
                    }
                    r.toolbar_state.toolbar_owner = None;
                    sync_toolbar_owner(r);
                    for c in &r.canvas_state.canvases {
                        let _ = c.window.window().show();
                    }
                    if let Some(w) = r.toolbar_state.main.get() {
                        let _ = w.show();
                    }
                    r.canvas_state.annotation_ready = true;
                    r.queue_render();
                }
            });
        }
        Command::Edit {
            session,
            operation,
            image,
            target,
        } if valid(session, operation) => {
            let frozen = FLOW.with(|f| {
                f.borrow()
                    .state
                    .long
                    .as_ref()
                    .is_some_and(|l| l.phase == Phase::Frozen)
            });
            if frozen {
                VIEW.with(|v| v.borrow_mut().frozen = Some(image.clone()));
                REGISTRY.with(|s| {
                    if let Some(r) = s.borrow_mut().as_mut()
                        && let Some(e) = r.screenshot.as_mut()
                    {
                        e.frames = Arc::new(vec![(*image).clone()]);
                        e.region = RegionEditor::new(image.bounds);
                        FLOW.with(|f| f.borrow_mut().frames = Some(e.frames.clone()));
                    }
                });
                choose(session, operation, target);
            } else {
                super::long_editor::open(session, operation, image);
            }
        }
        _ => {}
    }
}
fn choose(session: u64, operation: u64, target: Option<ScreenshotOutput>) {
    if target.is_none() {
        VIEW.with(|v| {
            v.borrow_mut()
                .finish_started
                .get_or_insert_with(std::time::Instant::now);
        });
    }
    let frozen = VIEW.with(|v| v.borrow().frozen.clone());
    if let Some(image) = frozen {
        if let Some(kind) = target {
            sync();
            output(kind);
        } else {
            send(Event::OpenEditor {
                session,
                operation,
                image,
            });
        }
    } else {
        send(Event::Finish {
            session,
            operation,
            target,
        });
    }
}
fn open_confirmation(session: u64, bounds: Rect) {
    clear();
    let created = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let w =
            crate::screen_interaction::create(&r.lifecycle, LongCaptureConfirmation::new).ok()?;
        crate::theme::apply(&w);
        w.set_confirmation(true);
        Some((w, r.lifecycle.annotation_foreground.as_ref().map(|q| q())))
    });
    let Some((w, origin)) = created else { return };
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.session = session;
        v.source = bounds;
        v.origin = origin;
        v.confirmation.open(w);
    });
    VIEW.with(|v| {
        let v = v.borrow();
        let w = v.confirmation.get().unwrap();
        let rev = v.confirmation.revision();
        let work = v.confirmation.input();
        w.on_cancel(move || work.post(move || cancel(session, 0, rev)));
        let work = v.confirmation.input();
        w.on_confirm(move || {
            work.post(move || {
                if current(session, 0, rev) {
                    clear();
                    send(Event::Start { session, bounds });
                }
            })
        });
    });
    prepare_confirmation(session, 0);
}
fn prepare_confirmation(session: u64, attempt: u8) {
    if !valid(session, 0) {
        return;
    }
    let result = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        VIEW.with(|v| {
            let v = v.borrow();
            let w = v.confirmation.get()?;
            let a = (r.lifecycle.popup_work_area)(Point {
                x: v.source.left,
                y: v.source.top,
            })?;
            let dpi = w.window().scale_factor();
            let width = (w.get_content_width() * dpi).min((a.right - a.left) as f32);
            let height = 168. * dpi;
            w.window().set_position(slint::PhysicalPosition::new(
                a.left + ((a.right - a.left - width as i32) / 2),
                a.top + ((a.bottom - a.top - height as i32) / 2),
            ));
            w.window()
                .set_size(slint::PhysicalSize::new(width as u32, height as u32));
            Some((
                (r.prepare)(w.window()),
                (r.lifecycle.configure_menu_window)(w.window()),
            ))
        })
    });
    if matches!(result, Some((PassiveWindowPreparation::Ready, true))) && attempt > 0 {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow().as_ref() {
                VIEW.with(|v| {
                    let v = v.borrow();
                    if let Some(w) = v.confirmation.get() {
                        let weak = w.as_weak();
                        let sink = v.confirmation.input().pointer_sink(move |e| {
                            if let Some(w) = weak.upgrade() {
                                crate::bridge::dispatch_pointer_input(w.window(), e);
                            }
                        });
                        let _ = w.show();
                        (r.lifecycle.complete_passive_window_show)(w.window(), sink);
                    }
                });
            }
        });
    } else if attempt < 20 && !matches!(result, None | Some((PassiveWindowPreparation::Failed, _)))
    {
        VIEW.with(|v| {
            v.borrow()
                .confirmation
                .prepare_after(Duration::from_millis(16), move || {
                    prepare_confirmation(session, attempt + 1)
                })
        });
    } else {
        clear();
    }
}
fn intersects(a: Rect, b: Rect) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}
/// Prefer external placement; the capture adapter excludes these owned windows even when clamped inside.
fn placement(source: Rect, areas: &[Rect], w: i32, h: i32, gap: i32) -> Option<Rect> {
    for a in areas {
        if w > a.right - a.left - 2 * gap || h > a.bottom - a.top - 2 * gap {
            continue;
        }
        let at = |x: i32, y: i32| {
            let x = x.clamp(a.left + gap, a.right - w - gap);
            let y = y.clamp(a.top + gap, a.bottom - h - gap);
            Rect {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            }
        };
        for (x, y) in [
            (source.left, source.bottom + gap),
            (source.left, source.top - h - gap),
        ] {
            let b = at(x, y);
            if !intersects(b, source) {
                return Some(b);
            }
        }
        return Some(at(source.left, source.bottom + gap));
    }
    None
}
fn open_control(session: u64, operation: u64, bounds: Rect) {
    clear();
    let started = std::time::Instant::now();
    let made = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        let screen = (r.lifecycle.annotation_displays)().into_iter().find(|a| {
            bounds.left >= a.left
                && bounds.top >= a.top
                && bounds.right <= a.right
                && bounds.bottom <= a.bottom
        })?;
        let area = (r.lifecycle.popup_work_area)(Point {
            x: bounds.left,
            y: bounds.top,
        })?;
        let w = crate::screen_interaction::create(&r.lifecycle, LongCaptureControl::new).ok()?;
        let preview =
            crate::screen_interaction::create(&r.lifecycle, LongCapturePreview::new).ok()?;
        let range = crate::screen_interaction::create(&r.lifecycle, LongCaptureRange::new).ok()?;
        crate::theme::apply(&w);
        crate::theme::apply(&preview);
        crate::theme::apply(&range);
        Some((
            w,
            preview,
            range,
            screen,
            area,
            r.lifecycle.annotation_foreground.as_ref().map(|q| q()),
        ))
    });
    let Some((w, preview, range, screen, area, origin)) = made else {
        fail(session, operation);
        return;
    };
    let original = REGISTRY.with(|s| {
        s.borrow()
            .as_ref()?
            .screenshot
            .as_ref()
            .map(|e| (e.frames.clone(), e.capture.clone()))
    });
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.session = session;
        v.operation = operation;
        v.started = Some(started);
        v.source = bounds;
        v.screen = screen;
        v.area = area;
        v.origin = origin;
        v.original = original;
        v.control.open(w);
        v.preview.open(preview);
        v.range.open(range);
    });
    VIEW.with(|v| {
        let v = v.borrow();
        let w = v.control.get().unwrap();
        let rev = v.control.revision();
        let generation = REGISTRY.with(|r| r.borrow().as_ref().unwrap().generation);
        hints::bind_long_control(
            w,
            generation,
            hints::Owner::LongControl {
                session,
                operation,
                revision: rev,
            },
        );
        let work = v.control.input();
        w.on_cancel(move || work.post(move || cancel(session, operation, rev)));
        let work = v.control.input();
        w.on_finish(move || {
            work.post(move || {
                if current(session, operation, rev) {
                    choose(session, operation, None);
                }
            })
        });
        let work = v.control.input();
        w.on_output(move |kind| {
            work.post(move || {
                if current(session, operation, rev) {
                    choose(
                        session,
                        operation,
                        Some(match kind {
                            1 => ScreenshotOutput::Save,
                            2 => ScreenshotOutput::Pin,
                            _ => ScreenshotOutput::Copy,
                        }),
                    );
                }
            })
        });
        let work = v.control.input();
        w.on_pause(move || {
            work.post(move || {
                if current(session, operation, rev) {
                    let paused = FLOW.with(|f| {
                        f.borrow()
                            .state
                            .long
                            .as_ref()
                            .is_some_and(|l| l.phase == Phase::Capturing)
                    });
                    send(Event::Pause {
                        session,
                        operation,
                        paused,
                    });
                }
            })
        });
        w.on_drag_requested(move |x, y| {
            if current(session, operation, rev) {
                REGISTRY.with(|s| {
                    if let Some(r) = s.borrow_mut().as_mut() {
                        r.hints.clear();
                    }
                });
                let window =
                    VIEW.with(|v| v.borrow().control.get().map(ComponentHandle::clone_strong));
                if let Some(window) = window {
                    // Native geometry queries may dispatch input; do not retain a View borrow.
                    let d = window.window().scale_factor();
                    let drag = toolbar::Drag::new(window.window().position(), x * d, y * d, d);
                    if current(session, operation, rev) {
                        VIEW.with(|v| v.borrow_mut().drag = Some(drag));
                    }
                }
            }
        });
        let work = v.control.input();
        w.window().on_close_requested(move || {
            work.post(move || cancel(session, operation, rev));
            slint::CloseRequestResponse::KeepWindowShown
        });
    });
    prepare(session, operation, 0);
}
fn prepare(session: u64, operation: u64, attempt: u8) {
    if !valid(session, operation) {
        return;
    }
    let prepared = REGISTRY.with(|s| {
        let s = s.borrow();
        let r = s.as_ref()?;
        VIEW.with(|v| {
            let mut v = v.borrow_mut();
            let w = v.control.get()?;
            let dpi = w.window().scale_factor().max(0.1);
            let a = v.area;
            let source = v.source;
            let screen = v.screen;
            let b = placement(
                source,
                &[a],
                (w.get_content_width() * dpi).ceil() as i32,
                (w.get_content_height() * dpi).ceil() as i32,
                (8. * dpi).ceil() as i32,
            )?;
            w.window()
                .set_position(slint::PhysicalPosition::new(b.left, b.top));
            w.window().set_size(slint::PhysicalSize::new(
                (b.right - b.left) as u32,
                (b.bottom - b.top) as u32,
            ));
            let range = v.range.get()?;
            let native = range.window();
            native.set_position(slint::PhysicalPosition::new(screen.left, screen.top));
            native.set_size(slint::PhysicalSize::new(
                (screen.right - screen.left) as u32,
                (screen.bottom - screen.top) as u32,
            ));
            let rd = native.scale_factor().max(0.1);
            range.set_selection_x((source.left - screen.left) as f32 / rd);
            range.set_selection_y((source.top - screen.top) as f32 / rd);
            range.set_selection_width((source.right - source.left) as f32 / rd);
            range.set_selection_height((source.bottom - source.top) as f32 / rd);
            let preview = v.preview.get()?;
            preview
                .window()
                .set_position(slint::PhysicalPosition::new(source.right + 8, source.top));
            preview
                .window()
                .set_size(slint::PhysicalSize::new((204. * dpi) as u32, 100));
            let states = [
                (r.prepare)(w.window()),
                (r.prepare)(range.window()),
                (r.prepare)(preview.window()),
            ];
            let owned =
                r.canvas_state.canvases.first().is_some_and(|c| {
                    (r.lifecycle.attach_tool_window)(w.window(), c.window.window())
                });
            let roles = owned
                && (r.lifecycle.configure_menu_window)(w.window())
                && (r.lifecycle.configure_hint_window)(range.window())
                && (r.lifecycle.configure_hint_window)(preview.window());
            let through = (r.lifecycle.configure_capture_feedback)(range.window())
                && (r.lifecycle.configure_capture_feedback)(preview.window());
            v.preview_size = (
                (200. * dpi).round() as usize,
                ((a.bottom - a.top) as f32 - 20. * dpi).max(1.) as usize,
            );
            Some((states, roles && through))
        })
    });
    if prepared.as_ref().is_some_and(|(states, roles)| {
        *roles && states.iter().all(|s| *s == PassiveWindowPreparation::Ready)
    }) && attempt > 0
    {
        let excluded = REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut()?;
            keyboard::finish(r);
            size_input::close(r);
            handoff::cancel(r);
            r.close_panel();
            r.hints.clear();
            r.toolbar_state.main.cancel_scheduled();
            for c in &r.canvas_state.canvases {
                let _ = c.window.window().hide();
            }
            if let Some(w) = r.toolbar_state.main.get() {
                let _ = w.hide();
            }
            r.canvas_state.annotation_ready = false;
            if let Some(e) = r.screenshot.as_mut() {
                e.preview = None;
                e.revision = e.revision.wrapping_add(1);
            }
            let mut excluded = capture_exclusions(r);
            VIEW.with(|v| {
                let v = v.borrow();
                let w = v.control.get()?;
                let range = v.range.get()?;
                let preview = v.preview.get()?;
                for native in [w.window(), range.window(), preview.window()] {
                    excluded.push((r.lifecycle.annotation_window_token)(native)?);
                }
                range.show().ok()?;
                let color = range.global::<crate::Colors>().get_primary();
                let frame = feedback::range(
                    v.screen,
                    v.source,
                    range.window().scale_factor(),
                    color,
                    range.global::<crate::Colors>().get_capture_mask(),
                )?;
                if !(r.lifecycle.annotation_frame_presenter)(
                    range.window(),
                    frame.data(),
                    frame.width(),
                    frame.height(),
                ) {
                    return None;
                }
                let weak = w.as_weak();
                let rev = v.control.revision();
                let work = v.control.input();
                let sink = work.pointer_sink(move |event| {
                    if !current(session, operation, rev) {
                        return;
                    }
                    if move_control(event) {
                        return;
                    }
                    if let Some(w) = weak.upgrade() {
                        crate::bridge::dispatch_pointer_input(w.window(), event);
                    }
                });
                w.show().ok()?;
                if !(r.lifecycle.complete_passive_window_show)(w.window(), sink) {
                    return None;
                }
                Some(())
            })?;
            Some(excluded)
        });
        if let Some(excluded) = excluded {
            let preview_size = VIEW.with(|v| v.borrow().preview_size);
            send(Event::Ready {
                session,
                operation,
                excluded,
                preview_size,
            });
        } else {
            fail(session, operation);
        }
    } else if attempt < 20
        && prepared.as_ref().is_some_and(|(states, _)| {
            states
                .iter()
                .all(|s| *s != PassiveWindowPreparation::Failed)
        })
    {
        VIEW.with(|v| {
            v.borrow()
                .control
                .prepare_after(Duration::from_millis(16), move || {
                    prepare(session, operation, attempt + 1)
                })
        });
    } else {
        fail(session, operation);
    }
}
fn move_control(event: PopupPointerInput) -> bool {
    let Some(lifecycle) = REGISTRY.with(|s| s.borrow().as_ref().map(|r| r.lifecycle.clone()))
    else {
        return false;
    };
    let Some((session, operation, revision, drag, window, area)) = VIEW.with(|v| {
        let v = v.borrow();
        Some((
            v.session,
            v.operation,
            v.control.revision(),
            v.drag?,
            v.control.get()?.clone_strong(),
            v.area,
        ))
    }) else {
        return false;
    };
    if !current(session, operation, revision) {
        return false;
    }
    match event {
        PopupPointerInput::Moved { .. } | PopupPointerInput::LeftReleased { .. } => {
            if let Some(p) = (lifecycle.toolbar_cursor_position)() {
                let dpi = window.window().scale_factor().max(0.1);
                let raw = drag.position(slint::PhysicalPosition::new(p.x, p.y), dpi);
                let area = (lifecycle.popup_work_area)(p).unwrap_or(area);
                let size = slint::PhysicalSize::new(
                    (window.get_content_width() * dpi) as u32,
                    (window.get_content_height() * dpi) as u32,
                );
                // WM_SIZE / pointer messages can reenter this route. Keep both registries unborrowed.
                window.window().set_size(size);
                if current(session, operation, revision) {
                    window.window().set_position(slint::PhysicalPosition::new(
                        raw.x
                            .clamp(area.left, (area.right - size.width as i32).max(area.left)),
                        raw.y
                            .clamp(area.top, (area.bottom - size.height as i32).max(area.top)),
                    ));
                }
            }
        }
        _ => {}
    }
    if matches!(
        event,
        PopupPointerInput::LeftReleased { .. } | PopupPointerInput::LeftCancelled { .. }
    ) && current(session, operation, revision)
    {
        VIEW.with(|v| v.borrow_mut().drag = None);
    }
    matches!(event, PopupPointerInput::Moved { .. })
}
fn present_preview(p: Preview) {
    REGISTRY.with(|s| {
        let s = s.borrow();
        let Some(r) = s.as_ref() else { return };
        VIEW.with(|v| {
            let v = v.borrow();
            let Some(w) = v.preview.get() else { return };
            let d = w.window().scale_factor().max(0.1);
            let gap = (8. * d) as i32;
            let a = v.area;
            let frame = feedback::preview(
                &p,
                d,
                w.global::<crate::Colors>().get_primary(),
                w.global::<crate::Colors>().get_success(),
            );
            let Some(frame) = frame else { return };
            let width = frame.width() as i32;
            let height = frame.height() as i32;
            let src = v.source;
            let x = if src.right + gap + width <= a.right {
                src.right + gap
            } else if src.left - gap - width >= a.left {
                src.left - gap - width
            } else {
                (src.right - width - gap).clamp(a.left, a.right - width)
            };
            let y = (src.bottom - height)
                .clamp(a.top + gap, (a.bottom - height - gap).max(a.top + gap));
            w.window().set_position(slint::PhysicalPosition::new(x, y));
            w.window()
                .set_size(slint::PhysicalSize::new(width as u32, height as u32));
            w.set_preview(image(&p.image));
            let span = (p.visible.bottom - p.visible.top).max(1) as f32;
            w.set_current_top((p.current.top.max(p.visible.top) - p.visible.top) as f32 / span);
            w.set_current_height(
                (p.current.bottom - p.current.top.max(p.visible.top)).max(0) as f32 / span,
            );
            w.set_clipped(p.visible.top > 0);
            if !w.window().is_visible() {
                let _ = w.show();
                (r.lifecycle.configure_capture_feedback)(w.window());
            }
            (r.lifecycle.annotation_frame_presenter)(
                w.window(),
                frame.data(),
                frame.width(),
                frame.height(),
            );
        });
    });
}
pub(in crate::annotation) fn sync() {
    let state = FLOW.with(|f| f.borrow().state.clone());
    let next = notification(&state);
    let changed = VIEW.with(|slot| {
        let mut v = slot.borrow_mut();
        if v.notification == next {
            None
        } else {
            v.notification = next.clone();
            v.notification_revision = v.notification_revision.wrapping_add(1);
            Some((v.notification_revision, v.control.revision()))
        }
    });
    if state.phase == ScreenshotPhase::Output
        || state
            .long
            .as_ref()
            .is_none_or(|l| l.phase == Phase::Editing)
    {
        dismiss_notifications();
    } else if let Some((version, control)) = changed {
        if let Some((message, error)) = next {
            if let Some(l) = state.long.as_ref() {
                let (session, operation) = (state.session, l.operation);
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if current(session, operation, control)
                        && VIEW.with(|v| v.borrow().notification_revision == version)
                    {
                        super::show_long_feedback(session, operation, &message, error);
                    }
                });
            }
        } else {
            super::clear_long_feedback();
        }
    }
    VIEW.with(|slot| {
        let v = slot.borrow();
        let (Some(l), Some(w)) = (state.long.as_ref(), v.control.get()) else {
            return;
        };
        if l.operation != v.operation {
            return;
        }
        w.set_ready(l.sequence > 0);
        w.set_paused(l.phase == Phase::Paused);
        w.set_frozen(l.phase == Phase::Frozen);
        w.set_finishing(l.phase == Phase::Finishing || state.phase == ScreenshotPhase::Output);
        w.set_dimensions(format!("{} × {} px", l.width, l.height).into());
        if state.phase == ScreenshotPhase::Output {
            let _ = w.hide();
            if let Some(range) = v.range.get() {
                let _ = range.hide();
            }
            if let Some(preview) = v.preview.get() {
                let _ = preview.hide();
            }
        } else if l.phase == Phase::Frozen && !w.window().is_visible() {
            // A cancelled dialog or failed output retains the frozen result, not a capture worker.
            if let Some(range) = v.range.get() {
                let _ = range.show();
            }
            if let Some(preview) = v.preview.get() {
                let _ = preview.show();
            }
            let _ = w.show();
        }
    });
}
pub(in crate::annotation) fn source_area() -> Option<Rect> {
    VIEW.with(|v| {
        let v = v.borrow();
        (v.operation != 0).then_some(v.area)
    })
}
pub(in crate::annotation) fn set_editor(window: &AnnotationCanvas) {
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        v.editor = Some(window.as_weak());
    });
}
pub(in crate::annotation) fn editor() -> Option<AnnotationCanvas> {
    VIEW.with(|v| v.borrow().editor.as_ref().and_then(|w| w.upgrade()))
}

/// Retire capture UI only after the replacement renderer has produced the feedback frame.
pub(in crate::annotation) fn editor_presented(session: u64) {
    if !FLOW.with(|f| {
        let f = f.borrow();
        f.state.session == session
            && f.state.phase == ScreenshotPhase::Editing
            && f.state
                .long
                .as_ref()
                .is_some_and(|l| l.phase == Phase::Editing)
    }) {
        return;
    }
    let pending = VIEW.with(|v| {
        let v = v.borrow();
        if v.session != session || v.control.get().is_none() {
            return None;
        }
        Some((v.editor.as_ref()?.upgrade()?, v.origin))
    });
    let Some((w, origin)) = pending else { return };
    if w.window().take_snapshot().is_err() {
        return;
    }
    REGISTRY.with(|s| {
        if let Some(r) = s.borrow().as_ref()
            && origin
                .zip(r.lifecycle.annotation_foreground.as_ref().map(|q| q()))
                .is_some_and(|(a, b)| a.is_some() && a == b)
        {
            (r.lifecycle.activate_user_requested_window)(w.window());
            w.invoke_focus_input();
        }
    });
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        if v.session == session {
            if let Some(start) = v.finish_started.take() {
                tracing::debug!(
                    session,
                    elapsed_ms = start.elapsed().as_secs_f64() * 1000.,
                    "Long image editor first feedback presented"
                );
            }
            v.control.clear();
            v.range.clear();
            v.preview.clear();
            v.frozen = None;
            v.drag = None;
            v.origin = None;
            v.retired_canvases.clear();
        }
    });
}
pub(in crate::annotation) fn editor_origin(
    origin: Option<Option<lexift_core::ports::magnifier::WindowToken>>,
) {
    VIEW.with(|v| v.borrow_mut().origin = origin);
}

pub(in crate::annotation) fn retain_canvases(old: Vec<CanvasLayer>) {
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        if v.retired_canvases.is_empty() {
            v.retired_canvases = old;
        }
    });
}
pub(in crate::annotation) fn editor_unavailable(session: u64, operation: u64) {
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        if v.session == session && v.operation == operation {
            v.finish_started = None;
        }
    });
    send(Event::EditorUnavailable { session, operation });
}

pub(in crate::annotation) fn frozen() -> bool {
    FLOW.with(|f| {
        f.borrow()
            .state
            .long
            .as_ref()
            .is_some_and(|l| l.phase == Phase::Frozen)
    })
}

pub(in crate::annotation) fn take_retired_canvases() -> Vec<CanvasLayer> {
    VIEW.with(|v| std::mem::take(&mut v.borrow_mut().retired_canvases))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{TestPlatform, lifecycle};
    use super::*;
    fn fixture() -> (Rc<RefCell<Vec<ScreenshotEvent>>>, Rect) {
        fixture_on(Box::new(TestPlatform))
    }
    fn fixture_on(
        platform: Box<dyn slint::platform::Platform>,
    ) -> (Rc<RefCell<Vec<ScreenshotEvent>>>, Rect) {
        slint::platform::set_platform(platform).unwrap();
        let screen = Rect {
            left: 0,
            top: 0,
            right: 1200,
            bottom: 900,
        };
        let mut life = lifecycle();
        life.annotation_displays = Rc::new(move || vec![screen]);
        life.popup_work_area = Rc::new(move |_| Some(screen));
        crate::annotation::init(|_| PassiveWindowPreparation::Ready, life);
        let events = Rc::new(RefCell::new(vec![]));
        let copied = events.clone();
        crate::annotation::set_handler(Rc::new(move |event| {
            if let lexift_core::AppEvent::Screenshot(e) = event {
                copied.borrow_mut().push(e);
            }
        }));
        let bounds = Rect {
            left: 100,
            top: 100,
            right: 300,
            bottom: 500,
        };
        FLOW.with(|f| {
            f.borrow_mut().state = lexift_core::domain::screenshot::ScreenshotState {
                session: 81,
                phase: ScreenshotPhase::Editing,
                bounds: Some(bounds),
                layout: vec![screen],
                ..Default::default()
            }
        });
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.generation = 9;
            r.document.mode = InteractionMode::Mouse;
            r.toolbar_state.main.open(AnnotationToolbar::new().unwrap());
            let w = AnnotationCanvas::new().unwrap();
            w.window().set_size(slint::PhysicalSize::new(1200, 900));
            r.canvas_state.canvases.push(CanvasLayer::new(w, screen));
            let frames = Arc::new(vec![CapturedRegion {
                bounds: screen,
                pixels: [120, 130, 140, 255].repeat(1200 * 900),
            }]);
            r.screenshot = Some(Editing {
                pin: None,
                id: 81,
                frames: frames.clone(),
                capture: Arc::new(ScreenCaptureFrame {
                    revision: 1,
                    serial: 1,
                    regions: frames.as_ref().clone(),
                }),
                region: RegionEditor::new(bounds),
                preview: None,
                revision: 0,
                presented: 0,
                feedback_placement: None,
                toolbar_adjustment: 0,
                keyboard: Default::default(),
                initial_focus: None,
                help: Default::default(),
                size_input: Default::default(),
            });
        });
        (events, bounds)
    }
    fn pump() {
        for _ in 0..30 {
            slint::platform::update_timers_and_animations();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn control_hints_and_notifications_have_delay_deduplication_and_retired_ownership() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::cell::Cell;
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        let (_, bounds) = fixture_on(Box::new(Clock(clock.clone())));
        let advance = |ms| {
            clock.set(Duration::from_millis(ms));
            slint::platform::update_timers_and_animations();
        };
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.lifecycle.configure_hint_window = Rc::new(|_| true);
            r.lifecycle.attach_tool_window = Rc::new(|_, _| true);
            r.lifecycle.complete_toolbar_show = Rc::new(|_, _| true);
        });
        FLOW.with(|f| {
            f.borrow_mut().state.long = Some(lexift_core::domain::long_capture::workflow::State {
                operation: 3,
                source: bounds,
                phase: Phase::Capturing,
                sequence: 1,
                width: 200,
                height: 400,
                feedback: Feedback::Unchanged,
                target: None,
            })
        });
        REGISTRY.with(|r| {
            let mut r = r.borrow_mut();
            let r = r.as_mut().unwrap();
            r.lifecycle.configure_capture_feedback = Rc::new(|_| true);
            r.lifecycle.annotation_frame_presenter = Rc::new(|_, pixels, width, height| {
                assert_eq!(pixels.len(), width as usize * height as usize * 4);
                assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| p[..3].iter().all(|c| *c <= p[3]))
            });
        });
        let control = LongCaptureControl::new().unwrap();
        control.show().unwrap();
        let rev = VIEW.with(|v| {
            let mut v = v.borrow_mut();
            v.session = 81;
            v.operation = 3;
            v.area = Rect {
                left: 0,
                top: 0,
                right: 1200,
                bottom: 900,
            };
            v.control.open(control.clone_strong())
        });
        hints::bind_long_control(
            &control,
            9,
            hints::Owner::LongControl {
                session: 81,
                operation: 3,
                revision: rev,
            },
        );
        let hint_visible = || {
            REGISTRY.with(|r| {
                r.borrow()
                    .as_ref()
                    .unwrap()
                    .hints
                    .window
                    .get()
                    .is_some_and(|w| w.window().is_visible())
            })
        };
        control.global::<crate::AnnotationHints>().invoke_requested(
            "Copy image".into(),
            160.,
            4.,
            32.,
            32.,
        );
        advance(0);
        advance(999);
        assert!(!hint_visible());
        advance(1000);
        assert!(hint_visible());
        control
            .global::<crate::AnnotationHints>()
            .invoke_cancelled(160., 4.);
        advance(1001);
        assert!(!hint_visible());
        sync();
        advance(1002);
        assert!(FLOW.with(|f| f.borrow().toast.get().is_none()));
        FLOW.with(|f| f.borrow_mut().state.long.as_mut().unwrap().phase = Phase::Paused);
        sync();
        advance(1010);
        advance(1030);
        advance(1050);
        let toast_visible = || {
            FLOW.with(|f| {
                f.borrow()
                    .toast
                    .get()
                    .is_some_and(|w| w.window().is_visible())
            })
        };
        assert!(toast_visible());
        let revision = FLOW.with(|f| f.borrow().toast.revision());
        for _ in 0..20 {
            sync();
        }
        advance(1060);
        assert_eq!(FLOW.with(|f| f.borrow().toast.revision()), revision);
        advance(4051);
        assert!(!toast_visible());
        sync();
        advance(4060);
        assert!(!toast_visible());
        FLOW.with(|f| f.borrow_mut().state.long.as_mut().unwrap().phase = Phase::Capturing);
        sync();
        FLOW.with(|f| f.borrow_mut().state.long.as_mut().unwrap().phase = Phase::Paused);
        sync();
        // A resume received before a deferred pause notification invalidates it.
        FLOW.with(|f| f.borrow_mut().state.long.as_mut().unwrap().phase = Phase::Capturing);
        sync();
        advance(4070);
        assert!(!toast_visible());
        FLOW.with(|f| f.borrow_mut().state.long.as_mut().unwrap().feedback = Feedback::Uncertain);
        sync();
        advance(4080);
        advance(4100);
        advance(4120);
        assert!(toast_visible());
        FLOW.with(|f| f.borrow_mut().state.phase = ScreenshotPhase::Output);
        sync();
        assert!(!toast_visible());
        // Delayed input from a retained, replaced control must never resurrect its hint.
        FLOW.with(|f| f.borrow_mut().state.phase = ScreenshotPhase::Editing);
        VIEW.with(|v| {
            v.borrow_mut()
                .control
                .open(LongCaptureControl::new().unwrap());
        });
        control.global::<crate::AnnotationHints>().invoke_requested(
            "retired".into(),
            160.,
            4.,
            32.,
            32.,
        );
        advance(4200);
        advance(5300);
        assert!(!hint_visible());
        let revision = VIEW.with(|v| {
            let mut v = v.borrow_mut();
            v.drag = Some(toolbar::Drag::new(
                slint::PhysicalPosition::new(0, 0),
                0.,
                0.,
                1.,
            ));
            v.control.revision()
        });
        let queried = Rc::new(Cell::new(false));
        let observed = queried.clone();
        REGISTRY.with(|r| {
            r.borrow_mut()
                .as_mut()
                .unwrap()
                .lifecycle
                .toolbar_cursor_position = Rc::new(move || {
                // Native geometry bridges may reenter input and hint ownership resolution.
                assert!(current(81, 3, revision));
                assert!(REGISTRY.with(|r| r.try_borrow_mut().is_ok()));
                assert!(VIEW.with(|v| v.try_borrow_mut().is_ok()));
                observed.set(true);
                Some(Point { x: 40, y: 80 })
            });
        });
        assert!(move_control(PopupPointerInput::Moved { x: 40., y: 80. }));
        assert!(queried.get());
        move_control(PopupPointerInput::LeftReleased { x: 40., y: 80. });
        assert!(VIEW.with(|v| v.borrow().drag.is_none()));
        clear();
        assert!(FLOW.with(|f| f.borrow().toast.get().is_none()));
    }
    #[test]
    fn live_range_preview_and_direct_output_share_revision_and_retire_callbacks() {
        let (events, bounds) = fixture();
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.lifecycle.configure_hint_window = Rc::new(|_| true);
            r.lifecycle.configure_capture_feedback = Rc::new(|_| true);
            r.lifecycle.annotation_click_through = Rc::new(|_, _| true);
            r.lifecycle.annotation_window_token =
                Rc::new(|_| Some(lexift_core::ports::magnifier::WindowToken(91)));
        });
        FLOW.with(|f| {
            f.borrow_mut().state.long = Some(lexift_core::domain::long_capture::workflow::State {
                operation: 3,
                source: bounds,
                phase: Phase::Preparing,
                sequence: 0,
                width: 0,
                height: 0,
                feedback: Feedback::Unchanged,
                target: None,
            })
        });
        execute(Command::Prepare {
            session: 81,
            operation: 3,
            bounds,
        });
        pump();
        assert!(events.borrow().iter().any(|e|matches!(e,ScreenshotEvent::Long(Event::Ready{preview_size,..}) if preview_size.0>0&&preview_size.1>0)));
        VIEW.with(|v| {
            let v = v.borrow();
            assert!(v.range.get().unwrap().window().is_visible());
            assert!(!v.preview.get().unwrap().window().is_visible());
            assert!(v.control.get().unwrap().get_content_height() < 100.);
        });
        FLOW.with(|f| {
            let mut f = f.borrow_mut();
            let l = f.state.long.as_mut().unwrap();
            l.phase = Phase::Capturing;
            l.sequence = 1;
            l.width = 200;
            l.height = 800;
        });
        let image = Arc::new(CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 200,
                bottom: 800,
            },
            pixels: vec![255; 200 * 800 * 4],
        });
        let preview = Preview {
            image: Arc::new(CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            }),
            extent: image.bounds,
            visible: Rect {
                top: 600,
                ..image.bounds
            },
            current: Rect {
                top: 700,
                ..image.bounds
            },
        };
        execute(Command::Present {
            session: 81,
            operation: 3,
            sequence: 1,
            first: true,
            preview: preview.clone(),
        });
        let old = VIEW.with(|v| {
            let v = v.borrow();
            assert!(v.preview.get().unwrap().window().is_visible());
            assert_eq!(v.preview.get().unwrap().get_current_top(), 0.5);
            v.control.get().unwrap().clone_strong()
        });
        execute(Command::Present {
            session: 81,
            operation: 3,
            sequence: 0,
            first: true,
            preview,
        });
        VIEW.with(|v| assert_eq!(v.borrow().presented, 1));
        old.invoke_output(1);
        pump();
        assert!(events.borrow().iter().any(|e| matches!(
            e,
            ScreenshotEvent::Long(Event::Finish {
                target: Some(ScreenshotOutput::Save),
                ..
            })
        )));
        FLOW.with(|f| {
            let mut f = f.borrow_mut();
            f.state.long.as_mut().unwrap().phase = Phase::Frozen;
            f.state.bounds = Some(image.bounds);
        });
        execute(Command::Edit {
            session: 81,
            operation: 3,
            image,
            target: Some(ScreenshotOutput::Copy),
        });
        pump();
        assert!(events.borrow().iter().any(|e|matches!(e,ScreenshotEvent::Output{kind:ScreenshotOutput::Copy,request,..} if request.snapshot.monitor.bottom==800&&request.snapshot.scale==1.)));
        let count = events.borrow().len();
        clear();
        old.invoke_output(2);
        pump();
        assert_eq!(events.borrow().len(), count);
        crate::annotation::shutdown();
    }
    #[test]
    fn presented_long_image_revision_rejects_late_preview_failure() {
        let (events, bounds) = fixture();
        FLOW.with(|f| {
            f.borrow_mut().state.long = Some(lexift_core::domain::long_capture::workflow::State {
                operation: 3,
                source: bounds,
                phase: Phase::Editing,
                sequence: 3,
                width: 200,
                height: 800,
                feedback: Feedback::Accepted,
                target: None,
            })
        });
        let w = AnnotationCanvas::new().unwrap();
        set_editor(&w);
        let (generation, revision) = REGISTRY.with(|s| {
            let s = s.borrow();
            let r = s.as_ref().unwrap();
            (r.generation, r.canvas_state.canvases[0].revision())
        });
        super::super::super::frozen_editor::present(
            generation,
            81,
            11,
            Some(vec![
                tiny_skia::Pixmap::new(1, 1).unwrap(),
                tiny_skia::Pixmap::new(1, 1).unwrap(),
            ]),
            vec![(revision, None)],
        );
        REGISTRY.with(|s| {
            assert_eq!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .screenshot
                    .as_ref()
                    .unwrap()
                    .presented,
                11
            )
        });
        super::super::super::frozen_editor::present(
            generation,
            81,
            10,
            None,
            vec![(revision, None)],
        );
        assert!(
            !events
                .borrow()
                .iter()
                .any(|e| matches!(e, ScreenshotEvent::Failed { .. }))
        );
        clear();
        crate::annotation::shutdown();
    }
    #[test]
    fn retired_desktop_toolbar_owner_cannot_index_replacement_image_canvas() {
        fixture();
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.toolbar_state.toolbar_owner = Some((1, 999, 999));
            // No physical toolbar hit: this must exercise the stale cached-owner fallback.
            r.canvas_state.canvases = vec![CanvasLayer::new(
                AnnotationCanvas::new().unwrap(),
                Rect {
                    left: 4000,
                    top: 4000,
                    right: 4600,
                    bottom: 6000,
                },
            )];
            assert_eq!(r.canvas_state.canvases.len(), 1);
            assert!(sync_toolbar_owner(r));
            assert_eq!(r.toolbar_state.toolbar_owner.unwrap().0, 0);
        });
        clear();
        crate::annotation::shutdown();
    }
    #[test]
    fn confirmation_does_not_clear_until_first_frame_and_old_callbacks_retire() {
        let (events, bounds) = fixture();
        REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            r.document
                .session
                .add(super::super::tests::object(Bounds::from_corners(
                    (120., 120.),
                    (160., 160.),
                )));
        });
        request();
        pump();
        assert!(busy());
        let (w, revision) = VIEW.with(|v| {
            let v = v.borrow();
            (
                v.confirmation.get().unwrap().clone_strong(),
                v.confirmation.revision(),
            )
        });
        assert!(w.get_confirmation());
        assert!(
            events
                .borrow()
                .iter()
                .all(|e| !matches!(e, ScreenshotEvent::Long(_)))
        );
        w.invoke_cancel();
        pump();
        assert!(!busy());
        REGISTRY.with(|s| assert!(s.borrow().as_ref().unwrap().document.session.has_content()));
        w.invoke_confirm();
        pump();
        assert!(
            events
                .borrow()
                .iter()
                .all(|e| !matches!(e, ScreenshotEvent::Long(_))),
            "retired confirmation cannot start a new capture"
        );
        assert!(!current(81, 0, revision));
        // A new confirmed start still leaves the original document until a successful fresh frame.
        request();
        pump();
        VIEW.with(|v| v.borrow().confirmation.get().unwrap().invoke_confirm());
        pump();
        assert!(
            matches!(events.borrow().last(),Some(ScreenshotEvent::Long(Event::Start{session:81,bounds:b})) if *b==bounds)
        );
        clear();
        crate::annotation::shutdown();
    }
    #[test]
    fn stale_progress_and_restore_cannot_replace_new_session_or_document() {
        let (_events, bounds) = fixture();
        let state = lexift_core::domain::long_capture::workflow::State {
            operation: 3,
            source: bounds,
            phase: Phase::Capturing,
            sequence: 3,
            width: 200,
            height: 800,
            feedback: Feedback::Accepted,
            target: None,
        };
        FLOW.with(|f| f.borrow_mut().state.long = Some(state));
        VIEW.with(|v| {
            let mut v = v.borrow_mut();
            v.session = 81;
            v.operation = 3;
            v.presented = 3;
            v.committed = true;
        });
        let thumbnail = Arc::new(CapturedRegion {
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            pixels: vec![255; 4],
        });
        execute(Command::Present {
            session: 81,
            operation: 3,
            sequence: 2,
            first: true,
            preview: Preview {
                image: thumbnail.clone(),
                extent: thumbnail.bounds,
                visible: thumbnail.bounds,
                current: thumbnail.bounds,
            },
        });
        VIEW.with(|v| assert_eq!(v.borrow().presented, 3));
        FLOW.with(|f| {
            let mut f = f.borrow_mut();
            f.state.long.as_mut().unwrap().phase = Phase::Editing;
        });
        execute(Command::Present {
            session: 81,
            operation: 3,
            sequence: 4,
            first: true,
            preview: Preview {
                image: thumbnail.clone(),
                extent: thumbnail.bounds,
                visible: thumbnail.bounds,
                current: thumbnail.bounds,
            },
        });
        VIEW.with(|v| assert_eq!(v.borrow().presented, 3));
        execute(Command::Restore {
            session: 81,
            operation: 3,
        });
        assert!(
            image_editing(),
            "old restore cannot clear the result editor"
        );
        let (generation, old_canvas) = REGISTRY.with(|s| {
            let mut s = s.borrow_mut();
            let r = s.as_mut().unwrap();
            let old = r.canvas_state.canvases[0].revision();
            let w = AnnotationCanvas::new().unwrap();
            r.canvas_state.canvases = vec![CanvasLayer::new(w, bounds)];
            (r.generation, old)
        });
        super::super::super::frozen_editor::present(
            generation,
            81,
            100,
            None,
            vec![(old_canvas, None)],
        );
        check_display_layout();
        REGISTRY.with(|s| {
            assert!(
                s.borrow().as_ref().unwrap().screenshot.is_some(),
                "image-space bounds are not a changed monitor layout"
            )
        });
        assert!(
            !_events
                .borrow()
                .iter()
                .any(|e| matches!(e, ScreenshotEvent::Failed { .. })),
            "failed old canvas previews cannot close the replacement editor"
        );
        clear();
        crate::annotation::shutdown();
    }
    #[test]
    fn controls_render_every_language_theme_and_dpi() {
        use slint::platform::{
            Platform, PointerEventButton, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Backend(Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for Backend {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
                self.0.borrow_mut().push(w.clone());
                Ok(w)
            }
        }
        let windows = Rc::new(RefCell::new(vec![]));
        slint::platform::set_platform(Box::new(Backend(windows.clone()))).unwrap();
        let w = LongCaptureControl::new().unwrap();
        let adapter = windows.borrow()[0].clone();
        let confirmation = LongCaptureConfirmation::new().unwrap();
        let confirm_adapter = windows.borrow()[1].clone();
        let editor = AnnotationCanvas::new().unwrap();
        let editor_adapter = windows.borrow()[2].clone();
        editor.set_image_view(true);
        let mut image = vec![255u8; 600 * 2000 * 4];
        for y in 0..2000 {
            if y % 100 < 2 {
                for x in 0..600 {
                    let i = (y * 600 + x) * 4;
                    image[i..i + 3].copy_from_slice(&[80, 100, 140]);
                }
            }
        }
        editor.set_frame(slint::Image::from_rgba8_premultiplied(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&image, 600, 2000),
        ));
        let dragged = Rc::new(std::cell::Cell::new(0));
        let observed = dragged.clone();
        editor.on_header_drag_requested(move || observed.set(observed.get() + 1));
        let draw = |adapter: &Rc<MinimalSoftwareWindow>, name: String, width: u32, height: u32| {
            adapter.set_size(slint::PhysicalSize::new(width, height));
            let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
            adapter.request_redraw();
            adapter.draw_if_needed(|r| {
                r.render(&mut pixels, width as usize);
            });
            assert!(pixels.windows(2).any(|p| p[0] != p[1]));
            if let Ok(directory) = std::env::var("LEXIFT_LONG_RENDER_DIR") {
                crate::i18n::write_test_bitmap(
                    &directory,
                    &name,
                    width,
                    height,
                    pixels.iter().map(|p| [p.r, p.g, p.b, 255]),
                );
            }
        };
        w.set_ready(true);
        w.set_dimensions("600 × 2400 px".into());
        for language in lexift_core::domain::ui_language::UiLanguage::ALL {
            crate::i18n::select(language);
            for dark in [false, true] {
                w.global::<crate::Colors>().set_preference(i32::from(dark));
                confirmation
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                editor
                    .global::<crate::Colors>()
                    .set_preference(i32::from(dark));
                for scale in [1., 1.25] {
                    for adapter in [&adapter, &confirm_adapter, &editor_adapter] {
                        adapter
                            .window()
                            .dispatch_event(WindowEvent::ScaleFactorChanged {
                                scale_factor: scale,
                            });
                    }
                    draw(
                        &adapter,
                        format!("control-{}-{dark}-{scale}", language.code()),
                        (w.get_content_width() * scale) as u32,
                        (w.get_content_height() * scale) as u32,
                    );
                    let fixed_width = w.get_content_width();
                    assert_eq!(w.get_content_height(), 40.);
                    for (name, dimensions, paused, finishing, ready) in [
                        ("maximum", "32000000 × 1 px", false, false, true),
                        ("paused", "32768 × 32768 px", true, false, true),
                        ("busy", "600 × 2400 px", false, true, true),
                        ("preparing", "0 × 0 px", false, false, false),
                    ] {
                        w.set_dimensions(dimensions.into());
                        w.set_paused(paused);
                        w.set_finishing(finishing);
                        w.set_ready(ready);
                        assert_eq!(w.get_content_width(), fixed_width);
                        draw(
                            &adapter,
                            format!("control-{name}-{}-{dark}-{scale}", language.code()),
                            (fixed_width * scale) as u32,
                            (40. * scale) as u32,
                        );
                    }
                    w.set_ready(true);
                    w.set_paused(false);
                    w.set_finishing(false);
                    w.set_dimensions("600 × 2400 px".into());
                    draw(
                        &confirm_adapter,
                        format!("confirmation-{}-{dark}-{scale}", language.code()),
                        (confirmation.get_content_width() * scale) as u32,
                        (confirmation.get_content_height() * scale) as u32,
                    );
                    editor.set_image_pixel_scale(scale);
                    editor_adapter.set_size(slint::PhysicalSize::new(
                        (960. * scale) as u32,
                        (760. * scale) as u32,
                    ));
                    crate::annotation::image_navigation::bind(&editor);
                    editor.invoke_fit_image();
                    draw(
                        &editor_adapter,
                        format!("editor-{}-{dark}-{scale}", language.code()),
                        (960. * scale) as u32,
                        (760. * scale) as u32,
                    );
                    assert!(editor.get_image_zoom() * scale <= 1.);
                    let before = dragged.get();
                    editor.window().dispatch_event(WindowEvent::PointerPressed {
                        position: slint::LogicalPosition::new(200., 16.),
                        button: PointerEventButton::Left,
                    });
                    editor
                        .window()
                        .dispatch_event(WindowEvent::PointerReleased {
                            position: slint::LogicalPosition::new(200., 16.),
                            button: PointerEventButton::Left,
                        });
                    assert_eq!(
                        dragged.get(),
                        before + 1,
                        "the visible top title bar must own its drag hit target"
                    );
                }
            }
        }
    }
    #[test]
    fn controls_never_overlap_capture_even_at_edges() {
        let area = Rect {
            left: -1600,
            top: 0,
            right: 0,
            bottom: 900,
        };
        for s in [1., 1.25] {
            for source in [
                Rect {
                    left: -1300,
                    top: 100,
                    right: -700,
                    bottom: 800,
                },
                Rect {
                    left: -1600,
                    top: 0,
                    right: -1000,
                    bottom: 600,
                },
            ] {
                let b = placement(
                    source,
                    &[area],
                    (464. * s) as i32,
                    (72. * s) as i32,
                    (8. * s) as i32,
                )
                .unwrap();
                assert!(!intersects(b, source));
                assert!(
                    b.left >= area.left
                        && b.right <= area.right
                        && b.top >= area.top
                        && b.bottom <= area.bottom
                );
            }
        }
        assert!(placement(area, &[area], 296, 72, 8).is_some());
    }
}
