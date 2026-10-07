//! Window-owned discovery and translation drafts. All service work is injected through Core ports.
mod detection;

use crate::{ProviderModelRow, SettingsWindow};
use lexift_core::ports::llm::{LlmHeaderUpdate, LlmQueryMode};
use lexift_core::{
    AppEvent, Result,
    domain::{llm::*, runtime_config::ProviderConfig, settings::SettingsChange},
    ports::credential::CredentialSecret,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub type LlmQueryCompletion = Box<dyn FnOnce(Result<Vec<LlmModel>>) + Send>;
pub type LlmQueryCancellation = Box<dyn Fn()>;
pub(crate) type LlmQuery =
    Rc<dyn Fn(LlmConnection, LlmQueryMode, LlmQueryCompletion) -> LlmQueryCancellation>;
struct Task(LlmQueryCancellation);
impl Drop for Task {
    fn drop(&mut self) {
        (self.0)();
    }
}
/// A nested request dialog rolls back to the page draft, not to persisted settings.
struct RequestSnapshot {
    connection: LlmConnection,
    headers: Vec<LlmHeaderUpdate>,
    base: String,
    auth: i32,
}
struct Page {
    config: RefCell<LlmSettings>,
    draft: RefCell<LlmConnection>,
    pending_headers: RefCell<Vec<LlmHeaderUpdate>>,
    awaiting_apply: Cell<bool>,
    request_snapshot: RefCell<Option<RequestSnapshot>>,
    send: Rc<dyn Fn(AppEvent)>,
    cache: RefCell<BTreeMap<LlmProvider, Vec<LlmModel>>>,
    version: Arc<AtomicU64>,
    task: RefCell<Option<Task>>,
    status_source: RefCell<String>,
    probe: RefCell<Option<detection::Run>>,
}
thread_local! { static PAGES: RefCell<BTreeMap<u64, Weak<Page>>> = const { RefCell::new(BTreeMap::new()) }; }
static NEXT_PAGE: AtomicU64 = AtomicU64::new(1);
fn lookup_page(window: &SettingsWindow) -> Option<Rc<Page>> {
    PAGES.with(|pages| {
        pages
            .borrow()
            .get(&(window.get_llm_page_token() as u64))
            .and_then(Weak::upgrade)
    })
}
fn invalidate(page: &Page) {
    page.version.fetch_add(1, Ordering::SeqCst);
    page.task.borrow_mut().take();
    page.probe.borrow_mut().take();
}
fn service_kind(window: &SettingsWindow) -> LlmProvider {
    LlmProvider::ALL[window.get_llm_service_index().clamp(0, 2) as usize]
}
fn translation_kind(window: &SettingsWindow) -> Option<LlmProvider> {
    usize::try_from(window.get_llm_translation_provider_index() - 1)
        .ok()
        .and_then(|i| LlmProvider::ALL.get(i).copied())
}
fn status(window: &SettingsWindow, text: &str) {
    if let Some(page) = lookup_page(window) {
        *page.status_source.borrow_mut() = text.into();
    }
    window.set_llm_status(crate::i18n::tr(text).into());
}
fn model_list(window: &SettingsWindow, page: &Page, provider: LlmProvider) {
    let cache = page.cache.borrow();
    let usable: Vec<slint::SharedString> = cache
        .get(&provider)
        .into_iter()
        .flatten()
        .filter(|m| page.config.borrow().model_enabled(provider, &m.id))
        .map(|m| m.id.clone().into())
        .collect();
    let selected = window.get_llm_selected_model();
    window.set_llm_model_index(
        usable
            .iter()
            .position(|m| m == &selected)
            .map_or(-1, |i| i as i32),
    );
    window.set_llm_models(ModelRc::new(VecModel::from(usable)));
    let config = page.config.borrow();
    let note = if !config.connection(provider).enabled {
        "Provider is disabled"
    } else if !selected.is_empty() && !config.model_enabled(provider, selected.as_str()) {
        "Selected model is not enabled or needs confirmation in Provider settings"
    } else {
        ""
    };
    window.set_llm_selection_note(crate::i18n::tr(note).into());
}
fn populate_cache(page: &Page) {
    let mut cache = page.cache.borrow_mut();
    cache.clear();
    for m in &page.config.borrow().catalog {
        cache.entry(m.provider).or_default().push(m.model.clone());
    }
}
fn refresh_provider_states(window: &SettingsWindow, config: &LlmSettings) {
    let states = std::iter::once(config.deepl_enabled)
        .chain(LlmProvider::ALL.map(|p| config.connection(p).enabled))
        .collect::<Vec<_>>();
    window.set_provider_enabled_states(ModelRc::new(VecModel::from(states)));
}
fn refresh_catalog(window: &SettingsWindow, page: &Page) {
    refresh_provider_states(window, &page.config.borrow());
    let config = page.config.borrow();
    let provider = service_kind(window);
    let key = config.connection(provider).endpoint_key();
    let search = window.get_llm_model_search().to_lowercase();
    let filter = window.get_llm_model_filter();
    let rows = config
        .catalog
        .iter()
        .filter(|m| m.provider == provider && m.model.id.to_lowercase().contains(&search))
        .filter(|m| match filter {
            1 => m.model.issue.is_none(),
            2 => m.model.issue.as_deref() == Some("Embedding-only models cannot translate text"),
            3 => m.model.issue.as_ref().is_some_and(|issue| {
                issue.contains("unknown") || issue.contains("does not report")
            }),
            4 => !m.available || m.model.issue.is_some() || m.endpoint != key,
            _ => true,
        })
        .map(|m| {
            let usable = m.available && m.endpoint == key && m.model.issue.is_none();
            let note = if m.endpoint != key {
                "Connection changed; sync models to confirm"
            } else if !m.available {
                "Selected model is unavailable"
            } else if let Some(issue) = &m.model.issue {
                issue
            } else if m.enabled {
                "Enabled"
            } else {
                "Not enabled"
            };
            ProviderModelRow {
                id: m.model.id.clone().into(),
                note: crate::i18n::tr(note).into(),
                enabled: m.enabled,
                usable,
                text: m.model.issue.is_none(),
                thinking: m.model.reasoning.iter().any(|r| r != "off"),
            }
        })
        .collect::<Vec<_>>();
    let enabled = rows
        .iter()
        .filter(|m| m.enabled)
        .cloned()
        .collect::<Vec<_>>();
    window.set_llm_enabled_catalog(ModelRc::new(VecModel::from(enabled)));
    window.set_llm_enabled_count(
        config
            .catalog
            .iter()
            .filter(|m| m.provider == provider && m.enabled)
            .count() as i32,
    );
    window.set_llm_catalog_count(
        config
            .catalog
            .iter()
            .filter(|m| m.provider == provider)
            .count() as i32,
    );
    window.set_llm_catalog(ModelRc::new(VecModel::from(rows)));
}
fn refresh_headers(window: &SettingsWindow, page: &Page) {
    let protocol = page.draft.borrow().protocol;
    let mut names = page
        .draft
        .borrow()
        .profile()
        .headers
        .into_iter()
        .map(|h| h.name)
        .collect::<Vec<_>>();
    for update in page
        .pending_headers
        .borrow()
        .iter()
        .filter(|h| h.protocol == protocol)
    {
        names.retain(|name| !name.eq_ignore_ascii_case(&update.name));
        if update.secret.is_some() {
            names.push(update.name.clone());
        }
    }
    window.set_llm_headers(ModelRc::new(VecModel::from(
        names
            .into_iter()
            .map(Into::into)
            .collect::<Vec<slint::SharedString>>(),
    )));
}
pub(crate) fn secure_completed(window: &SettingsWindow, busy: bool, success: bool) {
    if !busy
        && window.get_credential_busy()
        && let Some(page) = lookup_page(window)
        && page.awaiting_apply.replace(false)
        && success
    {
        page.pending_headers.borrow_mut().clear();
        refresh_headers(window, &page);
        refresh_dirty(window, &page);
    }
}
fn load_profile(window: &SettingsWindow, connection: &LlmConnection) {
    let profile = connection.profile();
    window.set_llm_protocol_index(
        LlmProtocol::ALL
            .iter()
            .position(|p| *p == connection.protocol)
            .unwrap() as i32,
    );
    window.set_llm_base_url(profile.base_url.into());
    window.set_llm_auth_index(match profile.authentication {
        Authentication::Automatic => 0,
        Authentication::Headers => 1,
        Authentication::None => 2,
    });
    window.set_llm_headers(ModelRc::new(VecModel::from(
        profile
            .headers
            .into_iter()
            .map(|h| h.name.into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    update_endpoints(window, connection);
}
fn update_endpoints(window: &SettingsWindow, connection: &LlmConnection) {
    let base = window.get_llm_base_url();
    let (list, generate) = match connection.protocol {
        LlmProtocol::Native => match connection.provider {
            LlmProvider::Ollama => ("/api/tags", "/api/chat"),
            LlmProvider::LmStudio => ("/api/v1/models", "/api/v1/chat"),
            LlmProvider::DeepSeek => ("/models", "/chat/completions"),
        },
        LlmProtocol::ChatCompletions => ("/models", "/chat/completions"),
        LlmProtocol::Responses => ("/models", "/responses"),
    };
    window.set_llm_endpoints(
        format!(
            "GET {}{}\nPOST {}{}",
            base.trim_end_matches('/'),
            list,
            base.trim_end_matches('/'),
            generate
        )
        .into(),
    );
}
fn draft_connection(window: &SettingsWindow, page: &Page) -> Result<LlmConnection> {
    let mut connection = page.draft.borrow().clone();
    connection.address = service_address(window.get_llm_address().as_str())?;
    connection.enabled = window.get_llm_enabled();
    let mut profile = connection.profile();
    profile.base_url = api_base_address(window.get_llm_base_url().as_str())?;
    profile.authentication = match window.get_llm_auth_index() {
        1 => Authentication::Headers,
        2 => Authentication::None,
        _ => Authentication::Automatic,
    };
    connection
        .protocols
        .retain(|p| p.protocol != connection.protocol);
    connection.protocols.push(profile);
    connection.validate()?;
    Ok(connection)
}
fn refresh_dirty(window: &SettingsWindow, page: &Page) {
    let dirty = if window.get_provider_detail_index() == 0 {
        window.get_llm_enabled() != page.config.borrow().deepl_enabled
    } else {
        draft_connection(window, page).map_or(true, |draft| {
            let saved = page.config.borrow().connection(draft.provider);
            !draft.same_request_configuration(&saved)
                || LlmProtocol::ALL.iter().any(|protocol| {
                    let mut a = draft.clone();
                    let mut b = saved.clone();
                    a.protocol = *protocol;
                    b.protocol = *protocol;
                    a.profile() != b.profile()
                })
                || !page.pending_headers.borrow().is_empty()
        })
    };
    window.set_llm_connection_dirty(dirty);
}
fn clear_dialogs(window: &SettingsWindow, page: &Page) {
    page.request_snapshot.borrow_mut().take();
    window.set_llm_advanced(false);
    window.set_llm_model_dialog(false);
    detection::clear(window);
    window.set_llm_header_editing(false);
    window.set_llm_header_value("".into());
}
fn load_service(window: &SettingsWindow, page: &Page) {
    clear_dialogs(window, page);
    window.set_llm_search_open(false);
    window.set_llm_filter_open(false);
    window.set_llm_model_search("".into());
    window.set_llm_model_filter(0);
    page.pending_headers.borrow_mut().clear();
    page.awaiting_apply.set(false);
    let config = page.config.borrow().connection(service_kind(window));
    *page.draft.borrow_mut() = config.clone();
    window.set_llm_address(config.address.clone().into());
    window.set_llm_enabled(if window.get_provider_detail_index() == 0 {
        page.config.borrow().deepl_enabled
    } else {
        config.enabled
    });
    window.set_llm_key_configured(config.credential_id.is_some());
    window.set_llm_key_draft("".into());
    window.set_llm_header_name("".into());
    window.set_llm_header_value("".into());
    load_profile(window, &config);
    refresh_catalog(window, page);
    refresh_dirty(window, page);
}
fn persist(page: &Page, config: LlmSettings) {
    *page.config.borrow_mut() = config.clone();
    populate_cache(page);
    (page.send)(AppEvent::SettingsChangeRequested {
        change: SettingsChange::Llm(config),
    });
}
fn run_query(
    window: &SettingsWindow,
    page: &Rc<Page>,
    query: Option<&LlmQuery>,
    mode: LlmQueryMode,
) {
    if matches!(mode, LlmQueryMode::Models) {
        refresh_dirty(window, page);
        if window.get_llm_connection_dirty() {
            status(window, "Apply connection settings before syncing models");
            return;
        }
    }
    invalidate(page);
    let connection = match draft_connection(window, page) {
        Ok(c) => c,
        Err(e) => {
            status(window, &e.to_string());
            return;
        }
    };
    let Some(query) = query else {
        status(window, "Model service discovery is unavailable");
        return;
    };
    window.set_llm_syncing(true);
    status(window, "Loading…");
    let version = page.version.load(Ordering::SeqCst);
    let generation = page.version.clone();
    let weak = window.as_weak();
    let captured = connection.clone();
    let sync = matches!(mode, LlmQueryMode::Models);
    let pending_headers = !page.pending_headers.borrow().is_empty();
    let mode = if page.pending_headers.borrow().is_empty() {
        mode
    } else {
        LlmQueryMode::Draft {
            operation: Box::new(mode),
            headers: page.pending_headers.borrow().clone(),
        }
    };
    let cancel = query(
        connection,
        mode,
        Box::new(move |result| {
            let _ = weak.upgrade_in_event_loop(move |w| {
                if generation.load(Ordering::SeqCst) != version {
                    return;
                }
                let Some(page) = lookup_page(&w) else {
                    return;
                };
                if draft_connection(&w, &page).ok().as_ref() != Some(&captured) {
                    w.set_llm_syncing(false);
                    return;
                }
                w.set_llm_syncing(false);
                match result {
                    Ok(models) => {
                        status(&w, "Connected");
                        if sync {
                            if pending_headers
                                || !page
                                    .config
                                    .borrow()
                                    .connection(captured.provider)
                                    .same_request_configuration(&captured)
                            {
                                status(&w, "Apply connection settings before syncing models");
                                return;
                            }
                            let mut config = page.config.borrow().clone();
                            config.merge_models(&captured, models);
                            persist(&page, config);
                            refresh_catalog(&w, &page);
                            if let Some(provider) = translation_kind(&w) {
                                model_list(&w, &page, provider);
                                let reasoning = selected_reasoning(&w, &page);
                                load_reasoning(&w, &page, &reasoning);
                            }
                        }
                    }
                    Err(e) => status(&w, &e.to_string()),
                }
            });
        }),
    );
    *page.task.borrow_mut() = Some(Task(cancel));
}
fn load_translation(window: &SettingsWindow, page: &Page, model: Option<String>) {
    let Some(provider) = translation_kind(window) else {
        return;
    };
    let config = page.config.borrow();
    let selected = model
        .or_else(|| config.selected(provider).map(|m| m.model.clone()))
        .unwrap_or_default();
    let p = config
        .models
        .iter()
        .find(|m| m.provider == provider && m.model == selected)
        .map(|m| m.parameters.clone())
        .unwrap_or_default();
    window.set_llm_selected_model(selected.into());
    window.set_llm_temperature_custom(p.temperature.is_some());
    window.set_llm_temperature(i32::from(p.temperature.unwrap_or(100)));
    window.set_llm_top_p_custom(p.top_p.is_some());
    window.set_llm_top_p(i32::from(p.top_p.unwrap_or(100)));
    window.set_llm_temperature_max(if provider == LlmProvider::LmStudio {
        100
    } else {
        200
    });
    drop(config);
    model_list(window, page, provider);
    load_reasoning(window, page, &p.reasoning);
}
fn load_reasoning(window: &SettingsWindow, page: &Page, selected: &str) {
    let options = translation_kind(window)
        .and_then(|p| {
            page.cache
                .borrow()
                .get(&p)
                .and_then(|models| {
                    models
                        .iter()
                        .find(|m| m.id == window.get_llm_selected_model().as_str())
                })
                .map(|m| m.reasoning.clone())
        })
        .unwrap_or_default();
    let index = options.iter().position(|s| s == selected).or_else(|| {
        if !options.contains(&"off".into()) && !options.is_empty() {
            Some(0)
        } else {
            None
        }
    });
    let labels: Vec<slint::SharedString> = options
        .iter()
        .map(|s| {
            crate::i18n::tr(match s.as_str() {
                "off" => "Off",
                "on" => "On",
                "low" => "Low",
                "medium" => "Medium",
                "high" => "High",
                "max" => "Maximum",
                other => other,
            })
            .into()
        })
        .collect();
    window.set_llm_reasoning_options(ModelRc::new(VecModel::from(labels)));
    window.set_llm_reasoning_index(index.map_or(-1, |i| i as i32));
    update_sampling(window, page);
}
fn selected_reasoning(window: &SettingsWindow, page: &Page) -> String {
    translation_kind(window)
        .and_then(|p| {
            page.cache
                .borrow()
                .get(&p)
                .and_then(|models| {
                    models
                        .iter()
                        .find(|m| m.id == window.get_llm_selected_model().as_str())
                })
                .and_then(|m| {
                    usize::try_from(window.get_llm_reasoning_index())
                        .ok()
                        .and_then(|i| m.reasoning.get(i))
                })
                .cloned()
        })
        .unwrap_or_else(|| "off".into())
}
fn update_sampling(window: &SettingsWindow, page: &Page) {
    let provider = translation_kind(window);
    let p = selected_reasoning(window, page);
    let thinking = p != "off";
    window.set_llm_temperature_available(provider != Some(LlmProvider::DeepSeek) || !thinking);
    window.set_llm_top_p_available(provider != Some(LlmProvider::DeepSeek) || thinking);
    window.set_llm_top_p_min(if provider == Some(LlmProvider::DeepSeek) && thinking {
        95
    } else {
        1
    });
    window.set_llm_top_p(window.get_llm_top_p().max(window.get_llm_top_p_min()));
    let cache = page.cache.borrow();
    let options = provider
        .and_then(|p| cache.get(&p))
        .and_then(|models| {
            models
                .iter()
                .find(|m| m.id == window.get_llm_selected_model().as_str())
        })
        .map(|m| &m.reasoning);
    let note = if options.is_none_or(|v| v.is_empty()) {
        "Thinking capability is unavailable or unknown"
    } else if options.is_some_and(|v| !v.contains(&"off".into())) {
        "This model requires thinking"
    } else if provider == Some(LlmProvider::DeepSeek) {
        "Sampling controls depend on the selected thinking mode"
    } else {
        ""
    };
    window.set_llm_reasoning_note(crate::i18n::tr(note).into());
}
pub(crate) fn refresh_localized(window: &SettingsWindow) {
    if let Some(page) = lookup_page(window) {
        let reasoning = selected_reasoning(window, &page);
        load_reasoning(window, &page, &reasoning);
        refresh_catalog(window, &page);
        if let Some(provider) = translation_kind(window) {
            model_list(window, &page, provider);
        }
        window.set_llm_status(crate::i18n::tr(&page.status_source.borrow()).into());
    }
}
pub(crate) fn sync(window: &SettingsWindow, config: &LlmSettings) {
    window.set_deepl_provider_enabled(config.deepl_enabled);
    refresh_provider_states(window, config);
    if let Some(page) = lookup_page(window) {
        *page.config.borrow_mut() = config.clone();
        populate_cache(&page);
        let saved = config.connection(service_kind(window));
        page.draft.borrow_mut().credential_id = saved.credential_id.clone();
        // Header references are saved independently; preserve unsaved address/protocol drafts.
        for profile in &mut page.draft.borrow_mut().protocols {
            profile.headers = saved
                .protocols
                .iter()
                .find(|p| p.protocol == profile.protocol)
                .map(|p| p.headers.clone())
                .unwrap_or_default();
        }
        window.set_llm_key_configured(saved.credential_id.is_some());
        refresh_headers(window, &page);
        refresh_catalog(window, &page);
        refresh_dirty(window, &page);
        if let Some(provider) = translation_kind(window) {
            model_list(window, &page, provider);
        }
    }
}
/// Reconcile a failed provider change without disturbing unrelated service drafts.
pub(crate) fn sync_provider(window: &SettingsWindow, provider: &str) {
    window.set_llm_translation_provider_index(match provider {
        "ollama" => 1,
        "lmstudio" => 2,
        "deepseek" => 3,
        _ => 0,
    });
    if let Some(page) = lookup_page(window) {
        invalidate(&page);
        window.set_llm_syncing(false);
        load_translation(window, &page, None);
    }
}
pub(crate) fn reset(window: &SettingsWindow, config: &LlmSettings, provider: ProviderConfig) {
    sync(window, config);
    window.set_llm_translation_provider_index(match provider {
        ProviderConfig::DeepL => 0,
        ProviderConfig::Ollama => 1,
        ProviderConfig::LmStudio => 2,
        ProviderConfig::DeepSeek => 3,
    });
    window.set_llm_prompt(config.prompt.clone().into());
    if let Some(page) = lookup_page(window) {
        invalidate(&page);
        window.set_llm_syncing(false);
        load_service(window, &page);
        load_translation(window, &page, None);
    }
}

pub(crate) fn wire(
    window: &SettingsWindow,
    config: LlmSettings,
    query: Option<LlmQuery>,
    handler: Rc<dyn Fn(AppEvent)>,
) {
    let token = NEXT_PAGE.fetch_add(1, Ordering::Relaxed);
    window.set_llm_page_token(token as i32);
    let page = Rc::new(Page {
        config: RefCell::new(config.clone()),
        draft: RefCell::new(config.connection(LlmProvider::Ollama)),
        pending_headers: RefCell::new(Vec::new()),
        awaiting_apply: Cell::new(false),
        request_snapshot: RefCell::new(None),
        send: handler.clone(),
        cache: Default::default(),
        version: Default::default(),
        task: Default::default(),
        status_source: Default::default(),
        probe: Default::default(),
    });
    populate_cache(&page);
    PAGES.with(|pages| {
        let mut pages = pages.borrow_mut();
        pages.retain(|_, v| v.strong_count() > 0);
        pages.insert(token, Rc::downgrade(&page));
    });
    let weak = window.as_weak();
    let current = page.clone();
    window.on_llm_service_selected(move |_| {
        invalidate(&current);
        if let Some(w) = weak.upgrade() {
            w.set_llm_syncing(false);
            status(&w, "");
            load_service(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_draft_changed(move || {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            refresh_dirty(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_base_edited(move || {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            update_endpoints(&w, &current.draft.borrow());
            refresh_dirty(&w, &current);
            status(&w, "");
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_request_open(move || {
        if let Some(w) = weak.upgrade() {
            if let Ok(connection) = draft_connection(&w, &current) {
                *current.draft.borrow_mut() = connection;
            }
            *current.request_snapshot.borrow_mut() = Some(RequestSnapshot {
                connection: current.draft.borrow().clone(),
                headers: current.pending_headers.borrow().clone(),
                base: w.get_llm_base_url().to_string(),
                auth: w.get_llm_auth_index(),
            });
            w.set_llm_header_value("".into());
            w.set_llm_header_editing(false);
            status(&w, "");
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_request_close(move |confirm| {
        if let Some(w) = weak.upgrade() {
            if confirm {
                if !w.get_llm_header_value().is_empty() {
                    status(&w, "Save or cancel the header before confirming");
                    return;
                }
                match draft_connection(&w, &current) {
                    Ok(connection) => {
                        *current.draft.borrow_mut() = connection;
                    }
                    Err(e) => {
                        status(&w, &e.to_string());
                        return;
                    }
                }
                current.request_snapshot.borrow_mut().take();
            } else if let Some(snapshot) = current.request_snapshot.borrow_mut().take() {
                *current.draft.borrow_mut() = snapshot.connection;
                *current.pending_headers.borrow_mut() = snapshot.headers;
                load_profile(&w, &current.draft.borrow());
                w.set_llm_base_url(snapshot.base.into());
                w.set_llm_auth_index(snapshot.auth);
            }
            invalidate(&current);
            w.set_llm_syncing(false);
            w.set_llm_advanced(false);
            w.set_llm_header_value("".into());
            w.set_llm_header_name("".into());
            w.set_llm_header_editing(false);
            refresh_headers(&w, &current);
            refresh_dirty(&w, &current);
            status(&w, "");
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_models_close(move || {
        invalidate(&current);
        if let Some(w) = weak.upgrade() {
            w.set_llm_syncing(false);
            w.set_llm_model_search("".into());
            w.set_llm_model_filter(0);
            refresh_catalog(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_page_left(move || {
        invalidate(&current);
        if let Some(w) = weak.upgrade() {
            w.set_llm_syncing(false);
            load_service(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_address_edited(move || {
        invalidate(&current);
        if let Some(w) = weak.upgrade() {
            w.set_llm_syncing(false);
            if let Ok(address) = service_address(w.get_llm_address().as_str()) {
                let mut draft = current.draft.borrow_mut();
                let old = draft.profile().base_url;
                if address != draft.address && w.get_llm_base_url().as_str() == old {
                    draft.address = address.clone();
                    let base = if draft.protocol == LlmProtocol::Native
                        || draft.provider == LlmProvider::DeepSeek
                    {
                        address
                    } else {
                        format!("{address}/v1")
                    };
                    w.set_llm_base_url(base.into());
                }
            }
            update_endpoints(&w, &current.draft.borrow());
            status(&w, "");
            refresh_dirty(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_save_connection(move || {
        let Some(w) = weak.upgrade() else {
            return;
        };
        let mut config = current.config.borrow().clone();
        if w.get_provider_detail_index() == 0 {
            config.deepl_enabled = w.get_llm_enabled();
        } else {
            let connection = match draft_connection(&w, &current) {
                Ok(c) => c,
                Err(e) => {
                    status(&w, &e.to_string());
                    return;
                }
            };
            config
                .connections
                .retain(|c| c.provider != connection.provider);
            config.connections.push(connection.clone());
            *current.draft.borrow_mut() = connection.clone();
            *current.config.borrow_mut() = config.clone();
            populate_cache(&current);
            current.awaiting_apply.set(true);
            (current.send)(AppEvent::LlmConnectionApplyRequested {
                connection,
                headers: current.pending_headers.borrow().clone(),
            });
            refresh_catalog(&w, &current);
            refresh_dirty(&w, &current);
            if let Some(p) = translation_kind(&w) {
                model_list(&w, &current, p);
            }
            return;
        }
        persist(&current, config);
        refresh_dirty(&w, &current);
        refresh_catalog(&w, &current);
        if let Some(p) = translation_kind(&w) {
            model_list(&w, &current, p);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_cancel_connection(move || {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            load_service(&w, &current);
            status(&w, "");
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_protocol_selected(move |index| {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            if let Ok(c) = draft_connection(&w, &current) {
                *current.draft.borrow_mut() = c;
            }
            current.draft.borrow_mut().protocol = LlmProtocol::ALL[index.clamp(0, 2) as usize];
            load_profile(&w, &current.draft.borrow());
            refresh_headers(&w, &current);
            refresh_dirty(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_filter_models(move || {
        if let Some(w) = weak.upgrade() {
            refresh_catalog(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_model_toggle(move |id, enabled| {
        if let Some(w) = weak.upgrade() {
            let provider = service_kind(&w);
            let mut config = current.config.borrow().clone();
            let endpoint = config.connection(provider).endpoint_key();
            if let Some(m) = config
                .catalog
                .iter_mut()
                .find(|m| m.provider == provider && m.model.id == id.as_str())
            {
                m.enabled =
                    enabled && m.available && m.model.issue.is_none() && m.endpoint == endpoint;
            }
            persist(&current, config);
            refresh_catalog(&w, &current);
            if let Some(p) = translation_kind(&w) {
                model_list(&w, &current, p);
            }
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_header_save(move || {
        if let Some(w) = weak.upgrade() {
            let name = w.get_llm_header_name().to_string();
            let value = w.get_llm_header_value().to_string();
            if let Err(e) = validate_header_name(&name) {
                status(&w, &e.to_string());
                return;
            }
            if value.contains(['\r', '\n']) {
                status(&w, "Invalid request header value");
                return;
            }
            invalidate(&current);
            w.set_llm_syncing(false);
            let protocol = current.draft.borrow().protocol;
            current
                .pending_headers
                .borrow_mut()
                .retain(|h| h.protocol != protocol || !h.name.eq_ignore_ascii_case(&name));
            current.pending_headers.borrow_mut().push(LlmHeaderUpdate {
                protocol,
                name,
                secret: Some(CredentialSecret::new(value)),
            });
            w.set_llm_header_value("".into());
            w.set_llm_header_name("".into());
            w.set_llm_header_editing(false);
            refresh_headers(&w, &current);
            refresh_dirty(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_header_remove(move |name| {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            let protocol = current.draft.borrow().protocol;
            current
                .pending_headers
                .borrow_mut()
                .retain(|h| h.protocol != protocol || !h.name.eq_ignore_ascii_case(name.as_str()));
            current.pending_headers.borrow_mut().push(LlmHeaderUpdate {
                protocol,
                name: name.to_string(),
                secret: None,
            });
            refresh_headers(&w, &current);
            refresh_dirty(&w, &current);
        }
    });
    let weak = window.as_weak();
    let send = handler.clone();
    let current = page.clone();
    window.on_llm_save_key(move |remove| {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            current.cache.borrow_mut().remove(&service_kind(&w));
            send(AppEvent::LlmCredentialChangeRequested {
                provider: service_kind(&w),
                secret: if remove {
                    None
                } else {
                    Some(CredentialSecret::new(w.get_llm_key_draft().to_string()))
                },
            });
            w.set_llm_key_draft("".into());
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    let send = handler.clone();
    window.on_llm_provider_selected(move |index| {
        if let Some(w) = weak.upgrade() {
            invalidate(&current);
            w.set_llm_syncing(false);
            status(&w, "");
            let provider = match index {
                1 => ProviderConfig::Ollama,
                2 => ProviderConfig::LmStudio,
                3 => ProviderConfig::DeepSeek,
                _ => ProviderConfig::DeepL,
            };
            w.set_draft_provider_id(provider.id().into());
            load_translation(&w, &current, None);
            send(AppEvent::SettingsChangeRequested {
                change: SettingsChange::Provider(provider),
            });
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_model_selected(move |index| {
        if let Some(w) = weak.upgrade()
            && let Ok(i) = usize::try_from(index)
            && let Some(model) = w.get_llm_models().row_data(i)
        {
            load_translation(&w, &current, Some(model.to_string()));
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_reasoning_selected(move |_| {
        if let Some(w) = weak.upgrade() {
            update_sampling(&w, &current);
        }
    });
    let weak = window.as_weak();
    window.on_llm_reset_prompt(move || {
        if let Some(w) = weak.upgrade() {
            w.set_llm_prompt(DEFAULT_TRANSLATION_PROMPT.into());
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    let send = handler;
    window.on_llm_save_translation(move || {
        let Some(w) = weak.upgrade() else {
            return;
        };
        let Some(provider) = translation_kind(&w) else {
            return;
        };
        let mut config = current.config.borrow().clone();
        config.prompt = w.get_llm_prompt().to_string();
        let name = w.get_llm_selected_model().to_string();
        if name.is_empty() || !config.model_enabled(provider, &name) {
            status(&w, "Select a model");
            return;
        }
        let model = TranslationModel {
            provider,
            model: name.clone(),
            parameters: GenerationParameters {
                reasoning: selected_reasoning(&w, &current),
                temperature: if w.get_llm_temperature_custom() && w.get_llm_temperature_available()
                {
                    Some(w.get_llm_temperature() as u16)
                } else {
                    None
                },
                top_p: if w.get_llm_top_p_custom() && w.get_llm_top_p_available() {
                    Some(w.get_llm_top_p() as u16)
                } else {
                    None
                },
            },
        };
        config
            .models
            .retain(|m| m.provider != provider || m.model != name);
        config.models.push(model);
        if let Err(e) = config.validate() {
            status(&w, &e.to_string());
            return;
        }
        *current.config.borrow_mut() = config.clone();
        send(AppEvent::SettingsChangeRequested {
            change: SettingsChange::Llm(config),
        });
    });
    let current = page.clone();
    let weak = window.as_weak();
    let services = query.clone();
    window.on_llm_refresh(move |_| {
        if let Some(w) = weak.upgrade() {
            run_query(&w, &current, services.as_ref(), LlmQueryMode::Models);
        }
    });
    detection::wire(window, page.clone(), query.clone());
    let current = page;
    let weak = window.as_weak();
    let services = query;
    window.on_llm_check_connection(move || {
        if let Some(w) = weak.upgrade() {
            detection::open(&w, &current, services.as_ref());
        }
    });
    load_service(window, &lookup_page(window).unwrap());
    window.set_llm_prompt(config.prompt.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    type QueuedEvents = Arc<std::sync::Mutex<Vec<Box<dyn FnOnce() + Send>>>>;
    struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>, QueuedEvents);
    struct Proxy(QueuedEvents);
    impl slint::platform::EventLoopProxy for Proxy {
        fn quit_event_loop(&self) -> std::result::Result<(), slint::EventLoopError> {
            Ok(())
        }
        fn invoke_from_event_loop(
            &self,
            event: Box<dyn FnOnce() + Send>,
        ) -> std::result::Result<(), slint::EventLoopError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }
    fn drain(queue: &QueuedEvents) {
        loop {
            let events = std::mem::take(&mut *queue.lock().unwrap());
            if events.is_empty() {
                break;
            }
            for event in events {
                event();
            }
        }
    }
    impl Platform for TestPlatform {
        fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
            Some(Box::new(Proxy(self.1.clone())))
        }
        fn create_window_adapter(
            &self,
        ) -> std::result::Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            *self.0.borrow_mut() = Some(window.clone());
            Ok(window)
        }
    }
    fn detection_regression(window: &SettingsWindow, page: &Rc<Page>, queue: &QueuedEvents) {
        let original = page.config.borrow().clone();
        let selected = window.get_llm_selected_model();
        let pending = Arc::new(std::sync::Mutex::new(Vec::<LlmQueryCompletion>::new()));
        let recorded = Rc::new(RefCell::new(Vec::<(LlmConnection, LlmQueryMode)>::new()));
        let callbacks = pending.clone();
        let requests = recorded.clone();
        let cancelled = Rc::new(Cell::new(0));
        let cancelled_query = cancelled.clone();
        let query: LlmQuery = Rc::new(move |connection, mode, complete| {
            requests.borrow_mut().push((connection, mode));
            callbacks.lock().unwrap().push(complete);
            let counter = cancelled_query.clone();
            Box::new(move || counter.set(counter.get() + 1))
        });
        detection::wire(window, page.clone(), Some(query));
        for (index, provider) in LlmProvider::ALL.into_iter().enumerate() {
            let mut config = original.clone();
            config.catalog.retain(|m| m.provider != provider);
            let endpoint = config.connection(provider).endpoint_key();
            for (id, enabled, available, reasoning, issue) in [
                ("first", true, true, vec!["off".into(), "high".into()], None),
                ("second", true, true, vec!["high".into()], None),
                ("missing", true, false, Vec::new(), None),
                ("unused", false, true, Vec::new(), None),
                (
                    "embedding",
                    true,
                    true,
                    Vec::new(),
                    Some("Model generation capability is unknown".into()),
                ),
            ] {
                config.catalog.push(CatalogModel {
                    provider,
                    model: LlmModel {
                        id: id.into(),
                        aliases: Vec::new(),
                        reasoning,
                        issue,
                    },
                    enabled,
                    available,
                    endpoint: endpoint.clone(),
                });
            }
            *page.config.borrow_mut() = config.clone();
            window.set_provider_detail_index(index as i32 + 1);
            window.set_llm_service_index(index as i32);
            load_service(window, page);
            window.set_credential_editing(true);
            window.invoke_llm_probe_open();
            assert!(!window.get_llm_probe_dialog());
            window.set_credential_editing(false);
            window.invoke_llm_probe_open();
            assert_eq!(window.get_llm_probe_models().row_count(), 4);
            window.set_llm_probe_index(2);
            window.invoke_llm_probe_selected();
            assert!(!window.get_llm_probe_can_start());
            window.set_llm_probe_index(0);
            window.invoke_llm_probe_selected();
            // Tests use the connection draft, without applying or modifying translation selection.
            window.set_llm_base_url("http://127.0.0.1:54321".into());
            window.invoke_llm_base_edited();
            window.set_llm_probe_all(true);
            window.invoke_llm_probe_selected();
            window.invoke_llm_probe_start();
            let count = recorded.borrow().len();
            assert!(window.get_llm_probe_running());
            assert!(!window.get_llm_syncing());
            window.invoke_llm_probe_start();
            assert_eq!(
                recorded.borrow().len(),
                count,
                "double start must not enqueue"
            );
            pending.lock().unwrap().remove(0)(Err(lexift_core::Error::new(
                "Model request timed out",
            )));
            drain(queue);
            assert_eq!(
                recorded.borrow().len(),
                count + 1,
                "batch proceeds after one failure"
            );
            pending.lock().unwrap().remove(0)(Ok(Vec::new()));
            drain(queue);
            assert!(!window.get_llm_probe_running());
            assert_eq!(window.get_llm_probe_results().row_count(), 4);
            assert!(window.get_llm_probe_results().row_data(0).unwrap().failed);
            assert_eq!(
                window.get_llm_probe_results().row_data(1).unwrap().state,
                "Passed"
            );
            assert_eq!(
                window.get_llm_probe_results().row_data(2).unwrap().state,
                "Skipped"
            );
            assert_eq!(
                window.get_llm_probe_results().row_data(3).unwrap().state,
                "Skipped"
            );
            for (i, (connection, mode)) in recorded.borrow().iter().rev().take(2).rev().enumerate()
            {
                assert_eq!(connection.profile().base_url, "http://127.0.0.1:54321");
                let LlmQueryMode::Test(request) = mode else {
                    panic!("probe must generate")
                };
                assert_eq!(request.system, "Reply with OK only.");
                assert_eq!(request.input, "Connection test");
                assert_eq!(
                    request.parameters.reasoning,
                    if i == 0 { "off" } else { "high" }
                );
                assert_eq!(request.parameters.temperature, None);
                assert_eq!(request.parameters.top_p, None);
            }
            assert_eq!(
                *page.config.borrow(),
                config,
                "testing must not persist configuration"
            );
            assert_eq!(window.get_llm_selected_model(), selected);
            window.set_llm_probe_all(false);
            window.invoke_llm_probe_selected();
            window.invoke_llm_probe_start();
            pending.lock().unwrap().remove(0)(Err(lexift_core::Error::new(
                "sensitive adapter detail",
            )));
            drain(queue);
            assert_eq!(window.get_llm_probe_results().row_count(), 1);
            assert!(
                !window
                    .get_llm_probe_results()
                    .row_data(0)
                    .unwrap()
                    .detail
                    .contains("sensitive")
            );
            window.invoke_llm_probe_start();
            let late = pending.lock().unwrap().remove(0);
            let before = cancelled.get();
            window.invoke_llm_probe_close();
            assert_eq!(cancelled.get(), before + 1);
            window.invoke_llm_probe_open();
            let count = recorded.borrow().len();
            late(Ok(Vec::new()));
            drain(queue);
            assert_eq!(window.get_llm_probe_results().row_count(), 0);
            assert_eq!(
                recorded.borrow().len(),
                count,
                "stale completion must not start next model"
            );
            window.invoke_llm_probe_close();
            // A provider switch cancels an active probe, including its frozen headers.
            window.invoke_llm_probe_open();
            window.invoke_llm_probe_start();
            let late = pending.lock().unwrap().remove(0);
            window.invoke_llm_service_selected(index as i32 + 1);
            late(Ok(Vec::new()));
            drain(queue);
            assert!(!window.get_llm_probe_dialog());
        }
        page.config.borrow_mut().catalog.clear();
        let count = recorded.borrow().len();
        window.invoke_llm_probe_open();
        assert_eq!(window.get_llm_probe_models().row_count(), 0);
        assert!(!window.get_llm_probe_can_start());
        window.invoke_llm_probe_start();
        assert_eq!(recorded.borrow().len(), count);
        window.invoke_llm_probe_close();
        *page.config.borrow_mut() = original;
        window.set_provider_detail_index(3);
        window.set_llm_service_index(2);
        load_service(window, page);
    }
    #[test]
    fn model_parameters_connection_drafts_and_window_tasks_are_isolated() {
        let adapter = Rc::new(RefCell::new(None));
        let queue = QueuedEvents::default();
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone(), queue.clone())))
            .unwrap();
        let window = SettingsWindow::new().unwrap();
        let mut config = LlmSettings {
            models: vec![
                TranslationModel {
                    provider: LlmProvider::Ollama,
                    model: "model-a".into(),
                    parameters: GenerationParameters {
                        temperature: Some(25),
                        ..Default::default()
                    },
                },
                TranslationModel {
                    provider: LlmProvider::LmStudio,
                    model: "model-b".into(),
                    parameters: GenerationParameters {
                        temperature: Some(75),
                        ..Default::default()
                    },
                },
            ],
            ..Default::default()
        };
        for m in &config.models {
            config.catalog.push(CatalogModel {
                provider: m.provider,
                model: LlmModel {
                    id: m.model.clone(),
                    aliases: Vec::new(),
                    reasoning: Vec::new(),
                    issue: None,
                },
                enabled: true,
                available: true,
                endpoint: config.connection(m.provider).endpoint_key(),
            });
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = events.clone();
        let cancellations = Rc::new(std::cell::Cell::new(0));
        let cancelled = cancellations.clone();
        wire(
            &window,
            config.clone(),
            Some(Rc::new(move |_, _, _| {
                let cancelled = cancelled.clone();
                Box::new(move || cancelled.set(cancelled.get() + 1))
            })),
            Rc::new(move |event| recorded.borrow_mut().push(event)),
        );
        reset(&window, &config, ProviderConfig::Ollama);
        window.set_provider_detail_index(1);
        assert_eq!(window.get_llm_temperature(), 25);
        assert_eq!(window.get_llm_selected_model(), "model-a");
        assert_eq!(
            window.get_llm_models().row_count(),
            1,
            "saved enabled models must populate without a refresh"
        );
        assert_eq!(window.get_llm_models().row_data(0).unwrap(), "model-a");
        window.set_llm_translation_provider_index(2);
        window.invoke_llm_provider_selected(2);
        assert_eq!(window.get_llm_selected_model(), "model-b");
        assert_eq!(window.get_llm_temperature(), 75);
        window.set_llm_temperature(60);
        window.invoke_llm_save_translation();
        let page = lookup_page(&window).unwrap();
        assert_eq!(
            page.config
                .borrow()
                .selected(LlmProvider::Ollama)
                .unwrap()
                .parameters
                .temperature,
            Some(25)
        );
        assert_eq!(
            page.config
                .borrow()
                .selected(LlmProvider::LmStudio)
                .unwrap()
                .parameters
                .temperature,
            Some(60)
        );
        page.cache.borrow_mut().insert(
            LlmProvider::LmStudio,
            vec![LlmModel {
                id: "model-b".into(),
                aliases: Vec::new(),
                reasoning: vec!["on".into()],
                issue: None,
            }],
        );
        load_translation(&window, &page, None);
        assert_eq!(selected_reasoning(&window, &page), "on");
        assert_eq!(
            window.get_llm_reasoning_note(),
            "This model requires thinking"
        );
        window.invoke_llm_refresh(false);
        let old = page.version.load(Ordering::SeqCst);
        window.set_llm_address("http://192.168.1.20:11434".into());
        window.invoke_llm_address_edited();
        assert!(page.version.load(Ordering::SeqCst) > old);
        assert_eq!(cancellations.get(), 1);
        window.invoke_llm_save_connection();
        assert_eq!(
            page.config.borrow().connection(LlmProvider::Ollama).address,
            "http://192.168.1.20:11434"
        );
        assert!(page.config.borrow().selected(LlmProvider::Ollama).is_some());
        assert!(
            page.config
                .borrow()
                .selected(LlmProvider::LmStudio)
                .is_some()
        );
        window.set_llm_prompt("missing placeholder".into());
        let count = events.borrow().len();
        window.invoke_llm_save_translation();
        assert_eq!(events.borrow().len(), count);
        window.invoke_llm_reset_prompt();
        assert_eq!(window.get_llm_prompt(), DEFAULT_TRANSLATION_PROMPT);
        window.set_llm_translation_provider_index(3);
        window.invoke_llm_provider_selected(3);
        page.cache.borrow_mut().insert(
            LlmProvider::DeepSeek,
            vec![LlmModel {
                id: "cloud".into(),
                aliases: Vec::new(),
                reasoning: vec!["off".into(), "high".into()],
                issue: None,
            }],
        );
        model_list(&window, &page, LlmProvider::DeepSeek);
        assert_eq!(
            window.get_llm_model_index(),
            -1,
            "discovery must not select the first model"
        );
        assert!(window.get_llm_selected_model().is_empty());
        load_translation(&window, &page, Some("cloud".into()));
        assert!(window.get_llm_temperature_available());
        assert!(!window.get_llm_top_p_available());
        window.set_llm_reasoning_index(1);
        window.invoke_llm_reasoning_selected(1);
        assert!(!window.get_llm_temperature_available());
        assert!(window.get_llm_top_p_available());
        assert_eq!(window.get_llm_top_p_min(), 95);
        window.set_llm_service_index(2);
        window.set_provider_detail_index(3);
        window.invoke_llm_service_selected(3);
        let event_count = events.borrow().len();
        window.set_llm_header_name("X-Private".into());
        window.set_llm_header_value("draft-secret-test-value".into());
        window.invoke_llm_header_save();
        assert_eq!(
            events.borrow().len(),
            event_count,
            "editing headers must not save before Apply"
        );
        assert_eq!(page.pending_headers.borrow().len(), 1);
        assert_eq!(window.get_llm_headers().row_count(), 1);
        assert!(window.get_llm_connection_dirty());
        window.invoke_llm_request_open();
        let base = window.get_llm_base_url();
        window.set_llm_base_url("https://gateway.example/v1".into());
        window.invoke_llm_base_edited();
        window.set_llm_header_name("X-Dialog".into());
        window.set_llm_header_value("dialog-only-secret".into());
        window.invoke_llm_header_save();
        assert_eq!(page.pending_headers.borrow().len(), 2);
        window.invoke_llm_request_close(false);
        assert_eq!(window.get_llm_base_url(), base);
        assert_eq!(
            page.pending_headers.borrow().len(),
            1,
            "dialog Cancel preserves earlier page drafts"
        );
        assert!(window.get_llm_header_value().is_empty());
        window.invoke_llm_request_open();
        window.set_llm_base_url("https://gateway.example/v1".into());
        window.invoke_llm_base_edited();
        window.invoke_llm_request_close(true);
        assert_eq!(window.get_llm_base_url(), "https://gateway.example/v1");
        assert!(window.get_llm_connection_dirty());
        let version = page.version.load(Ordering::SeqCst);
        window.invoke_llm_refresh(false);
        assert_eq!(
            page.version.load(Ordering::SeqCst),
            version,
            "unsaved connections must not query or replace the directory"
        );
        window.invoke_llm_cancel_connection();
        assert!(page.pending_headers.borrow().is_empty());
        assert_eq!(window.get_llm_headers().row_count(), 0);
        let mut saved = page.config.borrow().clone();
        saved.merge_models(
            &saved.connection(LlmProvider::DeepSeek),
            vec![
                LlmModel {
                    id: "cloud".into(),
                    aliases: Vec::new(),
                    reasoning: vec!["off".into(), "high".into()],
                    issue: None,
                },
                LlmModel {
                    id: "embedding-only".into(),
                    aliases: Vec::new(),
                    reasoning: Vec::new(),
                    issue: Some("Embedding-only models cannot translate text".into()),
                },
            ],
        );
        sync(&window, &saved);
        window.set_provider_detail_index(3);
        window.set_llm_service_index(2);
        window.invoke_llm_service_selected(3);
        window.invoke_llm_model_toggle("cloud".into(), true);
        assert_eq!(window.get_llm_models().row_count(), 1);
        assert_eq!(window.get_llm_enabled_catalog().row_count(), 1);
        assert_eq!(window.get_llm_catalog().row_count(), 2);
        assert_eq!(window.get_llm_models().row_data(0).unwrap(), "cloud");
        window.invoke_llm_model_toggle("cloud".into(), false);
        assert_eq!(window.get_llm_models().row_count(), 0);
        assert_eq!(window.get_llm_selected_model(), "cloud");
        window.invoke_llm_model_toggle("cloud".into(), true);
        window.set_llm_model_search("embedding".into());
        window.invoke_llm_filter_models();
        assert_eq!(window.get_llm_catalog().row_count(), 1);
        window.set_llm_model_search("".into());
        window.invoke_llm_filter_models();
        assert_eq!(window.get_llm_catalog().row_count(), 2);
        assert_eq!(window.get_llm_catalog().row_count(), 2);
        window.set_llm_enabled(false);
        assert_eq!(
            window.get_provider_enabled_states().row_data(3),
            Some(true),
            "green indicator follows saved settings, not the switch draft"
        );
        window.invoke_llm_save_connection();
        assert_eq!(
            window.get_provider_enabled_states().row_data(3),
            Some(false)
        );
        assert!(
            !page
                .config
                .borrow()
                .connection(LlmProvider::DeepSeek)
                .enabled
        );
        assert!(!window.get_llm_selection_note().is_empty());
        window.set_llm_enabled(true);
        window.invoke_llm_save_connection();
        load_translation(&window, &page, Some("cloud".into()));
        if let Ok(directory) = std::env::var("LEXIFT_LLM_SNAPSHOT_DIR") {
            let adapter = adapter.borrow().as_ref().unwrap().clone();
            window.set_llm_key_configured(true); // Render only a masked fixture; no credential is stored.
            for language in lexift_core::domain::ui_language::UiLanguage::ALL {
                crate::i18n::select(language);
                refresh_localized(&window);
                for scale in [1., 1.25] {
                    adapter
                        .window()
                        .dispatch_event(WindowEvent::ScaleFactorChanged {
                            scale_factor: scale,
                        });
                    for category in [1, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] {
                        let width = (if [10, 11, 15].contains(&category) {
                            520.
                        } else {
                            900.
                        } * scale) as u32;
                        let height = (if [10, 11, 15].contains(&category) {
                            640.
                        } else {
                            1050.
                        } * scale) as u32;
                        adapter.set_size(slint::PhysicalSize::new(width, height));
                        window.set_llm_probe_dialog(false);
                        window.set_llm_probe_running(false);
                        if category >= 12 {
                            window.invoke_llm_probe_open();
                            window.set_llm_probe_all(category == 13 || category == 14);
                            window.invoke_llm_probe_selected();
                            if category == 14 || category == 16 {
                                window.set_llm_probe_running(category == 16);
                                window.set_llm_probe_progress("1 / 2".into());
                                window.set_llm_probe_results(ModelRc::new(VecModel::from(vec![
                                    crate::ProviderProbeRow {
                                        id: "cloud".into(),
                                        state: crate::i18n::tr("Passed").into(),
                                        elapsed: "324 ms".into(),
                                        ..Default::default()
                                    },
                                    crate::ProviderProbeRow {
                                        id: "unavailable-example".into(),
                                        state: crate::i18n::tr(if category == 16 {
                                            "Testing…"
                                        } else {
                                            "Failed"
                                        })
                                        .into(),
                                        detail: crate::i18n::tr("Model request timed out").into(),
                                        failed: category == 14,
                                        ..Default::default()
                                    },
                                ])));
                            }
                        }
                        window.set_llm_advanced(category == 6);
                        window.set_llm_model_dialog(category == 7 || category == 11);
                        window.set_llm_search_open(category == 8);
                        window.set_llm_filter_open(category == 8);
                        if category == 9 {
                            window.set_llm_enabled_catalog(ModelRc::default());
                            window.set_llm_enabled_count(0);
                        } else {
                            refresh_catalog(&window, &page);
                        }
                        let active = if category >= 5 { 5 } else { category };
                        window.set_active_category(active);
                        adapter.request_redraw();
                        let mut pixels =
                            vec![slint::Rgb8Pixel::default(); (width * height) as usize];
                        assert!(adapter.draw_if_needed(|renderer| {
                            renderer.render(&mut pixels, width as usize);
                        }));
                        if category == 1 {
                            window.invoke_scroll_model_settings(false);
                            adapter.request_redraw();
                            assert!(adapter.draw_if_needed(|renderer| {
                                renderer.render(&mut pixels, width as usize);
                            }));
                        }
                        crate::i18n::write_test_bitmap(
                            &directory,
                            &format!("{}-{scale}-{category}", language.code()),
                            width,
                            height,
                            pixels.into_iter().map(|p| [p.r, p.g, p.b, 255]),
                        );
                    }
                }
            }
        }
        window.set_llm_key_configured(false);
        crate::i18n::select(lexift_core::domain::ui_language::UiLanguage::EnglishUs);
        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1. });
        let surface = adapter.borrow().as_ref().unwrap().clone();
        surface.set_size(slint::PhysicalSize::new(900, 1050));
        window.set_llm_advanced(false);
        window.set_llm_model_dialog(false);
        window.set_llm_probe_dialog(false);
        window.set_llm_probe_running(false);
        window.set_active_category(1);
        window.set_llm_translation_provider_index(1);
        let draw = || {
            surface.request_redraw();
            surface.draw_if_needed(|renderer| {
                renderer.render(&mut vec![slint::Rgb8Pixel::default(); 900 * 1050], 900);
            });
        };
        draw();
        window.invoke_scroll_model_settings(false);
        draw();
        let position = slint::LogicalPosition::new(600., 160.);
        window.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
        window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            });
        draw();
        for key in [
            slint::platform::Key::DownArrow,
            slint::platform::Key::Return,
        ] {
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
            window
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
        }
        assert_eq!(
            window.get_llm_translation_provider_index(),
            2,
            "provider picker must open on click and accept keyboard selection"
        );
        window.set_active_category(5);
        window.invoke_open_provider_request();
        draw();
        assert!(window.get_llm_advanced());
        let original_base = window.get_llm_base_url();
        window.set_llm_base_url("https://cancelled.example/v1".into());
        window.invoke_llm_base_edited();
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        assert!(
            !window.get_llm_advanced(),
            "Escape cancels the nested request dialog"
        );
        assert_eq!(window.get_llm_base_url(), original_base);
        window.invoke_open_provider_models();
        draw();
        assert!(window.get_llm_model_dialog());
        let before_close = cancellations.get();
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::Escape.into(),
        });
        assert!(!window.get_llm_model_dialog());
        assert_eq!(
            cancellations.get(),
            before_close + 1,
            "closing the directory cancels its discovery"
        );
        detection_regression(&window, &page, &queue);
        window.invoke_llm_refresh(false);
        let before_drop = cancellations.get();
        drop(page);
        drop(window);
        assert_eq!(
            cancellations.get(),
            before_drop + 1,
            "closing destroys the owned query"
        );
    }
}
