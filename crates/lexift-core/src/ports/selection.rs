use crate::{Result, domain::selection::Selection};

pub trait SelectionPort: Send + Sync {
    fn selected_text(&self) -> Result<Option<Selection>>;

    /// Reads a selection without generating input or changing the clipboard.
    fn selected_text_passive(&self) -> Result<Option<Selection>> {
        Ok(None)
    }
}
