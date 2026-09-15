use crate::{
    Result,
    domain::geometry::{Point, Rect},
};

/// Provides physical screen coordinates without exposing platform window types.
pub trait ScreenPort: Send + Sync {
    fn cursor_position(&self) -> Result<Point>;
    fn work_area_for_point(&self, point: Point) -> Result<Rect>;
}
