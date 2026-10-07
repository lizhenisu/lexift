use super::geometry::{Point, Rect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub text: String,
    pub anchor: Option<Point>,
    /// Visible selected text rectangles in physical virtual-screen coordinates.
    pub bounds: Vec<Rect>,
    /// Original mouse-down point for conservative placement when bounds are absent.
    pub gesture_start: Option<Point>,
}

/// Native input target for a passive selection request. Identity includes the
/// foreground window, focused control and owning process/thread, never text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionTarget {
    pub window: crate::ports::magnifier::WindowToken,
    pub focus: Option<crate::ports::magnifier::WindowToken>,
    pub process: u32,
    pub thread: u32,
}
