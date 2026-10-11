//! OCR Provider settings presentation; independent of selection windows.
use lexift_core::{AppEvent, domain::ocr::OcrPhase};
use std::rc::Rc;
/// Forward OCR settings and resource actions through the same versioned Core path.
pub(crate) fn wire_settings(settings: &crate::SettingsWindow, handler: Rc<dyn Fn(AppEvent)>) {
    use lexift_core::domain::settings::SettingsChange;
    let h = handler.clone();
    settings.on_ocr_language_selected(move |index| {
        if let Some(value) = lexift_core::domain::ocr::language_at_index(index) {
            h(AppEvent::SettingsChangeRequested {
                change: SettingsChange::OcrLanguage(value),
            });
        }
    });
    let h = handler.clone();
    settings.on_ocr_korean_auto_toggled(move |value| {
        h(AppEvent::SettingsChangeRequested {
            change: SettingsChange::OcrKoreanAuto(value),
        });
    });
    let h = handler.clone();
    settings.on_ocr_korean_download_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::DownloadKorean,
            ),
        ));
    });
    let h = handler.clone();
    settings.on_ocr_korean_remove_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::RemoveKorean,
            ),
        ));
    });
    let h = handler.clone();
    settings.on_ocr_download_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::Download,
            ),
        ))
    });
    let h = handler.clone();
    settings.on_ocr_download_cancelled(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::CancelResourceDownload,
        ))
    });
    let h = handler.clone();
    settings.on_ocr_remove_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::Remove,
            ),
        ))
    });
    let h = handler.clone();
    settings.on_ocr_refresh_requested(move || {
        h(AppEvent::Ocr(
            lexift_core::domain::ocr::OcrEvent::ManageResources(
                lexift_core::domain::ocr::ResourceOperation::Check,
            ),
        ))
    });
}

pub(crate) fn apply_settings(window: &crate::SettingsWindow, state: &lexift_core::AppState) {
    let o = &state.ocr;
    window.set_draft_ocr_hotkey_label(state.desired_settings.ocr_hotkey.to_string().into());
    window.set_ocr_language_index(lexift_core::domain::ocr::language_index(
        state.desired_settings.ocr_language.as_deref(),
    ));
    use lexift_core::{domain::ocr::ResourceOperation, ports::ocr::OcrResourceStatus};
    crate::llm_settings::sync_ocr_enabled(window, state.settings.ocr_enabled);
    let manager = &o.resources;
    let info = manager.info.as_ref();
    let base_busy = manager
        .busy
        .filter(|op| op.kind() == lexift_core::ports::ocr::OcrResourceKind::Base);
    let korean_busy = manager
        .busy
        .filter(|op| op.kind() == lexift_core::ports::ocr::OcrResourceKind::Korean);
    let korean = info.and_then(|r| r.korean.as_deref());
    window.set_ocr_korean_auto(state.desired_settings.ocr_korean_auto);
    window.set_ocr_korean_auto_enabled(
        (korean.is_some_and(|r| r.ready) || state.desired_settings.ocr_korean_auto)
            && manager.busy.is_none()
            && state.settings_saving_field
                != Some(lexift_core::domain::settings::SettingsField::OcrKoreanAuto),
    );
    let korean_bytes = korean.map_or(0, |r| r.download_bytes);
    window.set_ocr_korean_ready(korean.is_some_and(|r| r.ready));
    window.set_ocr_korean_version(korean.map_or("—", |r| r.version.as_str()).into());
    window
        .set_ocr_korean_download_size(format!("{:.1} MiB", korean_bytes as f64 / 1048576.).into());
    window.set_ocr_korean_download_exact(
        format!(
            "{:.1} MiB ({} bytes)",
            korean_bytes as f64 / 1048576.,
            korean_bytes
        )
        .into(),
    );
    window.set_ocr_korean_installed_size(
        format!(
            "{:.1} MiB",
            korean.map_or(0, |r| r.installed_bytes) as f64 / 1048576.
        )
        .into(),
    );
    window.set_ocr_korean_damaged(korean.is_some_and(|r| r.status == OcrResourceStatus::Damaged));
    window.set_ocr_korean_downloading(korean_busy.is_some_and(ResourceOperation::downloading));
    window.set_ocr_korean_progress(
        (manager.received as f64 / korean_bytes.max(1) as f64).clamp(0., 1.) as f32,
    );
    window.set_ocr_korean_status(resource_status(korean, korean_busy).into());
    window.set_ocr_korean_status_kind(resource_status_kind(korean, korean_busy));
    let korean_error = manager
        .last_operation
        .is_some_and(|op| op.kind() == lexift_core::ports::ocr::OcrResourceKind::Korean)
        && !manager.error.is_empty();
    window.set_ocr_korean_note_error(korean_error);
    window.set_ocr_korean_note(if korean_error {
        crate::i18n::error(&manager.error).into()
    } else {
        "".into()
    });
    let downloading =
        base_busy == Some(ResourceOperation::Download) || o.phase == OcrPhase::Downloading;
    let pending = info.is_some_and(|i| i.status == OcrResourceStatus::PendingRemoval);
    let active = matches!(
        o.phase,
        OcrPhase::Checking | OcrPhase::Capturing | OcrPhase::Recognizing
    );
    window.set_ocr_korean_can_download(
        manager.busy.is_none()
            && !active
            && info.is_some_and(|r| r.ready)
            && !korean.is_some_and(|r| r.ready),
    );
    window.set_ocr_korean_can_remove(
        manager.busy.is_none() && !active && korean.is_some_and(|r| r.installed_bytes > 0),
    );
    window.set_ocr_downloading(downloading);
    window.set_ocr_resource_busy(base_busy.is_some());
    window.set_ocr_can_download(
        manager.busy.is_none() && !active && !pending && !info.is_some_and(|i| i.ready),
    );
    window.set_ocr_can_remove(
        manager.busy.is_none()
            && !active
            && info.is_some_and(|i| {
                i.installed_bytes > 0 || i.status == OcrResourceStatus::PendingRemoval
            }),
    );
    window.set_ocr_resource_damaged(info.is_some_and(|i| i.status == OcrResourceStatus::Damaged));
    let bytes = info.map_or(o.download_bytes, |i| i.download_bytes);
    window.set_ocr_download_size(
        format!("{:.1} MiB ({} bytes)", bytes as f64 / 1048576., bytes).into(),
    );
    window.set_ocr_download_size_compact(format!("{:.1} MiB", bytes as f64 / 1048576.).into());
    window.set_ocr_resource_ready(info.is_some_and(|i| i.ready));
    window.set_ocr_resource_status_kind(if base_busy.is_some() || downloading {
        2
    } else {
        match info.map(|i| i.status) {
            Some(OcrResourceStatus::Ready) => 1,
            Some(OcrResourceStatus::Damaged) => 3,
            Some(OcrResourceStatus::PendingRemoval) => 4,
            _ => 0,
        }
    });
    let received = if base_busy == Some(ResourceOperation::Download) {
        manager.received
    } else {
        o.downloaded_bytes
    };
    window.set_ocr_download_progress((received as f64 / bytes.max(1) as f64).clamp(0., 1.) as f32);
    window.set_ocr_resource_version(info.map_or("—", |i| i.version.as_str()).into());
    window.set_ocr_installed_size(
        format!(
            "{:.1} MiB",
            info.map_or(0, |i| i.installed_bytes) as f64 / 1048576.
        )
        .into(),
    );
    let mut details = Vec::new();
    if let Some(info) = info {
        for (title, prefix) in [
            ("Detection model", "det."),
            ("Multilingual recognition", "universal."),
            ("Shared runtime and licenses", ""),
        ] {
            let bytes: u64 = info
                .entries
                .iter()
                .filter(|e| {
                    if prefix.is_empty() {
                        !["det.", "universal.", "korean."]
                            .iter()
                            .any(|p| e.id.starts_with(p))
                    } else {
                        e.id.starts_with(prefix)
                    }
                })
                .map(|e| e.bytes)
                .sum();
            details.push(crate::OcrResourceRow {
                label: crate::i18n::tr(title).into(),
                size: format!("{:.1} MiB", bytes as f64 / 1048576.).into(),
            });
        }
    }
    window.set_ocr_model_rows(std::rc::Rc::new(slint::VecModel::from(details)).into());
    let status = if let Some(operation) = base_busy {
        match operation {
            ResourceOperation::Check => crate::i18n::tr("Checking OCR resources"),
            ResourceOperation::Remove | ResourceOperation::RemoveKorean => {
                crate::i18n::tr("Deleting OCR resources")
            }
            ResourceOperation::Download | ResourceOperation::DownloadKorean => format!(
                "{} — {:.0}%",
                crate::i18n::tr("Downloading OCR resources"),
                manager.received as f64 / bytes.max(1) as f64 * 100.
            ),
        }
    } else {
        crate::i18n::tr(match info.map(|i| i.status) {
            Some(OcrResourceStatus::PendingRemoval) => {
                "OCR resources will be removed after restart"
            }
            Some(OcrResourceStatus::Damaged) => "OCR resources are damaged",
            Some(OcrResourceStatus::Ready) => "OCR resources ready",
            _ => "OCR resources not installed",
        })
    };
    window.set_ocr_status(status.into());
    let note = if !manager.error.is_empty() && !korean_error {
        crate::i18n::error(&manager.error)
    } else if active {
        crate::i18n::tr("Finish the current OCR task before managing resources")
    } else if pending {
        crate::i18n::tr("Restart Lexift to finish deletion before downloading again")
    } else if base_busy == Some(ResourceOperation::Download) {
        crate::i18n::tr("Cancel the download before deleting resources")
    } else if !o.error.is_empty() {
        crate::i18n::error(&o.error)
    } else {
        String::new()
    };
    window.set_ocr_resource_note_error(
        (!manager.error.is_empty() && !korean_error)
            || (!active && !pending && !downloading && !o.error.is_empty()),
    );
    window.set_ocr_resource_note(note.into());
    if o.phase == OcrPhase::DownloadRequired
        && window.get_ocr_confirm_generation() != o.generation as i32
    {
        window.set_ocr_confirm_generation(o.generation as i32);
        window.set_active_category(5);
        use slint::Model;
        if let Some(index) = window
            .get_provider_navigation()
            .iter()
            .position(|p| p.provider_id == "paddleocr")
        {
            window.invoke_credential_hide_requested();
            window.set_provider_detail_index(index as i32);
            window.invoke_llm_service_selected(index as i32);
        }
        window.set_ocr_resource_target(
            if o.required_resource == lexift_core::ports::ocr::OcrResourceKind::Korean {
                1
            } else {
                0
            },
        );
        window.set_ocr_download_dialog(true);
    }
}
fn resource_status_kind(
    info: Option<&lexift_core::ports::ocr::OcrResources>,
    operation: Option<lexift_core::domain::ocr::ResourceOperation>,
) -> i32 {
    if operation.is_some() {
        return 2;
    }
    use lexift_core::ports::ocr::OcrResourceStatus;
    match info.map(|r| r.status) {
        Some(OcrResourceStatus::Ready) => 1,
        Some(OcrResourceStatus::Damaged) => 3,
        Some(OcrResourceStatus::PendingRemoval) => 4,
        _ => 0,
    }
}
fn resource_status(
    info: Option<&lexift_core::ports::ocr::OcrResources>,
    operation: Option<lexift_core::domain::ocr::ResourceOperation>,
) -> String {
    use lexift_core::{domain::ocr::ResourceOperation, ports::ocr::OcrResourceStatus};
    crate::i18n::tr(match operation {
        Some(ResourceOperation::Check) => "Checking OCR resources",
        Some(op) if op.downloading() => "Downloading OCR resources",
        Some(_) => "Deleting OCR resources",
        None => match info.map(|r| r.status) {
            Some(OcrResourceStatus::Ready) => "OCR resources ready",
            Some(OcrResourceStatus::Damaged) => "OCR resources are damaged",
            Some(OcrResourceStatus::PendingRemoval) => {
                "OCR resources will be removed after restart"
            }
            _ => "OCR resources not installed",
        },
    })
}
