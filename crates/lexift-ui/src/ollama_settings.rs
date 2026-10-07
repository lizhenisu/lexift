//! Per-settings-window discovery. The injected executor only calls the Core service port.
use crate::SettingsWindow;
use lexift_core::{
    AppEvent, Result,
    domain::{
        ollama::{OllamaConfig, OllamaConnectionState, OllamaModel, OllamaModelIssue},
        settings::SettingsChange,
    },
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub type OllamaQueryCompletion = Box<dyn FnOnce(Result<Vec<OllamaModel>>) + Send>;
pub type OllamaQueryCancellation = Box<dyn Fn()>;
pub(crate) type OllamaQuery = Rc<dyn Fn(String, OllamaQueryCompletion) -> OllamaQueryCancellation>;

struct QueryTask(OllamaQueryCancellation);
impl Drop for QueryTask {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[derive(Default)]
struct PageQueries {
    version: Arc<AtomicU64>,
    task: RefCell<Option<QueryTask>>,
}
impl PageQueries {
    fn invalidate(&self) -> u64 {
        self.task.borrow_mut().take();
        self.version.fetch_add(1, Ordering::SeqCst) + 1
    }
}

pub(crate) fn wire(
    window: &SettingsWindow,
    query: Option<OllamaQuery>,
    handler: Rc<dyn Fn(AppEvent)>,
) {
    let page = Rc::new(PageQueries::default());
    let weak = window.as_weak();
    let edited_page = page.clone();
    window.on_ollama_address_edited(move || {
        edited_page.invalidate();
        if let Some(window) = weak.upgrade() {
            window.set_ollama_model("".into());
            window.set_ollama_models(ModelRc::default());
            window.set_ollama_model_index(-1);
            set_status(&window, OllamaConnectionState::Idle, "", false);
        }
    });
    let weak = window.as_weak();
    window.on_ollama_refresh_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let version = page.invalidate();
        let address = window.get_ollama_address().to_string();
        if let Err(error) = (OllamaConfig {
            address: address.clone(),
            model: String::new(),
        })
        .validate()
        {
            set_status(
                &window,
                OllamaConnectionState::Failed,
                &error.to_string(),
                true,
            );
            return;
        }
        let Some(query) = query.as_ref() else {
            set_status(
                &window,
                OllamaConnectionState::Failed,
                "Ollama service discovery is unavailable",
                true,
            );
            return;
        };
        set_status(&window, OllamaConnectionState::Checking, "Loading…", false);
        let weak = window.as_weak();
        let current = page.version.clone();
        let captured_address = address.clone();
        let cancel = query(
            address,
            Box::new(move |result| {
                let _ = weak.upgrade_in_event_loop(move |window| {
                    if !result_is_current(
                        current.load(Ordering::SeqCst),
                        version,
                        window.get_ollama_address().as_str(),
                        &captured_address,
                    ) {
                        return;
                    }
                    match result {
                        Ok(models) => apply_models(&window, models),
                        Err(error) => set_status(
                            &window,
                            OllamaConnectionState::Failed,
                            &error.to_string(),
                            true,
                        ),
                    }
                });
            }),
        );
        *page.task.borrow_mut() = Some(QueryTask(cancel));
    });
    let weak = window.as_weak();
    window.on_ollama_apply_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let config = OllamaConfig {
            address: window.get_ollama_address().to_string(),
            model: window.get_ollama_model().to_string(),
        };
        match config.validate() {
            Ok(()) => handler(AppEvent::SettingsChangeRequested {
                change: SettingsChange::Ollama(config),
            }),
            Err(error) => set_status(
                &window,
                OllamaConnectionState::Failed,
                &error.to_string(),
                true,
            ),
        }
    });
}

fn result_is_current(
    current_version: u64,
    request_version: u64,
    current_address: &str,
    request_address: &str,
) -> bool {
    current_version == request_version && current_address == request_address
}

fn set_status(window: &SettingsWindow, state: OllamaConnectionState, text: &str, error: bool) {
    window.set_ollama_checking(state == OllamaConnectionState::Checking);
    window.set_ollama_status_source(text.into());
    refresh_localized_status(window);
    window.set_ollama_status_error(error);
}

fn apply_models(window: &SettingsWindow, models: Vec<OllamaModel>) {
    let selected = window.get_ollama_model();
    let selected_issue = models
        .iter()
        .find(|m| m.name == selected.as_str())
        .and_then(|m| m.issue.as_ref());
    let usable: Vec<_> = models
        .iter()
        .filter(|m| m.issue.is_none())
        .map(|m| slint::SharedString::from(&m.name))
        .collect();
    let message = if let Some(issue) = selected_issue {
        match issue {
            OllamaModelIssue::Remote => "Cloud Ollama models are not supported",
            OllamaModelIssue::EmbeddingOnly => "Embedding-only models cannot translate text",
            OllamaModelIssue::UnknownCapabilities => {
                "Ollama model does not report text generation capability"
            }
        }
    } else if !selected.is_empty() && !models.iter().any(|m| m.name == selected.as_str()) {
        "Saved Ollama model is unavailable; choose another model"
    } else if usable.is_empty() {
        "Connected; no compatible local text generation models found"
    } else {
        "Connected; select a local model"
    };
    let mut status = message.to_owned();
    for model in &models {
        if let Some(issue) = &model.issue {
            let key = match issue {
                OllamaModelIssue::Remote => "Cloud Ollama models are not supported",
                OllamaModelIssue::EmbeddingOnly => "Embedding-only models cannot translate text",
                OllamaModelIssue::UnknownCapabilities => {
                    "Ollama model does not report text generation capability"
                }
            };
            status.push_str(&format!("\n{}: {}", model.name, key));
        }
    }
    window.set_ollama_model_index(
        usable
            .iter()
            .position(|name| name == &selected)
            .map_or(-1, |index| index as i32),
    );
    window.set_ollama_models(ModelRc::new(VecModel::from(usable)));
    set_status(
        window,
        OllamaConnectionState::Connected,
        &status,
        selected_issue.is_some()
            || (!selected.is_empty() && !models.iter().any(|m| m.name == selected.as_str())),
    );
}

pub(crate) fn select_model(window: &SettingsWindow, index: i32) {
    if let Ok(index) = usize::try_from(index)
        && let Some(name) = window.get_ollama_models().row_data(index)
    {
        window.set_ollama_model(name);
        window.set_ollama_model_index(index as i32);
        set_status(window, OllamaConnectionState::Connected, "Connected", false);
    }
}

/// Retranslate transient status when the user changes interface language in an open window.
pub(crate) fn refresh_localized_status(window: &SettingsWindow) {
    let text = window.get_ollama_status_source();
    let translated = text
        .lines()
        .map(|line| {
            if let Some((name, issue)) = line.split_once(": ") {
                format!("{name}: {}", crate::i18n::tr(issue))
            } else {
                crate::i18n::tr(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    window.set_ollama_status(translated.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::sync::atomic::AtomicUsize;
    struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for TestPlatform {
        fn create_window_adapter(
            &self,
        ) -> std::result::Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(window.clone());
            Ok(window)
        }
    }
    #[test]
    fn stale_queries_cannot_replace_another_address_or_request() {
        assert!(result_is_current(4, 4, "a", "a"));
        assert!(!result_is_current(5, 4, "a", "a"));
        assert!(!result_is_current(4, 4, "b", "a"));
    }
    #[test]
    fn page_keeps_explicit_selection_applies_atomically_and_cancels_owned_queries() {
        let adapter = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        let window = SettingsWindow::new().unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = events.clone();
        let cancellations = Arc::new(AtomicUsize::new(0));
        let cancelled = cancellations.clone();
        wire(
            &window,
            Some(Rc::new(move |_, _| {
                let cancelled = cancelled.clone();
                Box::new(move || {
                    cancelled.fetch_add(1, Ordering::SeqCst);
                })
            })),
            Rc::new(move |event| recorded.borrow_mut().push(event)),
        );
        let models = vec![
            OllamaModel {
                name: "local:small".into(),
                issue: None,
            },
            OllamaModel {
                name: "embedding".into(),
                issue: Some(OllamaModelIssue::EmbeddingOnly),
            },
        ];
        apply_models(&window, models.clone());
        assert_eq!(window.get_ollama_model(), "");
        assert_eq!(window.get_ollama_models().row_count(), 1);
        window.set_ollama_model("missing".into());
        apply_models(&window, models.clone());
        assert_eq!(window.get_ollama_model(), "missing");
        assert!(window.get_ollama_status_error());
        select_model(&window, 0);
        assert_eq!(window.get_ollama_model(), "local:small");
        window.invoke_ollama_apply_requested();
        assert!(
            matches!(&events.borrow()[0], AppEvent::SettingsChangeRequested { change: SettingsChange::Ollama(config) } if config.model == "local:small" && config.address == OllamaConfig::default().address)
        );
        window.invoke_ollama_refresh_requested();
        assert!(window.get_ollama_checking());
        window.set_ollama_address("http://localhost:11434".into());
        window.invoke_ollama_address_edited();
        assert_eq!(cancellations.load(Ordering::SeqCst), 1);
        assert!(window.get_ollama_model().is_empty());
        assert_eq!(window.get_ollama_models().row_count(), 0);
        assert!(!window.get_ollama_checking());
        window.set_ollama_address("http://remote:11434".into());
        window.invoke_ollama_apply_requested();
        assert_eq!(events.borrow().len(), 1, "invalid draft cannot be saved");
        window.set_ollama_address(OllamaConfig::default().address.into());
        apply_models(&window, models);
        window.set_draft_provider_id("ollama".into());
        window.set_active_category(5);
        let adapter = adapter.borrow().as_ref().unwrap().clone();
        if let Ok(directory) = std::env::var("LEXIFT_OLLAMA_SNAPSHOT_DIR") {
            for language in lexift_core::domain::ui_language::UiLanguage::ALL {
                crate::i18n::select(language);
                refresh_localized_status(&window);
                for (scale, width) in [(1., 960), (1.25, 1200), (1., 580)] {
                    adapter
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        });
                    adapter.set_size(slint::PhysicalSize::new(width, (900. * scale) as u32));
                    adapter.request_redraw();
                    let height = (900. * scale) as u32;
                    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                    assert!(adapter.draw_if_needed(|renderer| {
                        renderer.render(&mut pixels, width as usize);
                    }));
                    crate::i18n::write_test_bitmap(
                        &directory,
                        &format!("{}-{scale}-{width}", language.code()),
                        width,
                        height,
                        pixels.into_iter().map(|p| [p.r, p.g, p.b, 255]),
                    );
                }
            }
        }
        window.invoke_ollama_refresh_requested();
        drop(window);
        assert_eq!(
            cancellations.load(Ordering::SeqCst),
            2,
            "closing a page cancels discovery"
        );
    }
}
