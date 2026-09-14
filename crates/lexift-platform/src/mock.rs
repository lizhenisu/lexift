use lexift_core::{Result, domain::selection::Selection, ports::selection::SelectionPort};

pub(crate) struct MockSelectionPort;

impl SelectionPort for MockSelectionPort {
    fn selected_text(&self) -> Result<Option<Selection>> {
        Ok(Some(Selection {
            text: "Hello world".into(),
            anchor: None,
        }))
    }
}
