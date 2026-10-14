//! Window-free editor document shared by live annotation and frozen screenshots.
use super::*;
#[derive(Clone)]
pub(super) struct EditorDocument {
    pub session: Session,
    pub mode: InteractionMode,
    pub selected: [i32; 7],
    pub parameter_values: parameters::State,
}
impl Default for EditorDocument {
    fn default() -> Self {
        let mut session = Session::new();
        session.set_edit_scope(EditScope::Geometry);
        Self {
            session,
            mode: InteractionMode::Tool(0),
            selected: [0, 4, 7, 9, 10, 11, 13],
            parameter_values: parameters::State::new(),
        }
    }
}
