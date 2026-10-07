//! Window-owned, sequential model probes; no translation settings or catalog mutations.
use super::*;
use crate::ProviderProbeRow;
use std::time::Instant;

struct Item {
    model: LlmModel,
    skip: Option<String>,
}
pub(super) struct Run {
    connection: LlmConnection,
    headers: Vec<LlmHeaderUpdate>,
    items: Vec<Item>,
    next: usize,
    rows: Vec<ProviderProbeRow>,
    query: LlmQuery,
}

fn items(window: &SettingsWindow, page: &Page) -> Vec<Item> {
    let config = page.config.borrow();
    let provider = service_kind(window);
    let endpoint = config.connection(provider).endpoint_key();
    config
        .catalog
        .iter()
        .filter(|m| {
            m.provider == provider
                && m.enabled
                && m.model.issue.as_deref() != Some("Embedding-only models cannot translate text")
        })
        .map(|m| {
            let skip = if m.endpoint != endpoint {
                Some("Connection changed; sync models to confirm".into())
            } else if !m.available {
                Some("Selected model is unavailable".into())
            } else {
                m.model.issue.clone()
            };
            Item {
                model: m.model.clone(),
                skip,
            }
        })
        .collect()
}

pub(super) fn open(window: &SettingsWindow, page: &Rc<Page>, query: Option<&LlmQuery>) {
    if window.get_credential_editing()
        || window.get_credential_busy()
        || window.get_credential_request_pending()
    {
        status(window, "Save or cancel credential editing before testing");
        return;
    }
    if query.is_none() {
        status(window, "Model service discovery is unavailable");
        return;
    }
    if let Err(e) = draft_connection(window, page) {
        status(window, &e.to_string());
        return;
    }
    invalidate(page);
    window.set_llm_syncing(false);
    let entries = items(window, page);
    let config = page.config.borrow();
    let selected = config
        .selected(service_kind(window))
        .map(|m| m.model.as_str());
    let index = entries
        .iter()
        .position(|m| m.skip.is_none() && Some(m.model.id.as_str()) == selected)
        .or_else(|| entries.iter().position(|m| m.skip.is_none()))
        .map_or(-1, |i| i as i32);
    window.set_llm_probe_models(ModelRc::new(VecModel::from(
        entries
            .iter()
            .map(|m| m.model.id.clone().into())
            .collect::<Vec<_>>(),
    )));
    window.set_llm_probe_index(index);
    window.set_llm_probe_all(false);
    window.set_llm_probe_running(false);
    window.set_llm_probe_results(ModelRc::default());
    window.set_llm_probe_progress("".into());
    window.set_llm_probe_dialog(true);
    drop(config);
    selected_changed(window, page);
}

fn selected_changed(window: &SettingsWindow, page: &Page) {
    if window.get_llm_probe_running() {
        return;
    }
    let entries = items(window, page);
    let selected = usize::try_from(window.get_llm_probe_index())
        .ok()
        .and_then(|i| entries.get(i));
    let note = if entries.is_empty() {
        "No enabled models"
    } else if window.get_llm_probe_all() {
        if entries.iter().any(|m| m.skip.is_none()) {
            ""
        } else {
            "Selected model is unavailable"
        }
    } else {
        selected.and_then(|m| m.skip.as_deref()).unwrap_or("")
    };
    window.set_llm_probe_note(crate::i18n::tr(note).into());
    window.set_llm_probe_can_start(if window.get_llm_probe_all() {
        entries.iter().any(|m| m.skip.is_none())
    } else {
        selected.is_some_and(|m| m.skip.is_none())
    });
    window.set_llm_probe_results(ModelRc::default());
    window.set_llm_probe_progress("".into());
}

fn publish(window: &SettingsWindow, run: &Run) {
    window.set_llm_probe_results(ModelRc::new(VecModel::from(run.rows.clone())));
    window.set_llm_probe_progress(format!("{} / {}", run.next, run.items.len()).into());
}

fn row(id: &str, state: &str, detail: &str, elapsed: String, failed: bool) -> ProviderProbeRow {
    ProviderProbeRow {
        id: id.into(),
        state: crate::i18n::tr(state).into(),
        detail: crate::i18n::tr(detail).into(),
        elapsed: elapsed.into(),
        failed,
    }
}

fn next(window: &SettingsWindow, page: &Rc<Page>) {
    let Some(query) = page.probe.borrow().as_ref().map(|run| run.query.clone()) else {
        return;
    };
    page.task.borrow_mut().take();
    let (connection, headers, model) = {
        let mut active = page.probe.borrow_mut();
        let Some(run) = active.as_mut() else {
            return;
        };
        while let Some(item) = run.items.get(run.next) {
            if let Some(reason) = &item.skip {
                run.rows
                    .push(row(&item.model.id, "Skipped", reason, String::new(), false));
                run.next += 1;
            } else {
                break;
            }
        }
        publish(window, run);
        let Some(item) = run.items.get(run.next) else {
            window.set_llm_probe_running(false);
            return;
        };
        let model = item.model.clone();
        run.rows
            .push(row(&model.id, "Testing…", "", String::new(), false));
        publish(window, run);
        (run.connection.clone(), run.headers.clone(), model)
    };
    let reasoning = if model.reasoning.is_empty() || model.reasoning.iter().any(|r| r == "off") {
        "off".into()
    } else {
        model.reasoning[0].clone()
    };
    let mode = LlmQueryMode::Test(GenerateRequest {
        model: model.id.clone(),
        system: "Reply with OK only.".into(),
        input: "Connection test".into(),
        parameters: GenerationParameters {
            reasoning,
            temperature: None,
            top_p: None,
        },
    });
    let mode = if headers.is_empty() {
        mode
    } else {
        LlmQueryMode::Draft {
            operation: Box::new(mode),
            headers,
        }
    };
    let version = page.version.load(Ordering::SeqCst);
    let generation = page.version.clone();
    let page_token = window.get_llm_page_token();
    let weak = window.as_weak();
    let started = Instant::now();
    let captured = connection.clone();
    let cancel = query(
        connection,
        mode,
        Box::new(move |result| {
            let _ = weak.upgrade_in_event_loop(move |window| {
                if generation.load(Ordering::SeqCst) != version
                    || !window.get_llm_probe_dialog()
                    || window.get_llm_page_token() != page_token
                {
                    return;
                }
                let Some(page) = lookup_page(&window) else {
                    return;
                };
                if draft_connection(&window, &page).ok().as_ref() != Some(&captured) {
                    close(&window, &page);
                    return;
                }
                {
                    let mut active = page.probe.borrow_mut();
                    let Some(run) = active.as_mut() else {
                        return;
                    };
                    let (state, detail, failed) = match result {
                        Ok(_) => ("Passed", String::new(), false),
                        Err(e) => ("Failed", safe_error(&e.to_string()).into(), true),
                    };
                    *run.rows.last_mut().expect("active probe row") = row(
                        &model.id,
                        state,
                        &detail,
                        format!("{} ms", started.elapsed().as_millis()),
                        failed,
                    );
                    run.next += 1;
                    publish(&window, run);
                }
                next(&window, &page);
            });
        }),
    );
    *page.task.borrow_mut() = Some(Task(cancel));
}

// Never display arbitrary adapter text: credential failures can originate outside HTTP adapters.
fn safe_error(text: &str) -> &'static str {
    match text {
        "Provider is disabled" => "Provider is disabled",
        "Model request timed out" => "Model request timed out",
        "Cannot connect to model service" => "Cannot connect to model service",
        "Model service authentication failed" | "DeepSeek API key is not configured" => {
            "Model service authentication failed"
        }
        "Model or API not found; check service version" => {
            "Model or API not found; check service version"
        }
        "Model service rate limit exceeded" => "Model service rate limit exceeded",
        "Model returned an empty translation" | "Model returned an incomplete translation" => {
            "Model returned no complete text"
        }
        _ => "Model test failed; check connection and model compatibility",
    }
}

fn close(window: &SettingsWindow, page: &Page) {
    invalidate(page);
    clear(window);
}

pub(super) fn clear(window: &SettingsWindow) {
    window.set_llm_probe_dialog(false);
    window.set_llm_probe_running(false);
    window.set_llm_probe_results(ModelRc::default());
    window.set_llm_probe_models(ModelRc::default());
    window.set_llm_probe_note("".into());
    window.set_llm_probe_progress("".into());
}

pub(super) fn wire(window: &SettingsWindow, page: Rc<Page>, query: Option<LlmQuery>) {
    let current = page.clone();
    let services = query.clone();
    let weak = window.as_weak();
    window.on_llm_probe_open(move || {
        if let Some(w) = weak.upgrade() {
            open(&w, &current, services.as_ref());
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_probe_close(move || {
        if let Some(w) = weak.upgrade() {
            close(&w, &current);
        }
    });
    let current = page.clone();
    let weak = window.as_weak();
    window.on_llm_probe_selected(move || {
        if let Some(w) = weak.upgrade() {
            selected_changed(&w, &current);
        }
    });
    let weak = window.as_weak();
    window.on_llm_probe_start(move || {
        let Some(w) = weak.upgrade() else {
            return;
        };
        if !w.get_llm_probe_dialog() || w.get_llm_probe_running() {
            return;
        }
        let Some(query) = query.as_ref() else {
            return;
        };
        let connection = match draft_connection(&w, &page) {
            Ok(c) => c,
            Err(e) => {
                w.set_llm_probe_note(crate::i18n::tr(&e.to_string()).into());
                return;
            }
        };
        let mut entries = items(&w, &page);
        if !w.get_llm_probe_all() {
            let Some(index) = usize::try_from(w.get_llm_probe_index())
                .ok()
                .filter(|i| *i < entries.len())
            else {
                return;
            };
            let item = entries.remove(index);
            if item.skip.is_some() {
                return;
            }
            entries = vec![item];
        }
        if !entries.iter().any(|m| m.skip.is_none()) {
            return;
        }
        invalidate(&page);
        *page.probe.borrow_mut() = Some(Run {
            connection,
            headers: page.pending_headers.borrow().clone(),
            items: entries,
            next: 0,
            rows: Vec::new(),
            query: query.clone(),
        });
        w.set_llm_probe_running(true);
        next(&w, &page);
    });
}
