use crate::{Result, domain::selection::Selection};

pub trait SelectionPort {
    fn selected_text(&self) -> Result<Option<Selection>>;
}
