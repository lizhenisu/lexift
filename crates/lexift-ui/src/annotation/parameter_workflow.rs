//! Annotation parameter workflow; scoped to its owning session.
use super::*;
pub(super) fn current_style(r: &Registry, tool: i32) -> Style {
    parameters::style(r.values(tool as usize), tool)
}

pub(super) fn chosen_font(r: &Registry, tool: usize) -> String {
    r.fonts
        .names()
        .get(r.values(tool).font.max(0) as usize)
        .cloned()
        .unwrap_or_else(|| r.preferred_font(tool))
}

pub(super) fn text_from_values(r: &Registry, content: String) -> TextAnnotation {
    parameters::text(r.values(8), chosen_font(r, 8), content)
}

pub(super) fn watermark_from_values(r: &Registry, template: String, content: String) -> Watermark {
    parameters::watermark(r.values(9), chosen_font(r, 9), template, content)
}

pub(super) fn adopt_watermark_style(r: &mut Registry) {
    let font_index = r
        .session
        .watermark
        .as_ref()
        .and_then(|watermark| {
            r.fonts
                .names()
                .iter()
                .position(|name| name == &watermark.font_family)
        })
        .unwrap_or(0) as i32;
    r.parameter_values
        .project_watermark(r.session.watermark.as_ref(), font_index);
    choice::close_if_context_changed(r);
    if let Some(panel) = r.toolbar_state.panel.get()
        && panel.get_tool() == 9
        && !panel.get_menu()
    {
        panel.set_values(r.values(9).clone());
    }
}

pub(super) fn toolbar_tools(r: &Registry) -> Vec<i32> {
    let mut selected = r.selected;
    if selected[0] != 2 && r.values(selected[0] as usize).shape == 1 {
        selected[0] = 1;
    }
    selected.to_vec()
}

pub(super) fn capture_creation(r: &mut Registry, tool: usize) -> AnnotationValues {
    if matches!(tool, 8 | 10) {
        r.ensure_fonts();
    }
    let mut values = r.values(tool).clone();
    values.text = Default::default();
    values.sequence_label = Default::default();
    if tool == 10 {
        values.start = r.session.next_sequence;
    }
    r.parameter_values.remember_creation(tool, &values);
    values
}

pub(super) fn deselect_object(r: &mut Registry) {
    r.session.selected = None;
    r.parameter_values.deselect();
    if let Some(panel) = r
        .toolbar_state
        .panel
        .get()
        .filter(|panel| !panel.get_menu())
    {
        panel.set_values(r.values(panel.get_tool() as usize).clone());
    }
}

pub(super) fn project_object_parameters(r: &mut Registry, index: usize) -> [u8; 3] {
    let object = &r.session.objects[index];
    let font_family = match &object.extra {
        Extra::Text(text) => Some(&text.font_family),
        Extra::Sequence(sequence) => Some(&sequence.caption.font_family),
        _ => None,
    };
    let font_index = font_family
        .and_then(|family| r.fonts.names().iter().position(|name| name == family))
        .unwrap_or(0) as i32;
    let preview_color = object.style.color;
    r.parameter_values.project(
        object,
        font_index,
        r.session.next_sequence,
        r.session.spotlight_opacity,
    );
    preview_color
}

pub(super) fn restore_parameter_projection(r: &mut Registry) {
    if let Some(index) = r.session.selected {
        project_object_parameters(r, index);
    } else {
        r.parameter_values.deselect();
    }
    choice::close_if_context_changed(r);
    if let Some(panel) = r
        .toolbar_state
        .panel
        .get()
        .filter(|panel| !panel.get_menu())
    {
        let tool = panel.get_tool() as usize;
        r.parameter_values.bind(r.parameter_object(tool));
        panel.set_values(r.values(tool).clone());
    }
}

/// Refill only when a canvas operation changes the panel's target. Merely
/// rebinding would leave stale object values available to the next user edit.
pub(super) fn refresh_parameter_target(r: &mut Registry) {
    choice::close_if_context_changed(r);
    let Some(panel) = r
        .toolbar_state
        .panel
        .get()
        .filter(|panel| !panel.get_menu())
    else {
        return;
    };
    let target = r.parameter_object(panel.get_tool() as usize);
    if r.parameter_values.bound_object() != target {
        restore_parameter_projection(r);
    }
}

pub(super) fn adopt_selected_style(r: &mut Registry, index: usize) -> bool {
    if !r
        .session
        .objects
        .get(index)
        .is_some_and(|object| r.session.can_edit(object))
    {
        return false;
    }
    let object = &r.session.objects[index];
    let tool = object.kind.tool();
    let group = if matches!(tool, 11 | 12) {
        5
    } else if tool == 10 {
        4
    } else if tool == 8 {
        3
    } else if tool >= 5 {
        2
    } else {
        usize::from(tool >= 3)
    };
    let preview_color = project_object_parameters(r, index);
    choice::close_if_context_changed(r);
    r.selected[group] = tool as i32;
    r.set_mode(InteractionMode::Tool(group));
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
    }
    if let Some(panel) = r.toolbar_state.panel.get()
        && !panel.get_menu()
        && panel.get_tool() == tool as i32
    {
        panel.set_values(r.values(tool).clone());
        let [red, green, blue] = preview_color;
        panel.set_custom_preview(slint::Color::from_rgb_u8(red, green, blue));
    }
    !r.toolbar_state
        .panel
        .get()
        .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == tool as i32)
}

pub(super) fn magnifier_connector(index: i32) -> MagnifierConnector {
    match index {
        1 => MagnifierConnector::Dot,
        2 => MagnifierConnector::Frame,
        3 => MagnifierConnector::None,
        _ => MagnifierConnector::Plain,
    }
}

pub(super) fn active_kind(r: &Registry) -> Option<Kind> {
    if r.mode.is_mouse() {
        return None;
    }
    let tool = r.selected[r.mode.selected_group() as usize] as usize;
    let capture = r
        .lifecycle
        .annotation_screen_capture
        .as_ref()
        .is_some_and(|p| p.is_available());
    match tools::drawing(tool, r.values(tool).shape, capture) {
        tools::Drawing::Object(kind) => Some(kind),
        _ => None,
    }
}

pub(super) fn sync_sequence_counter(r: &mut Registry) {
    r.parameter_values.sequence_counter(r.session.next_sequence);
    if let Some(panel) = r.toolbar_state.panel.get()
        && panel.get_tool() == 10
    {
        let mut v = panel.get_values();
        v.start = r.values(10).start;
        v.sequence_label = r.values(10).sequence_label.clone();
        panel.set_values(v);
    }
}
pub(super) fn update_sequence_style(
    object: &mut Object,
    values: &AnnotationValues,
    fonts: &[String],
    style: Style,
) {
    parameters::sequence_style(object, values, fonts, style);
    crate::annotation_sequence::layout(object);
}

pub(super) fn sequence_from_values(r: &Registry, at: (f32, f32), scale: f32) -> Object {
    let v = r.parameter_values.defaults(10);
    let style = parameters::style(v, 10);
    let mut caption = text_from_values(r, String::new());
    caption.outline = None;
    caption.background = None;
    caption.bold = false;
    caption.italic = false;
    let b = Bounds::from_corners(at, at);
    let mut object = Object {
        erased: Vec::new(),
        id: Default::default(),
        kind: Kind::Sequence,
        bounds: b,
        style,
        points: Vec::new(),
        extra: Extra::Sequence(SequenceAnnotation {
            value: r.session.next_sequence.max(1),
            format: SequenceFormat::Decimal,
            shape: SequenceShape::Square,
            size: 16.,
            scale,
            caption,
            caption_bounds: b,
            caption_color: style.color,
            caption_custom_color: false,
            caption_ratio: 2,
            guide: true,
            endpoint: Endpoint::FilledArrow,
        }),
    };
    update_sequence_style(&mut object, v, r.fonts.names(), style);
    object
}

/// The press snapshot includes selection and numbering; no undo entry exists until release.
pub(super) fn begin_sequence_placement(r: &mut Registry, point: (f32, f32), scale: f32) {
    if !r.ensure_fonts() {
        return;
    }
    capture_creation(r, 10);
    let object = sequence_from_values(r, point, scale);
    r.begin_document_gesture();
    let Some(placement) = gestures::SequencePlacement::insert(&mut r.session, point, object) else {
        return;
    };
    deselect_object(r);
    r.gesture = Some(Active::Sequence(placement));
    sync_sequence_counter(r);
}

pub(super) fn update_sequence_placement(r: &mut Registry, point: (f32, f32)) {
    if let Some(placement) = r.gesture.as_ref().and_then(Active::sequence) {
        placement.update(&mut r.session, point);
    }
}

pub(super) fn cancel_sequence_placement(r: &mut Registry) -> bool {
    matches!(r.gesture(), Some(Gesture::PlaceSequence)) && cancel_active_gesture(r)
}

/// Retire an object operation before another document command can change its target.
pub(super) fn cancel_object_gesture(r: &mut Registry) -> bool {
    matches!(
        r.gesture(),
        Some(
            Gesture::Erase
                | Gesture::PlaceSequence
                | Gesture::Move
                | Gesture::Resize { .. }
                | Gesture::TextHandle { .. }
        )
    ) && cancel_active_gesture(r)
}

/// Roll back the active input operation, leaving the editor and committed history alive.
/// Pending capture validation is owned by the mosaic coordinator, not this gesture.
pub(super) fn cancel_active_gesture(r: &mut Registry) -> bool {
    let active = r.gesture.take();
    let gesture = active.as_ref().map(Active::route);
    if active.is_none() && r.pending_drawing.is_none() {
        return false;
    }
    if let Some(original) = active.and_then(|active| active.cancel(&mut r.session)) {
        apply_editing_object(r, original);
    }
    if let Some(edit) = r.editor.edit_mut() {
        edit.selection_anchor = None;
    }
    r.replay_deferred_parameters(gesture);

    r.pending_drawing.cancel();

    r.pending_edit_panel = false;
    restore_parameter_projection(r);
    sync_sequence_counter(r);
    cursor::refresh(r);
    true
}

/// Apply one validated request before another command can retire its editor.
/// Property observers may echo the resulting values, but do not own application.
pub(super) fn apply_geometry_values(
    r: &mut Registry,
    target: parameter_edit::Target,
    values: AnnotationValues,
) -> Option<u64> {
    let tool = target.tool;
    if !r
        .toolbar_state
        .panel
        .get()
        .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == tool)
    {
        return None;
    }
    if r.parameter_target(tool) != target {
        return None;
    }
    if r.gesture().is_some_and(Gesture::blocks_parameter_edits) {
        r.deferred_parameters.accept(target, values);
        return None;
    }
    apply_parameter_values(r, target, values)
}

/// Execute an accepted parameter request using its explicit object/default target.
/// Validation and geometry deferral belong to the input/replay boundary above.
pub(super) fn apply_parameter_values(
    r: &mut Registry,
    target: parameter_edit::Target,
    values: AnnotationValues,
) -> Option<u64> {
    let generation = r.generation;
    let tool = target.tool;
    if !r.parameter_entry_active(tool)
        || target
            .object
            .is_some_and(|id| r.parameter_object(tool as usize) != Some(id))
    {
        return None;
    }
    let source_object = target.object;
    if tool == 13 && r.values(13).shape != values.shape {
        r.session.finish_rectangle_erase();
    }
    let previous_values = if tool == 10 {
        r.parameter_values.values(10, source_object)
    } else {
        r.values(tool as usize)
    }
    .clone();
    if previous_values == values {
        return None;
    }
    let token = r.parameter_edit.touch(&mut r.session, target);
    if tool == 8 {
        r.parameter_values.remember_creation(tool as usize, &values);
    }
    let edited = values;
    if tool == 10 {
        r.parameter_values
            .replace(10, source_object, edited.clone());
    } else {
        r.replace_values(tool as usize, edited.clone());
    }
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_tools(ModelRc::new(VecModel::from(toolbar_tools(r))));
    }
    let style = parameters::style(&edited, tool);
    if let Some(panel) = r.toolbar_state.panel.get() {
        panel.set_custom_preview(slint::Color::from_rgb_u8(
            style.color[0],
            style.color[1],
            style.color[2],
        ));
    }
    let kind = active_kind(r);
    let text_scale = r
        .session
        .selected
        .and_then(|index| r.session.objects.get(index))
        .and_then(|object| {
            r.canvas_state.canvases.iter().find(|layer| {
                let center = object.bounds.center();
                center.0 >= layer.bounds.left as f32
                    && center.0 < layer.bounds.right as f32
                    && center.1 >= layer.bounds.top as f32
                    && center.1 < layer.bounds.bottom as f32
            })
        })
        .map_or(1., |layer| layer.window.window().scale_factor().max(0.1));
    let text_style = (tool == 8).then(|| text_from_values(r, String::new()));
    let text_scale = r
        .session
        .selected
        .and_then(|index| r.session.objects.get(index))
        .filter(|object| object.kind == Kind::Text)
        .map_or(text_scale, crate::annotation_text::object_scale);
    let displays: Vec<_> = if r.editor.edit().is_some() && (tool == 8 || tool == 10) {
        r.canvas_state
            .canvases
            .iter()
            .map(|layer| layer.bounds)
            .collect()
    } else {
        Vec::new()
    };
    if let Some(next) = &text_style {
        r.editor
            .apply_text_style(next, style.color, &previous_values, &edited, &displays);
    }
    let watermark = if tool == 9 {
        r.session.watermark.as_ref().map(|current| {
            watermark_from_values(r, current.template.clone(), current.content.clone())
        })
    } else {
        None
    };
    if tool == 2 {
        r.session.set_spotlight_opacity(r.values(2).strength / 100.);
    }
    if tool == 10 {
        if edited.start != previous_values.start {
            r.session.next_sequence = edited.start.max(1);
        }
        let fonts = r.fonts.names();
        r.session.update_selected(|object| {
            if Some(object.id) != source_object {
                return;
            }
            if object.kind == Kind::Sequence {
                update_sequence_style(object, &edited, fonts, style);
            }
        });
        if let Some(index) = r.editor.edit().and_then(|edit| edit.index(&r.session))
            && let Some(object) = r.session.objects.get(index)
        {
            r.editor.apply_caption_style(object, &displays);
        }
    } else if let Some(kind) = kind {
        r.session.update_selected(|object| {
            if Some(object.id) != source_object {
                return;
            }
            if object.kind.tool() == kind.tool() {
                if object.kind != kind && kind.tool() == 4 {
                    if kind == Kind::HighlightLine {
                        object.points = vec![
                            (object.bounds.left, object.bounds.top),
                            (object.bounds.right, object.bounds.bottom),
                        ];
                    } else {
                        object.points.clear();
                    }
                }
                if !matches!(kind.tool(), 11 | 12) {
                    object.kind = kind;
                }
                object.style = style;
                match kind {
                    Kind::StickerArea | Kind::StickerBrush => {
                        if let Extra::Sticker(value) = &mut object.extra {
                            value.texture = if edited.mode == 1 {
                                lexift_core::domain::annotation::StickerTexture::Frost
                            } else {
                                lexift_core::domain::annotation::StickerTexture::Glass
                            };
                            value.opacity = edited.strength.round().clamp(0., 100.) as u8;
                            if edited.zoom != previous_values.zoom {
                                value.set_displayed_size(edited.zoom as f32);
                            }
                            if edited.size != previous_values.size
                                && object.kind == Kind::StickerBrush
                            {
                                value.diameter =
                                    edited.size.clamp(1, 100) as f32 * value.creation_scale;
                            }
                        }
                        if object.kind == Kind::StickerBrush {
                            object.refresh_bounds();
                        }
                    }
                    Kind::MosaicArea | Kind::MosaicBrush => {
                        if let Extra::Mosaic(value) = &mut object.extra {
                            value.effect = if edited.mode == 1 {
                                lexift_core::domain::annotation::MosaicEffect::Blur
                            } else {
                                lexift_core::domain::annotation::MosaicEffect::Pixelate
                            };
                            value.strength = edited.strength.round().clamp(0., 100.) as u8;
                            if edited.size != previous_values.size
                                && object.kind == Kind::MosaicBrush
                            {
                                value.diameter =
                                    edited.size.clamp(1, 100) as f32 * value.creation_scale;
                            }
                        }
                        if object.kind == Kind::MosaicBrush {
                            object.refresh_bounds();
                        }
                    }
                    Kind::Arrow => {
                        if let Extra::Arrow {
                            head, start, end, ..
                        } = &mut object.extra
                        {
                            *head = edited.head;
                            *start = endpoint(edited.line_start);
                            *end = endpoint(edited.line_end);
                        }
                    }
                    Kind::Polyline => {
                        object.extra = Extra::Polyline {
                            curved: edited.shape == 1,
                            head: edited.head,
                            start: endpoint(edited.line_start),
                            end: endpoint(edited.line_end),
                        };
                    }
                    Kind::Magnifier => {
                        object.set_magnifier_zoom_centered(edited.zoom as f32 / 100.);
                        if let Extra::Magnifier {
                            ellipse,
                            connector,
                            erase_annotations,
                            antialias,
                            shadow,
                            ..
                        } = &mut object.extra
                        {
                            *ellipse = edited.shape == 1;
                            *connector = magnifier_connector(edited.connector_style);
                            *erase_annotations = !edited.include_annotations;
                            *antialias = edited.antialias;
                            *shadow = edited.shadow;
                        }
                    }
                    Kind::Text => {
                        if let (Extra::Text(existing), Some(updated)) =
                            (&mut object.extra, &text_style)
                        {
                            *existing = parameters::updated_text_style(
                                existing,
                                updated,
                                &previous_values,
                                &edited,
                            );
                            object.bounds = crate::annotation_text::text_bounds(
                                (object.bounds.left, object.bounds.top),
                                existing,
                                text_scale,
                            );
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    if tool == 9
        && let Some(watermark) = watermark
    {
        r.session.set_watermark(Some(watermark));
    }
    if let Some(main) = r.toolbar_state.main.get() {
        main.set_can_undo(r.session.can_undo());
    }
    if tool == 10 {
        sync_sequence_counter(r);
    }
    position_panel(r);
    r.parameter_edit.finish_after(token, move || {
        REGISTRY.with(|s| {
            if let Some(r) = s.borrow_mut().as_mut().filter(|r| {
                r.generation == generation
                    && r.parameter_edit
                        .ready_to_finish(token, choice::color_dragging(r))
            }) {
                r.finish_style_edit();
            }
        });
    });
    Some(r.generation)
}

pub(super) fn update_geometry_patch(
    generation: u64,
    target: parameter_edit::Target,
    patch: parameters::Patch,
) {
    let edit = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        let mut values = r.pending_parameter_values(target).clone();
        patch.apply(&mut values);
        apply_geometry_values(r, target, values)
    });
    if let Some(generation) = edit {
        schedule_render(generation);
    }
}

#[cfg(test)]
pub(super) fn update_geometry_values(
    generation: u64,
    target: parameter_edit::Target,
    values: AnnotationValues,
) {
    let edit = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| r.generation == generation)?;
        apply_geometry_values(r, target, values)
    });
    if let Some(generation) = edit {
        schedule_render(generation);
    }
}

impl Registry {
    pub(super) fn scroll_numeric(
        &mut self,
        target: parameter_edit::Target,
        parameter: numeric::Parameter,
        delta: f32,
    ) {
        let values = self.pending_parameter_values(target);
        if self.parameter_target(target.tool) != target || !parameter.enabled(target.tool, values) {
            return;
        }
        if let Some(value) = parameter.step(parameter.read(values), target.tool, delta) {
            self.apply_menu_edit(target, parameters::Edit::Numeric(parameter, value));
            choice::sync_numeric(self);
        }
    }

    pub(super) fn parameter_entry_active(&self, tool: i32) -> bool {
        !self.mode.is_mouse()
            && parameters::compatible_tools(
                self.selected[self.mode.selected_group() as usize] as usize,
                tool as usize,
            )
    }

    pub(super) fn parameter_object(&self, tool: usize) -> Option<ObjectId> {
        self.session
            .selected
            .and_then(|index| self.session.objects.get(index))
            .filter(|object| {
                self.session.can_edit(object)
                    && parameters::compatible_tools(object.kind.tool(), tool)
            })
            .map(|object| object.id)
    }

    pub(super) fn values(&self, tool: usize) -> &AnnotationValues {
        if tool == 8
            && let Some(values) = self.editor.draft_values()
        {
            return values;
        }
        self.parameter_values
            .values(tool, self.parameter_object(tool))
    }

    pub(super) fn parameter_target(&self, tool: i32) -> parameter_edit::Target {
        parameter_edit::Target {
            panel: self.toolbar_state.panel.revision(),
            tool,
            object: self.parameter_object(tool as usize),
            editor: self.editor.edit().map(|edit| edit.id),
            watermark: if tool == 9 {
                self.parameter_values.watermark_revision()
            } else {
                0
            },
            projection: if tool != 9 {
                self.parameter_values.projection_revision()
            } else {
                0
            },
        }
    }

    #[cfg(test)]
    pub(super) fn edit_values(&mut self, tool: usize) -> &mut AnnotationValues {
        if tool == 8 && self.editor.draft_values().is_some() {
            return self.editor.draft_values_mut().unwrap();
        }
        let object = self.parameter_object(tool);
        self.parameter_values.edit(tool, object)
    }

    pub(super) fn replace_values(&mut self, tool: usize, values: AnnotationValues) {
        if tool == 8
            && let Some(draft) = self.editor.draft_values_mut()
        {
            *draft = values;
            return;
        }
        let object = self.parameter_object(tool);
        self.parameter_values.replace(tool, object, values);
    }

    pub(super) fn pending_parameter_values(
        &self,
        target: parameter_edit::Target,
    ) -> &AnnotationValues {
        self.deferred_parameters
            .values(target)
            .unwrap_or_else(|| self.values(target.tool as usize))
    }

    /// Apply a decoded popup result to current parameters. The popup does not
    /// own document transactions, font preferences or the editor's draft.
    pub(super) fn apply_menu_edit(
        &mut self,
        target: parameter_edit::Target,
        edit: parameters::Edit,
    ) {
        if matches!(
            edit,
            parameters::Edit::Choice(parameters::ChoiceTarget::Font, _)
        ) && !self.fonts.is_loaded()
        {
            return;
        }
        if self.parameter_target(target.tool) != target
            || !self
                .toolbar_state
                .panel
                .get()
                .is_some_and(|panel| !panel.get_menu() && panel.get_tool() == target.tool)
        {
            return;
        }
        let tool = target.tool as usize;
        let mut values = self.pending_parameter_values(target).clone();
        edit.apply(&mut values);
        if let parameters::Edit::Choice(parameters::ChoiceTarget::Font, index) = &edit
            && let Some(font_tool) = fonts::Tool::from_index(tool)
        {
            self.fonts.choose(font_tool, *index);
            self.parameter_values.defaults_mut(tool).font = *index;
        }
        let changed = apply_geometry_values(self, target, values);
        if let Some(panel) = self.toolbar_state.panel.get() {
            panel.set_values(self.pending_parameter_values(target).clone());
        }
        if changed.is_some() {
            // Document and draft changes are applied; the owned timer renders after this borrow.
            self.queue_render();
        }
    }

    pub(super) fn ensure_fonts(&mut self) -> bool {
        font_workflow::ensure(self, false)
    }

    pub(super) fn preferred_font(&self, tool: usize) -> String {
        fonts::Tool::from_index(tool)
            .map(|tool| self.fonts.preferred(tool, crate::i18n::language()))
            .unwrap_or_default()
    }

    /// Compute UI protection from the active document; the directory never owns objects.
    pub(super) fn reset_font_defaults(&mut self) {
        for tool in 8..=10 {
            let editing = self.editor.edit().is_some_and(|e| {
                if tool == 8 {
                    !editing_sequence(self)
                        && e.index(&self.session).is_none_or(|i| {
                            self.session
                                .objects
                                .get(i)
                                .is_some_and(|o| o.kind == Kind::Text)
                        })
                } else {
                    tool == 10 && editing_sequence(self)
                }
            });
            if editing {
                continue;
            }
            let name = self.preferred_font(tool);
            if let Some(index) = self
                .fonts
                .names()
                .iter()
                .position(|f| f.eq_ignore_ascii_case(&name))
            {
                self.parameter_values.defaults_mut(tool).font = index as i32;
            }
        }
    }

    /// Replay accepted toolbar input before geometry projection changes its identity.
    pub(super) fn flush_deferred_parameters(&mut self) {
        self.replay_deferred_parameters(None);
    }

    /// Cancelling placement restores selection before defaults are replayed. Keep
    /// the accepted default target without rebinding it to the restored object.
    pub(super) fn replay_deferred_parameters(&mut self, cancelled: Option<Gesture>) {
        if self.gesture().is_some_and(Gesture::blocks_parameter_edits) {
            return;
        }
        for (target, values) in self.deferred_parameters.take() {
            if !self.parameter_entry_active(target.tool) {
                continue;
            }
            let context = parameters::ReplayContext {
                current: self.parameter_target(target.tool),
                panel_tool: self
                    .toolbar_state
                    .panel
                    .get()
                    .filter(|panel| !panel.get_menu())
                    .map(|panel| panel.get_tool()),
                bound_object: self.parameter_values.bound_object(),
                cancelled_sequence: matches!(cancelled, Some(Gesture::PlaceSequence)),
            };
            match context.resolve(target) {
                parameters::Replay::Current => {
                    apply_geometry_values(self, target, values);
                }
                parameters::Replay::SequenceDefaults => {
                    apply_parameter_values(self, target, values);
                }
                parameters::Replay::Discard => {}
            }
        }
    }

    pub(super) fn finish_style_edit(&mut self) {
        if self.parameter_edit.finish(&mut self.session)
            && let Some(main) = self.toolbar_state.main.get()
        {
            main.set_can_undo(self.session.can_undo());
            if let Some(panel) = self.toolbar_state.panel.get() {
                panel.set_can_clear(self.session.has_content());
            }
        }
    }
}
