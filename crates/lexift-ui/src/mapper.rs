use lexift_core::AppState;

pub(crate) fn status_text(state: &AppState) -> String {
    state.status.clone()
}
