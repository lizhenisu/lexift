use lexift_core::AppState;
use slint::ComponentHandle;

use crate::{AppWindow, binding, mapper};

pub fn run(state: &AppState) -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    binding::apply(&window, mapper::status_text(state));
    window.run()
}
