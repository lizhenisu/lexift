//! OCR owns temporary windows and input independently of annotation.
use crate::screen_interaction::Host;
use crate::{
    OcrSelectionWindow, PassiveWindowPreparation, PopupPointerInput, WindowLifecycleCallbacks,
};
use lexift_core::{
    AppEvent,
    domain::{
        geometry::{Point, Rect},
        ocr::{OcrEvent, OcrPhase},
        region_selection::{RegionSelection, SelectionContext},
    },
    ports::screen_capture::CapturedRegion,
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

thread_local! { static STATE: RefCell<Option<Registry>> = const { RefCell::new(None) }; }
struct Registry {
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    generation: u64,
    phase: OcrPhase,
    windows: Vec<Host<OcrSelectionWindow>>,
    gesture: RegionSelection,
    frames: Option<Arc<Vec<CapturedRegion>>>,
    native_handoff: Vec<bool>,
    timer: slint::Timer,
    attempts: u32,
}
pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(Registry {
            prepare,
            lifecycle,
            handler: None,
            generation: 0,
            phase: OcrPhase::Idle,
            windows: vec![],
            gesture: RegionSelection::default(),
            frames: None,
            native_handoff: vec![],
            timer: Default::default(),
            attempts: 0,
        })
    });
}
pub(crate) fn set_handler(handler: Rc<dyn Fn(AppEvent)>) {
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.handler = Some(handler);
        }
    });
}
pub(crate) fn active() -> bool {
    STATE.with(|s| s.borrow().as_ref().is_some_and(|r| !r.windows.is_empty()))
}
pub(crate) fn screen_busy() -> bool {
    STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| matches!(r.phase, OcrPhase::Checking | OcrPhase::Capturing))
    })
}
pub(crate) fn request() {
    emit(OcrEvent::Requested {
        annotation_active: crate::annotation::is_open(),
    });
}
fn emit(event: OcrEvent) {
    let handler = STATE.with(|s| s.borrow().as_ref().and_then(|r| r.handler.clone()));
    if let Some(h) = handler {
        h(AppEvent::Ocr(event));
    }
}
pub(crate) fn close() {
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.timer.stop();
            r.gesture.cancel();
            r.frames = None;
            r.native_handoff.clear();
            r.windows.clear();
        }
    });
}
pub(crate) fn sync(generation: u64, phase: OcrPhase) {
    let retire = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.generation != generation || phase != OcrPhase::Capturing)
    });
    if retire {
        close();
    }
    STATE.with(|s| {
        if let Some(r) = s.borrow_mut().as_mut() {
            r.generation = generation;
            r.phase = phase;
        }
    });
}
pub(crate) fn show(generation: u64, frames: Arc<Vec<CapturedRegion>>) {
    if !STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.generation == generation && r.phase == OcrPhase::Capturing)
    }) {
        return;
    }
    close();
    crate::bridge::cancel_idle_memory_trim();
    let result = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut().ok_or(())?;
        r.generation = generation;
        r.attempts = 0;
        r.frames = Some(frames.clone());
        for frame in frames.iter() {
            let window = crate::screen_interaction::create(&r.lifecycle, OcrSelectionWindow::new)
                .map_err(|_| ())?;
            let b = frame.bounds;
            let cancel_window = window.as_weak();
            window.on_cancelled(move || {
                let Some(window) = cancel_window.upgrade() else {
                    return;
                };
                let current = STATE.with(|s| {
                    s.borrow().as_ref().is_some_and(|r| {
                        r.generation == generation
                            && r.windows.iter().any(
                                |Host {
                                     window: w, bounds, ..
                                 }| {
                                    *bounds == b && std::ptr::eq(w.window(), window.window())
                                },
                            )
                    })
                });
                if current {
                    close();
                    emit(OcrEvent::Cancelled);
                }
            });
            let width = (b.right - b.left) as u32;
            let height = (b.bottom - b.top) as u32;
            let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &frame.pixels,
                width,
                height,
            );
            window.set_frozen_image(slint::Image::from_rgba8(pixels));
            window
                .window()
                .set_position(slint::PhysicalPosition::new(b.left, b.top));
            window
                .window()
                .set_size(slint::PhysicalSize::new(width, height));
            let weak = window.as_weak();
            let finished = Arc::new(AtomicBool::new(false));
            let done = finished.clone();
            let notifier_weak = weak.clone();
            let installed = window
                .window()
                .set_rendering_notifier(move |state, _| {
                    if matches!(state, slint::RenderingState::AfterRendering)
                        && !done.load(Ordering::Relaxed)
                    {
                        schedule_handoff(generation, b, notifier_weak.clone(), done.clone());
                    }
                })
                .is_ok();
            // Software renderers have no rendering notifier; native WM_PAINT still
            // brackets their redraw dispatch and defers handoff until it returns.
            r.native_handoff.push(!installed);
            r.windows.push(Host::new(window, b));
        }
        r.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || prepare(generation),
        );
        Ok::<(), ()>(())
    });
    if result.is_err() {
        close();
        emit(OcrEvent::Failed {
            generation,
            error: "Could not prepare OCR selection".into(),
        });
    }
}

/// Forward OCR settings and resource actions through the same versioned Core path.
pub(crate) fn wire_settings(settings: &crate::SettingsWindow, handler: Rc<dyn Fn(AppEvent)>) {
    use lexift_core::domain::settings::SettingsChange;
    let h = handler.clone();
    settings.on_ocr_language_selected(move |index| {
        if let Some(value) = lexift_core::domain::ocr::language_at_index(index) {
            h(AppEvent::SettingsChangeRequested {
                change: SettingsChange::OcrLanguage(value),
            });
        }
    });
    let h = handler.clone();
    settings.on_ocr_korean_auto_toggled(move |value| {
        h(AppEvent::SettingsChangeRequested {
            change: SettingsChange::OcrKoreanAuto(value),
        });
    });
    let h = handler.clone();
    settings.on_ocr_korean_download_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::DownloadKorean,
            ),
        ));
    });
    let h = handler.clone();
    settings.on_ocr_korean_remove_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::RemoveKorean,
            ),
        ));
    });
    let h = handler.clone();
    settings.on_ocr_download_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::Download,
            ),
        ))
    });
    let h = handler.clone();
    settings.on_ocr_download_cancelled(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::CancelResourceDownload,
        ))
    });
    let h = handler.clone();
    settings.on_ocr_remove_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::Remove,
            ),
        ))
    });
    let h = handler.clone();
    settings.on_ocr_refresh_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::Check,
            ),
        ))
    });
}

pub(crate) fn apply_settings(window: &crate::SettingsWindow, state: &lexift_core::AppState) {
    let o = &state.ocr;
    window.set_draft_ocr_hotkey_label(state.desired_settings.ocr_hotkey.to_string().into());
    window.set_ocr_language_index(lexift_core::domain::ocr::language_index(
        state.desired_settings.ocr_language.as_deref(),
    ));
    use lexift_core::{domain::ocr::ResourceOperation, ports::ocr::OcrResourceStatus};
    crate::llm_settings::sync_ocr_enabled(window, state.settings.ocr_enabled);
    let manager = &o.resources;
    let info = manager.info.as_ref();
    let base_busy = manager
        .busy
        .filter(|op| op.kind() == lexift_core::ports::ocr::OcrResourceKind::Base);
    let korean_busy = manager
        .busy
        .filter(|op| op.kind() == lexift_core::ports::ocr::OcrResourceKind::Korean);
    let korean = info.and_then(|r| r.korean.as_deref());
    window.set_ocr_korean_auto(state.desired_settings.ocr_korean_auto);
    window.set_ocr_korean_auto_enabled(
        (korean.is_some_and(|r| r.ready) || state.desired_settings.ocr_korean_auto)
            && manager.busy.is_none()
            && state.settings_saving_field
                != Some(lexift_core::domain::settings::SettingsField::OcrKoreanAuto),
    );
    let korean_bytes = korean.map_or(0, |r| r.download_bytes);
    window.set_ocr_korean_ready(korean.is_some_and(|r| r.ready));
    window.set_ocr_korean_version(korean.map_or("—", |r| r.version.as_str()).into());
    window
        .set_ocr_korean_download_size(format!("{:.1} MiB", korean_bytes as f64 / 1048576.).into());
    window.set_ocr_korean_download_exact(
        format!(
            "{:.1} MiB ({} bytes)",
            korean_bytes as f64 / 1048576.,
            korean_bytes
        )
        .into(),
    );
    window.set_ocr_korean_installed_size(
        format!(
            "{:.1} MiB",
            korean.map_or(0, |r| r.installed_bytes) as f64 / 1048576.
        )
        .into(),
    );
    window.set_ocr_korean_damaged(korean.is_some_and(|r| r.status == OcrResourceStatus::Damaged));
    window.set_ocr_korean_downloading(korean_busy.is_some_and(ResourceOperation::downloading));
    window.set_ocr_korean_progress(
        (manager.received as f64 / korean_bytes.max(1) as f64).clamp(0., 1.) as f32,
    );
    window.set_ocr_korean_status(resource_status(korean, korean_busy).into());
    window.set_ocr_korean_status_kind(resource_status_kind(korean, korean_busy));
    let korean_error = manager
        .last_operation
        .is_some_and(|op| op.kind() == lexift_core::ports::ocr::OcrResourceKind::Korean)
        && !manager.error.is_empty();
    window.set_ocr_korean_note_error(korean_error);
    window.set_ocr_korean_note(if korean_error {
        crate::i18n::error(&manager.error).into()
    } else {
        "".into()
    });
    let downloading =
        base_busy == Some(ResourceOperation::Download) || o.phase == OcrPhase::Downloading;
    let pending = info.is_some_and(|i| i.status == OcrResourceStatus::PendingRemoval);
    let active = matches!(
        o.phase,
        OcrPhase::Checking | OcrPhase::Capturing | OcrPhase::Recognizing
    );
    window.set_ocr_korean_can_download(
        manager.busy.is_none()
            && !active
            && info.is_some_and(|r| r.ready)
            && !korean.is_some_and(|r| r.ready),
    );
    window.set_ocr_korean_can_remove(
        manager.busy.is_none() && !active && korean.is_some_and(|r| r.installed_bytes > 0),
    );
    window.set_ocr_downloading(downloading);
    window.set_ocr_resource_busy(base_busy.is_some());
    window.set_ocr_can_download(
        manager.busy.is_none() && !active && !pending && !info.is_some_and(|i| i.ready),
    );
    window.set_ocr_can_remove(
        manager.busy.is_none()
            && !active
            && info.is_some_and(|i| {
                i.installed_bytes > 0 || i.status == OcrResourceStatus::PendingRemoval
            }),
    );
    window.set_ocr_resource_damaged(info.is_some_and(|i| i.status == OcrResourceStatus::Damaged));
    let bytes = info.map_or(o.download_bytes, |i| i.download_bytes);
    window.set_ocr_download_size(
        format!("{:.1} MiB ({} bytes)", bytes as f64 / 1048576., bytes).into(),
    );
    window.set_ocr_download_size_compact(format!("{:.1} MiB", bytes as f64 / 1048576.).into());
    window.set_ocr_resource_ready(info.is_some_and(|i| i.ready));
    window.set_ocr_resource_status_kind(if base_busy.is_some() || downloading {
        2
    } else {
        match info.map(|i| i.status) {
            Some(OcrResourceStatus::Ready) => 1,
            Some(OcrResourceStatus::Damaged) => 3,
            Some(OcrResourceStatus::PendingRemoval) => 4,
            _ => 0,
        }
    });
    let received = if base_busy == Some(ResourceOperation::Download) {
        manager.received
    } else {
        o.downloaded_bytes
    };
    window.set_ocr_download_progress((received as f64 / bytes.max(1) as f64).clamp(0., 1.) as f32);
    window.set_ocr_resource_version(info.map_or("—", |i| i.version.as_str()).into());
    window.set_ocr_installed_size(
        format!(
            "{:.1} MiB",
            info.map_or(0, |i| i.installed_bytes) as f64 / 1048576.
        )
        .into(),
    );
    let mut details = Vec::new();
    if let Some(info) = info {
        for (title, prefix) in [
            ("Detection model", "det."),
            ("Multilingual recognition", "universal."),
            ("Shared runtime and licenses", ""),
        ] {
            let bytes: u64 = info
                .entries
                .iter()
                .filter(|e| {
                    if prefix.is_empty() {
                        !["det.", "universal.", "korean."]
                            .iter()
                            .any(|p| e.id.starts_with(p))
                    } else {
                        e.id.starts_with(prefix)
                    }
                })
                .map(|e| e.bytes)
                .sum();
            details.push(crate::OcrResourceRow {
                label: crate::i18n::tr(title).into(),
                size: format!("{:.1} MiB", bytes as f64 / 1048576.).into(),
            });
        }
    }
    window.set_ocr_model_rows(std::rc::Rc::new(slint::VecModel::from(details)).into());
    let status = if let Some(operation) = base_busy {
        match operation {
            ResourceOperation::Check => crate::i18n::tr("Checking OCR resources"),
            ResourceOperation::Remove | ResourceOperation::RemoveKorean => {
                crate::i18n::tr("Deleting OCR resources")
            }
            ResourceOperation::Download | ResourceOperation::DownloadKorean => format!(
                "{} — {:.0}%",
                crate::i18n::tr("Downloading OCR resources"),
                manager.received as f64 / bytes.max(1) as f64 * 100.
            ),
        }
    } else {
        crate::i18n::tr(match info.map(|i| i.status) {
            Some(OcrResourceStatus::PendingRemoval) => {
                "OCR resources will be removed after restart"
            }
            Some(OcrResourceStatus::Damaged) => "OCR resources are damaged",
            Some(OcrResourceStatus::Ready) => "OCR resources ready",
            _ => "OCR resources not installed",
        })
    };
    window.set_ocr_status(status.into());
    let note = if !manager.error.is_empty() && !korean_error {
        crate::i18n::error(&manager.error)
    } else if active {
        crate::i18n::tr("Finish the current OCR task before managing resources")
    } else if pending {
        crate::i18n::tr("Restart Lexift to finish deletion before downloading again")
    } else if base_busy == Some(ResourceOperation::Download) {
        crate::i18n::tr("Cancel the download before deleting resources")
    } else if !o.error.is_empty() {
        crate::i18n::error(&o.error)
    } else {
        String::new()
    };
    window.set_ocr_resource_note_error(
        (!manager.error.is_empty() && !korean_error)
            || (!active
                && !pending
                && !downloading
                && !o.error.is_empty()
                && o.error != "OCR resources ready. Press the shortcut again"),
    );
    window.set_ocr_resource_note(note.into());
    if o.phase == OcrPhase::DownloadRequired
        && window.get_ocr_confirm_generation() != o.generation as i32
    {
        window.set_ocr_confirm_generation(o.generation as i32);
        window.set_active_category(5);
        use slint::Model;
        if let Some(index) = window
            .get_provider_navigation()
            .iter()
            .position(|p| p.provider_id == "paddleocr")
        {
            window.invoke_credential_hide_requested();
            window.set_provider_detail_index(index as i32);
            window.invoke_llm_service_selected(index as i32);
        }
        window.set_ocr_resource_target(
            if o.required_resource == lexift_core::ports::ocr::OcrResourceKind::Korean {
                1
            } else {
                0
            },
        );
        window.set_ocr_download_dialog(true);
    }
}
fn resource_status_kind(
    info: Option<&lexift_core::ports::ocr::OcrResources>,
    operation: Option<lexift_core::domain::ocr::ResourceOperation>,
) -> i32 {
    if operation.is_some() {
        return 2;
    }
    use lexift_core::ports::ocr::OcrResourceStatus;
    match info.map(|r| r.status) {
        Some(OcrResourceStatus::Ready) => 1,
        Some(OcrResourceStatus::Damaged) => 3,
        Some(OcrResourceStatus::PendingRemoval) => 4,
        _ => 0,
    }
}
fn resource_status(
    info: Option<&lexift_core::ports::ocr::OcrResources>,
    operation: Option<lexift_core::domain::ocr::ResourceOperation>,
) -> String {
    use lexift_core::{domain::ocr::ResourceOperation, ports::ocr::OcrResourceStatus};
    crate::i18n::tr(match operation {
        Some(ResourceOperation::Check) => "Checking OCR resources",
        Some(op) if op.downloading() => "Downloading OCR resources",
        Some(_) => "Deleting OCR resources",
        None => match info.map(|r| r.status) {
            Some(OcrResourceStatus::Ready) => "OCR resources ready",
            Some(OcrResourceStatus::Damaged) => "OCR resources are damaged",
            Some(OcrResourceStatus::PendingRemoval) => {
                "OCR resources will be removed after restart"
            }
            _ => "OCR resources not installed",
        },
    })
}

fn prepare(generation: u64) {
    let result = STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let r = slot.as_mut()?;
        if r.generation != generation {
            return None;
        }
        r.attempts += 1;
        for Host { window, .. } in &r.windows {
            match (r.prepare)(window.window()) {
                PassiveWindowPreparation::Ready => {}
                PassiveWindowPreparation::Pending if r.attempts < 20 => return None,
                _ => return Some(false),
            }
        }
        let frames = r.frames.as_ref()?;
        for (host, frame) in r.windows.iter().zip(frames.iter()) {
            match host.prepare(&r.lifecycle, Some(frame)) {
                PassiveWindowPreparation::Ready => {}
                PassiveWindowPreparation::Pending if r.attempts < 20 => return None,
                _ => return Some(false),
            }
        }
        // All native windows must be ready before any screen is covered.
        for host in &r.windows {
            let window = &host.window;
            let bounds = &host.bounds;
            let bounds = *bounds;
            let weak = window.as_weak();
            let sink =
                host.input().pointer_sink(move |event| {
                    let Some(window) = weak.upgrade() else { return };
                    let weak = window.as_weak();
                    let _ =
                        slint::invoke_from_event_loop(move || {
                            let Some(window) = weak.upgrade() else { return };
                            let current =
                                STATE.with(|s| {
                                    s.borrow().as_ref().is_some_and(|r| {
                                        r.generation == generation
                                && r.windows.iter().any(|Host {window: w, bounds: b, ..}| {
                                    *b == bounds && std::ptr::eq(w.window(), window.window())
                                })
                                    })
                                });
                            if current {
                                pointer(generation, bounds, window.window().scale_factor(), event);
                            }
                        });
                });
            if !(r.lifecycle.complete_toolbar_show)(window.window(), sink)
                || !(r.lifecycle.ocr_dismissal)(window.window(), true)
            {
                return Some(false);
            }
        }
        for (index, host) in r.windows.iter().enumerate() {
            let window = &host.window;
            let bounds = &host.bounds;
            // Reapply physical geometry after native non-client sizing was removed.
            window
                .window()
                .set_position(slint::PhysicalPosition::new(bounds.left, bounds.top));
            window.window().set_size(slint::PhysicalSize::new(
                (bounds.right - bounds.left) as u32,
                (bounds.bottom - bounds.top) as u32,
            ));
            if host.show().is_err() {
                return Some(false);
            }
            if r.native_handoff[index] {
                schedule_handoff(
                    generation,
                    *bounds,
                    window.as_weak(),
                    Arc::new(AtomicBool::new(false)),
                );
            }
        }
        r.frames = None;
        r.timer.stop();
        Some(true)
    });
    if result == Some(false) {
        close();
        emit(OcrEvent::Failed {
            generation,
            error: "Could not prepare OCR selection".into(),
        });
    }
}
/// AfterRendering runs before the renderer's present returns. Queue the native
/// handoff for a later event-loop turn and validate the owning window instance.
fn schedule_handoff(
    generation: u64,
    bounds: Rect,
    weak: slint::Weak<OcrSelectionWindow>,
    done: Arc<AtomicBool>,
) {
    let _ = slint::invoke_from_event_loop(move || finish_handoff(generation, bounds, weak, done));
}
fn finish_handoff(
    generation: u64,
    bounds: Rect,
    weak: slint::Weak<OcrSelectionWindow>,
    done: Arc<AtomicBool>,
) {
    if done.load(Ordering::Relaxed) {
        return;
    }
    let Some(window) = weak.upgrade() else {
        return;
    };
    let callback = STATE.with(|slot| {
        let slot = slot.borrow();
        let r = slot.as_ref()?;
        (r.generation == generation
            && r.phase == OcrPhase::Capturing
            && r.windows.iter().any(
                |Host {
                     window: current,
                     bounds: b,
                     ..
                 }| {
                    *b == bounds && std::ptr::eq(current.window(), window.window())
                },
            ))
        .then(|| r.lifecycle.complete_region_frame.clone())
    });
    if let Some(callback) = callback
        && window.window().is_visible()
        && callback(window.window())
    {
        STATE.with(|s| {
            if let Some(r) = s.borrow().as_ref() {
                for host in &r.windows {
                    if std::ptr::eq(host.window.window(), window.window()) {
                        host.presented();
                    }
                }
            }
        });
        done.store(true, Ordering::Relaxed);
        crate::bridge::repair_window_paint(window.window());
    }
}

fn pointer(generation: u64, bounds: Rect, _scale: f32, event: PopupPointerInput) {
    // Native passive input owns capture; forward hover only so Slint can select its crosshair.
    if let Some(window) = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|r| r.generation == generation)
            .and_then(|r| {
                r.windows
                    .iter()
                    .find(|host| host.bounds == bounds)
                    .map(|host| host.window.clone_strong())
            })
    }) {
        let scale = window.window().scale_factor();
        match event {
            PopupPointerInput::Moved { x, y } => {
                window
                    .window()
                    .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(x / scale, y / scale),
                    })
            }
            PopupPointerInput::Exited => window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerExited),
            _ => {}
        }
    }
    let mut outcome = None;
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        if r.generation != generation || r.windows.is_empty() {
            return;
        }
        let point = |x: f32, y: f32| {
            let (x, y) = crate::screen_interaction::screen_point(bounds, (x, y), 1.);
            Point {
                x: x.round() as i32,
                y: y.round() as i32,
            }
        };
        match event {
            PopupPointerInput::LeftPressed { x, y } => r.gesture.begin(point(x, y)),
            PopupPointerInput::LeftReleased { x, y } => {
                if let Some(selection) = r
                    .gesture
                    .finish(point(x, y), SelectionContext::standalone(generation))
                {
                    outcome = Some(OcrEvent::SelectionSubmitted { selection });
                } else {
                    for Host { window, .. } in &r.windows {
                        window.set_selecting(false);
                    }
                }
            }
            PopupPointerInput::RightPressed
            | PopupPointerInput::LeftCancelled { .. }
            | PopupPointerInput::DismissRequested { .. } => outcome = Some(OcrEvent::Cancelled),
            PopupPointerInput::Moved { x, y } => {
                r.gesture.update(point(x, y));
                for Host {
                    window, bounds: b, ..
                } in &r.windows
                {
                    if let Some(preview) = r.gesture.preview(*b) {
                        let scale = window.window().scale_factor();
                        window.set_selecting(true);
                        let (left, top) = crate::screen_interaction::local_point(
                            *b,
                            (preview.left as f32, preview.top as f32),
                            scale,
                        );
                        window.set_selection_left(left);
                        window.set_selection_top(top);
                        window.set_selection_width((preview.right - preview.left) as f32 / scale);
                        window.set_selection_height((preview.bottom - preview.top) as f32 / scale);
                    }
                }
            }
            _ => {}
        }
    });
    if let Some(event) = outcome {
        close();
        emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn lifecycle() -> WindowLifecycleCallbacks {
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        lifecycle.complete_toolbar_show = Rc::new(|_, _| true);
        lifecycle.ocr_dismissal = Rc::new(|_, _| true);
        lifecycle.prepare_screen_window = Rc::new(|_, _, _| PassiveWindowPreparation::Ready);
        lifecycle.complete_region_frame = Rc::new(|_| true);
        lifecycle
    }
    fn frames() -> Arc<Vec<CapturedRegion>> {
        Arc::new(vec![
            CapturedRegion {
                bounds: Rect {
                    left: -100,
                    top: 0,
                    right: 0,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
        ])
    }
    #[test]
    fn any_native_preparation_failure_destroys_the_group_before_showing() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let mut l = lifecycle();
        l.prepare_screen_window = Rc::new(|w, _, f| {
            let f = f.unwrap();
            assert!(!w.is_visible());
            if f.bounds.left == 0 {
                PassiveWindowPreparation::Failed
            } else {
                PassiveWindowPreparation::Ready
            }
        });
        init(|_| PassiveWindowPreparation::Ready, l);
        sync(3, OcrPhase::Capturing);
        show(3, frames());
        assert!(active());
        prepare(3);
        assert!(!active());
    }
    #[test]
    fn cancelled_preparation_and_old_same_generation_frame_cannot_touch_reopened_windows() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let calls = Rc::new(std::cell::Cell::new(0));
        let c = calls.clone();
        let mut l = lifecycle();
        l.complete_region_frame = Rc::new(move |_| {
            c.set(c.get() + 1);
            true
        });
        init(|_| PassiveWindowPreparation::Ready, l);
        sync(3, OcrPhase::Capturing);
        show(3, frames());
        sync(4, OcrPhase::Idle);
        prepare(3);
        assert!(!active());
        sync(5, OcrPhase::Capturing);
        show(5, frames());
        prepare(5);
        let old = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[0]
                .window
                .clone_strong()
        });
        show(5, frames());
        prepare(5);
        finish_handoff(
            5,
            frames()[0].bounds,
            old.as_weak(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(calls.get(), 0);
        let current = STATE.with(|s| {
            s.borrow().as_ref().unwrap().windows[0]
                .window
                .clone_strong()
        });
        finish_handoff(
            5,
            frames()[0].bounds,
            current.as_weak(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(calls.get(), 1);
        close();
        finish_handoff(
            5,
            frames()[0].bounds,
            current.as_weak(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(calls.get(), 1);
    }
    #[test]
    fn all_overlays_show_after_preparation_and_stale_frames_cannot_reopen_them() {
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let mut lifecycle = WindowLifecycleCallbacks::new(
            |_, _| true,
            |_| true,
            |_| true,
            |_, _, _| true,
            |_, _| true,
            |_, _| true,
            || true,
        );
        lifecycle.complete_toolbar_show = Rc::new(|_, _| true);
        lifecycle.ocr_dismissal = Rc::new(|_, _| true);
        lifecycle.prepare_screen_window = Rc::new(|_, _, _| PassiveWindowPreparation::Ready);
        lifecycle.complete_region_frame = Rc::new(|_| true);
        init(|_| PassiveWindowPreparation::Ready, lifecycle);
        sync(7, OcrPhase::Capturing);
        let frames = Arc::new(vec![
            CapturedRegion {
                bounds: Rect {
                    left: -100,
                    top: 0,
                    right: 0,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
            CapturedRegion {
                bounds: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 100,
                },
                pixels: vec![255; 100 * 100 * 4],
            },
        ]);
        show(7, frames.clone());
        prepare(7);
        STATE.with(|s| {
            assert!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .windows
                    .iter()
                    .all(|host| host.window.window().is_visible())
            )
        });
        let events = Rc::new(RefCell::new(vec![]));
        let e = events.clone();
        set_handler(Rc::new(move |event| e.borrow_mut().push(event)));
        pointer(
            7,
            frames[0].bounds,
            1.,
            PopupPointerInput::LeftPressed { x: 90., y: 80. },
        );
        pointer(
            7,
            frames[0].bounds,
            1.,
            PopupPointerInput::LeftReleased { x: 110., y: 20. },
        );
        assert!(!active());
        assert!(matches!(
            &events.borrow()[0],
            AppEvent::Ocr(OcrEvent::SelectionSubmitted {
                selection: lexift_core::domain::region_selection::RegionSelectionResult {
                    context: SelectionContext {
                        source_session: 7,
                        request_id: 7,
                        snapshot_id: 7
                    },
                    bounds: Rect {
                        left: -10,
                        top: 20,
                        right: 10,
                        bottom: 80
                    }
                }
            })
        ));
        sync(8, OcrPhase::Idle);
        show(7, frames);
        assert!(!active());
    }
}
