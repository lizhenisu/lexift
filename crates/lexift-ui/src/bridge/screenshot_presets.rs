use super::*;
use crate::{PresetRow, ScreenshotPresetsWindow};
use lexift_core::domain::{
    screenshot::{LastRegion, ScreenshotEvent, ShortcutTrigger},
    screenshot_presets::{self, Edit, Preset},
    settings::SettingsChange,
};
use slint::ComponentHandle;

struct Panel {
    window: ScreenshotPresetsWindow,
    region: Option<LastRegion>,
    pending: Option<u64>,
    generation: u64,
}
struct Registry {
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    handler: Option<Rc<dyn Fn(AppEvent)>>,
    panel: Option<Panel>,
    presets: Vec<Preset>,
    busy: bool,
    generation: u64,
    request: u64,
}
thread_local! { static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) }; }

pub(super) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
    presets: Vec<Preset>,
) {
    REGISTRY.with(|r| {
        *r.borrow_mut() = Some(Registry {
            prepare,
            lifecycle,
            handler: None,
            panel: None,
            presets,
            busy: false,
            generation: 0,
            request: 0,
        })
    });
}
pub(super) fn set_handler(handler: Rc<dyn Fn(AppEvent)>) {
    REGISTRY.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.handler = Some(handler);
        }
    });
}
pub(super) fn active() -> bool {
    REGISTRY.with(|r| r.borrow().as_ref().is_some_and(|r| r.panel.is_some()))
}
pub(super) fn apply_theme() {
    REGISTRY.with(|r| {
        if let Some(p) = r.borrow().as_ref().and_then(|r| r.panel.as_ref()) {
            crate::theme::apply(&p.window);
        }
    });
}

pub(crate) fn close() {
    REGISTRY.with(|r| {
        if let Some(panel) = r.borrow_mut().as_mut().and_then(|r| r.panel.take()) {
            let _ = panel.window.hide();
        }
    });
    schedule_idle_memory_trim();
}

pub(super) fn shutdown() {
    let retired = REGISTRY.with(|r| r.borrow_mut().take());
    if let Some(panel) = retired.as_ref().and_then(|r| r.panel.as_ref()) {
        let _ = panel.window.hide();
    }
}

pub(crate) fn exclusion() -> Option<lexift_core::ports::magnifier::WindowToken> {
    REGISTRY.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        (r.lifecycle.annotation_window_token)(r.panel.as_ref()?.window.window())
    })
}

fn layout(r: &Registry) -> Vec<lexift_core::domain::geometry::Rect> {
    (r.lifecycle.screenshot_displays)()
        .into_iter()
        .map(|(bounds, _)| bounds)
        .collect()
}

fn render(r: &Registry) {
    let Some(p) = &r.panel else { return };
    let layout = layout(r);
    let rows = r
        .presets
        .iter()
        .map(|preset| {
            let available = screenshot_presets::matches_layout(&preset.region.layout, &layout)
                && screenshot_presets::covers(preset.region.bounds, &layout);
            let b = preset.region.bounds;
            let suffix = if available { "" } else { " · " };
            let status = if available {
                String::new()
            } else {
                crate::i18n::tr("Display layout changed. Reselect the screenshot region.")
            };
            PresetRow {
                label: format!(
                    "{} · {} × {}{suffix}{status}",
                    preset.name,
                    i64::from(b.right) - i64::from(b.left),
                    i64::from(b.bottom) - i64::from(b.top)
                )
                .into(),
                available,
            }
        })
        .collect::<Vec<_>>();
    p.window.set_callable(p.region.is_none() && !r.busy);
    p.window.set_busy(p.pending.is_some());
    p.window
        .set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

pub(super) fn sync(state: &AppState) {
    REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        r.busy = !lexift_core::domain::screenshot::delay_available(state)
            || state.editor_ocr.request.is_some();
        let selection = r
            .panel
            .as_ref()
            .and_then(|p| usize::try_from(p.window.get_selected()).ok())
            .and_then(|i| r.presets.get(i))
            .map(|p| p.id);
        r.presets = state.settings.screenshot_presets.clone();
        if let Some(panel) = &r.panel {
            panel.window.set_selected(
                selection
                    .and_then(|id| r.presets.iter().position(|p| p.id == id))
                    .map_or(-1, |i| i as i32),
            );
        }
        render(r);
    });
}

pub(crate) fn open(region: Option<LastRegion>) {
    close();
    let prepared = REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
        let r = slot.as_mut()?;
        let window =
            crate::screen_interaction::create(&r.lifecycle, ScreenshotPresetsWindow::new).ok()?;
        crate::theme::apply(&window);
        r.generation += 1;
        let generation = r.generation;
        window.set_saving_region(region.is_some());
        window.on_action(move |action| {
            slint::Timer::single_shot(Duration::ZERO, move || act(generation, action))
        });
        window.window().on_close_requested(move || {
            slint::Timer::single_shot(Duration::ZERO, move || act(generation, 5));
            slint::CloseRequestResponse::KeepWindowShown
        });
        let point = (r.lifecycle.toolbar_cursor_position)()?;
        let area = (r.lifecycle.popup_work_area)(point)?;
        window
            .window()
            .set_position(slint::PhysicalPosition::new(area.left + 30, area.top + 40));
        let foreground = r
            .lifecycle
            .annotation_foreground
            .as_ref()
            .and_then(|query| query());
        r.panel = Some(Panel {
            window,
            region,
            pending: None,
            generation,
        });
        render(r);
        Some((generation, foreground))
    });
    if let Some((generation, foreground)) = prepared {
        cancel_idle_memory_trim();
        prepare(generation, foreground, 0);
    }
}

fn prepare(
    generation: u64,
    foreground: Option<lexift_core::ports::magnifier::WindowToken>,
    attempt: u8,
) {
    let result = REGISTRY.with(|r| {
        let r = r.borrow();
        let r = r.as_ref()?;
        let p = r.panel.as_ref().filter(|p| p.generation == generation)?;
        let w = &p.window;
        Some(match (r.prepare)(w.window()) {
            PassiveWindowPreparation::Ready => {
                let weak = w.as_weak();
                let sink = Rc::new(move |event| {
                    if let Some(w) = weak.upgrade() {
                        dispatch_pointer_input(w.window(), event);
                    }
                });
                let shown = (r.lifecycle.exclude_view_from_capture)(w.window())
                    && w.show().is_ok()
                    && (r.lifecycle.complete_passive_window_show)(w.window(), sink);
                if shown
                    && foreground.is_some()
                    && r.lifecycle.annotation_foreground.as_ref().and_then(|q| q()) == foreground
                {
                    (r.lifecycle.activate_user_requested_window)(w.window());
                }
                Some(shown)
            }
            PassiveWindowPreparation::Pending if attempt < 20 => None,
            _ => Some(false),
        })
    });
    match result {
        Some(None) => slint::Timer::single_shot(Duration::from_millis(16), move || {
            prepare(generation, foreground, attempt + 1)
        }),
        Some(Some(false)) => close(),
        _ => {}
    }
}

fn act(generation: u64, action: i32) {
    let task = REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
        let r = slot.as_mut()?;
        let current_layout = layout(r);
        let p = r.panel.as_mut().filter(|p| p.generation == generation)?;
        if action == 5 {
            return Some(None);
        }
        if p.pending.is_some() {
            return None;
        }
        let index = usize::try_from(p.window.get_selected()).ok();
        let selected = index.and_then(|i| r.presets.get(i));
        let name = p.window.get_name().to_string();
        let event = if action == 0 {
            if r.busy || p.region.is_some() {
                return None;
            }
            let preset = selected?;
            if !screenshot_presets::matches_layout(&preset.region.layout, &current_layout) {
                p.window.set_feedback(
                    crate::i18n::tr("Display layout changed. Reselect the screenshot region.")
                        .into(),
                );
                return None;
            }
            AppEvent::Screenshot(ScreenshotEvent::PresetRequested {
                id: preset.id,
                layout: current_layout,
                trigger: ShortcutTrigger {
                    point: (r.lifecycle.toolbar_cursor_position)(),
                    foreground: None,
                },
            })
        } else {
            let edit = match action {
                1 => Edit::Create {
                    name,
                    region: p.region.clone()?,
                },
                2 => Edit::Update {
                    id: selected?.id,
                    region: p.region.clone()?,
                },
                3 => Edit::Rename {
                    id: selected?.id,
                    name,
                },
                4 if p.window.get_confirming() => Edit::Delete { id: selected?.id },
                _ => return None,
            };
            let mut expected = r.presets.clone();
            if let Err(message) = screenshot_presets::apply(&mut expected, &edit) {
                p.window.set_feedback(crate::i18n::tr(message).into());
                return None;
            }
            if expected == r.presets {
                return None;
            }
            r.request += 1;
            p.pending = Some(r.request);
            p.window.set_busy(true);
            AppEvent::SettingsChangeRequested {
                change: SettingsChange::ScreenshotPresets {
                    request: r.request,
                    edit,
                },
            }
        };
        Some(Some((r.handler.clone()?, event)))
    });
    match task {
        Some(Some((handler, mut event))) => {
            if let AppEvent::Screenshot(ScreenshotEvent::PresetRequested { trigger, .. }) =
                &mut event
            {
                close();
                trigger.foreground = REGISTRY.with(|r| {
                    r.borrow()
                        .as_ref()?
                        .lifecycle
                        .annotation_foreground
                        .as_ref()
                        .and_then(|query| query())
                });
            }
            handler(event);
        }
        Some(None) => close(),
        None => {}
    }
}

pub(super) fn completed(request: u64, saved: bool) {
    REGISTRY.with(|r| {
        let mut slot = r.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(p) = r.panel.as_mut().filter(|p| p.pending == Some(request)) else {
            return;
        };
        p.pending = None;
        p.window.set_feedback(if saved {
            "".into()
        } else {
            crate::i18n::tr("Could not save screenshot preset").into()
        });
        if saved {
            p.window.set_confirming(false);
        }
        render(r);
    });
}
