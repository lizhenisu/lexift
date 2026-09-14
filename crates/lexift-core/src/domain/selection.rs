use super::geometry::Point;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub text: String,
    pub anchor: Option<Point>,
}
