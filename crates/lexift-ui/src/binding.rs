use crate::AppWindow;

pub(crate) fn apply(window: &AppWindow, status: String) {
    window.set_status_text(status.into());
}
