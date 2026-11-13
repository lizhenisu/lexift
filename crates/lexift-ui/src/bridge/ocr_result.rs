use super::*;
use slint::ComponentHandle;
use std::sync::atomic::AtomicBool;

pub type CopyResultText =
    Arc<dyn Fn(String, Arc<AtomicBool>, Box<dyn FnOnce(bool) + Send>) + Send + Sync>;
type Preparation = fn(&slint::Window) -> PassiveWindowPreparation;

struct ResultView {
    window: crate::OcrResultWindow,
    cancel: Arc<AtomicBool>,
    copy: CopyResultText,
    request: u64,
    busy: bool,
}
impl Drop for ResultView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.window.hide();
    }
}

thread_local! {
    static SERVICES: RefCell<Option<(Preparation, WindowLifecycleCallbacks)>> = const { RefCell::new(None) };
    static VIEW: RefCell<Option<ResultView>> = const { RefCell::new(None) };
}

pub(super) fn init(prepare: Preparation, lifecycle: WindowLifecycleCallbacks) {
    SERVICES.with(|s| *s.borrow_mut() = Some((prepare, lifecycle)));
}

pub(super) fn active() -> bool {
    VIEW.with(|v| v.borrow().is_some())
}

pub(super) fn apply_theme() {
    VIEW.with(|v| {
        if let Some(view) = v.borrow().as_ref() {
            crate::theme::apply(&view.window);
        }
    });
}

pub(crate) fn exclusion() -> Option<lexift_core::ports::magnifier::WindowToken> {
    let (_, lifecycle) = SERVICES.with(|s| s.borrow().clone())?;
    VIEW.with(|v| (lifecycle.annotation_window_token)(v.borrow().as_ref()?.window.window()))
}

pub(super) fn close() {
    VIEW.with(|v| {
        v.borrow_mut().take();
    });
    super::schedule_idle_memory_trim();
}

fn selected_text(text: &str, anchor: i32, cursor: i32) -> Option<&str> {
    let start = usize::try_from(anchor.min(cursor)).ok()?;
    let end = usize::try_from(anchor.max(cursor)).ok()?;
    text.get(start..end).filter(|s| !s.is_empty())
}

fn copy_text(identity: &Arc<AtomicBool>, range: Option<(i32, i32)>) {
    let task = VIEW.with(|v| {
        let mut v = v.borrow_mut();
        let view = v
            .as_mut()
            .filter(|view| !view.busy && Arc::ptr_eq(&view.cancel, identity))?;
        let content = view.window.get_result_text();
        let text = match range {
            Some((anchor, cursor)) => selected_text(content.as_str(), anchor, cursor)?,
            None => content.as_str(),
        }
        .to_owned();
        view.busy = true;
        view.request += 1;
        Some((view.copy.clone(), text, view.cancel.clone(), view.request))
    });
    if let Some((copy, text, cancel, request)) = task {
        let identity = cancel.clone();
        copy(
            text,
            cancel,
            Box::new(move |success| {
                let _ = slint::invoke_from_event_loop(move || {
                    VIEW.with(|v| {
                        if let Some(view) = v.borrow_mut().as_mut().filter(|view| {
                            Arc::ptr_eq(&view.cancel, &identity) && view.request == request
                        }) {
                            view.busy = false;
                            let message = if success {
                                "Recognized text copied"
                            } else {
                                "Could not copy recognized text"
                            };
                            view.window.set_feedback(crate::i18n::tr(message).into());
                        }
                    });
                });
            }),
        );
    }
}

pub(super) fn show(text: lexift_core::domain::ocr::ResultText, copy: CopyResultText) {
    let Some((_, lifecycle)) = SERVICES.with(|s| s.borrow().clone()) else {
        return;
    };
    let Ok(window) = crate::screen_interaction::create(&lifecycle, crate::OcrResultWindow::new)
    else {
        return;
    };
    crate::theme::apply(&window);
    window.set_result_text(text.0.into());
    let cancel = Arc::new(AtomicBool::new(false));
    let identity = cancel.clone();
    window.on_close_result(move || post_close(identity.clone()));
    let identity = cancel.clone();
    window.window().on_close_requested(move || {
        post_close(identity.clone());
        slint::CloseRequestResponse::KeepWindowShown
    });
    let identity = cancel.clone();
    window.on_copy_selection(move |anchor, cursor| copy_text(&identity, Some((anchor, cursor))));
    let identity = cancel.clone();
    window.on_copy_all(move || copy_text(&identity, None));
    let area =
        (lifecycle.toolbar_cursor_position)().and_then(|point| (lifecycle.popup_work_area)(point));
    VIEW.with(|v| {
        *v.borrow_mut() = Some(ResultView {
            window,
            cancel: cancel.clone(),
            copy,
            request: 0,
            busy: false,
        })
    });
    super::cancel_idle_memory_trim();
    prepare(cancel, area, 0);
}

fn post_close(identity: Arc<AtomicBool>) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        if current(&identity) {
            close();
        }
    });
}

fn current(identity: &Arc<AtomicBool>) -> bool {
    VIEW.with(|v| {
        v.borrow()
            .as_ref()
            .is_some_and(|view| Arc::ptr_eq(&view.cancel, identity))
    })
}

fn prepare(
    identity: Arc<AtomicBool>,
    area: Option<lexift_core::domain::geometry::Rect>,
    attempt: u8,
) {
    let Some((prepare_window, lifecycle)) = SERVICES.with(|s| s.borrow().clone()) else {
        return;
    };
    let Some(window) = VIEW.with(|v| {
        v.borrow()
            .as_ref()
            .filter(|v| Arc::ptr_eq(&v.cancel, &identity))
            .map(|v| v.window.clone_strong())
    }) else {
        return;
    };
    match prepare_window(window.window()) {
        PassiveWindowPreparation::Ready => {
            if let Some(area) = area {
                let dpi = window.window().scale_factor();
                let width = (640. * dpi).min((area.right - area.left) as f32 * 0.9) as u32;
                let height = (420. * dpi).min((area.bottom - area.top) as f32 * 0.9) as u32;
                window
                    .window()
                    .set_size(slint::PhysicalSize::new(width, height));
                window.window().set_position(slint::PhysicalPosition::new(
                    area.left + (area.right - area.left - width as i32) / 2,
                    area.top + (area.bottom - area.top - height as i32) / 2,
                ));
            }
            let weak = window.as_weak();
            let sink = Rc::new(move |input| {
                if let Some(window) = weak.upgrade() {
                    super::dispatch_pointer_input(window.window(), input);
                }
            });
            if !(lifecycle.exclude_view_from_capture)(window.window())
                || window.show().is_err()
                || !(lifecycle.complete_passive_window_show)(window.window(), sink)
            {
                close();
            }
        }
        PassiveWindowPreparation::Failed => close(),
        _ if attempt < 20 => slint::Timer::single_shot(Duration::from_millis(16), move || {
            prepare(identity, area, attempt + 1)
        }),
        _ => close(),
    }
}

#[test]
fn selected_text_preserves_utf8_and_line_breaks_in_both_directions() {
    let text = "前\nOCR";
    assert_eq!(selected_text(text, 5, 0), Some("前\nO"));
    assert_eq!(selected_text(text, 1, 5), None);
    assert_eq!(selected_text(text, 3, 3), None);
}
