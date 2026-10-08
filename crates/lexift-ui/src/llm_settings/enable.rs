//! Correlated immediate enablement saves, independent of connection drafts.
use super::*;
static NEXT_ENABLE: AtomicU64 = AtomicU64::new(1);
fn provider(index: i32) -> ProviderConfig {
    [
        ProviderConfig::DeepL,
        ProviderConfig::Ollama,
        ProviderConfig::LmStudio,
        ProviderConfig::DeepSeek,
    ][index.clamp(0, 3) as usize]
}
pub(super) fn refresh(window: &SettingsWindow, page: &Page) {
    let index = window.get_provider_detail_index();
    let pending = page.enables.borrow();
    window.set_llm_enable_saving(pending.contains_key(&index));
    window.set_llm_enabled(pending.get(&index).map_or_else(
        || page.config.borrow().provider_enabled(provider(index)),
        |(_, enabled)| *enabled,
    ));
}
pub(super) fn wire(window: &SettingsWindow, page: &Rc<Page>) {
    let weak = window.as_weak();
    let page = page.clone();
    window.on_llm_enable_requested(move |enabled| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let index = window.get_provider_detail_index();
        if page.enables.borrow().contains_key(&index) {
            return;
        }
        let request_id = NEXT_ENABLE.fetch_add(1, Ordering::Relaxed);
        page.enables
            .borrow_mut()
            .insert(index, (request_id, enabled));
        refresh(&window, &page);
        (page.send)(AppEvent::SettingsChangeRequested {
            change: SettingsChange::ProviderEnabled {
                request_id,
                provider: provider(index),
                enabled,
            },
        });
    });
}
pub(super) fn completed(
    window: &SettingsWindow,
    request_id: u64,
    selected: ProviderConfig,
    saved: bool,
) -> bool {
    let Some(page) = lookup_page(window) else {
        return false;
    };
    let index = (0..4).find(|i| provider(*i) == selected).unwrap();
    let mut pending = page.enables.borrow_mut();
    let Some((id, enabled)) = pending.get(&index).copied() else {
        return false;
    };
    if id != request_id {
        return false;
    }
    pending.remove(&index);
    drop(pending);
    if saved {
        page.config
            .borrow_mut()
            .set_provider_enabled(selected, enabled);
    }
    refresh_provider_states(window, &page.config.borrow());
    refresh(window, &page);
    refresh_dirty(window, &page);
    if let Some(provider) = translation_kind(window) {
        model_list(window, &page, provider);
    }
    translation::choices(window, &page);
    true
}
