use lexift_core::{Error, Result, domain::selection::Selection, ports::selection::SelectionPort};
use windows::{
    Win32::{
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::Accessibility::{
            CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
            UIA_E_NOTSUPPORTED, UIA_TextPatternId,
        },
    },
    core::{HRESULT, IUnknown},
};

const MAX_PARENT_DEPTH: usize = 8;
const E_POINTER: HRESULT = HRESULT(0x8000_4003_u32 as i32);

/// Reads selected text from the foreground Windows application through UI Automation.
pub(crate) struct WindowsSelectionPort;

impl WindowsSelectionPort {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl SelectionPort for WindowsSelectionPort {
    fn selected_text(&self) -> Result<Option<Selection>> {
        let _apartment = ComApartment::initialize()?;
        let automation = create_automation()?;
        let focused = unsafe { automation.GetFocusedElement() }
            .map_err(|_| Error::new("Could not access the focused Windows control"))?;

        selected_text_from_ancestors(&automation, focused)
            .map(|text| text.map(|text| Selection { text, anchor: None }))
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|_| Error::new("Could not initialize Windows UI Automation"))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn create_automation() -> Result<IUIAutomation> {
    unsafe { CoCreateInstance(&CUIAutomation8, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
        .map_err(|_| Error::new("Could not create the Windows UI Automation client"))
}

fn selected_text_from_ancestors(
    automation: &IUIAutomation,
    mut element: IUIAutomationElement,
) -> Result<Option<String>> {
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|_| Error::new("Could not navigate the Windows accessibility tree"))?;

    for depth in 0..=MAX_PARENT_DEPTH {
        match text_pattern_selection(&element)? {
            Some(text) => return Ok(Some(text)),
            None if depth == MAX_PARENT_DEPTH => break,
            None => {}
        }

        element = match unsafe { walker.GetParentElement(&element) } {
            Ok(parent) => parent,
            Err(error) if is_unsupported(&error) || error.code() == E_POINTER => {
                break;
            }
            Err(_) => return Err(Error::new("Could not inspect the focused Windows control")),
        };
    }

    Ok(None)
}

fn text_pattern_selection(element: &IUIAutomationElement) -> Result<Option<String>> {
    let pattern =
        match unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
        {
            Ok(pattern) => pattern,
            Err(error) if is_unsupported(&error) => return Ok(None),
            Err(_) => return Err(Error::new("Could not inspect the focused Windows control")),
        };

    let ranges = unsafe { pattern.GetSelection() }
        .map_err(|_| Error::new("Could not read the selected text"))?;
    let length =
        unsafe { ranges.Length() }.map_err(|_| Error::new("Could not read the selected text"))?;
    let mut selected_ranges = Vec::with_capacity(length.max(0) as usize);

    for index in 0..length {
        let range = unsafe { ranges.GetElement(index) }
            .map_err(|_| Error::new("Could not read the selected text"))?;
        let text = unsafe { range.GetText(-1) }
            .map_err(|_| Error::new("Could not read the selected text"))?;
        selected_ranges.push(text.to_string());
    }

    Ok(merge_selected_ranges(selected_ranges))
}

fn is_unsupported(error: &windows::core::Error) -> bool {
    matches!(error.code(), HRESULT(code) if code == UIA_E_NOTSUPPORTED as i32)
}

fn merge_selected_ranges(ranges: impl IntoIterator<Item = String>) -> Option<String> {
    let ranges: Vec<_> = ranges
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect();
    (!ranges.is_empty()).then(|| ranges.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::merge_selected_ranges;

    #[test]
    fn merges_non_empty_ranges_without_changing_their_text() {
        assert_eq!(
            merge_selected_ranges([
                " first range ".to_owned(),
                " \n\t ".to_owned(),
                "second range".to_owned(),
            ]),
            Some(" first range \nsecond range".to_owned())
        );
    }

    #[test]
    fn empty_or_whitespace_only_ranges_are_not_a_selection() {
        assert_eq!(
            merge_selected_ranges([String::new(), " \n\t ".to_owned()]),
            None
        );
    }
}
