#[test]
fn full_paint_repair_restores_discarded_pixels_where_plain_redraw_does_not() {
    use slint::{
        ComponentHandle,
        platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        },
    };
    use std::{cell::RefCell, rc::Rc};
    struct ReusedPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
    impl Platform for ReusedPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let adapter = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
            *self.0.borrow_mut() = Some(adapter.clone());
            Ok(adapter)
        }
    }
    let slot = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(ReusedPlatform(slot.clone()))).unwrap();
    let settings = crate::SettingsWindow::new().unwrap();
    settings
        .window()
        .set_size(slint::PhysicalSize::new(900, 700));
    settings.show().unwrap();
    let adapter = slot.borrow().as_ref().unwrap().clone();
    let mut pixels = vec![slint::Rgb8Pixel::default(); 900 * 700];
    adapter.draw_if_needed(|r| {
        r.render(&mut pixels, 900);
    });
    let expected = pixels.clone();
    pixels.fill(slint::Rgb8Pixel::default());
    settings.window().request_redraw();
    adapter.draw_if_needed(|r| {
        r.render(&mut pixels, 900);
    });
    assert_ne!(
        pixels, expected,
        "ordinary redraw cannot repair a discarded reused surface"
    );
    super::repair_window_paint(settings.window());
    adapter.draw_if_needed(|r| {
        r.render(&mut pixels, 900);
    });
    assert_eq!(pixels, expected, "full repair restores every client pixel");
}

use lexift_core::domain::{
    geometry::{Point, Rect},
    language::Language,
};

use super::{
    IdleTrimGeneration, MainWindowClosePolicy, ManualPopupSize, POPUP_MIN_SOURCE_HEIGHT,
    PendingPopupShow, PopupDragPhase, PopupDragScheduler, SettingsToastRecord, clamp_range,
    clamp_source_card_height, close_policy, credential_session_is_current, effective_manual_layout,
    has_valid_native_client_size, insert_settings_toast_record, language_index,
    manual_size_after_external_resize, max_source_card_height, popup_min_height,
    popup_min_window_height, popup_show_retry_delay, remove_settings_toast_record,
    source_language_for_index, toolbar_opacity,
};

#[test]
fn settings_keep_real_callbacks_and_clear_window_owned_editing_state() {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use std::cell::RefCell;
    struct TestPlatform;
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
        }
    }
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let settings = SettingsWindow::new().unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let generation = Arc::new(AtomicU64::new(0));
    wire_settings_callbacks(
        &settings,
        Rc::new(move |event| recorded.borrow_mut().push(event)),
        generation.clone(),
        Arc::new(Mutex::new(Vec::new())),
    );
    settings.invoke_select_category(1);
    settings.set_draft_target_index(0);
    settings.invoke_settings_menu_selected(4);
    assert!(
        matches!(&events.borrow()[0], AppEvent::SettingsChangeRequested { change: SettingsChange::TargetLanguage(language) } if language == &language_for_index(4))
    );
    assert_eq!(settings.get_appearance_language_index(), 2);
    events.borrow_mut().clear();

    settings.invoke_select_category(5);
    settings.set_draft_provider_id("".into());
    let recorded = events.clone();
    crate::llm_settings::wire(
        &settings,
        Default::default(),
        None,
        Rc::new(move |event| recorded.borrow_mut().push(event)),
    );
    settings.invoke_llm_provider_selected(0);
    assert!(matches!(
        &events.borrow()[0],
        AppEvent::SettingsChangeRequested {
            change: SettingsChange::Provider(
                lexift_core::domain::runtime_config::ProviderConfig::DeepL
            )
        }
    ));
    settings.invoke_credential_save_requested("test-only-placeholder".into());
    settings.invoke_credential_reveal_requested();
    settings.invoke_credential_copy_requested();
    settings.invoke_credential_edit_requested();
    settings.invoke_credential_remove_requested();
    {
        let events = events.borrow();
        assert!(matches!(
            &events[1],
            AppEvent::CredentialSaveRequested { .. }
        ));
        assert!(matches!(
            &events[2],
            AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Reveal,
                ..
            }
        ));
        assert!(matches!(
            &events[3],
            AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Copy,
                ..
            }
        ));
        assert!(matches!(
            &events[4],
            AppEvent::CredentialAccessRequested {
                purpose: CredentialAccessPurpose::Edit,
                ..
            }
        ));
        assert!(matches!(&events[5], AppEvent::CredentialRemoveRequested));
    }
    events.borrow_mut().clear();
    for (index, provider) in lexift_core::domain::llm::LlmProvider::ALL
        .into_iter()
        .enumerate()
    {
        settings.set_provider_detail_index(index as i32 + 1);
        settings.set_llm_service_index(index as i32);
        settings.invoke_credential_hide_requested();
        for purpose in [
            CredentialAccessPurpose::Reveal,
            CredentialAccessPurpose::Copy,
            CredentialAccessPurpose::Edit,
        ] {
            match purpose {
                CredentialAccessPurpose::Reveal => settings.invoke_credential_reveal_requested(),
                CredentialAccessPurpose::Copy => settings.invoke_credential_copy_requested(),
                CredentialAccessPurpose::Edit => settings.invoke_credential_edit_requested(),
            }
            assert!(
                matches!(events.borrow().last().unwrap(),AppEvent::LlmCredentialAccessRequested{provider:p,purpose:requested,..} if *p==provider && *requested==purpose)
            );
        }
        settings.invoke_credential_remove_requested();
        assert!(
            matches!(events.borrow().last().unwrap(),AppEvent::LlmCredentialChangeRequested{provider:p,secret:None} if *p==provider)
        );
    }
    settings.set_credential_draft("test-only-placeholder".into());
    settings.set_credential_transient_secret("test-only-placeholder".into());
    settings.set_credential_editing(true);
    let old_generation = generation.load(Ordering::Relaxed);
    settings.invoke_select_category(0);
    assert!(settings.get_credential_draft().is_empty());
    assert!(settings.get_credential_transient_secret().is_empty());
    assert!(!settings.get_credential_editing());
    assert!(generation.load(Ordering::Relaxed) > old_generation);
    drop(settings);
    let reopened = SettingsWindow::new().unwrap();
    assert_eq!(reopened.get_active_category(), 0);
    assert!(!reopened.get_hotkey_capturing());
    assert!(reopened.get_credential_draft().is_empty());
    assert!(!reopened.get_credential_editing());
}

#[test]
fn toolbar_fade_follows_distance_and_scale() {
    let bounds = Rect {
        left: 100,
        top: 100,
        right: 200,
        bottom: 148,
    };
    assert_eq!(toolbar_opacity(Point { x: 150, y: 120 }, bounds, 1.0), 1.0);
    assert_eq!(toolbar_opacity(Point { x: 224, y: 120 }, bounds, 1.0), 1.0);
    assert!((toolbar_opacity(Point { x: 322, y: 120 }, bounds, 1.0) - 0.5).abs() < 0.001);
    assert_eq!(toolbar_opacity(Point { x: 420, y: 120 }, bounds, 1.0), 0.0);
    assert!((toolbar_opacity(Point { x: 444, y: 120 }, bounds, 2.0) - 0.5).abs() < 0.001);
    assert_eq!(toolbar_opacity(Point { x: 224, y: 120 }, bounds, 1.0), 1.0);
}

#[test]
fn toolbar_fade_handles_negative_screen_coordinates() {
    let bounds = Rect {
        left: -1400,
        top: -800,
        right: -1300,
        bottom: -752,
    };
    assert!(
        (toolbar_opacity(Point { x: -1500, y: -780 }, bounds, 1.0) - 120.0 / 196.0).abs() < 0.001
    );
    assert_eq!(
        toolbar_opacity(Point { x: -1620, y: -780 }, bounds, 1.0),
        0.0
    );
}

#[test]
fn size_clamps_survive_an_inverted_or_missing_range() {
    // The maximised-popup case: a minimum height above the monitor's maximum.
    assert_eq!(clamp_range(1156.4, 1156.4, 1152.0), 1152.0);
    assert_eq!(clamp_range(100.0, 1156.4, 1152.0), 1152.0);
    assert_eq!(clamp_range(500.0, 340.0, 1920.0), 500.0);
    assert_eq!(clamp_range(10.0, f32::NAN, 100.0), 10.0);
    assert_eq!(clamp_range(10.0, 0.0, f32::NAN), 10.0);
}

#[test]
fn popup_minimum_height_never_exceeds_the_maximum() {
    // 1152 (work area) - 81.6 (source card) + 86 (minimum source card) = 1156.4.
    assert_eq!(popup_min_height(1156.4, 1152.0), 1152.0);
    assert_eq!(popup_min_height(500.0, 1152.0), 500.0);
    assert_eq!(popup_min_height(100.0, 1152.0), 336.0);
    assert_eq!(popup_min_height(900.0, 200.0), 336.0);
    assert_eq!(popup_min_height(f32::NAN, 1152.0), 336.0);
}

#[test]
fn popup_minimum_height_follows_the_layout_not_the_current_height() {
    // 240 (title, language bar, spacing and result-card minimum) + 86 (source card) = 326,
    // raised to the design floor. It must stay independent of the window height, otherwise a
    // full-height popup with a source card at its minimum would report min == max and Windows
    // would refuse every height change.
    let reserved_height = 240.0;
    let min_source_height = POPUP_MIN_SOURCE_HEIGHT;
    let work_area_height = 1152.0;

    let minimum = popup_min_window_height(reserved_height, min_source_height, work_area_height);
    assert_eq!(minimum, 336.0);
    // The state that froze the corners: a source card at its minimum inside a full-height
    // popup must still leave room to shrink.
    assert!(
        minimum < work_area_height,
        "a full-height popup must stay shrinkable, got {minimum}"
    );
    // A short work area still cannot invert the range.
    assert_eq!(
        popup_min_window_height(reserved_height, min_source_height, 200.0),
        336.0
    );
    assert_eq!(
        popup_min_window_height(reserved_height, 900.0, 1152.0),
        1140.0
    );
    assert_eq!(
        popup_min_window_height(f32::NAN, min_source_height, 1152.0),
        336.0
    );
    assert_eq!(
        popup_min_window_height(reserved_height, min_source_height, f32::INFINITY),
        336.0
    );
}

#[test]
fn idle_working_set_trim_is_invalidated_by_window_reopen_or_new_close() {
    let mut generation = IdleTrimGeneration::default();
    let first = generation.schedule();
    assert!(generation.is_current(first));

    generation.invalidate();
    assert!(!generation.is_current(first));

    let second = generation.schedule();
    let third = generation.schedule();
    assert!(!generation.is_current(second));
    assert!(generation.is_current(third));
}

#[test]
fn manual_popup_size_is_a_floor_and_only_new_content_adds_growth() {
    let manual = ManualPopupSize {
        width: 360.0,
        height: 350.0,
        source_height: 90.0,
        baseline_source_height: 120.0,
        baseline_remainder_height: 240.0,
    };

    assert_eq!(
        effective_manual_layout(manual, 300.0, 100.0),
        (360.0, 350.0, 90.0)
    );
    assert_eq!(
        effective_manual_layout(manual, 410.0, 140.0),
        (360.0, 400.0, 110.0)
    );
}

#[test]
fn popup_source_height_budget_keeps_the_language_and_result_rows_visible() {
    let reserved_height = 240.0;
    let minimum_source_height = POPUP_MIN_SOURCE_HEIGHT;

    assert_eq!(
        max_source_card_height(336.0, reserved_height, minimum_source_height),
        96.0
    );
    assert_eq!(
        clamp_source_card_height(1_000.0, 336.0, reserved_height, minimum_source_height),
        96.0
    );
    assert_eq!(
        clamp_source_card_height(20.0, 336.0, reserved_height, minimum_source_height),
        minimum_source_height
    );
    assert_eq!(
        clamp_source_card_height(1_000.0, 1_000.0, reserved_height, minimum_source_height),
        760.0
    );
}

#[test]
fn restoring_a_snapped_popup_reclamps_the_source_card_into_the_layout_budget() {
    // A Popup snapped to the 1152px work area stores a 912px source card. Dragging its title row
    // makes Windows restore the pre-snap size while the modal move loop blocks the event loop,
    // so `popup.slint` caps the rendered height with the same budget; this is the value the
    // release path then saves for the restored window.
    let reserved_height = 240.0;
    let restored_height = 440.0;

    let clamped = clamp_source_card_height(
        912.0,
        restored_height,
        reserved_height,
        POPUP_MIN_SOURCE_HEIGHT,
    );
    assert_eq!(clamped, 200.0);
    assert_eq!(
        max_source_card_height(restored_height, reserved_height, POPUP_MIN_SOURCE_HEIGHT),
        clamped
    );
    assert!(
        reserved_height + clamped <= restored_height,
        "the source card must leave room for the language row and the result card"
    );
    // The snapped height itself is inside the budget and stays untouched.
    assert_eq!(
        clamp_source_card_height(912.0, 1152.0, reserved_height, POPUP_MIN_SOURCE_HEIGHT),
        912.0
    );
}

#[test]
fn reopening_an_oversized_manual_popup_clamps_its_source_card() {
    let manual = ManualPopupSize {
        width: 420.0,
        height: 1_000.0,
        source_height: 900.0,
        baseline_source_height: 100.0,
        baseline_remainder_height: 200.0,
    };
    let (_, height, source_height) = effective_manual_layout(manual, 300.0, 100.0);

    assert_eq!(height, 1_000.0);
    assert_eq!(
        clamp_source_card_height(source_height, height, 240.0, POPUP_MIN_SOURCE_HEIGHT),
        760.0
    );
}

#[test]
fn restored_popup_size_reconciles_manual_layout_without_old_snap_height() {
    let restored = manual_size_after_external_resize(
        420.0,
        390.0,
        900.0,
        240.0,
        POPUP_MIN_SOURCE_HEIGHT,
        100.0,
        200.0,
    );

    assert_eq!(restored.source_height, 150.0);
    assert_eq!(
        effective_manual_layout(restored, 300.0, 100.0),
        (420.0, 390.0, 150.0)
    );
    assert!(
        restored.height - restored.source_height - 240.0 >= 0.0,
        "the language and result area reserve must remain available after restore"
    );
}

#[test]
fn minimized_or_invalid_native_sizes_are_not_reconciled() {
    assert!(!has_valid_native_client_size(0.0, 390.0));
    assert!(!has_valid_native_client_size(420.0, 0.0));
    assert!(!has_valid_native_client_size(-1.0, 390.0));
    assert!(!has_valid_native_client_size(f32::NAN, 390.0));
    assert!(has_valid_native_client_size(420.0, 390.0));
}

#[test]
fn popup_drag_waits_for_one_frame_and_dispatches_once() {
    let mut scheduler = PopupDragScheduler::default();

    assert_eq!(scheduler.phase(7), PopupDragPhase::Idle);
    assert!(scheduler.request(7));
    assert!(!scheduler.request(7));
    assert_eq!(scheduler.phase(7), PopupDragPhase::WaitingForFrame);
    assert!(scheduler.frame_rendered(7));
    assert!(!scheduler.frame_rendered(7));
    assert_eq!(scheduler.phase(7), PopupDragPhase::DispatchQueued);
    assert!(scheduler.take_dispatch(7));
    assert!(!scheduler.take_dispatch(7));
    assert_eq!(scheduler.phase(7), PopupDragPhase::Idle);
}

#[test]
fn popup_drag_can_be_cancelled_before_rendering() {
    let mut scheduler = PopupDragScheduler::default();

    assert!(scheduler.request(9));
    scheduler.cancel(9);
    assert_eq!(scheduler.phase(9), PopupDragPhase::Idle);
    assert!(!scheduler.frame_rendered(9));
    assert!(!scheduler.take_dispatch(9));
}

#[test]
fn popup_native_window_retry_yields_once_then_uses_frame_intervals() {
    assert_eq!(popup_show_retry_delay(false), std::time::Duration::ZERO);
    assert_eq!(
        popup_show_retry_delay(true),
        std::time::Duration::from_millis(16)
    );
}

#[test]
fn popup_native_window_creation_is_requested_only_once() {
    let mut request = PendingPopupShow::new(None, None, 1, None);

    assert!(request.request_native_window_creation());
    assert!(!request.request_native_window_creation());
}

#[test]
fn popup_source_language_indices_are_reduced_and_auto_detect_is_none() {
    assert_eq!(source_language_for_index(0), None);
    for (index, code) in [
        (1, "en-US"),
        (2, "zh-CN"),
        (3, "ja"),
        (4, "ko"),
        (5, "de"),
        (6, "fr"),
        (7, "es"),
        (8, "it"),
        (9, "pt-PT"),
    ] {
        assert_eq!(
            source_language_for_index(index),
            Some(Language(code.into()))
        );
    }
    assert_eq!(source_language_for_index(10), None);
}

#[test]
fn repeated_popup_show_keeps_the_native_creation_request() {
    let mut first = PendingPopupShow::new(None, None, 1, None);
    assert!(first.request_native_window_creation());
    first.retried_once = true;

    let replacement = PendingPopupShow::new(None, None, 2, Some(first));

    assert_eq!(replacement.generation, 2);
    assert!(replacement.retried_once);
    assert!(replacement.native_window_creation_requested);
}

#[test]
fn settings_feedback_refreshes_duplicates_and_does_not_expire_replacements() {
    let records = std::sync::Mutex::new(Vec::new());
    let toast = |id, message: &str, error| SettingsToastRecord {
        id,
        message: message.into(),
        error,
    };
    insert_settings_toast_record(&records, toast(1, "Theme saved", false));
    insert_settings_toast_record(&records, toast(2, "Theme saved", false));
    assert_eq!(records.lock().unwrap().len(), 1);
    assert!(
        !remove_settings_toast_record(&records, 1),
        "old timeout must not expire refreshed notice"
    );
    insert_settings_toast_record(&records, toast(3, "Provider wasn't saved", true));
    insert_settings_toast_record(&records, toast(4, "Interface language saved", false));
    assert_eq!(
        records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [3, 4]
    );
    insert_settings_toast_record(&records, toast(5, "Provider wasn't saved", true));
    assert!(!remove_settings_toast_record(&records, 3));
    insert_settings_toast_record(&records, toast(6, "Credential operation failed", true));
    insert_settings_toast_record(&records, toast(7, "Shortcut wasn't saved", true));
    assert_eq!(
        records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [5, 6, 7]
    );
    assert!(remove_settings_toast_record(&records, 6));
    assert_eq!(
        records
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [5, 7]
    );
}

#[test]
fn settings_toasts_are_independent_instances() {
    let records = std::sync::Mutex::new(vec![
        SettingsToastRecord {
            id: 1,
            message: "Shortcut saved".into(),
            error: false,
        },
        SettingsToastRecord {
            id: 2,
            message: "API key saved".into(),
            error: false,
        },
        SettingsToastRecord {
            id: 3,
            message: "Provider wasn't saved".into(),
            error: true,
        },
    ]);

    assert!(remove_settings_toast_record(&records, 2));
    assert!(!remove_settings_toast_record(&records, 2));
    let records = records.lock().unwrap();
    assert_eq!(
        records.iter().map(|toast| toast.id).collect::<Vec<_>>(),
        [1, 3]
    );
}

#[test]
fn stale_credential_result_cannot_affect_the_current_session() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let generation = AtomicU64::new(3);
    assert!(credential_session_is_current(&generation, 3));
    generation.fetch_add(1, Ordering::SeqCst);
    assert!(!credential_session_is_current(&generation, 3));
}

#[test]
fn main_window_only_hides_when_tray_registration_succeeded() {
    assert_eq!(close_policy(true), MainWindowClosePolicy::HideToTray);
    assert_eq!(close_policy(false), MainWindowClosePolicy::Exit);
}

#[test]
fn maps_canonical_languages_to_selector_indices() {
    for (index, code) in [
        "zh-CN", "zh-TW", "en-US", "en-GB", "ja", "ko", "de", "fr", "es", "it", "pt-PT", "pt-BR",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(language_index(&Language(code.into())), index as i32);
    }
}
