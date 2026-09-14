use lexift_core::{Error, Result, domain::selection::Selection, ports::selection::SelectionPort};
use windows::{
    Win32::{
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
            UIA_TextPatternId,
        },
        UI::WindowsAndMessaging::GetForegroundWindow,
    },
    core::IUnknown,
};

const MAX_PARENT_DEPTH: usize = 8;

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
        probe_foreground_window(&automation)?;
        let focused = unsafe { automation.GetFocusedElement() }
            .map_err(|error| uia_error(error, "Could not access the focused Windows control"))?;

        selected_text_from_ancestors(&automation, focused)
            .map(|text| text.map(|text| Selection { text, anchor: None }))
    }
}

fn probe_foreground_window(automation: &IUIAutomation) -> Result<()> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return Err(Error::new("Could not access the foreground Windows window"));
    }

    // Chromium may not expose its usable accessibility provider until the native
    // foreground window has been resolved through UI Automation at least once.
    unsafe { automation.ElementFromHandle(foreground) }
        .map(|_| ())
        .map_err(|error| uia_error(error, "Could not access the foreground Windows window"))
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| uia_error(error, "Could not initialize Windows UI Automation"))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn create_automation() -> Result<IUIAutomation> {
    // CUIAutomation has broader proxy-provider compatibility than CUIAutomation8.
    // Browser accessibility providers are part of the compatibility matrix here.
    unsafe { CoCreateInstance(&CUIAutomation, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
        .map_err(|error| uia_error(error, "Could not create the Windows UI Automation client"))
}

fn selected_text_from_ancestors(
    automation: &IUIAutomation,
    focused: IUIAutomationElement,
) -> Result<Option<String>> {
    if let Some(text) = text_pattern_selection(&focused) {
        return Ok(Some(text));
    }

    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| uia_error(error, "Could not navigate the Windows accessibility tree"))?;
    let mut element = focused;

    for _ in 0..MAX_PARENT_DEPTH {
        element = match unsafe { walker.GetParentElement(&element) } {
            Ok(parent) => parent,
            Err(error) => {
                trace_unavailable_pattern("get Control View parent", &error);
                break;
            }
        };

        if let Some(text) = text_pattern_selection(&element) {
            return Ok(Some(text));
        }
    }

    Ok(None)
}

fn text_pattern_selection(element: &IUIAutomationElement) -> Option<String> {
    let pattern =
        match unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
        {
            Ok(pattern) => pattern,
            Err(error) => {
                trace_unavailable_pattern("get TextPattern", &error);
                return None;
            }
        };

    let ranges = match unsafe { pattern.GetSelection() } {
        Ok(ranges) => ranges,
        Err(error) => {
            trace_unavailable_pattern("get TextPattern selection", &error);
            return None;
        }
    };
    let length = match unsafe { ranges.Length() } {
        Ok(length) => length,
        Err(error) => {
            trace_unavailable_pattern("get TextPattern range count", &error);
            return None;
        }
    };
    let mut selected_ranges = Vec::with_capacity(length.max(0) as usize);

    for index in 0..length {
        let range = match unsafe { ranges.GetElement(index) } {
            Ok(range) => range,
            Err(error) => {
                trace_unavailable_pattern("get TextPattern range", &error);
                continue;
            }
        };
        match unsafe { range.GetText(-1) } {
            Ok(text) => selected_ranges.push(text.to_string()),
            Err(error) => trace_unavailable_pattern("read TextPattern range", &error),
        }
    }

    merge_selected_ranges(selected_ranges)
}

fn trace_unavailable_pattern(operation: &'static str, error: &windows::core::Error) {
    tracing::debug!(
        operation,
        hresult = format_args!("{:#010X}", error.code().0 as u32),
        "Windows UI Automation selection path was unavailable"
    );
}

fn uia_error(error: windows::core::Error, message: &'static str) -> Error {
    tracing::warn!(
        operation = message,
        hresult = format_args!("{:#010X}", error.code().0 as u32),
        "Windows UI Automation call failed"
    );
    Error::new(message)
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
