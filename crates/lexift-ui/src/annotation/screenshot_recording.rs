//! One recording view borrows the screenshot input canvas, retaining its exact
//! document and frozen feedback for cancellation. Native capture/encoding stay outside UI.
use super::*;
use crate::RecordingControl;
#[cfg(test)]
#[path = "recording_tests.rs"]
mod tests;
use lexift_core::ports::{
    annotation_render::{RenderStamp, RenderSurface},
    screen_recording::SaveRequest,
};
use lexift_core::{
    Error, Result,
    domain::{
        screen_recording::{Action, Event as Input, Identity, Phase},
        screenshot::recording::{Command, Event},
    },
};

#[derive(Default)]
struct View {
    original_renderer: Option<Arc<dyn lexift_core::ports::annotation_render::AnnotationRenderPort>>,
    identity: Option<Identity>,
    request: Option<ExportRequest>,
    document: Option<editor_document::EditorDocument>,
    desktop: Option<Arc<Session>>,
    geometry: Vec<(
        Rect,
        slint::PhysicalPosition,
        slint::PhysicalSize,
        frame::PresentedFrame,
    )>,
    index: usize,
    toolbar: Option<slint::PhysicalPosition>,
    toolbar_size: Option<slint::PhysicalSize>,
    toolbar_collapsed: bool,
    control: window::Slot<RecordingControl>,
    range: window::Slot<crate::LongCaptureRange>,
    compiler: super::super::recording_scene::Compiler,
    revision: u64,
    fence: Option<(Identity, RenderStamp)>,
    prepared: Option<(Identity, RenderStamp)>,
    pending_stamp: Option<RenderStamp>,
    last_presented: Option<RenderStamp>,
    pending_effects: Vec<(ObjectId, Vec<Rect>)>,
    waiting_since: Option<Instant>,
    timer: slint::Timer,
}
thread_local! { static VIEW:RefCell<View> = RefCell::new(View::default()); }
impl View {
    fn observe_presentation(
        &mut self,
        frame: &lexift_core::ports::annotation_render::RenderCompletion,
    ) -> bool {
        if !frame.presented
            || self
                .identity
                .is_none_or(|identity| identity.session != frame.stamp.session)
            || frame.stamp.revision > self.revision
        {
            return false;
        }
        if self
            .last_presented
            .is_none_or(|last| last.revision < frame.stamp.revision)
        {
            self.last_presented = Some(frame.stamp);
            // A renderer can keep making progress one revision behind a long
            // gesture. This does not acknowledge the final/pending revision.
            if self.waiting_since.is_some() {
                self.waiting_since = Some(Instant::now());
            }
        }
        true
    }
}
fn emit_recording(event: Event) {
    emit(ScreenshotEvent::Recording(event));
}
fn current() -> Option<lexift_core::domain::screenshot::recording::Session> {
    FLOW.with(|f| {
        let f = f.borrow();
        (f.state.phase == ScreenshotPhase::Recording)
            .then(|| f.state.recording.clone())
            .flatten()
    })
}
pub(in crate::annotation) fn active() -> bool {
    VIEW.with(|v| v.borrow().identity.is_some())
}
pub(in crate::annotation) fn can_annotate() -> bool {
    current().is_some_and(|s| s.lifecycle.can_annotate())
}
fn input(event: Input) {
    if let Some(state) = current() {
        emit_recording(Event::Input {
            identity: state.lifecycle.identity,
            event,
        });
    }
}
#[track_caller]
pub(in crate::annotation) fn cancel() {
    tracing::debug!(caller=%std::panic::Location::caller(),"Recording view cancellation requested");
    check_layout();
    input(Input::Cancel);
}
pub(in crate::annotation) fn check_layout() {
    let check = REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .map(|r| (r.lifecycle.screenshot_displays)())
    });
    if let (Some(layout), Some(state)) = (check, current())
        && !lexift_core::domain::screenshot::refresh::layout_matches(
            &layout,
            &state.request.snapshot.monitors,
        )
    {
        emit_recording(Event::LayoutChanged {
            session: state.lifecycle.identity.session,
            operation: state.lifecycle.identity.operation,
        });
    }
}
pub(in crate::annotation) fn available(r: &Registry) -> bool {
    let available = !active()
        && r.lifecycle.recording_window.is_some()
        && r.render_state.mosaic.pending.is_empty()
        && refresh::available(r)
        && recording_renderer(r).as_ref().is_some_and(|p| {
            p.capabilities().backend == lexift_core::ports::annotation_render::RenderBackend::Gpu
        })
        && r.screenshot
            .as_ref()
            .and_then(|e| e.region.bounds())
            .is_some_and(|b| {
                (r.lifecycle.screenshot_displays)().iter().any(|(m, _)| {
                    lexift_core::domain::screen_recording::recording_bounds(b, *m).is_ok()
                })
            });
    if !available && refresh::available(r) {
        thread_local! {static LAST:std::cell::Cell<Option<(bool,bool)>>=const{std::cell::Cell::new(None)};}
        let reasons = (
            r.lifecycle.recording_window.is_some(),
            recording_renderer(r).is_some(),
        );
        LAST.with(|last| {
            if last.replace(Some(reasons)) != Some(reasons) {
                tracing::info!(
                    window_bridge = reasons.0,
                    renderer = reasons.1,
                    "Recording preparation capability"
                );
            }
        });
    }
    available
}
fn recording_renderer(
    r: &Registry,
) -> Option<Arc<dyn lexift_core::ports::annotation_render::AnnotationRenderPort>> {
    r.lifecycle.annotation_renderer.clone().or_else(|| {
        FLOW.with(|f| {
            f.borrow()
                .recovery
                .as_ref()
                .and_then(|r| r.lifecycle.annotation_renderer.clone())
        })
    })
}
pub(in crate::annotation) fn request() {
    keyboard::commit_current();
    let request = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        if !available(r) {
            return None;
        }
        finish_text_edit_registry(r, true);
        r.finish_style_edit();
        size_input::close(r);
        r.close_panel();
        r.hints.clear();
        let e = r.screenshot.as_ref()?;
        Some(ExportRequest {
            session: e.id,
            request: FLOW.with(|f| f.borrow().state.operation + 1),
            snapshot: Arc::new(export::snapshot_at(
                r,
                e.region.bounds()?,
                r.canvas_state.canvases.first()?.scale(),
            )?),
        })
    });
    if let Some(request) = request {
        emit_recording(Event::Requested(request));
    }
}
pub(crate) fn clear() {
    super::super::recording_chrome::clear();
    VIEW.with(|v| *v.borrow_mut() = View::default());
}
pub(crate) fn execute(command: Command) {
    match command {
        Command::PrepareView {
            identity,
            bounds,
            request,
        } => prepare(identity, bounds, request),
        Command::Execute(command) => match command.action {
            Action::PresentFinalContent => {
                REGISTRY.with(|r| {
                    if let Some(r) = r.borrow_mut().as_mut() {
                        finish_text_edit_registry(r, true);
                        r.finish_style_edit();
                        r.hints.clear();
                        r.close_panel();
                        render(r);
                    }
                });
                VIEW.with(|v| {
                    let mut v = v.borrow_mut();
                    v.fence = Some((
                        command.identity,
                        RenderStamp {
                            session: command.identity.session,
                            revision: v.revision,
                        },
                    ));
                });
                arm();
            }
            Action::RestoreScreenshot => restore_editor(),
            Action::EndScreenshot => {
                clear();
                super::restore(command.identity.session);
            }
            _ => {}
        },
        _ => {}
    }
}
fn failed(identity: Identity, error: Error) {
    tracing::warn!(session=identity.session,operation=identity.operation,request=identity.request,%error,"Recording view preparation failed");
    emit_recording(Event::Failed { identity, error });
}
fn prepare(identity: Identity, bounds: Rect, request: ExportRequest) {
    let made = REGISTRY.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        Some((
            crate::screen_interaction::create(&r.lifecycle, RecordingControl::new).ok()?,
            crate::screen_interaction::create(&r.lifecycle, crate::LongCaptureRange::new).ok()?,
        ))
    });
    let Some((control, range)) = made else {
        failed(identity, Error::new("Recording view unavailable"));
        return;
    };
    crate::theme::apply(&control);
    crate::theme::apply(&range);
    let desktop = FLOW
        .with(|f| {
            f.borrow()
                .recovery
                .as_ref()
                .map(|r| Arc::new(r.document.session.clone()))
        })
        .unwrap_or_else(|| Arc::new(Session::new()));
    VIEW.with(|v| {
        let mut v = v.borrow_mut();
        *v = View::default();
        v.identity = Some(identity);
        v.request = Some(request);
        v.desktop = Some(desktop);
        v.control.open(control);
        v.range.open(range);
    });
    let result = REGISTRY.with(|slot| -> Result<()> {
        let mut slot = slot.borrow_mut();
        let r = slot
            .as_mut()
            .ok_or_else(|| Error::new("Recording view unavailable"))?;
        let index = r
            .canvas_state
            .canvases
            .iter()
            .position(|c| {
                bounds.left >= c.bounds.left
                    && bounds.top >= c.bounds.top
                    && bounds.right <= c.bounds.right
                    && bounds.bottom <= c.bounds.bottom
            })
            .ok_or_else(|| Error::new("Recording monitor unavailable"))?;
        super::super::recording_chrome::install(r)?;
        let renderer = recording_renderer(r);
        VIEW.with(|v| v.borrow_mut().original_renderer = r.lifecycle.annotation_renderer.take());
        r.lifecycle.annotation_renderer = renderer;
        handoff::cancel(r);
        r.render_state.frame_tasks.clear();
        if let Some(port) = &r.lifecycle.annotation_renderer {
            port.clear();
        }
        if let Some(e) = r.screenshot.as_mut() {
            e.preview = None;
            e.accepted_after = e.revision + 1;
        }
        VIEW.with(|v| {
            let mut v = v.borrow_mut();
            v.index = index;
            v.document = Some(r.document.clone());
            v.toolbar = r.toolbar_state.main.get().map(|w| w.window().position());
            v.toolbar_size = r.toolbar_state.main.get().map(|w| w.window().size());
            v.toolbar_collapsed = r
                .toolbar_state
                .main
                .get()
                .is_some_and(|w| w.get_collapsed());
            for c in &mut r.canvas_state.canvases {
                c.input().cancel_pending();
                (r.lifecycle.cancel_window_interaction)(c.window.window());
                let position = c.window.window().position();
                let size = c.window.window().size();
                v.geometry
                    .push((c.bounds, position, size, std::mem::take(&mut c.presented)));
                c.window
                    .window()
                    .hide()
                    .map_err(|e| Error::new(e.to_string()))?;
            }
            if let Some(main) = r.toolbar_state.main.get() {
                main.hide().map_err(|e| Error::new(e.to_string()))?;
            }
            Ok::<_, Error>(())
        })?;
        let windows: Vec<_> = r
            .canvas_state
            .canvases
            .iter()
            .filter_map(|c| (r.lifecycle.annotation_window_token)(c.window.window()))
            .collect();
        if !(r.lifecycle.complete_screenshot_refresh)(&windows, false) {
            return Err(Error::new("Recording suspension failed"));
        }
        let layer = &mut r.canvas_state.canvases[index];
        layer.host.bounds = bounds;
        layer
            .window
            .window()
            .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
        layer.window.window().set_size(slint::PhysicalSize::new(
            (bounds.right - bounds.left) as u32,
            (bounds.bottom - bounds.top) as u32,
        ));
        if let canvas::Surface::Canvas(w) = &layer.window {
            w.set_frame(slint::Image::default());
            w.set_controls(slint::Image::default());
        }
        r.document.mode = InteractionMode::Mouse;
        r.document.session.selected = None;
        r.sync_edit_scope();
        r.sync_mode_display();
        Ok(())
    });
    if let Err(error) = result {
        failed(identity, error);
        return;
    }
    prepare_windows(identity, bounds, 0);
}
fn prepare_windows(identity: Identity, bounds: Rect, attempt: u8) {
    if current().is_none_or(|s| s.lifecycle.identity != identity) {
        return;
    }
    let prepared = REGISTRY.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        VIEW.with(|v| {
            let v = v.borrow();
            let w = v.control.get()?;
            let range = v.range.get()?;
            let monitor = v
                .request
                .as_ref()?
                .snapshot
                .monitors
                .iter()
                .find(|(m, _)| {
                    bounds.left >= m.left
                        && bounds.top >= m.top
                        && bounds.right <= m.right
                        && bounds.bottom <= m.bottom
                })?
                .0;
            let scale = r.canvas_state.canvases[v.index].scale();
            let area = (r.lifecycle.popup_work_area)(Point {
                x: bounds.left,
                y: bounds.top,
            })?;
            range
                .window()
                .set_position(slint::PhysicalPosition::new(monitor.left, monitor.top));
            range.window().set_size(slint::PhysicalSize::new(
                (monitor.right - monitor.left) as u32,
                (monitor.bottom - monitor.top) as u32,
            ));
            range.set_selection_x((bounds.left - monitor.left) as f32 / scale);
            range.set_selection_y((bounds.top - monitor.top) as f32 / scale);
            range.set_selection_width((bounds.right - bounds.left) as f32 / scale);
            range.set_selection_height((bounds.bottom - bounds.top) as f32 / scale);
            let width = (380. * scale) as i32;
            let height = (40. * scale) as i32;
            w.window()
                .set_size(slint::PhysicalSize::new(width as u32, height as u32));
            let y = if bounds.bottom + height + 8 <= area.bottom {
                bounds.bottom + 8
            } else {
                (bounds.top - height - 8).max(area.top)
            };
            w.window().set_position(slint::PhysicalPosition::new(
                bounds
                    .left
                    .clamp(area.left, (area.right - width).max(area.left)),
                y,
            ));
            let states = [(r.prepare)(w.window()), (r.prepare)(range.window())];
            let configure = r.lifecycle.recording_window.as_ref()?;
            Some((
                states,
                configure(w.window(), crate::RecordingWindowRole::Control)
                    // Ownership also registers the control with the existing
                    // annotation ordering policy. Otherwise drawing raises a
                    // full-screen input canvas over controls placed inside ROI.
                    && (r.lifecycle.attach_tool_window)(
                        w.window(),
                        r.canvas_state.canvases[v.index].window.window(),
                    )
                    && configure(range.window(), crate::RecordingWindowRole::Feedback)
                    && configure(
                        r.canvas_state.canvases[v.index].window.window(),
                        crate::RecordingWindowRole::Interaction,
                    )
                    && r.toolbar_state.main.get().is_none_or(|main| {
                        configure(main.window(), crate::RecordingWindowRole::Control)
                    }),
            ))
        })
    });
    if prepared.as_ref().is_some_and(|(states, roles)| {
        *roles && states.iter().all(|s| *s == PassiveWindowPreparation::Ready)
    }) {
        let result = show_windows(identity, bounds);
        if let Err(error) = result {
            failed(identity, error);
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
                    prepare_windows(identity, bounds, attempt + 1)
                })
        });
    } else {
        failed(identity, Error::new("Recording window preparation failed"));
    }
}
fn show_windows(identity: Identity, bounds: Rect) -> Result<()> {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot
            .as_mut()
            .ok_or_else(|| Error::new("Recording view unavailable"))?;
        VIEW.with(|v| {
            let v = v.borrow();
            let w = v.control.get().unwrap();
            let range = v.range.get().unwrap();
            let revision = v.control.revision();
            hints::bind_recording_control(
                w,
                r.generation,
                hints::Owner::RecordingControl {
                    session: identity.session,
                    operation: identity.operation,
                    revision,
                },
            );
            let work = v.control.input();
            w.on_action(move |action| {
                work.post(move || choose(identity.session, identity.operation, revision, action))
            });
            let work = v.control.input();
            w.window().on_close_requested(move || {
                work.post(move || choose(identity.session, identity.operation, revision, 4));
                slint::CloseRequestResponse::KeepWindowShown
            });
            let lifecycle = r.lifecycle.clone();
            let weak = w.as_weak();
            w.on_drag_requested(move || {
                REGISTRY.with(|r| {
                    if let Some(r) = r.borrow_mut().as_mut() {
                        r.hints.clear();
                    }
                });
                if let Some(w) = weak.upgrade() {
                    (lifecycle.begin_window_drag)(w.window());
                }
            });
            let weak = w.as_weak();
            let work_area = r.lifecycle.popup_work_area.clone();
            let sink = v.control.input().pointer_sink(move |event| {
                if let Some(w) = weak.upgrade() {
                    if matches!(event, PopupPointerInput::NativeMoveFinished) {
                        let owned = VIEW.with(|v| {
                            let v = v.borrow();
                            v.identity.is_some_and(|i| {
                                i.session == identity.session && i.operation == identity.operation
                            }) && v.control.revision() == revision
                        });
                        if owned {
                            let position = w.window().position();
                            let size = w.window().size();
                            let center = Point {
                                x: position.x + size.width as i32 / 2,
                                y: position.y + size.height as i32 / 2,
                            };
                            if let Some(area) = work_area(center) {
                                w.window().set_position(slint::PhysicalPosition::new(
                                    position.x.clamp(
                                        area.left,
                                        (area.right - size.width as i32).max(area.left),
                                    ),
                                    position.y.clamp(
                                        area.top,
                                        (area.bottom - size.height as i32).max(area.top),
                                    ),
                                ));
                            }
                        }
                        return;
                    }
                    crate::bridge::dispatch_pointer_input(w.window(), event);
                }
            });
            w.set_dimensions(
                format!(
                    "{} × {} px",
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top
                )
                .into(),
            );
            w.show().map_err(|e| Error::new(e.to_string()))?;
            if !(r.lifecycle.complete_passive_window_show)(w.window(), sink) {
                return Err(Error::new("Recording controls unavailable"));
            }
            range.show().map_err(|e| Error::new(e.to_string()))?;
            let monitor = v
                .request
                .as_ref()
                .unwrap()
                .snapshot
                .monitors
                .iter()
                .find(|(m, _)| {
                    bounds.left >= m.left
                        && bounds.top >= m.top
                        && bounds.right <= m.right
                        && bounds.bottom <= m.bottom
                })
                .unwrap()
                .0;
            let pixels = long::feedback::range(
                monitor,
                bounds,
                range.window().scale_factor(),
                range.global::<crate::Colors>().get_primary(),
                range.global::<crate::Colors>().get_capture_mask(),
            )
            .ok_or_else(|| Error::new("Recording feedback allocation failed"))?;
            if !(r.lifecycle.annotation_frame_presenter)(
                range.window(),
                pixels.data(),
                pixels.width(),
                pixels.height(),
            ) {
                return Err(Error::new("Recording feedback presentation failed"));
            }
            let layer = &r.canvas_state.canvases[v.index];
            let blank = tiny_skia::Pixmap::new(
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
            )
            .ok_or_else(|| Error::new("Recording input allocation failed"))?;
            if !(r.lifecycle.annotation_frame_presenter)(
                layer.window.window(),
                blank.data(),
                blank.width(),
                blank.height(),
            ) || !(r.lifecycle.annotation_click_through)(layer.window.window(), true)
            {
                return Err(Error::new("Recording input preparation failed"));
            }
            layer
                .window
                .window()
                .show()
                .map_err(|e| Error::new(e.to_string()))?;
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_recording_mode(true);
                let (size, position) = toolbar::fit_layout(
                    main.window().position(),
                    main.get_toolbar_width(),
                    main.window().scale_factor(),
                    (r.lifecycle.popup_work_area)(Point {
                        x: bounds.left,
                        y: bounds.top,
                    }),
                );
                main.window().set_size(size);
                main.window().set_position(position);
                if let Some(area) = (r.lifecycle.popup_work_area)(Point {
                    x: bounds.left,
                    y: bounds.top,
                }) {
                    let control_size = w.window().size();
                    let (control, annotation) = super::super::recording_placement::bars(
                        bounds,
                        area,
                        (control_size.width as i32, control_size.height as i32),
                        (size.width as i32, size.height as i32),
                        (8. * main.window().scale_factor()).round() as i32,
                    );
                    w.window()
                        .set_position(slint::PhysicalPosition::new(control.x, control.y));
                    main.window()
                        .set_position(slint::PhysicalPosition::new(annotation.x, annotation.y));
                }
                main.show().map_err(|e| Error::new(e.to_string()))?;
            }
            Ok(())
        })?;
        if !crate::pinned_screenshot::capture_visibility(true) {
            return Err(Error::new("Recording pin restoration failed"));
        }
        r.canvas_state.annotation_ready = true;
        render(r);
        VIEW.with(|v| {
            let mut v = v.borrow_mut();
            v.prepared = Some((
                identity,
                RenderStamp {
                    session: identity.session,
                    revision: v.revision,
                },
            ));
        });
        Ok(())
    })?;
    arm();
    Ok(())
}
fn choose(session: u64, operation: u64, revision: u64, action: i32) {
    if !VIEW.with(|v| {
        let v = v.borrow();
        v.identity
            .is_some_and(|i| i.session == session && i.operation == operation)
            && v.control.revision() == revision
    }) {
        return;
    }
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.hints.clear();
        }
    });
    let Some(state) = current() else {
        return;
    };
    match action {
        0 => input(match state.lifecycle.phase {
            Phase::Ready => Input::Start,
            Phase::Paused => Input::Resume,
            _ => Input::Pause,
        }),
        1 => input(Input::Finish),
        2 => {
            if let Some(artifact) = state.lifecycle.artifact {
                let owner = REGISTRY.with(|r| {
                    r.borrow().as_ref().and_then(|r| {
                        VIEW.with(|v| {
                            v.borrow()
                                .control
                                .get()
                                .and_then(|w| (r.lifecycle.annotation_window_token)(w.window()))
                        })
                    })
                });
                emit_recording(Event::Save(SaveRequest {
                    identity: state.lifecycle.identity,
                    artifact,
                    owner,
                    default_name: format!(
                        "Lexift-recording-{}.mp4",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs()
                    ),
                    title: crate::i18n::tr("Save video"),
                    extension_mismatch_message: crate::i18n::tr("Use the .mp4 file extension."),
                }));
            }
        }
        _ => cancel(),
    }
}
pub(in crate::annotation) fn render(r: &mut Registry) {
    let Some(state) = current() else {
        return;
    };
    if matches!(
        state.lifecycle.phase,
        Phase::PendingSave | Phase::Saving | Phase::Retiring | Phase::Closed
    ) {
        return;
    }
    let result = VIEW.with(|slot| {
        let mut v = slot.borrow_mut();
        let index = v.index;
        let layer = r.canvas_state.canvases.get(index)?;
        let input_window = (r.lifecycle.annotation_window_token)(layer.window.window())?;
        v.revision += 1;
        let stamp = RenderStamp {
            session: state.lifecycle.identity.session,
            revision: v.revision,
        };
        let desktop = v.desktop.clone()?;
        let mut excluded = capture_exclusions(r);
        for window in [
            v.control.get().map(ComponentHandle::window),
            v.range.get().map(ComponentHandle::window),
        ]
        .into_iter()
        .flatten()
        {
            excluded.push((r.lifecycle.annotation_window_token)(window)?);
        }
        let frame = v.compiler.compile(super::super::recording_scene::Input {
            desktop: &desktop,
            recording: &r.document.session,
            draft: gestures::display_draft(&r.gesture, &r.pending_drawing),
            hovered: r.hovered,
            stamp,
            surface: RenderSurface {
                input_window,
                bounds: layer.bounds,
                scale: layer.scale(),
            },
            excluded: &excluded,
        })?;
        let interaction =
            crate::annotation_scene::compose_document(&frame.interaction, layer.bounds)?;
        if !(r.lifecycle.annotation_frame_presenter)(
            layer.window.window(),
            interaction.data(),
            interaction.width(),
            interaction.height(),
        ) {
            return None;
        }
        r.lifecycle
            .annotation_renderer
            .as_ref()?
            .submit(Arc::new(frame.content))
            .map_err(|error| tracing::warn!(%error,"Recording scene submission failed"))
            .ok()?;
        v.pending_stamp = Some(stamp);
        v.waiting_since.get_or_insert_with(Instant::now);
        v.pending_effects = frame.effects;
        Some(())
    });
    if result.is_none() {
        failed(
            state.lifecycle.identity,
            Error::new("Recording annotation presentation failed"),
        );
    } else {
        arm();
    }
}
fn arm() {
    VIEW.with(|v| {
        v.borrow().timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(16),
            poll,
        )
    });
}
fn poll() {
    if !active() {
        return;
    }
    let latest = REGISTRY.with(|r| {
        r.borrow()
            .as_ref()
            .and_then(|r| r.lifecycle.annotation_renderer.as_ref())
            .map(|p| p.latest())
    });
    let ready = VIEW.with(|v| {
        let mut v = v.borrow_mut();
        let Some(Ok(Some(frame))) = latest.as_ref() else {
            return None;
        };
        if !v.observe_presentation(frame) {
            return None;
        }
        if v.pending_stamp == Some(frame.stamp) {
            let confirmed: Vec<_> = v
                .pending_effects
                .iter()
                .filter(|(_, regions)| {
                    regions.iter().all(|r| {
                        frame.sampled_regions.iter().any(|s| {
                            s.left <= r.left
                                && s.top <= r.top
                                && s.right >= r.right
                                && s.bottom >= r.bottom
                        })
                    })
                })
                .map(|(id, _)| *id)
                .collect();
            REGISTRY.with(|r| {
                if let Some(r) = r.borrow_mut().as_mut() {
                    r.render_state.mosaic.confirm_native(&confirmed);
                }
            });
            if confirmed.len() != v.pending_effects.len() {
                return None;
            }
            v.pending_stamp = None;
            v.waiting_since = None;
        }
        if v.prepared.is_some_and(|(_, s)| {
            s.session == frame.stamp.session && s.revision <= frame.stamp.revision
        }) {
            return v.prepared.take().map(|(i, _)| (i, true));
        }
        if v.fence.is_some_and(|(_, s)| {
            s.session == frame.stamp.session && s.revision <= frame.stamp.revision
        }) {
            return v.fence.take().map(|(i, _)| (i, false));
        }
        None
    });
    if let Some((identity, preparation)) = ready {
        if preparation {
            let excluded = REGISTRY
                .with(|r| {
                    r.borrow().as_ref().map(|r| {
                        VIEW.with(|v| {
                            let v = v.borrow();
                            // Other screenshot canvases remain hidden for cancel
                            // recovery. They are not recording decorations and
                            // have not acquired the recording affinity policy.
                            let mut excluded: Vec<_> = r
                                .canvas_state
                                .canvases
                                .iter()
                                .filter(|c| c.window.window().is_visible())
                                .map(|c| c.window.window())
                                .chain(r.toolbar_state.main.get().map(ComponentHandle::window))
                                .filter_map(|w| (r.lifecycle.annotation_window_token)(w))
                                .collect();
                            for w in [
                                v.control.get().map(ComponentHandle::window),
                                v.range.get().map(ComponentHandle::window),
                            ]
                            .into_iter()
                            .flatten()
                            {
                                if let Some(t) = (r.lifecycle.annotation_window_token)(w) {
                                    excluded.push(t);
                                }
                            }
                            excluded
                        })
                    })
                })
                .unwrap_or_default();
            emit_recording(Event::ViewReady {
                identity,
                result: Ok(excluded),
            });
        } else {
            emit_recording(Event::Input {
                identity,
                event: Input::FinalContentPresented,
            });
        }
    } else if let Some(Err(error)) = latest.as_ref()
        && let Some(state) = current()
    {
        failed(state.lifecycle.identity, error.clone());
    }
    let timed_out = VIEW.with(|v| {
        v.borrow()
            .waiting_since
            .is_some_and(|t| t.elapsed() > Duration::from_secs(5))
    });
    if timed_out {
        if let Some(Ok(Some(frame))) = latest.as_ref() {
            let pending = VIEW.with(|v| v.borrow().pending_stamp);
            tracing::warn!(?pending, actual=?frame.stamp, presented=frame.presented,
                wait=?frame.wait, sampled_regions=frame.sampled_regions.len(),
                "Recording presentation fence timed out");
        }
        if let Some(state) = current() {
            failed(
                state.lifecycle.identity,
                Error::new("Recording presentation timed out"),
            );
        }
        return;
    }
    if VIEW.with(|v| {
        let v = v.borrow();
        v.prepared.is_some() || v.fence.is_some() || v.pending_stamp.is_some()
    }) {
        arm();
    }
}
pub(crate) fn sync(state: &ScreenshotState) {
    let Some(recording) = &state.recording else {
        return;
    };
    VIEW.with(|v| {
        let v = v.borrow();
        let Some(w) = v.control.get() else {
            return;
        };
        if v.identity.is_none_or(|i| {
            i.session != state.session || i.operation != recording.lifecycle.identity.operation
        }) {
            return;
        }
        w.set_phase(match recording.lifecycle.phase {
            Phase::Ready => 1,
            Phase::Recording => 2,
            Phase::Paused => 3,
            Phase::PresentingFinalContent | Phase::Finishing => 4,
            Phase::PendingSave => 5,
            Phase::Saving => 6,
            _ => 0,
        });
        w.set_interrupted(matches!(
            recording.lifecycle.finish_reason,
            lexift_core::domain::screen_recording::FinishReason::Interrupted(_)
        ));
        let seconds = recording
            .artifact
            .as_ref()
            .map(|a| a.duration)
            .or_else(|| recording.progress.map(|p| p.active_time))
            .unwrap_or_default()
            .as_secs();
        w.set_elapsed(format!("{:02}:{:02}", seconds / 60, seconds % 60).into());
    });
    if active() {
        REGISTRY.with(|r| {
            if let Some(r) = r.borrow_mut().as_mut() {
                if let Some(main) = r.toolbar_state.main.get() {
                    main.set_output_busy(!recording.lifecycle.can_annotate());
                }
                if matches!(
                    recording.lifecycle.phase,
                    Phase::PendingSave | Phase::Saving
                ) {
                    if let Some(port) = &r.lifecycle.annotation_renderer {
                        port.clear();
                    }
                    for c in &r.canvas_state.canvases {
                        let _ = c.window.window().hide();
                    }
                    if let Some(w) = r.toolbar_state.main.get() {
                        let _ = w.hide();
                    }
                }
            }
        });
        if matches!(
            recording.lifecycle.phase,
            Phase::Recording | Phase::Paused | Phase::Ready
        ) {
            check_layout();
        }
    }
}
fn restore_editor() {
    let result = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        if let Some(port) = &r.lifecycle.annotation_renderer {
            port.clear();
        }
        // Discard recording input before installing the original document; a
        // late release or text callback must not complete into that document.
        for canvas in &r.canvas_state.canvases {
            (r.lifecycle.cancel_window_interaction)(canvas.window.window());
            canvas.input().cancel_pending();
        }
        cancel_active_gesture(r);
        mosaic::cancel_pending(r);
        r.editor.clear();
        r.close_watermark_input();
        r.hovered = None;
        r.hints.clear();
        r.close_panel();
        VIEW.with(|slot| {
            let mut v = slot.borrow_mut();
            if let Some(document) = v.document.take() {
                r.document = document;
            }
            r.lifecycle.annotation_renderer = v.original_renderer.take();
            for (c, (bounds, position, size, presented)) in
                r.canvas_state.canvases.iter_mut().zip(v.geometry.drain(..))
            {
                let _ = c.window.window().hide();
                c.host.bounds = bounds;
                c.window.window().set_position(position);
                c.window.window().set_size(size);
                c.presented = presented;
                c.signature = None;
                c.presented.replay(|p| {
                    (r.lifecycle.annotation_frame_presenter)(
                        c.window.window(),
                        p.data(),
                        p.width(),
                        p.height(),
                    )
                });
                let _ = c.window.window().show();
                (r.lifecycle.annotation_click_through)(c.window.window(), false);
            }
            if let Some(main) = r.toolbar_state.main.get() {
                main.set_recording_mode(false);
                main.set_collapsed(v.toolbar_collapsed);
                if let Some(position) = v.toolbar {
                    main.window().set_position(position);
                }
                if let Some(size) = v.toolbar_size {
                    main.window().set_size(size);
                }
                let _ = main.show();
            }
        });
        r.sync_mode_display();
        r.canvas_state.annotation_ready = true;
        r.queue_render();
        Some(())
    });
    crate::pinned_screenshot::capture_visibility(false);
    clear();
    if result.is_none() {
        super::cancel();
    }
}
pub(in crate::annotation) fn with_hint_window<T>(
    session: u64,
    operation: u64,
    revision: u64,
    f: impl FnOnce(&slint::Window) -> T,
) -> Option<T> {
    VIEW.with(|v| {
        let v = v.borrow();
        if v.identity
            .is_some_and(|i| i.session == session && i.operation == operation)
            && v.control.revision() == revision
        {
            v.control.get().map(|w| f(w.window()))
        } else {
            None
        }
    })
}
