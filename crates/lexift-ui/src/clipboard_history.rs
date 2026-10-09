//! A history session owns its passive window, timers and callbacks independently of the toolbar.
use super::{
    CompletePassiveWindowShow, PassiveWindowPreparation, PopupPointerInput, PopupWorkArea,
    SetPopupDismissal,
};
use crate::{ClipboardHistoryRow, ClipboardHistoryWindow};
use lexift_core::{
    AppEvent, AppState,
    domain::{
        clipboard_history::{HistoryEvent, PasteTargetState, UNSUPPORTED_TARGET_MESSAGE},
        geometry::Point,
    },
};
use slint::{ComponentHandle, Model};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
thread_local! {pub(super) static HISTORY: RefCell<Option<Registry>> = const { RefCell::new(None) };}
struct Work {
    alive: Cell<bool>,
    drag_offset: Cell<Option<(f32, f32)>>,
    timer: slint::Timer,
    input: slint::Timer,
    toast: slint::Timer,
    toast_seen: Cell<u64>,
    pending: RefCell<Vec<AppEvent>>,
    id: u64,
}
impl Work {
    fn show_target_toast(self: &Rc<Self>, window: &ClipboardHistoryWindow, attempt: u64) {
        if attempt == 0 || self.toast_seen.replace(attempt) == attempt {
            return;
        }
        window.set_target_toast(crate::i18n::tr(UNSUPPORTED_TARGET_MESSAGE).into());
        let weak = Rc::downgrade(self);
        let window = window.as_weak();
        self.toast.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(5),
            move || {
                if weak
                    .upgrade()
                    .is_some_and(|w| w.alive.get() && w.toast_seen.get() == attempt)
                    && let Some(window) = window.upgrade()
                {
                    window.set_target_toast("".into());
                }
            },
        );
    }
    fn post(self: &Rc<Self>, event: AppEvent) {
        if !self.alive.get() {
            return;
        }
        self.pending.borrow_mut().push(event);
        if self.input.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.input
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                let Some(work) = weak.upgrade().filter(|w| w.alive.get()) else {
                    return;
                };
                let events = std::mem::take(&mut *work.pending.borrow_mut());
                for event in events {
                    let handler = HISTORY.with(|s| {
                        s.borrow()
                            .as_ref()
                            .filter(|r| r.work.as_ref().is_some_and(|w| Rc::ptr_eq(w, &work)))
                            .and_then(|r| r.handler.clone())
                    });
                    if let Some(h) = handler {
                        let settings = matches!(
                            event,
                            AppEvent::ClipboardHistory(HistoryEvent::OpenSettings { .. })
                        );
                        let terminal = settings
                            || matches!(
                                event,
                                AppEvent::ClipboardHistory(HistoryEvent::Close { .. })
                            );
                        h(event);
                        if settings {
                            let _ = slint::invoke_from_event_loop(|| {
                                super::APP_WINDOW_REGISTRY.with(|r| {
                                    if let Some(w) =
                                        r.borrow().as_ref().and_then(|r| r.settings.as_ref())
                                    {
                                        w.set_active_category(0);
                                    }
                                })
                            });
                        }
                        if terminal {
                            work.alive.set(false);
                            work.pending.borrow_mut().clear();
                            break;
                        }
                    }
                }
            });
    }
}
pub(super) struct Registry {
    pub window: Option<ClipboardHistoryWindow>,
    pub handler: Option<Rc<dyn Fn(AppEvent)>>,
    pub prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    pub complete: CompletePassiveWindowShow,
    pub dismiss: SetPopupDismissal,
    pub area: PopupWorkArea,
    work: Option<Rc<Work>>,
    anchor: Point,
    copy_seen: std::collections::BTreeMap<u64, u64>,
    rows: Rc<slint::VecModel<ClipboardHistoryRow>>,
}
impl Registry {
    pub fn new(
        prepare: fn(&slint::Window) -> PassiveWindowPreparation,
        complete: CompletePassiveWindowShow,
        dismiss: SetPopupDismissal,
        area: PopupWorkArea,
    ) -> Self {
        Self {
            window: None,
            handler: None,
            prepare,
            complete,
            dismiss,
            area,
            work: None,
            anchor: Point { x: 0, y: 0 },
            copy_seen: Default::default(),
            rows: Rc::new(slint::VecModel::default()),
        }
    }
    pub fn close(&mut self) {
        self.copy_seen.clear();
        self.rows.set_vec(Vec::new());
        if let Some(w) = self.work.take() {
            w.alive.set(false);
            w.drag_offset.set(None);
            w.timer.stop();
            w.input.stop();
            w.toast.stop();
            w.pending.borrow_mut().clear();
        }
        if let Some(w) = self.window.take() {
            let _ = (self.dismiss)(w.window(), false);
            let _ = w.hide();
        }
    }
    pub fn sync(&mut self, state: &AppState) {
        let Some(session) = state.clipboard_history.session.as_ref() else {
            self.close();
            return;
        };
        if self.work.as_ref().is_none_or(|w| w.id != session.id) {
            self.close();
            super::cancel_idle_memory_trim();
            let Ok(window) = ClipboardHistoryWindow::new() else {
                return;
            };
            crate::theme::apply(&window);
            window.set_entries(slint::ModelRc::from(self.rows.clone()));
            let work = Rc::new(Work {
                alive: Cell::new(true),
                drag_offset: Cell::new(None),
                timer: Default::default(),
                input: Default::default(),
                toast: Default::default(),
                toast_seen: Cell::new(0),
                pending: Default::default(),
                id: session.id,
            });
            let id = session.id;
            let action = work.clone();
            let weak = window.as_weak();
            window.on_target_toast_dismissed(move || {
                if action.alive.get() {
                    action.toast.stop();
                    if let Some(window) = weak.upgrade() {
                        window.set_target_toast("".into());
                    }
                }
            });
            let action = work.clone();
            let weak = window.as_weak();
            window.on_header_drag_requested(move |x, y| {
                if action.alive.get()
                    && let Some(window) = weak.upgrade()
                {
                    let scale = window.window().scale_factor();
                    action.drag_offset.set(Some((x * scale, y * scale)));
                }
            });
            let action = work.clone();
            window.on_close_requested(move || {
                action.post(AppEvent::ClipboardHistory(HistoryEvent::Close {
                    session: id,
                }))
            });
            let action = work.clone();
            window.window().on_close_requested(move || {
                action.post(AppEvent::ClipboardHistory(HistoryEvent::Close {
                    session: id,
                }));
                slint::CloseRequestResponse::KeepWindowShown
            });
            let action = work.clone();
            window.on_enable_requested(move || {
                action.post(AppEvent::ClipboardHistory(HistoryEvent::Enable {
                    session: id,
                }))
            });
            let action = work.clone();
            window.on_settings_requested(move || {
                action.post(AppEvent::ClipboardHistory(HistoryEvent::OpenSettings {
                    session: id,
                }));
            });
            let action = work.clone();
            window.on_paste_requested(move |entry| {
                if let Ok(entry) = entry.parse() {
                    action.post(AppEvent::ClipboardHistory(HistoryEvent::Paste {
                        session: id,
                        entry,
                    }));
                }
            });
            let action = work.clone();
            let copy_action = work.clone();
            window.on_copy_requested(move |entry| {
                if let Ok(entry) = entry.parse() {
                    copy_action.post(AppEvent::ClipboardHistory(HistoryEvent::Copy {
                        session: id,
                        entry,
                    }));
                }
            });
            window.on_delete_requested(move |entry| {
                if let Ok(entry) = entry.parse() {
                    action.post(AppEvent::ClipboardHistory(HistoryEvent::Delete {
                        session: id,
                        entry,
                    }));
                }
            });
            self.anchor = session.anchor;
            self.window = Some(window);
            self.work = Some(work.clone());
            schedule(&work, 0);
        }
        if let Some(w) = &self.window {
            let rows: Vec<_> = state
                .clipboard_history
                .entries
                .iter()
                .map(|e| ClipboardHistoryRow {
                    id: e.id.to_string().into(),
                    preview: e.text.chars().take(1000).collect::<String>().into(),
                    copy_pending: session.copy.get(&e.id).is_some_and(|c| c.pending),
                    copied: session.copy.get(&e.id).is_some_and(|c| c.completed != 0),
                })
                .collect();
            if !rows.iter().any(|row| row.id == w.get_delete_confirmation()) {
                w.set_delete_confirmation("".into());
            }
            self.copy_seen.retain(|entry, _| {
                state
                    .clipboard_history
                    .entries
                    .iter()
                    .any(|e| e.id == *entry)
            });
            for (entry, copy) in &session.copy {
                if copy.completed != 0 && self.copy_seen.get(entry) != Some(&copy.completed) {
                    self.copy_seen.insert(*entry, copy.completed);
                    let work = Rc::downgrade(self.work.as_ref().unwrap());
                    let session = session.id;
                    let entry = *entry;
                    let request = copy.completed;
                    slint::Timer::single_shot(Duration::from_millis(1500), move || {
                        if let Some(work) = work.upgrade() {
                            work.post(AppEvent::ClipboardHistory(
                                HistoryEvent::CopyFeedbackCleared {
                                    session,
                                    entry,
                                    request,
                                },
                            ));
                        }
                    });
                }
            }
            // Retain delegates across feedback updates so a timeout between press/release
            // cannot discard the button or row that owns the active pointer interaction.
            while self.rows.row_count() > rows.len() {
                self.rows.remove(self.rows.row_count() - 1);
            }
            for (index, row) in rows.into_iter().enumerate() {
                if index == self.rows.row_count() {
                    self.rows.push(row);
                } else if self.rows.row_data(index).as_ref() != Some(&row) {
                    self.rows.set_row_data(index, row);
                }
            }
            w.set_recording(
                state.settings.clipboard_history_enabled && state.clipboard_history.recording,
            );
            w.set_first_use(
                !state.settings.clipboard_history_enabled
                    && state.clipboard_history.entries.is_empty(),
            );
            w.set_saving(
                state.desired_settings.clipboard_history_enabled
                    != state.settings.clipboard_history_enabled,
            );
            w.set_ready(session.paste_target != PasteTargetState::Pending);
            if let Some(work) = self.work.as_ref() {
                work.show_target_toast(w, session.unsupported_attempt);
            }
            w.set_busy(session.busy);
            w.set_copying(session.copy.values().any(|c| c.pending));
            let error = if !session.error.is_empty() {
                &session.error
            } else {
                &state.clipboard_history.error
            };
            w.set_error_text(crate::i18n::tr(error).into());
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.close();
    }
}
fn schedule(work: &Rc<Work>, attempt: u8) {
    let weak = Rc::downgrade(work);
    work.timer.start(
        slint::TimerMode::SingleShot,
        if attempt == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(16)
        },
        move || {
            let Some(work) = weak.upgrade().filter(|w| w.alive.get()) else {
                return;
            };
            let failure = HISTORY.with(|s| {
                let mut s = s.borrow_mut();
                let r = s
                    .as_mut()
                    .filter(|r| r.work.as_ref().is_some_and(|w| Rc::ptr_eq(w, &work)))?;
                let w = r.window.as_ref()?;
                match (r.prepare)(w.window()) {
                    PassiveWindowPreparation::Pending if attempt < 19 => {
                        schedule(&work, attempt + 1);
                        return None;
                    }
                    PassiveWindowPreparation::Ready => {
                        if let Some(area) = (r.area)(r.anchor) {
                            let (p, size) =
                                history_placement(r.anchor, area, w.window().scale_factor());
                            if w.window().position() != p || w.window().size() != size {
                                w.window().set_size(size);
                                w.window().set_position(p);
                                if attempt < 19 {
                                    schedule(&work, attempt + 1);
                                    return None;
                                }
                                let handler = r.handler.clone();
                                let id = work.id;
                                r.close();
                                return handler.map(|h| (h, id));
                            }
                            if (r.complete)(w.window(), pointer(w.as_weak(), work.clone()))
                                && w.show().is_ok()
                                && (r.dismiss)(w.window(), true)
                            {
                                return None;
                            }
                        }
                    }
                    _ => {}
                }
                let handler = r.handler.clone();
                let id = work.id;
                r.close();
                handler.map(|h| (h, id))
            });
            if let Some((h, id)) = failure {
                h(AppEvent::ClipboardHistory(HistoryEvent::Close {
                    session: id,
                }));
            }
        },
    );
}
fn pointer(weak: slint::Weak<ClipboardHistoryWindow>, work: Rc<Work>) -> super::PopupPointerSink {
    Rc::new(move |input| {
        if !work.alive.get() {
            return;
        }
        let Some(w) = weak.upgrade() else {
            return;
        };
        if matches!(input, PopupPointerInput::DismissRequested { .. }) {
            work.post(AppEvent::ClipboardHistory(HistoryEvent::Close {
                session: work.id,
            }));
            return;
        }
        use slint::platform::{PointerEventButton, WindowEvent};
        let scale = w.window().scale_factor().max(0.1);
        // Native passive input already owns mouse capture. Move in physical coordinates
        // without entering a caption move loop that could activate the history window.
        if let PopupPointerInput::Moved { x, y } = input
            && let Some(offset) = work.drag_offset.get()
        {
            w.window()
                .set_position(drag_position(w.window().position(), offset, (x, y)));
            return;
        }
        if matches!(
            input,
            PopupPointerInput::LeftReleased { .. } | PopupPointerInput::LeftCancelled { .. }
        ) {
            work.drag_offset.set(None);
        }
        let p = |x, y| slint::LogicalPosition::new(x / scale, y / scale);
        let event = match input {
            PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved { position: p(x, y) },
            PopupPointerInput::Exited => WindowEvent::PointerExited,
            PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
                position: p(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::LeftReleased { x, y }
            | PopupPointerInput::LeftCancelled { x, y } => WindowEvent::PointerReleased {
                position: p(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::Scrolled {
                x,
                y,
                delta_x,
                delta_y,
            } => WindowEvent::PointerScrolled {
                position: p(x, y),
                delta_x: delta_x / scale,
                delta_y: delta_y / scale,
            },
            _ => return,
        };
        let _ = w.window().dispatch_event_with_result(event);
    })
}

fn drag_position(
    position: slint::PhysicalPosition,
    offset: (f32, f32),
    pointer: (f32, f32),
) -> slint::PhysicalPosition {
    slint::PhysicalPosition::new(
        position
            .x
            .saturating_add((pointer.0 - offset.0).round() as i32),
        position
            .y
            .saturating_add((pointer.1 - offset.1).round() as i32),
    )
}

fn history_placement(
    anchor: Point,
    area: lexift_core::domain::geometry::Rect,
    scale: f32,
) -> (slint::PhysicalPosition, slint::PhysicalSize) {
    let scale = scale.max(0.1);
    let width =
        (340.0 * scale).min((i64::from(area.right) - i64::from(area.left)).max(1) as f32) as u32;
    let height =
        (420.0 * scale).min((i64::from(area.bottom) - i64::from(area.top)).max(1) as f32) as u32;
    let x = anchor.x.clamp(
        area.left,
        area.right.saturating_sub(width as i32).max(area.left),
    );
    let y = if anchor.y.saturating_add(height as i32) <= area.bottom {
        anchor.y
    } else {
        anchor.y.saturating_sub(height as i32)
    };
    let y = y.clamp(
        area.top,
        area.bottom.saturating_sub(height as i32).max(area.top),
    );
    (
        slint::PhysicalPosition::new(x, y),
        slint::PhysicalSize::new(width, height),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use lexift_core::domain::geometry::Rect;
    #[test]
    fn history_target_toast_restarts_expires_and_cannot_escape_its_session() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct ClockPlatform(Rc<Cell<Duration>>);
        impl Platform for ClockPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(ClockPlatform(clock.clone()))).unwrap();
        let mut registry = Registry::new(
            |_| PassiveWindowPreparation::Pending,
            Rc::new(|_, _| false),
            Rc::new(|_, _| true),
            Rc::new(|_| None),
        );
        let mut state = AppState::new(Default::default());
        state.clipboard_history.session =
            Some(lexift_core::domain::clipboard_history::HistorySession {
                copy: Default::default(),
                id: 1,
                target: lexift_core::domain::selection::SelectionTarget {
                    window: lexift_core::ports::magnifier::WindowToken(1),
                    focus: None,
                    process: 2,
                    thread: 3,
                },
                anchor: Point { x: 0, y: 0 },
                paste_target: PasteTargetState::Pending,
                unsupported_attempt: 0,
                busy: false,
                request: 0,
                error: String::new(),
            });
        registry.sync(&state);
        let old = registry.window.as_ref().unwrap().clone_strong();
        assert!(!old.get_ready());
        state
            .clipboard_history
            .session
            .as_mut()
            .unwrap()
            .paste_target = PasteTargetState::Unsupported;
        registry.sync(&state);
        assert!(old.get_ready());
        assert!(old.get_target_toast().is_empty());
        assert!(old.get_error_text().is_empty());
        state
            .clipboard_history
            .session
            .as_mut()
            .unwrap()
            .unsupported_attempt = 1;
        registry.sync(&state);
        assert!(!old.get_target_toast().is_empty());
        clock.set(Duration::from_secs(4));
        slint::platform::update_timers_and_animations();
        state
            .clipboard_history
            .session
            .as_mut()
            .unwrap()
            .unsupported_attempt = 2;
        registry.sync(&state);
        clock.set(Duration::from_secs(5));
        slint::platform::update_timers_and_animations();
        assert!(
            !old.get_target_toast().is_empty(),
            "old expiry cannot clear a repeated click"
        );
        clock.set(Duration::from_secs(9));
        slint::platform::update_timers_and_animations();
        assert!(old.get_target_toast().is_empty());
        registry.sync(&state);
        assert!(
            old.get_target_toast().is_empty(),
            "state sync must not re-show expired feedback"
        );
        state
            .clipboard_history
            .session
            .as_mut()
            .unwrap()
            .unsupported_attempt = 3;
        registry.sync(&state);
        old.invoke_target_toast_dismissed();
        assert!(old.get_target_toast().is_empty());
        registry.sync(&state);
        assert!(old.get_target_toast().is_empty());
        state
            .clipboard_history
            .session
            .as_mut()
            .unwrap()
            .unsupported_attempt = 4;
        registry.sync(&state);
        let old_work = registry.work.as_ref().unwrap().clone();
        registry.close();
        assert!(!old_work.toast.running());
        clock.set(Duration::from_secs(10));
        let session = state.clipboard_history.session.as_mut().unwrap();
        session.id = 2;
        session.unsupported_attempt = 1;
        registry.sync(&state);
        let new = registry.window.as_ref().unwrap();
        old.invoke_target_toast_dismissed();
        clock.set(Duration::from_secs(14));
        slint::platform::update_timers_and_animations();
        assert!(
            !new.get_target_toast().is_empty(),
            "old window callbacks must not touch a new toast"
        );
        registry.close();
    }
    #[test]
    fn dragging_uses_physical_pointer_delta_across_negative_monitor_coordinates() {
        assert_eq!(
            drag_position(
                slint::PhysicalPosition::new(-1800, 40),
                (50., 25.),
                (150., -25.)
            ),
            slint::PhysicalPosition::new(-1700, -10)
        );
        // A 20 logical pixel movement at 125% is a 25 physical pixel delta.
        assert_eq!(
            drag_position(
                slint::PhysicalPosition::new(100, 100),
                (50., 31.25),
                (75., 56.25)
            ),
            slint::PhysicalPosition::new(125, 125)
        );
    }
    #[test]
    fn placement_stays_inside_negative_small_work_area_and_flips_above() {
        let area = Rect {
            left: -1920,
            top: -200,
            right: -1720,
            bottom: 100,
        };
        let (p, s) = history_placement(Point { x: -1800, y: 90 }, area, 1.25);
        assert_eq!(s, slint::PhysicalSize::new(200, 300));
        assert_eq!(p, slint::PhysicalPosition::new(-1920, -200));
        let (p, s) = history_placement(
            Point { x: 700, y: 550 },
            Rect {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
            1.0,
        );
        assert_eq!(s.height, 420);
        assert_eq!(p.y, 130);
        assert_eq!(p.x, 460);
    }
    #[test]
    fn retired_history_callbacks_and_preparation_cannot_touch_reopened_session() {
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
        let calls = Rc::new(Cell::new(0));
        let output = calls.clone();
        let mut registry = Registry::new(
            |_| PassiveWindowPreparation::Pending,
            Rc::new(|_, _| panic!("must stay hidden")),
            Rc::new(|_, _| true),
            Rc::new(|_| None),
        );
        registry.handler = Some(Rc::new(move |_| output.set(output.get() + 1)));
        HISTORY.with(|s| *s.borrow_mut() = Some(registry));
        let mut state = AppState::new(Default::default());
        state.clipboard_history.session =
            Some(lexift_core::domain::clipboard_history::HistorySession {
                copy: Default::default(),
                id: 1,
                target: lexift_core::domain::selection::SelectionTarget {
                    window: lexift_core::ports::magnifier::WindowToken(1),
                    focus: None,
                    process: 2,
                    thread: 3,
                },
                anchor: Point { x: 0, y: 0 },
                paste_target: PasteTargetState::Pending,
                unsupported_attempt: 0,
                busy: false,
                request: 0,
                error: String::new(),
            });
        HISTORY.with(|s| s.borrow_mut().as_mut().unwrap().sync(&state));
        let old = HISTORY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .clone_strong()
        });
        old.invoke_enable_requested();
        HISTORY.with(|s| s.borrow_mut().as_mut().unwrap().close());
        state.clipboard_history.session.as_mut().unwrap().id = 2;
        HISTORY.with(|s| s.borrow_mut().as_mut().unwrap().sync(&state));
        old.invoke_close_requested();
        old.invoke_header_drag_requested(20., 20.);
        HISTORY.with(|s| {
            assert!(
                s.borrow()
                    .as_ref()
                    .unwrap()
                    .work
                    .as_ref()
                    .unwrap()
                    .drag_offset
                    .get()
                    .is_none()
            )
        });
        old.invoke_paste_requested("1".into());
        slint::platform::update_timers_and_animations();
        assert_eq!(calls.get(), 0);
        HISTORY.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .window
                .as_ref()
                .unwrap()
                .invoke_close_requested()
        });
        slint::platform::update_timers_and_animations();
        assert_eq!(calls.get(), 1);
        HISTORY.with(|s| s.borrow_mut().take());
        old.invoke_settings_requested();
        slint::platform::update_timers_and_animations();
        assert_eq!(calls.get(), 1);
    }
}
