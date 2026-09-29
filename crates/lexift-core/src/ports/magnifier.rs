//! A live screen magnification capability for transient annotation objects.
use crate::{Result, domain::annotation::Bounds};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowToken(pub isize);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagnifierSpec {
    pub id: u64,
    /// A drawing preview keeps the native view warm without exposing its pixels.
    pub preview: bool,
    pub source: Bounds,
    pub output: Bounds,
    pub zoom: f32,
    pub ellipse: bool,
    pub antialias: bool,
}

pub trait MagnifierPort: Send + Sync {
    /// Synchronizes native views with the current transient annotation document.
    fn sync(&self, views: &[MagnifierSpec], overlay_windows: &[WindowToken]) -> Result<()>;
    fn refresh(&self) -> Result<()>;
    fn clear(&self);
}
