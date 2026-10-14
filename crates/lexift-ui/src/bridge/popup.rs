//! Popup lifecycle and view wiring.
use super::*;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PopupDragPhase {
    #[default]
    Idle,
    WaitingForFrame,
    DispatchQueued,
}

#[derive(Default)]
pub(super) struct PopupDragScheduler {
    pub(super) phases: HashMap<u64, PopupDragPhase>,
}

impl PopupDragScheduler {
    pub(super) fn request(&mut self, session_id: u64) -> bool {
        if self.phase(session_id) != PopupDragPhase::Idle {
            return false;
        }
        self.phases
            .insert(session_id, PopupDragPhase::WaitingForFrame);
        true
    }

    pub(super) fn frame_rendered(&mut self, session_id: u64) -> bool {
        if self.phase(session_id) != PopupDragPhase::WaitingForFrame {
            return false;
        }
        self.phases
            .insert(session_id, PopupDragPhase::DispatchQueued);
        true
    }

    pub(super) fn take_dispatch(&mut self, session_id: u64) -> bool {
        if self.phase(session_id) != PopupDragPhase::DispatchQueued {
            return false;
        }
        self.phases.remove(&session_id);
        true
    }

    pub(super) fn cancel(&mut self, session_id: u64) {
        self.phases.remove(&session_id);
    }

    pub(super) fn phase(&self, session_id: u64) -> PopupDragPhase {
        self.phases.get(&session_id).copied().unwrap_or_default()
    }
}

pub(super) struct PopupRegistry {
    pub(super) language_menu: Option<PopupLanguageMenuWindow>,
    pub(super) language_menu_owner: Option<LanguageMenuOwner>,
    pub(super) language_menu_pending: Option<PendingLanguageMenuShow>,
    pub(super) language_menu_dismissal_watched: bool,
    pub(super) windows: HashMap<u64, TranslationPopup>,
    pub(super) states: HashMap<u64, mapper::PopupUiState>,
    pub(super) work_areas: HashMap<u64, Rect>,
    pub(super) handler: Option<Rc<dyn Fn(AppEvent)>>,
    pub(super) dismissal_watches: HashSet<u64>,
    pub(super) drag_scheduler: PopupDragScheduler,
    pub(super) manual_sizes: HashMap<u64, ManualPopupSize>,
    pub(super) active_resizes: HashMap<u64, ActivePopupResize>,
    pub(super) pending_window_size_syncs: HashSet<u64>,
    pub(super) pending_shows: HashMap<u64, PendingPopupShow>,
    pub(super) next_show_generation: u64,
    pub(super) prepare_passive_window: fn(&slint::Window) -> PassiveWindowPreparation,
    pub(super) configure_translation_popup_corners: fn(&slint::Window) -> PopupCornerMode,
    pub(super) configure_resize_background: ConfigureResizeBackground,
    pub(super) configure_window_paint_repair: ConfigureWindowPaintRepair,
    pub(super) complete_passive_window_show: CompletePassiveWindowShow,
    pub(super) begin_window_drag: fn(&slint::Window) -> bool,
    pub(super) begin_window_resize: BeginWindowResize,
    pub(super) popup_work_area: PopupWorkArea,
    pub(super) set_popup_dismissal: SetPopupDismissal,
    pub(super) attach_tool_window: AttachToolWindow,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ManualPopupSize {
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) source_height: f32,
    pub(super) baseline_source_height: f32,
    pub(super) baseline_remainder_height: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ActivePopupResize {
    pub(super) edge: PopupResizeEdge,
    pub(super) base_height: f32,
    pub(super) base_source_height: f32,
}

pub(super) fn effective_manual_layout(
    manual: ManualPopupSize,
    auto_height: f32,
    auto_source_height: f32,
) -> (f32, f32, f32) {
    let auto_remainder_height = auto_height - auto_source_height;
    let source_growth = (auto_source_height - manual.baseline_source_height).max(0.0);
    let remainder_growth = (auto_remainder_height - manual.baseline_remainder_height).max(0.0);
    (
        manual.width,
        manual.height + source_growth + remainder_growth,
        manual.source_height + source_growth,
    )
}

pub(super) fn max_source_card_height(
    window_height: f32,
    reserved_height: f32,
    minimum: f32,
) -> f32 {
    (window_height - reserved_height).max(minimum)
}

pub(super) fn clamp_source_card_height(
    source_height: f32,
    window_height: f32,
    reserved_height: f32,
    minimum: f32,
) -> f32 {
    clamp_range(
        source_height,
        minimum,
        max_source_card_height(window_height, reserved_height, minimum),
    )
}

/// Clamps a value into an ordered range without panicking on an inverted or NaN bound.
///
/// The popup's size limits are computed from live layout metrics and the monitor work area, so a
/// maximised window can produce a minimum above the maximum. `f32::clamp` aborts the process in
/// that case (the release profile uses `panic = "abort"`), so every size clamp goes through here.
pub(super) fn clamp_range(value: f32, min: f32, max: f32) -> f32 {
    if !min.is_finite() || !max.is_finite() {
        return if value.is_finite() { value } else { 0.0 };
    }
    // An inverted range means the derived minimum outgrew the monitor limit; the maximum wins so
    // the window still fits the screen.
    value.clamp(min.min(max), max)
}

/// The popup's minimum height, kept at or below its maximum so the sizing loop stays valid.
pub(super) fn popup_min_height(requested: f32, max_height: f32) -> f32 {
    let floor = POPUP_MIN_SOURCE_HEIGHT.max(336.0);
    let requested = if requested.is_finite() {
        requested
    } else {
        floor
    };
    let max_height = if max_height.is_finite() {
        max_height
    } else {
        floor
    };
    clamp_range(requested.max(floor), floor, max_height.max(floor))
}

/// The smallest height the popup layout can show on its monitor.
///
/// Everything except the source card has a fixed vertical cost (`reserved_height`), and the source
/// card must keep `min_source_height`, so the layout floor does not depend on the current window
/// height. Deriving it from the live height instead (`height - source + min_source`) made a popup
/// whose source card already sat at its minimum report a minimum equal to its current height, often
/// also equal to the monitor maximum. Slint forwards both bounds to Winit, Winit writes them into
/// `WM_GETMINMAXINFO`, and Windows then refuses every height change — which froze the popup's edges
/// and corners.
pub(super) fn popup_min_window_height(
    reserved_height: f32,
    min_source_height: f32,
    max_height: f32,
) -> f32 {
    popup_min_height(reserved_height + min_source_height, max_height)
}

pub(super) fn has_valid_native_client_size(width: f32, height: f32) -> bool {
    width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0
}

pub(super) fn manual_size_after_external_resize(
    width: f32,
    height: f32,
    source_height: f32,
    reserved_height: f32,
    minimum_source_height: f32,
    baseline_source_height: f32,
    baseline_remainder_height: f32,
) -> ManualPopupSize {
    ManualPopupSize {
        width,
        height,
        source_height: clamp_source_card_height(
            source_height,
            height,
            reserved_height,
            minimum_source_height,
        ),
        baseline_source_height,
        baseline_remainder_height,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LanguageMenuKind {
    Source,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LanguageMenuOwner {
    pub(super) session_id: u64,
    pub(super) kind: LanguageMenuKind,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingLanguageMenuShow {
    pub(super) owner: LanguageMenuOwner,
    pub(super) generation: u64,
    pub(super) deadline: Instant,
    pub(super) retried_once: bool,
    pub(super) native_window_creation_requested: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingPopupShow {
    pub(super) anchor: Option<Point>,
    pub(super) work_area: Option<Rect>,
    pub(super) generation: u64,
    pub(super) deadline: Instant,
    pub(super) retried_once: bool,
    pub(super) native_window_creation_requested: bool,
}

impl PendingPopupShow {
    pub(super) fn new(
        anchor: Option<Point>,
        work_area: Option<Rect>,
        generation: u64,
        previous: Option<Self>,
    ) -> Self {
        let (retried_once, native_window_creation_requested) = previous
            .map(|request| {
                (
                    request.retried_once,
                    request.native_window_creation_requested,
                )
            })
            .unwrap_or_default();
        Self {
            anchor,
            work_area,
            generation,
            deadline: Instant::now() + POPUP_SHOW_TIMEOUT,
            retried_once,
            native_window_creation_requested,
        }
    }

    pub(super) fn request_native_window_creation(&mut self) -> bool {
        if self.native_window_creation_requested {
            return false;
        }
        self.native_window_creation_requested = true;
        true
    }
}

impl PopupRegistry {
    pub(super) fn window_for(&mut self, session_id: u64) -> Option<TranslationPopup> {
        if let Some(window) = self.windows.get(&session_id) {
            return Some(window.clone_strong());
        }
        let window = TranslationPopup::new().ok()?;
        crate::theme::apply(&window);
        wire_popup_layout(&window, session_id);
        let weak = window.as_weak();
        let configure = self.configure_resize_background.clone();
        window.on_theme_background_changed(move || {
            if let Some(window) = weak.upgrade() {
                install_resize_background(
                    &configure,
                    window.window(),
                    window.get_resize_fallback_color(),
                );
            }
        });
        if let Some(handler) = &self.handler {
            wire_popup_callbacks(&window, Rc::clone(handler));
            wire_popup_close(&window, Rc::clone(handler));
        }
        self.windows.insert(session_id, window.clone_strong());
        Some(window)
    }

    pub(super) fn update(&mut self, states: Vec<mapper::PopupUiState>) {
        let previous_states = std::mem::replace(
            &mut self.states,
            states
                .into_iter()
                .map(|state| (state.session_id, state))
                .collect(),
        );
        for (id, window) in &self.windows {
            if let Some(state) = self.states.get(id) {
                let geometry_locked = self.active_resizes.contains_key(id)
                    || self.pending_window_size_syncs.contains(id)
                    || self.pending_shows.contains_key(id);
                apply_popup_preserving_draft(
                    window,
                    state,
                    previous_states.get(id),
                    self.manual_sizes.get(id).copied(),
                    geometry_locked,
                    self.work_areas.get(id).copied(),
                );
                if !geometry_locked && let Some(work_area) = self.work_areas.get(id) {
                    apply_popup_resize_bounds(window, *work_area);
                    clamp_popup_to_work_area(window, *work_area);
                }
            }
        }
        let watch_updates = self
            .states
            .iter()
            .filter_map(|(id, state)| {
                self.existing_window(*id)
                    .filter(|window| window.window().is_visible())
                    .map(|window| {
                        (
                            *id,
                            window,
                            !state.pinned
                                && !self
                                    .language_menu_owner
                                    .is_some_and(|owner| owner.session_id == *id),
                        )
                    })
            })
            .collect::<Vec<_>>();
        for (id, window, enabled) in watch_updates {
            self.set_dismissal_watch(id, &window, enabled);
        }
        if self
            .language_menu
            .as_ref()
            .is_some_and(|menu| menu.window().is_visible())
            && let Some(owner) = self.language_menu_owner
            && let Some(parent) = self.existing_window(owner.session_id)
            && let Some(work_area) = self.work_areas.get(&owner.session_id).copied()
        {
            self.position_language_menu(&parent, owner.kind, work_area);
        }
    }

    pub(super) fn show(&mut self, session_id: u64, anchor: Option<Point>, work_area: Option<Rect>) {
        let Some(window) = self.window_for(session_id) else {
            tracing::error!(session_id, "translation popup could not be created");
            return;
        };
        if self
            .language_menu_owner
            .is_some_and(|owner| owner.session_id == session_id)
        {
            self.close_language_menu(false);
        }
        reset_popup_transient_ui(&window);
        if let Some(state) = self.states.get(&session_id) {
            // A hidden HWND must establish its actual DPI before long content can
            // request a large native surface. Finish sizing in finish_show().
            binding::apply_popup_content(&window, state);
            // A new hotkey capture replaces any unsubmitted text in a reused popup.
            window.set_source_text(state.source.clone().into());
        } else {
            window.set_session_id(session_id as i32);
        }
        self.disable_dismissal_watch(session_id, &window);
        self.drag_scheduler.cancel(session_id);
        self.next_show_generation = self.next_show_generation.wrapping_add(1).max(1);
        let generation = self.next_show_generation;
        let previous_request = self.pending_shows.get(&session_id).copied();
        self.pending_shows.insert(
            session_id,
            PendingPopupShow::new(anchor, work_area, generation, previous_request),
        );
        self.try_show(session_id, generation);
    }

    pub(super) fn try_show(&mut self, session_id: u64, generation: u64) {
        let Some(request) = self
            .pending_shows
            .get(&session_id)
            .copied()
            .filter(|request| request.generation == generation)
        else {
            return;
        };
        let Some(window) = self.existing_window(session_id) else {
            self.pending_shows.remove(&session_id);
            return;
        };
        match (self.prepare_passive_window)(window.window()) {
            PassiveWindowPreparation::Ready => {
                self.pending_shows.remove(&session_id);
                let corner_mode = (self.configure_translation_popup_corners)(window.window());
                window.set_corner_mode(corner_mode);
                self.install_popup_resize_background(&window);
                self.finish_show(session_id, window, request.anchor, request.work_area);
            }
            PassiveWindowPreparation::Pending if Instant::now() < request.deadline => {
                let delay = popup_show_retry_delay(request.retried_once);
                let request_native_window_creation = self
                    .pending_shows
                    .get_mut(&session_id)
                    .is_some_and(PendingPopupShow::request_native_window_creation);
                if request_native_window_creation && !prime_hidden_popup_window(session_id, &window)
                {
                    self.pending_shows.remove(&session_id);
                    return;
                }
                if let Some(request) = self.pending_shows.get_mut(&session_id) {
                    request.retried_once = true;
                }
                schedule_popup_show_retry(session_id, generation, delay);
            }
            PassiveWindowPreparation::Pending => {
                self.pending_shows.remove(&session_id);
                self.hide(session_id);
                tracing::error!(
                    session_id,
                    "translation popup native window was not created before the show timeout"
                );
            }
            PassiveWindowPreparation::Failed => {
                self.pending_shows.remove(&session_id);
                self.hide(session_id);
            }
        }
    }

    pub(super) fn existing_window(&self, session_id: u64) -> Option<TranslationPopup> {
        self.windows
            .get(&session_id)
            .map(ComponentHandle::clone_strong)
    }

    pub(super) fn finish_show(
        &mut self,
        session_id: u64,
        window: TranslationPopup,
        anchor: Option<Point>,
        work_area: Option<Rect>,
    ) {
        if window.window().is_minimized() {
            window.window().set_minimized(false);
        }
        if let (Some(anchor), Some(work_area)) = (anchor, work_area) {
            self.work_areas.insert(session_id, work_area);
            apply_popup_resize_bounds(&window, work_area);
            if let Some(state) = self.states.get(&session_id) {
                apply_popup_preserving_draft(
                    &window,
                    state,
                    Some(state),
                    self.manual_sizes.get(&session_id).copied(),
                    false,
                    Some(work_area),
                );
            }
            let size = window.window().size();
            let placement = placement::place_popup(
                anchor,
                size.width,
                size.height,
                work_area,
                POPUP_GAP_PX,
                WORK_AREA_MARGIN_PX,
            );
            let mut x = placement.position.x;
            let mut y = placement.position.y;
            let occupied = self
                .states
                .iter()
                .filter(|(id, state)| **id != session_id && state.pinned)
                .filter_map(|(id, _)| self.windows.get(id).map(ComponentHandle::clone_strong))
                .filter(|candidate| candidate.window().is_visible())
                .map(|candidate| candidate.window().position())
                .collect::<Vec<_>>();
            for _ in 0..occupied.len() {
                if !occupied
                    .iter()
                    .any(|position| (position.x - x).abs() < 24 && (position.y - y).abs() < 24)
                {
                    break;
                }
                x += 24;
                y += 24;
                let min_x = work_area.left + WORK_AREA_MARGIN_PX;
                let min_y = work_area.top + WORK_AREA_MARGIN_PX;
                let max_x = (work_area.right - size.width as i32 - WORK_AREA_MARGIN_PX).max(min_x);
                let max_y =
                    (work_area.bottom - size.height as i32 - WORK_AREA_MARGIN_PX).max(min_y);
                x = x.clamp(min_x, max_x);
                y = y.clamp(min_y, max_y);
            }
            window
                .window()
                .set_position(slint::PhysicalPosition::new(x, y));
        }
        if !window.window().is_visible() && window.show().is_err() {
            return;
        }
        let pointer_sink = popup_pointer_sink(window.as_weak());
        if !(self.complete_passive_window_show)(window.window(), pointer_sink) {
            let _ = window.hide();
            return;
        }
        let enabled = self
            .states
            .get(&session_id)
            .is_some_and(|state| !state.pinned);
        self.set_dismissal_watch(session_id, &window, enabled);
    }

    pub(super) fn start_resize(
        &mut self,
        session_id: u64,
        window: &TranslationPopup,
        edge: PopupResizeEdge,
    ) {
        if self
            .language_menu_owner
            .is_some_and(|owner| owner.session_id == session_id)
        {
            self.close_language_menu(false);
        }
        self.disable_dismissal_watch(session_id, window);
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let actual_size = window.window().size();
        let height = actual_size.height as f32 / scale;
        let min_source_height = window.get_resize_min_source_height();
        let reserved_height = window.get_resize_layout_reserve_height();
        let source_height = clamp_source_card_height(
            window.get_source_card_height(),
            height,
            reserved_height,
            min_source_height,
        );
        window.set_source_card_height(source_height);
        window.set_resize_min_height(popup_min_window_height(
            reserved_height,
            min_source_height,
            window.get_max_popup_height(),
        ));
        window.set_resize_active(true);
        window.set_resize_changes_height(edge.changes_height());
        window.set_resize_fixed_remainder(height - source_height);
        self.active_resizes.insert(
            session_id,
            ActivePopupResize {
                edge,
                base_height: height,
                base_source_height: source_height,
            },
        );
    }

    pub(super) fn finish_resize(
        &mut self,
        session_id: u64,
        window: &TranslationPopup,
        width: f32,
        height: f32,
    ) {
        self.refresh_popup_work_area(session_id, window);
        let Some(active) = self.active_resizes.remove(&session_id) else {
            return;
        };
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let actual_size = window.window().size();
        let width = if width > 0.0 {
            width
        } else {
            actual_size.width as f32 / scale
        };
        let height = if height > 0.0 {
            height
        } else {
            actual_size.height as f32 / scale
        };
        let width = clamp_range(width, POPUP_MIN_WIDTH, window.get_max_popup_width());
        let height = clamp_range(
            height,
            window.get_resize_min_height(),
            window.get_max_popup_height(),
        );
        let min_source_height = window.get_resize_min_source_height();
        let reserved_height = window.get_resize_layout_reserve_height();
        let requested_source_height = if active.edge.changes_height() {
            (active.base_source_height + height - active.base_height).max(min_source_height)
        } else {
            active.base_source_height
        };
        let source_height = clamp_source_card_height(
            requested_source_height,
            height,
            reserved_height,
            min_source_height,
        );
        window.set_resize_active(false);
        window.set_resize_changes_height(false);
        window.set_popup_width(width);
        window.set_popup_height(height);
        window.set_source_card_height(source_height);
        window.set_resize_min_height(popup_min_window_height(
            reserved_height,
            min_source_height,
            window.get_max_popup_height(),
        ));
        if self.states.contains_key(&session_id) {
            let (auto_height, auto_source_height) = measured_popup_metrics(window);
            self.manual_sizes.insert(
                session_id,
                ManualPopupSize {
                    width,
                    height,
                    source_height,
                    baseline_source_height: auto_source_height,
                    baseline_remainder_height: auto_height - auto_source_height,
                },
            );
        }
        if let Some(work_area) = self.work_areas.get(&session_id).copied() {
            clamp_popup_to_work_area(window, work_area);
        }
        let enabled = self
            .states
            .get(&session_id)
            .is_some_and(|state| !state.pinned);
        self.set_dismissal_watch(session_id, window, enabled);
    }

    pub(super) fn refresh_popup_work_area(
        &mut self,
        session_id: u64,
        window: &TranslationPopup,
    ) -> Option<Rect> {
        let position = window.window().position();
        let size = window.window().size();
        let center = Point {
            x: position.x.saturating_add((size.width / 2) as i32),
            y: position.y.saturating_add((size.height / 2) as i32),
        };
        let work_area = (self.popup_work_area)(center)?;
        self.work_areas.insert(session_id, work_area);
        apply_popup_resize_bounds(window, work_area);
        Some(work_area)
    }

    pub(super) fn install_popup_resize_background(&self, window: &TranslationPopup) {
        if !install_resize_background(
            &self.configure_resize_background,
            window.window(),
            window.get_resize_fallback_color(),
        ) {
            tracing::debug!("translation popup resize background fill could not be installed yet");
        }
        if !(self.configure_window_paint_repair)(window.window(), window_paint_repair(window)) {
            tracing::debug!("translation popup paint repair could not be installed yet");
        }
    }

    pub(super) fn sync_external_window_size(&mut self, session_id: u64, window: &TranslationPopup) {
        if self.active_resizes.contains_key(&session_id) {
            return;
        }
        self.refresh_popup_work_area(session_id, window);

        let physical_size = window.window().size();
        if !has_valid_native_client_size(physical_size.width as f32, physical_size.height as f32) {
            return;
        }
        let scale = window.window().scale_factor().max(f32::EPSILON);
        let width = physical_size.width as f32 / scale;
        let height = (physical_size.height as f32 / scale).min(window.get_max_popup_height());
        // A delayed native notification for our own resize must not turn an automatic
        // popup into a manually sized one, or a later short translation cannot shrink it.
        if (width - window.get_popup_width()).abs() < 1.
            && (height - window.get_popup_height().min(window.get_max_popup_height())).abs() < 1.
        {
            if window.get_popup_height() > window.get_max_popup_height()
                && let Some(state) = self.states.get(&session_id)
            {
                apply_popup_preserving_draft(
                    window,
                    state,
                    Some(state),
                    self.manual_sizes.get(&session_id).copied(),
                    false,
                    self.work_areas.get(&session_id).copied(),
                );
            }
            if let Some(area) = self.work_areas.get(&session_id) {
                clamp_popup_to_work_area(window, *area);
            }
            invalidate_popup_surface_cache(window);
            return;
        }
        let minimum_source_height = window.get_resize_min_source_height();
        let reserved_height = window.get_resize_layout_reserve_height();
        let (baseline_source_height, baseline_remainder_height) =
            if self.states.contains_key(&session_id) {
                window.set_popup_width(width);
                let (auto_height, auto_source_height) = measured_popup_metrics(window);
                (auto_source_height, auto_height - auto_source_height)
            } else if let Some(previous) = self.manual_sizes.get(&session_id) {
                (
                    previous.baseline_source_height,
                    previous.baseline_remainder_height,
                )
            } else {
                (0.0, 0.0)
            };
        let manual = manual_size_after_external_resize(
            width,
            height,
            window.get_source_card_height(),
            reserved_height,
            minimum_source_height,
            baseline_source_height,
            baseline_remainder_height,
        );

        // The native window already has this size. Update Slint's preferred
        // dimensions and the saved manual layout so later Core updates cannot
        // restore a stale pre-snap height; never write geometry back to HWND.
        window.set_popup_width(manual.width);
        window.set_popup_height(manual.height);
        window.set_source_card_height(manual.source_height);
        window.set_resize_min_height(popup_min_window_height(
            reserved_height,
            minimum_source_height,
            window.get_max_popup_height(),
        ));

        self.manual_sizes.insert(session_id, manual);
        if let Some(area) = self.work_areas.get(&session_id) {
            clamp_popup_to_work_area(window, *area);
        }
        invalidate_popup_surface_cache(window);
    }

    pub(super) fn cancel_resize(&mut self, session_id: u64, window: &TranslationPopup) {
        if self.active_resizes.remove(&session_id).is_none() {
            return;
        }
        window.set_resize_active(false);
        window.set_resize_changes_height(false);
        let enabled = self
            .states
            .get(&session_id)
            .is_some_and(|state| !state.pinned);
        self.set_dismissal_watch(session_id, window, enabled);
    }

    pub(super) fn hide(&mut self, session_id: u64) {
        if self
            .language_menu_owner
            .is_some_and(|owner| owner.session_id == session_id)
        {
            self.close_language_menu(false);
        }
        if let Some(window) = self.windows.remove(&session_id) {
            self.disable_dismissal_watch(session_id, &window);
            reset_popup_transient_ui(&window);
            let _ = window.hide();
            slint::Timer::single_shot(Duration::ZERO, move || drop(window));
        }
        self.states.remove(&session_id);
        self.work_areas.remove(&session_id);
        self.drag_scheduler.cancel(session_id);
        self.active_resizes.remove(&session_id);
        self.pending_window_size_syncs.remove(&session_id);
        self.manual_sizes.remove(&session_id);
        self.pending_shows.remove(&session_id);
        schedule_idle_memory_trim();
    }

    pub(super) fn set_dismissal_watch(
        &mut self,
        session_id: u64,
        window: &TranslationPopup,
        enabled: bool,
    ) {
        if enabled == self.dismissal_watches.contains(&session_id) {
            return;
        }
        if (self.set_popup_dismissal)(window.window(), enabled) {
            if enabled {
                self.dismissal_watches.insert(session_id);
            } else {
                self.dismissal_watches.remove(&session_id);
            }
        } else if !enabled {
            self.dismissal_watches.remove(&session_id);
        }
    }

    pub(super) fn disable_dismissal_watch(&mut self, session_id: u64, window: &TranslationPopup) {
        self.set_dismissal_watch(session_id, window, false);
    }
}

pub(super) fn prime_hidden_popup_window(session_id: u64, window: &TranslationPopup) -> bool {
    if let Err(error) = window.show() {
        tracing::error!(
            session_id,
            %error,
            "translation popup could not request native window creation"
        );
        return false;
    }
    if let Err(error) = window.hide() {
        let _ = window.hide();
        tracing::error!(
            session_id,
            %error,
            "translation popup could not remain hidden during native window creation"
        );
        return false;
    }
    true
}

pub(super) fn popup_show_retry_delay(retried_once: bool) -> Duration {
    if retried_once {
        POPUP_SHOW_RETRY_DELAY
    } else {
        Duration::ZERO
    }
}

pub(super) fn schedule_popup_show_retry(session_id: u64, generation: u64, delay: Duration) {
    slint::Timer::single_shot(delay, move || {
        POPUP_REGISTRY.with(|registry| {
            if let Some(registry) = registry.borrow_mut().as_mut() {
                registry.try_show(session_id, generation);
            }
        });
    });
}

pub(crate) fn measured_popup_metrics(popup: &TranslationPopup) -> (f32, f32) {
    let source = (popup.get_measured_source_height().ceil() + 58.).clamp(86., 140.);
    // The reserve includes the result minimum; replace that minimum with the measured text card.
    let reserve = popup.get_resize_layout_reserve_height();
    let result = (popup.get_measured_result_height().ceil() + 74.).max(104.);
    let height = (reserve - 104. + source + result).max(336.);
    (height.min(popup.get_max_popup_height()), source)
}

/// Drop geometry-dependent graphics caches once the native surface/DPI changes.
/// Merely marking damage leaves stale opaque bounds in reused software buffers.
pub(crate) fn invalidate_popup_surface_cache(popup: &TranslationPopup) {
    let size = popup.window().size();
    let scale = popup.window().scale_factor();
    if size.width == 0
        || size.height == 0
        || (popup.get_cached_surface_width() == size.width as i32
            && popup.get_cached_surface_height() == size.height as i32
            && popup.get_cached_surface_scale() == scale)
    {
        return;
    }
    let adapter = i_slint_core::window::WindowInner::from_pub(popup.window()).window_adapter();
    let component = i_slint_core::window::WindowInner::from_pub(popup.window()).component();
    let root = i_slint_core::item_tree::ItemRc::new_root(component.clone());
    let mut items = vec![root.clone()];
    root.visit_descendants(|item| {
        items.push(item.clone());
        std::ops::ControlFlow::<()>::Continue(())
    });
    if let Err(error) = adapter.renderer().free_graphics_resources(
        i_slint_core::item_tree::ItemTreeRc::borrow(&component),
        &mut items.iter().map(|item| item.borrow()),
    ) {
        tracing::debug!(%error, "translation surface graphics cache could not be released");
    }
    popup.set_cached_surface_width(size.width as i32);
    popup.set_cached_surface_height(size.height as i32);
    popup.set_cached_surface_scale(scale);
    repair_window_paint(popup.window());
}

/// The component owns the timer; callbacks read current state, never a delayed text snapshot.
pub(super) fn wire_popup_layout(popup: &TranslationPopup, session_id: u64) {
    let timer = Rc::new(slint::Timer::default());
    let weak = popup.as_weak();
    popup.on_layout_requested(move || {
        let weak = weak.clone();
        timer.start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
            let Some(popup) = weak.upgrade() else {
                return;
            };
            POPUP_REGISTRY.with(|registry| {
                let Ok(mut registry) = registry.try_borrow_mut() else {
                    return;
                };
                let Some(registry) = registry.as_mut() else {
                    return;
                };
                if registry.active_resizes.contains_key(&session_id)
                    || registry.pending_window_size_syncs.contains(&session_id)
                    || registry.pending_shows.contains_key(&session_id)
                {
                    return;
                }
                let Some(state) = registry.states.get(&session_id).cloned() else {
                    return;
                };
                registry.refresh_popup_work_area(session_id, &popup);
                apply_popup_preserving_draft(
                    &popup,
                    &state,
                    Some(&state),
                    registry.manual_sizes.get(&session_id).copied(),
                    false,
                    registry.work_areas.get(&session_id).copied(),
                );
                if let Some(area) = registry.work_areas.get(&session_id) {
                    clamp_popup_to_work_area(&popup, *area);
                }
                invalidate_popup_surface_cache(&popup);
            });
        });
    });
}

pub(super) fn apply_popup_preserving_draft(
    popup: &TranslationPopup,
    state: &mapper::PopupUiState,
    previous: Option<&mapper::PopupUiState>,
    manual_size: Option<ManualPopupSize>,
    geometry_locked: bool,
    work_area: Option<Rect>,
) {
    let draft = popup.get_source_text();
    let old_scroll = popup.get_result_scroll_y();
    let old_bottom =
        -(popup.get_result_content_height() - popup.get_result_viewport_height()).max(0.);
    let follow_bottom = (old_scroll - old_bottom).abs() <= 2.;
    let continuation = previous.is_some_and(|old| {
        old.busy
            && old.source == state.source
            && !state.translated.is_empty()
            && state.translated.starts_with(&old.translated)
            && state.error.is_empty()
    });
    let result_changed = popup.get_translated_text().as_str() != state.translated
        || popup.get_error_text().as_str() != state.error;
    let preserve_draft = previous.is_some_and(|previous| {
        previous.source == state.source && draft.as_str() != previous.source
    });
    // Avoid invalidating measured height twice when the user has an unsubmitted draft.
    let mut effective_state;
    let state = if preserve_draft {
        effective_state = state.clone();
        effective_state.source = draft.to_string();
        &effective_state
    } else {
        state
    };
    if geometry_locked {
        // Core snapshots may arrive before Winit has consumed a native resize.
        // Refresh content, but leave geometry untouched until it is reconciled.
        binding::apply_popup_content(popup, state);
    } else if let Some(manual) = manual_size {
        binding::apply_popup_content(popup, state);
        popup.set_popup_width(manual.width);
        let (auto_height, auto_source_height) = measured_popup_metrics(popup);
        let (width, height, source_height) =
            effective_manual_layout(manual, auto_height, auto_source_height);
        popup.set_popup_width(width);
        popup.set_source_card_height(source_height);
        popup.set_popup_height(height.min(popup.get_max_popup_height()));
        popup.set_resize_min_height(popup_min_window_height(
            popup.get_resize_layout_reserve_height(),
            popup.get_resize_min_source_height(),
            popup.get_max_popup_height(),
        ));
    } else {
        binding::apply_popup_content(popup, state);
        popup.set_popup_width(420.);
        let (height, source) = measured_popup_metrics(popup);
        popup.set_source_card_height(source);
        popup.set_popup_height(height);
    }
    if !geometry_locked {
        let min_source_height = popup.get_resize_min_source_height();
        let height = popup.get_popup_height();
        let reserved_height = popup.get_resize_layout_reserve_height();
        let source_height = clamp_source_card_height(
            popup.get_source_card_height(),
            height,
            reserved_height,
            min_source_height,
        );
        popup.set_source_card_height(source_height);
        popup.set_resize_min_height(popup_min_window_height(
            reserved_height,
            min_source_height,
            popup.get_max_popup_height(),
        ));
    }
    let scroll = if continuation {
        if follow_bottom {
            -(popup.get_result_content_height() - popup.get_result_viewport_height()).max(0.)
        } else {
            old_scroll
        }
    } else if result_changed {
        0.
    } else {
        old_scroll
    };
    popup.set_result_scroll_y(scroll.clamp(
        -(popup.get_result_content_height() - popup.get_result_viewport_height()).max(0.),
        0.,
    ));
    if !geometry_locked {
        let size = popup
            .window()
            .size()
            .to_logical(popup.window().scale_factor());
        if (size.width - popup.get_popup_width()).abs() >= 0.5
            || (size.height - popup.get_popup_height()).abs() >= 0.5
        {
            if let Some(area) = work_area {
                let scale = popup.window().scale_factor();
                clamp_popup_position(
                    popup,
                    area,
                    slint::PhysicalSize::new(
                        (popup.get_popup_width() * scale).ceil() as u32,
                        (popup.get_popup_height() * scale).ceil() as u32,
                    ),
                );
            }
            popup.window().set_size(slint::LogicalSize::new(
                popup.get_popup_width(),
                popup.get_popup_height(),
            ));
        }
    }
}

pub(super) fn schedule_popup_external_size_sync(
    session_id: u64,
    popup: slint::Weak<TranslationPopup>,
) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        let mut retry = false;
        POPUP_REGISTRY.with(|registry| {
            let Ok(mut registry) = registry.try_borrow_mut() else {
                retry = true;
                return;
            };
            let Some(registry) = registry.as_mut() else {
                return;
            };
            if !registry.pending_window_size_syncs.remove(&session_id)
                || registry.active_resizes.contains_key(&session_id)
            {
                return;
            }
            let Some(popup) = popup.upgrade().filter(|popup| {
                popup.window().is_visible() && popup.get_session_id().max(0) as u64 == session_id
            }) else {
                return;
            };
            registry.sync_external_window_size(session_id, &popup);
        });
        if retry {
            // A native resize can be reported from inside a registry operation.
            // Retry on the next event-loop turn, after that operation returns.
            schedule_popup_external_size_sync(session_id, popup.clone());
        }
    });
}

pub(super) fn popup_pointer_sink(popup: slint::Weak<TranslationPopup>) -> PopupPointerSink {
    Rc::new(move |input| {
        let Some(popup) = popup.upgrade() else {
            return;
        };
        let scale = popup.window().scale_factor().max(f32::EPSILON);
        let position = |x: f32, y: f32| slint::LogicalPosition::new(x / scale, y / scale);
        use slint::platform::{PointerEventButton, WindowEvent};
        let event = match input {
            PopupPointerInput::RightPressed { .. } | PopupPointerInput::RightReleased { .. } => {
                return;
            }
            PopupPointerInput::NativeTopResizeRequested => {
                let edge = PopupResizeEdge::Top;
                if let Some(begin_resize) = prepare_popup_resize(&popup, edge)
                    && !begin_resize(popup.window(), edge, true)
                {
                    cancel_popup_resize(&popup);
                }
                return;
            }
            PopupPointerInput::DismissRequested { .. } => {
                let session_id = popup.get_session_id().max(0) as u64;
                schedule_popup_dismissal(session_id);
                return;
            }
            PopupPointerInput::Resized { width, height } => {
                let session_id = popup.get_session_id().max(0) as u64;
                if !has_valid_native_client_size(width, height) {
                    return;
                }
                let should_schedule_sync = POPUP_REGISTRY.with(|registry| {
                    // Slint/Winit may synchronously emit WM_SIZE while a
                    // registry operation calls Window::set_size(). That size
                    // is already reflected by the caller, so ignore only that
                    // re-entrant notification instead of panicking across the
                    // native window procedure boundary.
                    let Ok(mut registry) = registry.try_borrow_mut() else {
                        return false;
                    };
                    if let Some(registry) = registry.as_mut() {
                        // A resize owned by this registry is applied once when the
                        // native loop ends; every other size change is external.
                        if registry.active_resizes.contains_key(&session_id) {
                            false
                        } else {
                            registry.pending_window_size_syncs.insert(session_id)
                        }
                    } else {
                        false
                    }
                });
                if should_schedule_sync {
                    schedule_popup_external_size_sync(session_id, popup.as_weak());
                }
                return;
            }
            PopupPointerInput::ResizeFinished { width, height } => {
                let session_id = popup.get_session_id().max(0) as u64;
                let weak = popup.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    let Some(popup) = weak.upgrade().filter(|popup| {
                        popup.window().is_visible()
                            && popup.get_session_id().max(0) as u64 == session_id
                    }) else {
                        return;
                    };
                    let scale = popup.window().scale_factor().max(f32::EPSILON);
                    POPUP_REGISTRY.with(|registry| {
                        let Ok(mut registry) = registry.try_borrow_mut() else {
                            return;
                        };
                        if let Some(registry) = registry.as_mut() {
                            registry.finish_resize(
                                session_id,
                                &popup,
                                width / scale,
                                height / scale,
                            );
                        }
                    });
                });
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
        };
        if let Err(error) = popup.window().dispatch_event_with_result(event) {
            tracing::warn!(%error, "translation popup pointer event could not be dispatched");
        }
    })
}

pub(super) fn schedule_popup_dismissal(session_id: u64) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        let handler = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let registry = registry.as_mut()?;
            if registry
                .states
                .get(&session_id)
                .is_none_or(|state| state.pinned)
            {
                return None;
            }
            let handler = registry.handler.as_ref().map(Rc::clone);
            registry.hide(session_id);
            handler
        });
        if let Some(handler) = handler {
            handler(AppEvent::PopupClosed {
                session_id: lexift_core::domain::translation::PopupSessionId::new(session_id),
            });
        }
    });
}

pub(super) fn clamp_popup_to_work_area(popup: &TranslationPopup, work_area: Rect) {
    clamp_popup_position(popup, work_area, popup.window().size());
}

pub(super) fn clamp_popup_position(
    popup: &TranslationPopup,
    work_area: Rect,
    size: slint::PhysicalSize,
) {
    let position = popup.window().position();
    let min_x = work_area.left;
    let min_y = work_area.top;
    let max_x = (work_area.right - size.width as i32).max(min_x);
    let max_y = (work_area.bottom - size.height as i32).max(min_y);
    let x = position.x.clamp(min_x, max_x.max(min_x));
    let y = position.y.clamp(min_y, max_y.max(min_y));
    if x != position.x || y != position.y {
        popup
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
}

pub(super) fn apply_popup_resize_bounds(popup: &TranslationPopup, work_area: Rect) {
    let scale = popup.window().scale_factor().max(f32::EPSILON);
    let width = ((work_area.right - work_area.left).max(1) as f32 / scale).max(POPUP_MIN_WIDTH);
    let height = ((work_area.bottom - work_area.top).max(1) as f32 / scale).max(336.0);
    // The maximum must never fall below the current minimum, or the sizing loop would receive an
    // inverted range.
    popup.set_max_popup_width(width.max(POPUP_MIN_WIDTH));
    popup.set_max_popup_height(height.max(popup.get_resize_min_height()).max(336.0));
}

pub(super) fn popup_session_id(
    popup: &TranslationPopup,
) -> lexift_core::domain::translation::PopupSessionId {
    lexift_core::domain::translation::PopupSessionId::new(popup.get_session_id().max(0) as u64)
}

pub(super) fn reset_popup_transient_ui(popup: &TranslationPopup) {
    popup.set_source_menu_open(false);
    popup.set_target_menu_open(false);
    popup.set_resize_active(false);
}

pub(super) fn wire_popup_close(popup: &TranslationPopup, handler: Rc<dyn Fn(AppEvent)>) {
    let weak = popup.as_weak();
    popup.window().on_close_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            reset_popup_transient_ui(&popup);
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.hide(session_id.value());
                } else {
                    let _ = popup.hide();
                }
            });
            handler(AppEvent::PopupClosed { session_id });
        }
        slint::CloseRequestResponse::HideWindow
    });
}

pub(super) fn install_popup_drag_render_notifier(popup: &TranslationPopup) -> bool {
    let weak = popup.as_weak();
    match popup.window().set_rendering_notifier(move |state, _| {
        if !matches!(state, slint::RenderingState::AfterRendering) {
            return;
        }
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        schedule_popup_drag_dispatch(popup.as_weak(), session_id, Duration::ZERO);
    }) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "translation popup could not synchronize dragging with rendering");
            false
        }
    }
}

pub(super) fn schedule_popup_drag_dispatch(
    popup: slint::Weak<TranslationPopup>,
    session_id: u64,
    delay: Duration,
) {
    slint::Timer::single_shot(delay, move || {
        // Copy the callback and release the registry borrow before entering the
        // native move loop. ReleaseCapture can synchronously dispatch UI events.
        let begin_window_drag = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let registry = registry.as_mut()?;
            if !registry.drag_scheduler.frame_rendered(session_id)
                || !registry.drag_scheduler.take_dispatch(session_id)
            {
                return None;
            }
            Some(registry.begin_window_drag)
        });
        let Some(popup) = popup.upgrade().filter(|popup| {
            popup.window().is_visible() && popup.get_session_id().max(0) as u64 == session_id
        }) else {
            return;
        };
        if let Some(begin_window_drag) = begin_window_drag {
            begin_window_drag(popup.window());
        }
    });
}

pub(super) fn prepare_popup_resize(
    popup: &TranslationPopup,
    edge: PopupResizeEdge,
) -> Option<BeginWindowResize> {
    let session_id = popup.get_session_id().max(0) as u64;
    POPUP_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let registry = registry.as_mut()?;
        if registry.active_resizes.contains_key(&session_id) {
            return None;
        }
        registry.refresh_popup_work_area(session_id, popup);
        registry.start_resize(session_id, popup, edge);
        Some(Rc::clone(&registry.begin_window_resize))
    })
}

pub(super) fn cancel_popup_resize(popup: &TranslationPopup) {
    let session_id = popup.get_session_id().max(0) as u64;
    POPUP_REGISTRY.with(|registry| {
        if let Some(registry) = registry.borrow_mut().as_mut() {
            registry.cancel_resize(session_id, popup);
        }
    });
}

pub(super) fn wire_popup_callbacks(popup: &TranslationPopup, handler: Rc<dyn Fn(AppEvent)>) {
    let drag_render_notifier_installed = install_popup_drag_render_notifier(popup);
    let weak = popup.as_weak();
    popup.on_drag_requested(move || {
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        let scheduled = POPUP_REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let Some(registry) = registry.as_mut() else {
                return false;
            };
            if registry
                .language_menu_owner
                .is_some_and(|owner| owner.session_id == session_id)
            {
                registry.close_language_menu(true);
            }
            registry.drag_scheduler.request(session_id)
        });
        if !scheduled {
            return;
        }
        popup.window().request_redraw();
        if !drag_render_notifier_installed {
            schedule_popup_drag_dispatch(popup.as_weak(), session_id, POPUP_DRAG_FALLBACK_DELAY);
        }
    });

    let weak = popup.as_weak();
    popup.on_resize_requested(move |edge_index| {
        let Some(edge) = PopupResizeEdge::from_index(edge_index) else {
            return;
        };
        let Some(popup) = weak.upgrade() else {
            return;
        };
        let Some(begin_resize) = prepare_popup_resize(&popup, edge) else {
            return;
        };
        let session_id = popup.get_session_id().max(0) as u64;
        let weak = popup.as_weak();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(popup) = weak.upgrade().filter(|popup| {
                popup.window().is_visible() && popup.get_session_id().max(0) as u64 == session_id
            }) else {
                return;
            };
            if !begin_resize(popup.window(), edge, false) {
                cancel_popup_resize(&popup);
            }
        });
    });

    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_translate_requested(move |text, source_index, target_index| {
        if let Some(popup) = weak.upgrade() {
            event_handler(AppEvent::PopupTranslationRequested {
                session_id: popup_session_id(&popup),
                text: text.to_string(),
                source_language: source_language_for_index(source_index),
                target_language: language_for_index(target_index),
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_source_menu_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.open_language_menu(session_id, &popup, LanguageMenuKind::Source);
                }
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_target_menu_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.open_language_menu(session_id, &popup, LanguageMenuKind::Target);
                }
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_language_menu_dismiss_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup.get_session_id().max(0) as u64;
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut()
                    && registry
                        .language_menu_owner
                        .is_some_and(|owner| owner.session_id == session_id)
                {
                    registry.close_language_menu(true);
                }
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_pin_requested(move |pinned| {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    let enabled = !pinned
                        && !registry
                            .language_menu_owner
                            .is_some_and(|owner| owner.session_id == session_id.value());
                    registry.set_dismissal_watch(session_id.value(), &popup, enabled);
                }
            });
            event_handler(AppEvent::PopupPinChanged { session_id, pinned });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_close_requested(move || {
        if let Some(popup) = weak.upgrade() {
            let session_id = popup_session_id(&popup);
            reset_popup_transient_ui(&popup);
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.hide(session_id.value());
                } else {
                    let _ = popup.hide();
                }
            });
            event_handler(AppEvent::PopupClosed { session_id });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    let expired_weak = popup.as_weak();
    let expired_handler = handler.clone();
    popup.on_copy_feedback_expired(move |source, request| {
        if let (Some(w), Ok(request)) = (expired_weak.upgrade(), request.parse()) {
            expired_handler(AppEvent::PopupCopyFeedbackCleared {
                session_id: popup_session_id(&w),
                source,
                request,
            });
        }
    });
    popup.on_copy_requested(move |source| {
        if let Some(popup) = weak.upgrade() {
            let text = if source {
                popup.get_source_text()
            } else {
                popup.get_translated_text()
            };
            event_handler(AppEvent::PopupCopyRequested {
                session_id: popup_session_id(&popup),
                source,
                text: text.to_string(),
            });
        }
    });
    let weak = popup.as_weak();
    let event_handler = Rc::clone(&handler);
    popup.on_speech_requested(move |source| {
        if let Some(popup) = weak.upgrade() {
            let text = if source {
                popup.get_source_text()
            } else {
                popup.get_translated_text()
            };
            event_handler(AppEvent::PopupSpeechRequested {
                session_id: popup_session_id(&popup),
                source,
                text: text.to_string(),
                language: if source {
                    let selected = popup.get_source_language();
                    let language = if selected.is_empty() {
                        popup.get_detected_language()
                    } else {
                        selected
                    };
                    if language.is_empty() {
                        None
                    } else {
                        Some(Language(language.to_string()))
                    }
                } else {
                    Some(language_for_index(popup.get_target_index()))
                },
            });
        }
    });
    let weak = popup.as_weak();
    popup.on_feedback_dismiss_requested(move || {
        if let Some(popup) = weak.upgrade() {
            handler(AppEvent::PopupFeedbackCleared {
                session_id: popup_session_id(&popup),
            });
        }
    });
}

impl UiHandle {
    pub fn show_popup(
        &self,
        session_id: lexift_core::domain::translation::PopupSessionId,
        anchor: Option<Point>,
        work_area: Option<Rect>,
    ) {
        let session_id = session_id.value();
        let _ = slint::invoke_from_event_loop(move || {
            cancel_idle_memory_trim();
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.show(session_id, anchor, work_area);
                }
            });
        });
    }
    pub fn hide_popup(&self, session_id: lexift_core::domain::translation::PopupSessionId) {
        let session_id = session_id.value();
        let _ = slint::invoke_from_event_loop(move || {
            POPUP_REGISTRY.with(|registry| {
                if let Some(registry) = registry.borrow_mut().as_mut() {
                    registry.hide(session_id);
                }
            });
        });
    }
}
