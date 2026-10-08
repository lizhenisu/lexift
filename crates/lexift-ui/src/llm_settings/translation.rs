//! Window-owned translation drafts, separate from persisted provider connections.
use super::*;

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
type Key = (LlmProvider, String);
#[derive(Default)]
pub(super) struct Drafts {
    current: Option<Key>,

    pending: Option<(u64, Key, GenerationParameters, String)>,
    selection: Option<(u64, i32, String)>,
}

fn reasoning(page: &Page, key: &Key, index: i32) -> String {
    page.cache
        .borrow()
        .get(&key.0)
        .and_then(|models| models.iter().find(|m| m.id == key.1))
        .and_then(|m| usize::try_from(index).ok().and_then(|i| m.reasoning.get(i)))
        .cloned()
        .unwrap_or_else(|| "off".into())
}
fn read(window: &SettingsWindow, page: &Page, key: &Key) -> GenerationParameters {
    GenerationParameters {
        reasoning: reasoning(page, key, window.get_llm_reasoning_index()),
        temperature: window
            .get_llm_temperature_custom()
            .then_some(window.get_llm_temperature() as u16),
        top_p: window
            .get_llm_top_p_custom()
            .then_some(window.get_llm_top_p() as u16),
    }
}
pub(super) fn loaded(window: &SettingsWindow, page: &Page, provider: LlmProvider) {
    page.translations.borrow_mut().current =
        Some((provider, window.get_llm_selected_model().to_string()));
    refresh(window, page);
}
pub(super) fn saved_value(page: &Page, key: &Key) -> GenerationParameters {
    let mut value = page
        .config
        .borrow()
        .models
        .iter()
        .find(|m| m.provider == key.0 && m.model == key.1)
        .map(|m| m.parameters.clone())
        .unwrap_or_default();
    let options = page
        .cache
        .borrow()
        .get(&key.0)
        .and_then(|models| models.iter().find(|m| m.id == key.1))
        .map(|m| m.reasoning.clone())
        .unwrap_or_default();
    if !options.contains(&value.reasoning) {
        value.reasoning = if options.iter().any(|r| r == "off") {
            "off".into()
        } else {
            options.first().cloned().unwrap_or_else(|| "off".into())
        };
    }
    value
}
// Disabled controls retain their draft values, but do not become an unsaved change
// until their mode permits sending them. This matches the existing request mapping.
fn comparable(window: &SettingsWindow, mut value: GenerationParameters) -> GenerationParameters {
    value.temperature = if window.get_llm_temperature_available() {
        value
            .temperature
            .map(|v| v.min(window.get_llm_temperature_max() as u16))
    } else {
        None
    };
    value.top_p = if window.get_llm_top_p_available() {
        value
            .top_p
            .map(|v| v.max(window.get_llm_top_p_min() as u16))
    } else {
        None
    };
    value
}
pub(super) fn refresh(window: &SettingsWindow, page: &Page) {
    window.set_llm_translation_saving(page.translations.borrow().pending.is_some());
    window.set_llm_prompt_valid(window.get_llm_prompt().contains("{target_language}"));
    let Some(provider) = translation_kind(window) else {
        window.set_llm_translation_dirty(false);
        window.set_llm_translation_can_apply(false);
        return;
    };
    let key = (provider, window.get_llm_selected_model().to_string());
    let config = page.config.borrow();
    let dirty = window.get_llm_prompt().as_str() != config.prompt
        || comparable(window, read(window, page, &key))
            != comparable(window, saved_value(page, &key));
    let valid = !key.1.is_empty()
        && config.model_enabled(provider, &key.1)
        && window.get_llm_prompt_valid();
    drop(config);
    window.set_llm_translation_dirty(dirty);
    window.set_llm_translation_can_apply(valid);
    window.set_llm_translation_saving(page.translations.borrow().pending.is_some());
}
// Authoritative state can arrive before completion feedback. Only the matching
// request acknowledgement closes a modal; unrelated state updates never do.
pub(super) fn synced(window: &SettingsWindow, page: &Page) {
    refresh(window, page);
}
#[cfg(test)]
pub(super) fn failed(window: &SettingsWindow, page: &Page) {
    page.translations.borrow_mut().pending = None;
    refresh(window, page);
}
pub(super) fn submitted(
    window: &SettingsWindow,
    page: &Page,
    model: &TranslationModel,
    prompt: &str,
) -> u64 {
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    page.translations.borrow_mut().pending = Some((
        request_id,
        (model.provider, model.model.clone()),
        model.parameters.clone(),
        prompt.into(),
    ));
    window.set_llm_translation_saving(true);
    request_id
}
pub(super) fn clear(page: &Page) {
    *page.translations.borrow_mut() = Drafts::default();
}
pub(super) fn has_unsaved(window: &SettingsWindow, page: &Page) -> bool {
    refresh(window, page);
    window.get_translation_settings_open() && window.get_llm_translation_dirty()
}
pub(super) fn cancel(window: &SettingsWindow, page: &Page) {
    if page.translations.borrow().pending.is_some() {
        return;
    }
    page.translations.borrow_mut().current = None;
    window.set_llm_prompt(page.config.borrow().prompt.clone().into());
    load_translation(window, page, None);
    status(window, "");
}
pub(super) fn wire(window: &SettingsWindow, page: &Rc<Page>) {
    let weak = window.as_weak();
    let current = page.clone();
    window.on_llm_translation_edited(move || {
        if let Some(w) = weak.upgrade() {
            refresh(&w, &current);
        }
    });
    let weak = window.as_weak();
    let current = page.clone();
    window.on_llm_cancel_translation(move || {
        if let Some(w) = weak.upgrade() {
            cancel(&w, &current);
        }
    });
}

/// Build the combined picker from saved local metadata; opening it never discovers models.
pub(super) fn choices(window: &SettingsWindow, page: &Page) {
    let query = window.get_translation_search().to_lowercase();
    let config = page.config.borrow();
    let mut rows = Vec::new();
    let mut top = 0;
    if query.is_empty() || "deepl".contains(&query) {
        rows.push(crate::TranslationChoice {
            provider: 0,
            id: "DeepL".into(),
            group: "DeepL".into(),
            usable: config.deepl_enabled,
            note: if config.deepl_enabled {
                "".into()
            } else {
                crate::i18n::tr("Provider is disabled").into()
            },
            top,
            ..Default::default()
        });
        top += 76;
    }
    for (index, provider) in LlmProvider::ALL.into_iter().enumerate() {
        let label = ["Ollama", "LM Studio", "DeepSeek"][index];
        let mut first = true;
        for entry in config
            .catalog
            .iter()
            .filter(|m| m.provider == provider && m.enabled)
        {
            if !query.is_empty()
                && !format!("{label} {}", entry.model.id)
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            let usable = config.connection(provider).enabled
                && config.model_enabled(provider, &entry.model.id);
            let note = if !config.connection(provider).enabled {
                "Provider is disabled"
            } else if !entry.available {
                "Selected model is unavailable"
            } else if entry.endpoint != config.connection(provider).endpoint_key() {
                "Connection changed; sync models to confirm"
            } else {
                entry.model.issue.as_deref().unwrap_or("")
            };
            rows.push(crate::TranslationChoice {
                provider: index as i32 + 1,
                id: entry.model.id.clone().into(),
                group: if first { label.into() } else { "".into() },
                note: crate::i18n::tr(note).into(),
                usable,
                thinking: entry.model.reasoning.iter().any(|r| r != "off"),
                top,
            });
            top += if first { 76 } else { 48 };
            first = false;
        }
    }
    drop(config);
    window.set_translation_choices(ModelRc::new(VecModel::from(rows)));
    window.set_translation_list_height(top);
    window.set_translation_highlight(-1);
}
pub(super) fn select(window: &SettingsWindow, page: &Page, index: i32, name: String) {
    if window.get_translation_selecting() || window.get_translation_settings_open() {
        return;
    }
    let provider = match index {
        1 => ProviderConfig::Ollama,
        2 => ProviderConfig::LmStudio,
        3 => ProviderConfig::DeepSeek,
        _ => ProviderConfig::DeepL,
    };
    if window.get_llm_translation_provider_index() == index
        && (index == 0 || window.get_llm_selected_model() == name)
    {
        window.invoke_finish_translation_choice();
        return;
    }
    let model = if let Some(kind) = LlmProvider::ALL
        .get(index.wrapping_sub(1) as usize)
        .copied()
    {
        if !page.config.borrow().connection(kind).enabled
            || !page.config.borrow().model_enabled(kind, &name)
        {
            return;
        }
        Some(TranslationModel {
            provider: kind,
            parameters: saved_value(page, &(kind, name.clone())),
            model: name.clone(),
        })
    } else {
        if !page.config.borrow().deepl_enabled {
            return;
        }
        None
    };
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    page.translations.borrow_mut().selection = Some((request_id, index, name));
    window.set_translation_selecting(true);
    window.set_translation_selector_open(false);
    (page.send)(AppEvent::SettingsChangeRequested {
        change: SettingsChange::TranslationSelection {
            request_id,
            provider,
            model,
        },
    });
}
pub(super) fn selection_saved(window: &SettingsWindow, page: &Page) {
    let selected = page.translations.borrow_mut().selection.take();
    window.set_translation_selecting(false);
    window.invoke_finish_translation_choice();
    if let Some((_, index, name)) = selected {
        clear(page);
        window.set_llm_translation_provider_index(index);
        window.set_draft_provider_id(
            ["deepl", "ollama", "lmstudio", "deepseek"][index as usize].into(),
        );
        load_translation(window, page, Some(name));
    }
}
pub(super) fn wire_picker(window: &SettingsWindow, page: &Rc<Page>) {
    let weak = window.as_weak();
    let current = page.clone();
    window.on_translation_filter(move || {
        if let Some(w) = weak.upgrade() {
            choices(&w, &current);
        }
    });
    let weak = window.as_weak();
    let current = page.clone();
    window.on_translation_selected(move |index| {
        if let Some(w) = weak.upgrade()
            && let Ok(i) = usize::try_from(index)
            && let Some(choice) = w.get_translation_choices().row_data(i)
            && choice.usable
        {
            select(&w, &current, choice.provider, choice.id.to_string());
        }
    });
    let weak = window.as_weak();
    window.on_translation_move(move |delta| {
        if let Some(w) = weak.upgrade() {
            let rows = w.get_translation_choices();
            let count = rows.row_count() as i32;
            if count == 0 {
                return;
            }
            let start = w.get_translation_highlight();
            for step in 1..=count {
                let i = (if start < 0 && delta < 0 { count } else { start } + step * delta)
                    .rem_euclid(count);
                if rows.row_data(i as usize).is_some_and(|r| r.usable) {
                    w.set_translation_highlight(i);
                    break;
                }
            }
        }
    });
    let weak = window.as_weak();
    let current = page.clone();
    window.on_translation_open_settings(move || {
        if let Some(w) = weak.upgrade() {
            clear(&current);
            w.set_llm_prompt(current.config.borrow().prompt.clone().into());
            load_translation(&w, &current, None);
        }
    });
    choices(window, page);
}

pub(super) fn completed(
    window: &SettingsWindow,
    page: &Page,
    request_id: u64,
    field: lexift_core::domain::settings::SettingsField,
    saved: bool,
) -> bool {
    use lexift_core::domain::settings::SettingsField;
    if field == SettingsField::TranslationSelection {
        if !page
            .translations
            .borrow()
            .selection
            .as_ref()
            .is_some_and(|s| s.0 == request_id)
        {
            return false;
        }
        if saved {
            selection_saved(window, page);
        } else {
            page.translations.borrow_mut().selection = None;
            window.set_translation_selecting(false);
            window.invoke_finish_translation_choice();
        }
    } else {
        if !page
            .translations
            .borrow()
            .pending
            .as_ref()
            .is_some_and(|s| s.0 == request_id)
        {
            return false;
        }
        page.translations.borrow_mut().pending = None;
        if saved && window.get_translation_settings_open() {
            window.invoke_finish_translation_settings();
        }
        refresh(window, page);
    }
    true
}
