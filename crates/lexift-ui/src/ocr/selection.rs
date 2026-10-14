//! OCR consumer of the shared frozen-screen selection host.
use crate::{
    PassiveWindowPreparation, WindowLifecycleCallbacks, screen_interaction::selection as host,
};
use lexift_core::{
    AppEvent,
    domain::ocr::{OcrEvent, OcrPhase},
    ports::screen_capture::CapturedRegion,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};
type Handler = Rc<dyn Fn(AppEvent)>;
thread_local! { static STATE: RefCell<(u64, OcrPhase, bool, Option<Handler>)> = const { RefCell::new((0,OcrPhase::Idle,false,None)) }; }
pub(crate) fn init(
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    lifecycle: WindowLifecycleCallbacks,
) {
    host::init(prepare, lifecycle);
}
pub(crate) fn set_handler(handler: Handler) {
    STATE.with(|s| s.borrow_mut().3 = Some(handler));
}
pub(crate) fn active() -> bool {
    STATE.with(|s| s.borrow().2) && host::active()
}
pub(crate) fn screen_busy() -> bool {
    STATE.with(|s| matches!(s.borrow().1, OcrPhase::Checking | OcrPhase::Capturing))
}
pub(crate) fn request() {
    crate::annotation::pin_editor::finish_now();
    if crate::annotation::screenshot::active() {
        return;
    }
    emit(OcrEvent::Requested {
        annotation_active: crate::annotation::is_open(),
    });
}
fn emit(event: OcrEvent) {
    let h = STATE.with(|s| s.borrow().3.clone());
    if let Some(h) = h {
        h(AppEvent::Ocr(event));
    }
}
pub(crate) fn close() {
    let owned = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let owned = s.2;
        s.2 = false;
        owned
    });
    if owned {
        host::close();
    }
}
pub(crate) fn sync(generation: u64, phase: OcrPhase) {
    if STATE.with(|s| {
        let s = s.borrow();
        s.2 && (s.0 != generation || phase != OcrPhase::Capturing)
    }) {
        close();
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.0 = generation;
        s.1 = phase;
    });
}
pub(crate) fn show(generation: u64, frames: Arc<Vec<CapturedRegion>>) {
    if !STATE.with(|s| {
        let s = s.borrow();
        s.0 == generation && s.1 == OcrPhase::Capturing
    }) {
        return;
    }
    STATE.with(|s| s.borrow_mut().2 = true);
    host::set_screenshot(false);
    host::set_handler(Rc::new(|event| {
        emit(match event {
            host::SelectionEvent::Cancelled => OcrEvent::Cancelled,
            host::SelectionEvent::Failed { generation, error } => {
                OcrEvent::Failed { generation, error }
            }
            host::SelectionEvent::SelectionSubmitted { selection } => {
                OcrEvent::SelectionSubmitted { selection }
            }
        })
    }));
    host::sync(generation, host::HostPhase::Capturing);
    host::show(generation, frames);
}
