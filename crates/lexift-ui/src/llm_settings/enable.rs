//! Correlated immediate enablement saves, independent of connection drafts.
use super::*;
static NEXT_ENABLE: AtomicU64 = AtomicU64::new(1);
pub(super) fn refresh(window: &SettingsWindow, page: &Page) {
    let Some(provider) = selected_provider(window) else {
        return;
    };
    let pending = page.enables.borrow();
    window.set_llm_enable_saving(pending.contains_key(&provider));
    let saved = match provider {
        ServiceProviderId::Translation(p) => page.config.borrow().provider_enabled(p),
        ServiceProviderId::PaddleOcr => page.ocr_enabled.get(),
    };
    let enabled = pending
        .get(&provider)
        .map_or(saved, |(_, enabled)| *enabled);
    window.set_llm_enabled(enabled);
    window.set_ocr_provider_enabled(page.ocr_enabled.get());
}
pub(super) fn wire(window: &SettingsWindow, page: &Rc<Page>) {
    let weak = window.as_weak();
    let page = page.clone();
    window.on_llm_enable_requested(move |enabled| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some(provider) = selected_provider(&window) else {
            return;
        };
        if page.enables.borrow().contains_key(&provider) {
            return;
        }
        let request_id = NEXT_ENABLE.fetch_add(1, Ordering::Relaxed);
        page.enables
            .borrow_mut()
            .insert(provider, (request_id, enabled));
        refresh(&window, &page);
        (page.send)(AppEvent::SettingsChangeRequested {
            change: SettingsChange::ProviderEnabled {
                request_id,
                provider,
                enabled,
            },
        });
    });
}
pub(super) fn completed(
    window: &SettingsWindow,
    request_id: u64,
    selected: ServiceProviderId,
    saved: bool,
) -> bool {
    let Some(page) = lookup_page(window) else {
        return false;
    };
    let mut pending = page.enables.borrow_mut();
    let Some((id, enabled)) = pending.get(&selected).copied() else {
        return false;
    };
    if id != request_id {
        return false;
    }
    pending.remove(&selected);
    drop(pending);
    if saved {
        match selected {
            ServiceProviderId::Translation(p) => {
                page.config.borrow_mut().set_provider_enabled(p, enabled)
            }
            ServiceProviderId::PaddleOcr => page.ocr_enabled.set(enabled),
        }
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
