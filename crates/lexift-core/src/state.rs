#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    pub status: String,
}

impl AppState {
    pub fn ready(status: impl Into<String>) -> Self {
        Self {
            status: status.into(),
        }
    }
}
