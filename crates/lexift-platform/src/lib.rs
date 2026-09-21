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

pub type WindowContextDismissHandler = std::sync::Arc<dyn Fn() + Send + Sync + 'static>;

/// Watches an armed owner context for real external interaction.
pub struct WindowContextMonitor {
    #[cfg(target_os = "windows")]
    inner: windows::WindowsWindowContextMonitor,
}

impl WindowContextMonitor {
    pub fn new(handler: WindowContextDismissHandler) -> lexift_core::Result<Self> {
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                inner: windows::WindowsWindowContextMonitor::new(handler)?,
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = handler;
            Ok(Self {})
        }
    }

    pub fn arm_context(
        &self,
        windows: &[&dyn raw_window_handle::HasWindowHandle],
    ) -> lexift_core::Result<()> {
        #[cfg(target_os = "windows")]
        {
            self.inner.arm_context(windows)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = windows;
            Ok(())
        }
    }

    pub fn disarm(&self) {
        #[cfg(target_os = "windows")]
        self.inner.disarm();
    }
}

/// Prevents a passive tool window from activating or appearing in the task switcher.
pub fn configure_passive_tool_window(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::configure_passive(window)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
    Ok(())
}

/// Configures a user-invoked transient window as an interactive tool window.
pub fn configure_interactive_tool_window(
    window: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::configure_interactive(window)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
    Ok(())
}

/// Assigns a logical owner to a transient top-level window.
pub fn set_transient_window_owner(
    child: &impl raw_window_handle::HasWindowHandle,
    owner: &impl raw_window_handle::HasWindowHandle,
) -> lexift_core::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::popup::set_transient_owner(child, owner)?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (child, owner);
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
