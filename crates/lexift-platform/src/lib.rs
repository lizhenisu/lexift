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

/// Reports whether the supplied window currently owns foreground activation.
pub fn is_foreground_window(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<bool> {
    #[cfg(target_os = "windows")]
    return windows::popup::is_foreground(window);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        Ok(true)
    }
}
