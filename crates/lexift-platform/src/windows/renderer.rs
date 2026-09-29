//! Chooses the renderer for Windows Slint windows.

/// Software rendering avoids the measured FemtoVG first-window creation pause.
/// `SLINT_BACKEND` remains available for per-machine diagnostics.
pub(crate) fn prefer_software_renderer() -> bool {
    true
}
