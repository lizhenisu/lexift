pub mod capabilities;
mod credential;
#[cfg(feature = "mock")]
mod mock;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use capabilities::PlatformCapabilities;

/// The outer-corner treatment available for the translation popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupCornerMode {
    /// Windows applies the rounded clip to the native window surface.
    NativeRounded,
    /// Keep the stable opaque HWND surface without drawing a second rounded edge.
    OpaqueSquare,
    /// Let the UI draw and clip its own rounded outer edge.
    SlintRounded,
}

/// A native pointer event captured from a translation popup tool window.
///
/// Positions and scroll deltas use physical client-area pixels. The UI adapter converts them to
/// its logical coordinate system before dispatching them to the rendering backend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PopupPointerEvent {
    Moved {
        x: f32,
        y: f32,
    },
    Exited,
    LeftPressed {
        x: f32,
        y: f32,
    },
    LeftReleased {
        x: f32,
        y: f32,
    },
    Scrolled {
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    },
    /// The user continued working outside an unpinned popup.
    DismissRequested,
    /// The native client area changed size, in physical pixels.
    Resized {
        width: f32,
        height: f32,
    },
    /// The native interactive resize loop ended at this client size in physical pixels.
    ResizeFinished {
        width: f32,
        height: f32,
    },
}

/// The edge or corner used to resize a translation popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Popup outer-window size limits in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupResizeBounds {
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
}

pub type PopupPointerHandler = Box<dyn Fn(PopupPointerEvent) + 'static>;

/// Reports whether a passive tool window has a native handle and is ready to be shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveToolWindowPreparation {
    Ready,
    Pending,
}

/// Prevents a passive tool window from activating or appearing in the task switcher.
pub fn configure_passive_tool_window(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<PassiveToolWindowPreparation> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::configure_passive(window)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        Ok(PassiveToolWindowPreparation::Ready)
    }
}

/// Configures the Translation Popup's one outer-corner treatment.
///
/// Windows uses DWM clipping when available and reports an opaque square fallback when it is not.
/// Other platforms continue to use the Slint-rounded surface. Other tool windows do not use this.
pub fn configure_translation_popup_corners(
    window: &impl raw_window_handle::HasWindowHandle,
) -> PopupCornerMode {
    #[cfg(target_os = "windows")]
    {
        match windows::popup::configure_translation_popup_corners(window) {
            Ok(()) => PopupCornerMode::NativeRounded,
            Err(error) => {
                tracing::debug!(%error, "native translation popup corner preference unavailable");
                PopupCornerMode::OpaqueSquare
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        PopupCornerMode::SlintRounded
    }
}

/// Makes a passively shown tool window accept normal activation on the next user click.
///
/// The native style transition itself uses `SWP_NOACTIVATE`, so completing the show does not
/// take focus away from the application where the translation was requested.
pub fn enable_tool_window_interaction(
    window: &impl raw_window_handle::HasWindowHandle,
    pointer_handler: PopupPointerHandler,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::enable_interaction_without_activation(window, pointer_handler)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, pointer_handler);
    Ok(())
}

/// Attaches a tool-window surface to its owning application window.
pub fn attach_tool_window(
    child: &impl raw_window_handle::HasWindowHandle,
    owner: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::attach_owner(child, owner)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (child, owner);
    Ok(())
}

/// Enables or disables automatic dismissal for a visible tool-window surface.
///
/// On Windows this watches pointer presses outside the popup and subsequent foreground-window
/// changes without intercepting either input path.
pub fn set_popup_dismissal(
    window: &impl raw_window_handle::HasWindowHandle,
    enabled: bool,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::set_dismissal(window, enabled)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, enabled);
    Ok(())
}

/// Restores and activates a top-level window after an explicit user request.
///
/// Passive windows such as the translation popup must not use this function.
pub fn activate_user_requested_window(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::activate_user_requested(window)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
    Ok(())
}

/// Starts the native system move operation for a custom title bar.
pub fn begin_window_drag(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    windows::popup::begin_drag(window)?;
    #[cfg(not(target_os = "windows"))]
    let _ = window;
    Ok(())
}

/// Asks Windows to evict this process's resident pages after an extended no-window idle period.
///
/// This only changes the resident working set. It does not decommit private allocations or
/// recreate Slint's backend; per-window renderer resources are released with their windows.
pub fn trim_process_working_set() -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    windows::memory::trim_working_set()?;
    Ok(())
}

/// Starts the native system resize operation for a popup source-card handle.
///
/// Returns `false` when the initiating left-button press has already ended.
#[cfg(target_os = "windows")]
pub fn begin_window_resize(
    window: &impl raw_window_handle::HasWindowHandle,
    edge: PopupResizeEdge,
    bounds: PopupResizeBounds,
) -> lexift_core::Result<bool> {
    windows::popup::begin_resize(window, edge, bounds)
}

/// Reports unsupported native popup resizing on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
pub fn begin_window_resize(
    _window: &impl raw_window_handle::HasWindowHandle,
    _edge: PopupResizeEdge,
    _bounds: PopupResizeBounds,
) -> lexift_core::Result<bool> {
    Ok(false)
}
