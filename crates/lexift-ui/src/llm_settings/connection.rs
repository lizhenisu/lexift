//! Per-provider page drafts, owned by the Settings window rather than persisted configuration.
use super::*;

/// Keep independently saved credential references current without replacing connection edits.
pub(super) fn refresh_credentials(connection: &mut LlmConnection, saved: &LlmConnection) {
    connection.enabled = saved.enabled;
    connection.credential_id = saved.credential_id.clone();
    for profile in &mut connection.protocols {
        profile.headers = saved
            .protocols
            .iter()
            .find(|p| p.protocol == profile.protocol)
            .map(|p| p.headers.clone())
            .unwrap_or_default();
    }
}

pub(super) enum Submission {
    Service {
        index: i32,
        headers: Vec<LlmHeaderUpdate>,
    },
}
pub(super) fn refresh_pending(window: &SettingsWindow, page: &Page) {
    let pending = page.awaiting_apply.borrow();
    window.set_llm_connection_pending(pending.is_some());
    window.set_llm_connection_saving(pending.as_ref().is_some_and(|submission| {
        let index = match submission {
            Submission::Service { index, .. } => *index,
        };
        index == window.get_provider_detail_index()
    }));
}
pub(super) fn completed(window: &SettingsWindow, page: &Page, success: bool) {
    let Some(submission) = page.awaiting_apply.borrow_mut().take() else {
        return;
    };
    let Submission::Service { index, headers } = submission;
    if success {
        if page.service_index.get() == index {
            page.pending_headers
                .borrow_mut()
                .retain(|h| !headers.contains(h));
            if let Some(snapshot) = page.request_snapshot.borrow_mut().as_mut() {
                snapshot.headers.retain(|h| !headers.contains(h));
                let saved = page
                    .config
                    .borrow()
                    .connection(snapshot.connection.provider);
                refresh_credentials(&mut snapshot.connection, &saved);
            }
        } else if let Some(draft) = page.connections.borrow_mut().get_mut(&index) {
            draft.headers.retain(|h| !headers.contains(h));
        }
    }
    refresh_headers(window, page);
    refresh_dirty(window, page);
}

pub(super) struct Draft {
    connection: LlmConnection,
    headers: Vec<LlmHeaderUpdate>,
    address: String,
    base: String,
    auth: i32,
}

/// Capture the previous provider even though Slint has already selected the next row.
pub(super) fn remember(window: &SettingsWindow, page: &Page) {
    let index = page.service_index.get();
    if !(1..=3).contains(&index) {
        return;
    }
    let mut drafts = page.connections.borrow_mut();
    if !window.get_llm_connection_dirty() {
        drafts.remove(&index);
        return;
    }
    // Unconfirmed modal changes are cancelled when navigating away.
    let (connection, headers, base, auth) =
        if let Some(snapshot) = page.request_snapshot.borrow().as_ref() {
            (
                snapshot.connection.clone(),
                snapshot.headers.clone(),
                snapshot.base.clone(),
                snapshot.auth,
            )
        } else {
            (
                page.draft.borrow().clone(),
                page.pending_headers.borrow().clone(),
                window.get_llm_base_url().to_string(),
                window.get_llm_auth_index(),
            )
        };
    drafts.insert(
        index,
        Draft {
            connection,
            headers,
            base,
            auth,
            address: window.get_llm_address().to_string(),
        },
    );
}

pub(super) fn restore(window: &SettingsWindow, page: &Page) {
    let index = window.get_provider_detail_index();
    page.service_index.set(index);
    if let Some(mut draft) = page.connections.borrow_mut().remove(&index) {
        // Credentials are independently saved; a dormant connection draft must not resurrect
        // a deleted key or obsolete header reference when it is restored or applied.
        let saved = page.config.borrow().connection(draft.connection.provider);
        refresh_credentials(&mut draft.connection, &saved);
        *page.draft.borrow_mut() = draft.connection.clone();
        *page.pending_headers.borrow_mut() = draft.headers.clone();
        window.set_llm_address(draft.address.clone().into());
        load_profile(window, &draft.connection);
        window.set_llm_base_url(draft.base.clone().into());
        window.set_llm_auth_index(draft.auth);
        update_endpoints(window, &draft.connection);
        refresh_headers(window, page);
    }
}

pub(super) fn discard(window: &SettingsWindow, page: &Page) {
    page.connections
        .borrow_mut()
        .remove(&window.get_provider_detail_index());
}
pub(super) fn has_unsaved(page: &Page) -> bool {
    let config = page.config.borrow();
    page.connections.borrow().iter().any(|(index, draft)| {
        if *index == 0 {
            return false;
        }
        let Ok(address) = service_address(&draft.address) else {
            return true;
        };
        let Ok(base) = api_base_address(&draft.base) else {
            return true;
        };
        let mut connection = draft.connection.clone();
        connection.address = address;
        connection.enabled = config.connection(connection.provider).enabled;
        let mut profile = connection.profile();
        profile.base_url = base;
        profile.authentication = match draft.auth {
            1 => Authentication::Headers,
            2 => Authentication::None,
            _ => Authentication::Automatic,
        };
        connection
            .protocols
            .retain(|p| p.protocol != connection.protocol);
        connection.protocols.push(profile);
        let saved = config.connection(connection.provider);
        !draft.headers.is_empty()
            || !connection.same_request_configuration(&saved)
            || LlmProtocol::ALL.iter().any(|protocol| {
                let mut a = connection.clone();
                let mut b = saved.clone();
                a.protocol = *protocol;
                b.protocol = *protocol;
                a.profile() != b.profile()
            })
    })
}
