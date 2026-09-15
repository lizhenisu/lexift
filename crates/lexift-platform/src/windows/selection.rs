use lexift_core::{Error, Result, domain::selection::Selection, ports::selection::SelectionPort};
use std::time::Instant;
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

#[derive(Debug, Default)]
struct SelectionCaptureMetrics {
    uia_latency_ms: u128,
    clipboard_latency_ms: Option<u128>,
    total_capture_ms: u128,
}

/// Reads selected text from the foreground Windows application through UI Automation.
pub(crate) struct WindowsSelectionPort;

impl WindowsSelectionPort {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl SelectionPort for WindowsSelectionPort {
    fn selected_text(&self) -> Result<Option<Selection>> {
        let started_at = Instant::now();
        let anchor = super::screen::cursor_position().ok();
        let uia_started_at = Instant::now();
        let uia_result = capture_with_uia();
        let mut metrics = SelectionCaptureMetrics {
            uia_latency_ms: uia_started_at.elapsed().as_millis(),
            ..Default::default()
        };
        let result = capture_with_fallback(uia_result, || {
            let clipboard_started_at = Instant::now();
            let result = super::clipboard_selection::capture_selected_text();
            metrics.clipboard_latency_ms = Some(clipboard_started_at.elapsed().as_millis());
            result
        });
        metrics.total_capture_ms = started_at.elapsed().as_millis();

        tracing::debug!(
            uia_latency_ms = metrics.uia_latency_ms,
            clipboard_latency_ms = metrics.clipboard_latency_ms,
            total_capture_ms = metrics.total_capture_ms,
            success = result.is_ok(),
            "Windows selection capture finished"
        );

        result.map(|text| text.map(|text| Selection { text, anchor }))
    }
}

fn capture_with_uia() -> Result<Option<String>> {
    let _apartment = ComApartment::initialize()?;
    let automation = create_automation()?;
    probe_foreground_window(&automation)?;
    let focused = unsafe { automation.GetFocusedElement() }
        .map_err(|error| uia_error(error, "Could not access the focused Windows control"))?;

    selected_text_from_ancestors(&automation, focused)
}

fn capture_with_fallback<F>(
    uia_result: Result<Option<String>>,
    clipboard_capture: F,
) -> Result<Option<String>>
where
    F: FnOnce() -> Result<Option<String>>,
{
    match uia_result {
        Ok(Some(text)) => {
            tracing::debug!(strategy = "uia", "Windows selection capture succeeded");
            Ok(Some(text))
        }
        Ok(None) => {
            tracing::debug!(strategy = "clipboard", "UI Automation found no selection");
            clipboard_capture()
        }
        Err(uia_error) => {
            tracing::warn!(error = %uia_error, "UI Automation selection capture failed; trying clipboard");
            match clipboard_capture() {
                Ok(Some(text)) => Ok(Some(text)),
                Ok(None) => Err(uia_error),
                Err(clipboard_error) => {
                    tracing::warn!(error = %clipboard_error, "Clipboard selection fallback also failed");
                    Err(Error::new("Could not capture selected text"))
                }
            }
        }
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
    use std::cell::Cell;

    use lexift_core::Error;

    use super::{capture_with_fallback, merge_selected_ranges};

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

    #[test]
    fn uia_selection_does_not_invoke_clipboard_fallback() {
        let invoked = Cell::new(false);

        let result = capture_with_fallback(Ok(Some("selected".into())), || {
            invoked.set(true);
            Ok(None)
        });

        assert_eq!(result, Ok(Some("selected".into())));
        assert!(!invoked.get());
    }

    #[test]
    fn empty_uia_selection_uses_clipboard_fallback() {
        let result = capture_with_fallback(Ok(None), || Ok(Some("copied".into())));

        assert_eq!(result, Ok(Some("copied".into())));
    }

    #[test]
    fn two_empty_strategies_report_no_selection() {
        let result = capture_with_fallback(Ok(None), || Ok(None));

        assert_eq!(result, Ok(None));
    }

    #[test]
    fn clipboard_error_is_returned_after_empty_uia_selection() {
        let result = capture_with_fallback(Ok(None), || Err(Error::new("clipboard failed")));

        assert_eq!(result, Err(Error::new("clipboard failed")));
    }

    #[test]
    fn clipboard_success_recovers_from_uia_error() {
        let result =
            capture_with_fallback(Err(Error::new("uia failed")), || Ok(Some("copied".into())));

        assert_eq!(result, Ok(Some("copied".into())));
    }

    #[test]
    fn original_uia_error_wins_when_clipboard_finds_nothing() {
        let result = capture_with_fallback(Err(Error::new("uia failed")), || Ok(None));

        assert_eq!(result, Err(Error::new("uia failed")));
    }

    #[test]
    fn two_strategy_errors_are_combined_into_a_stable_error() {
        let result = capture_with_fallback(Err(Error::new("uia failed")), || {
            Err(Error::new("clipboard failed"))
        });

        assert_eq!(result, Err(Error::new("Could not capture selected text")));
    }
}
