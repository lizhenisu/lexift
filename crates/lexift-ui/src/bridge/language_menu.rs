//! Language menu lifecycle and view wiring.
use super::*;
impl PopupRegistry {
    pub(super) fn open_language_menu(
        &mut self,
        session_id: u64,
        popup: &TranslationPopup,
        kind: LanguageMenuKind,
    ) {
        let owner = LanguageMenuOwner { session_id, kind };
        if self.language_menu_owner == Some(owner)
            && (self
                .language_menu
                .as_ref()
                .is_some_and(|menu| menu.window().is_visible())
                || self.language_menu_pending.is_some())
        {
            self.close_language_menu(true);
            return;
        }
        self.close_language_menu(true);
        let Some(work_area) = self.work_areas.get(&session_id).copied() else {
            return;
        };
        let menu = match self.language_menu.as_ref() {
            Some(menu) => menu.clone_strong(),
            None => {
                let Ok(menu) = PopupLanguageMenuWindow::new() else {
                    tracing::error!(session_id, "popup language menu could not be created");
                    return;
                };
                crate::theme::apply(&menu);
                wire_language_menu_callbacks(&menu);
                self.language_menu = Some(menu.clone_strong());
                menu
            }
        };
        self.disable_dismissal_watch(session_id, popup);
        popup.set_source_menu_open(kind == LanguageMenuKind::Source);
        popup.set_target_menu_open(kind == LanguageMenuKind::Target);
        self.language_menu_owner = Some(owner);
        match kind {
            LanguageMenuKind::Source => {
                menu.set_language_options(popup.get_source_language_options());
                menu.set_selected_index(popup.get_source_index());
            }
            LanguageMenuKind::Target => {
                menu.set_language_options(popup.get_target_language_options());
                menu.set_selected_index(popup.get_target_index());
            }
        }
        menu.set_menu_scroll_y(0.0);
        self.next_show_generation = self.next_show_generation.wrapping_add(1).max(1);
        let generation = self.next_show_generation;
        self.language_menu_pending = Some(PendingLanguageMenuShow {
            owner,
            generation,
            deadline: Instant::now() + POPUP_SHOW_TIMEOUT,
            retried_once: false,
            native_window_creation_requested: false,
        });
        self.try_show_language_menu(generation, work_area);
    }

    pub(super) fn try_show_language_menu(&mut self, generation: u64, work_area: Rect) {
        let Some(request) = self
            .language_menu_pending
            .filter(|request| request.generation == generation)
        else {
            return;
        };
        let Some(parent) = self
            .existing_window(request.owner.session_id)
            .filter(|window| {
                window.window().is_visible()
                    && match request.owner.kind {
                        LanguageMenuKind::Source => window.get_source_menu_open(),
                        LanguageMenuKind::Target => window.get_target_menu_open(),
                    }
            })
        else {
            self.close_language_menu(false);
            return;
        };
        let Some(menu) = self
            .language_menu
            .as_ref()
            .map(ComponentHandle::clone_strong)
        else {
            self.language_menu_pending = None;
            return;
        };
        match (self.prepare_passive_window)(menu.window()) {
            PassiveWindowPreparation::Ready => {
                self.language_menu_pending = None;
                self.finish_language_menu_show(request.owner, &parent, work_area);
            }
            PassiveWindowPreparation::Pending if Instant::now() < request.deadline => {
                let should_prime = self.language_menu_pending.as_mut().is_some_and(|pending| {
                    if pending.native_window_creation_requested {
                        false
                    } else {
                        pending.native_window_creation_requested = true;
                        true
                    }
                });
                if should_prime && !prime_hidden_language_menu(&menu) {
                    self.close_language_menu(true);
                    return;
                }
                let delay = popup_show_retry_delay(request.retried_once);
                if let Some(pending) = self.language_menu_pending.as_mut() {
                    pending.retried_once = true;
                }
                schedule_language_menu_show_retry(generation, work_area, delay);
            }
            PassiveWindowPreparation::Pending => {
                tracing::error!(
                    session_id = request.owner.session_id,
                    "popup language menu native window was not created before the show timeout"
                );
                self.close_language_menu(true);
            }
            PassiveWindowPreparation::Failed => self.close_language_menu(true),
        }
    }

    pub(super) fn finish_language_menu_show(
        &mut self,
        owner: LanguageMenuOwner,
        parent: &TranslationPopup,
        work_area: Rect,
    ) {
        self.position_language_menu(parent, owner.kind, work_area);
        let Some(menu) = self
            .language_menu
            .as_ref()
            .map(ComponentHandle::clone_strong)
        else {
            self.close_language_menu(false);
            return;
        };
        if !(self.attach_tool_window)(menu.window(), parent.window()) {
            self.close_language_menu(true);
            return;
        }
        if menu.show().is_err() {
            self.close_language_menu(true);
            return;
        }
        let pointer_sink = language_menu_pointer_sink(menu.as_weak());
        if !(self.complete_passive_window_show)(menu.window(), pointer_sink) {
            self.close_language_menu(true);
            return;
        }
        if (self.set_popup_dismissal)(menu.window(), true) {
            self.language_menu_dismissal_watched = true;
        }
        self.language_menu_owner = Some(owner);
    }

    pub(super) fn position_language_menu(
        &self,
        parent: &TranslationPopup,
        kind: LanguageMenuKind,
        work_area: Rect,
    ) {
        let scale = parent.window().scale_factor().max(f32::EPSILON);
        let parent_position = parent.window().position();
        let (anchor_x, anchor_y, anchor_width, anchor_height, option_count, selected_index) =
            match kind {
                LanguageMenuKind::Source => (
                    parent.get_source_menu_anchor_x(),
                    parent.get_source_menu_anchor_y(),
                    parent.get_source_menu_anchor_width(),
                    parent.get_source_menu_anchor_height(),
                    parent.get_source_language_options().row_count(),
                    parent.get_source_index(),
                ),
                LanguageMenuKind::Target => (
                    parent.get_target_menu_anchor_x(),
                    parent.get_target_menu_anchor_y(),
                    parent.get_target_menu_anchor_width(),
                    parent.get_target_menu_anchor_height(),
                    parent.get_target_language_options().row_count(),
                    parent.get_target_index(),
                ),
            };
        let anchor_left = parent_position.x + (anchor_x * scale).round() as i32;
        let anchor_top = parent_position.y + (anchor_y * scale).round() as i32;
        let anchor_width = (anchor_width * scale).round().max(1.0) as u32;
        let anchor_height = (anchor_height * scale).round().max(1.0) as u32;
        let visible_rows = option_count.min(LANGUAGE_MENU_VISIBLE_ROWS);
        let desired_height = (visible_rows as f32 * LANGUAGE_MENU_ROW_HEIGHT * scale)
            .round()
            .max(1.0) as u32;
        let placement = placement::place_attached_menu(
            Rect {
                left: anchor_left,
                top: anchor_top,
                right: anchor_left.saturating_add(anchor_width as i32),
                bottom: anchor_top.saturating_add(anchor_height as i32),
            },
            desired_height,
            work_area,
            LANGUAGE_MENU_GAP_PX,
            WORK_AREA_MARGIN_PX,
        );
        let logical_width = placement.width as f32 / scale;
        let logical_height = placement.height as f32 / scale;
        let Some(menu) = &self.language_menu else {
            return;
        };
        menu.set_menu_width(logical_width);
        menu.set_menu_height(logical_height);
        let max_scroll = (option_count as f32 * LANGUAGE_MENU_ROW_HEIGHT - logical_height).max(0.0);
        let selected = selected_index.max(0) as f32;
        menu.set_menu_scroll_y(
            -(((selected * LANGUAGE_MENU_ROW_HEIGHT - logical_height / 2.0
                + LANGUAGE_MENU_ROW_HEIGHT / 2.0)
                .clamp(0.0, max_scroll)
                / 4.0)
                .round()
                * 4.0),
        );
        menu.window().set_position(slint::PhysicalPosition::new(
            placement.position.x,
            placement.position.y,
        ));
        menu.window()
            .set_size(slint::LogicalSize::new(logical_width, logical_height));
    }

    pub(super) fn close_language_menu(&mut self, restore_parent_watch: bool) {
        self.language_menu_pending = None;
        if let Some(menu) = self.language_menu.take() {
            if self.language_menu_dismissal_watched {
                let _ = (self.set_popup_dismissal)(menu.window(), false);
            }
            self.language_menu_dismissal_watched = false;
            menu.set_menu_scroll_y(0.0);
            let _ = menu.hide();
            slint::Timer::single_shot(Duration::ZERO, move || drop(menu));
        }
        let Some(owner) = self.language_menu_owner.take() else {
            return;
        };
        let parent = self.existing_window(owner.session_id);
        if let Some(parent) = &parent {
            parent.set_source_menu_open(false);
            parent.set_target_menu_open(false);
        }
        if restore_parent_watch
            && let Some(parent) = parent
            && parent.window().is_visible()
            && self
                .states
                .get(&owner.session_id)
                .is_some_and(|state| !state.pinned)
        {
            self.set_dismissal_watch(owner.session_id, &parent, true);
        }
    }
}
pub(super) fn language_index(language: &Language) -> i32 {
    match language.0.as_str() {
        "zh-CN" => 0,
        "zh-TW" => 1,
        "en-US" => 2,
        "en-GB" => 3,
        "ja" => 4,
        "ko" => 5,
        "de" => 6,
        "fr" => 7,
        "es" => 8,
        "it" => 9,
        "pt-PT" => 10,
        "pt-BR" => 11,
        _ => 0,
    }
}

pub(super) fn language_for_index(index: i32) -> Language {
    let code = match index {
        0 => "zh-CN",
        1 => "zh-TW",
        2 => "en-US",
        3 => "en-GB",
        4 => "ja",
        5 => "ko",
        6 => "de",
        7 => "fr",
        8 => "es",
        9 => "it",
        10 => "pt-PT",
        _ => "pt-BR",
    };
    Language(code.into())
}

pub(super) fn source_language_for_index(index: i32) -> Option<Language> {
    let code = match index {
        0 => return None,
        1 => "en-US",
        2 => "zh-CN",
        3 => "ja",
        4 => "ko",
        5 => "de",
        6 => "fr",
        7 => "es",
        8 => "it",
        9 => "pt-PT",
        _ => return None,
    };
    Some(Language(code.into()))
}

pub(super) fn prime_hidden_language_menu(menu: &PopupLanguageMenuWindow) -> bool {
    if let Err(error) = menu.show() {
        tracing::error!(%error, "target language menu could not request native window creation");
        return false;
    }
    if let Err(error) = menu.hide() {
        let _ = menu.hide();
        tracing::error!(%error, "target language menu could not remain hidden during native window creation");
        return false;
    }
    true
}

pub(super) fn schedule_language_menu_show_retry(generation: u64, work_area: Rect, delay: Duration) {
    slint::Timer::single_shot(delay, move || {
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.try_show_language_menu(generation, work_area);
            }
        });
    });
}

pub(super) fn language_menu_pointer_sink(
    menu: slint::Weak<PopupLanguageMenuWindow>,
) -> PopupPointerSink {
    Rc::new(move |input| {
        let Some(menu) = menu.upgrade() else {
            return;
        };
        if matches!(input, PopupPointerInput::DismissRequested { .. }) {
            schedule_language_menu_dismissal();
            return;
        }
        let scale = menu.window().scale_factor().max(f32::EPSILON);
        let position = |x: f32, y: f32| slint::LogicalPosition::new(x / scale, y / scale);
        use slint::platform::{PointerEventButton, WindowEvent};
        let event = match input {
            PopupPointerInput::RightPressed { .. } | PopupPointerInput::RightReleased { .. } => {
                return;
            }
            PopupPointerInput::Moved { x, y } => WindowEvent::PointerMoved {
                position: position(x, y),
            },
            PopupPointerInput::Exited => WindowEvent::PointerExited,
            PopupPointerInput::LeftPressed { x, y } => WindowEvent::PointerPressed {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::LeftReleased { x, y }
            | PopupPointerInput::LeftCancelled { x, y } => WindowEvent::PointerReleased {
                position: position(x, y),
                button: PointerEventButton::Left,
            },
            PopupPointerInput::Scrolled {
                x,
                y,
                delta_x,
                delta_y,
            } => WindowEvent::PointerScrolled {
                position: position(x, y),
                delta_x: delta_x / scale,
                delta_y: delta_y / scale,
            },
            PopupPointerInput::DismissRequested { .. }
            | PopupPointerInput::NativeTopResizeRequested => {
                return;
            }
            PopupPointerInput::Resized { .. } | PopupPointerInput::ResizeFinished { .. } => return,
        };
        if let Err(error) = menu.window().dispatch_event_with_result(event) {
            tracing::warn!(%error, "target language menu pointer event could not be dispatched");
        }
    })
}

pub(super) fn schedule_language_menu_dismissal() {
    slint::Timer::single_shot(Duration::ZERO, move || {
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.close_language_menu(true);
            }
        });
    });
}

pub(super) fn wire_language_menu_callbacks(menu: &PopupLanguageMenuWindow) {
    menu.on_language_selected(move |selected_index| {
        let dispatch = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let registry = registry.as_mut()?;
            let owner = registry.language_menu_owner?;
            let popup = registry.existing_window(owner.session_id)?;
            let source = popup.get_source_text().to_string();
            let source_language = match owner.kind {
                LanguageMenuKind::Source => source_language_for_index(selected_index),
                LanguageMenuKind::Target => source_language_for_index(popup.get_source_index()),
            };
            let target_language = match owner.kind {
                LanguageMenuKind::Source => language_for_index(popup.get_target_index()),
                LanguageMenuKind::Target => language_for_index(selected_index),
            };
            let handler = registry.handler.as_ref().map(Rc::clone)?;
            registry.close_language_menu(true);
            Some((
                handler,
                owner.session_id,
                source,
                source_language,
                target_language,
            ))
        });
        if let Some((handler, session_id, source, source_language, target_language)) = dispatch {
            handler(AppEvent::PopupTranslationRequested {
                session_id: lexift_core::domain::translation::PopupSessionId::new(session_id),
                text: source,
                source_language,
                target_language,
            });
        }
    });
    menu.on_dismiss_requested(schedule_language_menu_dismissal);
    menu.window().on_close_requested(move || {
        schedule_language_menu_dismissal();
        slint::CloseRequestResponse::KeepWindowShown
    });
}
