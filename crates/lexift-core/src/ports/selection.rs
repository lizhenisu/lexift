use crate::{Result, domain::selection::Selection};

pub trait SelectionPort: Send + Sync {
    fn selected_text(&self) -> Result<Option<Selection>>;
}
