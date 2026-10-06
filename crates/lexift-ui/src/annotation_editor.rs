//! Text edit identity, draft layout and cancellable instance work; no session ownership.
use crate::{AnnotationTextInput, AnnotationValues};
use lexift_core::domain::annotation::{
    Bounds, Extra, Kind, Object, ObjectId, Session, TextAnnotation,
};
use lexift_core::domain::geometry::Rect;
use slint::ComponentHandle;
use std::{cell::Cell, rc::Rc, time::Duration};

struct Instance {
    window: AnnotationTextInput,
    edit: TextEdit,
    values: Option<AnnotationValues>,
}

impl Instance {
    fn retire(self) -> (TextEdit, Option<AnnotationValues>) {
        self.edit.stop();
        let _ = self.window.hide();
        (self.edit, self.values)
    }
}

pub(super) struct ClosedEdit {
    pub edit: TextEdit,
    pub content: String,
    pub values: Option<AnnotationValues>,
}

/// A retired instance settles its content without borrowing windows or mutating
/// the document. The coordinator applies this result through Core's transaction.
pub(super) enum EditChange {
    Caption {
        target: ObjectId,
        text: TextAnnotation,
    },
    Text {
        target: ObjectId,
        text: TextAnnotation,
        bounds: Bounds,
    },
    New(Box<Object>),
}

impl ClosedEdit {
    /// Missing committed targets must never turn into new drafts. Empty captions
    /// may clear an existing caption, while empty text retains the prior behavior.
    pub fn resolve(self, commit: bool, target_kind: Option<Kind>) -> Option<EditChange> {
        let Self {
            edit,
            content,
            values,
        } = self;
        if !commit || (edit.object.is_some() && target_kind.is_none()) {
            return None;
        }
        let mut text = edit.original;
        text.content = content;
        if target_kind == Some(Kind::Sequence) {
            return Some(EditChange::Caption {
                target: edit.object.expect("sequence edit has a target"),
                text,
            });
        }
        if text.content.trim().is_empty() {
            return None;
        }
        let bounds = if edit.laid_out_content == text.content {
            edit.bounds
        } else {
            crate::annotation_text::text_bounds(edit.at, &text, edit.scale)
        };
        Some(match edit.object {
            Some(target) => EditChange::Text {
                target,
                text,
                bounds,
            },
            None => EditChange::New(Box::new(Object {
                erased: Vec::new(),
                id: Default::default(),
                bounds,
                kind: Kind::Text,
                style: super::parameters::style(
                    values.as_ref().expect("new text owns draft parameters"),
                    8,
                ),
                points: Vec::new(),
                extra: Extra::Text(text),
            })),
        })
    }
}

/// A live editor always has both its input component and its instance state.
#[derive(Default)]
pub(super) struct State {
    active: Option<Instance>,
}

impl State {
    pub fn open(
        &mut self,
        window: AnnotationTextInput,
        edit: TextEdit,
        values: Option<AnnotationValues>,
    ) {
        assert_eq!(
            edit.object.is_none(),
            values.is_some(),
            "only new text owns draft parameters"
        );
        self.clear();
        self.active = Some(Instance {
            window,
            edit,
            values,
        });
    }

    /// A new text object has no Core identity yet. Its parameters belong to the
    /// editor, so cancelling it cannot change the subsequent drawing defaults.
    pub fn draft_values(&self) -> Option<&AnnotationValues> {
        self.active
            .as_ref()
            .and_then(|instance| instance.values.as_ref())
    }
    pub fn draft_values_mut(&mut self) -> Option<&mut AnnotationValues> {
        self.active
            .as_mut()
            .and_then(|instance| instance.values.as_mut())
    }
    pub fn sync_draft_dimensions(&mut self) -> bool {
        let Some(instance) = self
            .active
            .as_mut()
            .filter(|instance| instance.edit.object.is_none())
        else {
            return false;
        };
        super::parameters::text_dimensions_to_values(
            instance.values.as_mut().unwrap(),
            &instance.edit.original,
        );
        true
    }

    pub fn window(&self) -> Option<&AnnotationTextInput> {
        self.active.as_ref().map(|instance| &instance.window)
    }

    pub fn edit(&self) -> Option<&TextEdit> {
        self.active.as_ref().map(|instance| &instance.edit)
    }

    pub fn edit_mut(&mut self) -> Option<&mut TextEdit> {
        self.active.as_mut().map(|instance| &mut instance.edit)
    }

    pub fn parts_mut(&mut self) -> Option<(&AnnotationTextInput, &mut TextEdit)> {
        self.active
            .as_mut()
            .map(|instance| (&instance.window, &mut instance.edit))
    }

    /// Update the existing instance without resetting value, composition or selection.
    pub fn apply_text_style(
        &mut self,
        next: &TextAnnotation,
        color: [u8; 3],
        before: &AnnotationValues,
        after: &AnnotationValues,
        displays: &[Rect],
    ) -> bool {
        let Some(instance) = self.active.as_mut() else {
            return false;
        };
        instance.edit.original =
            super::parameters::updated_text_style(&instance.edit.original, next, before, after);
        configure(&instance.window, &instance.edit.original, color);
        let mut text = instance.edit.original.clone();
        text.content = preview(&instance.window).0;
        let bounds =
            crate::annotation_text::text_bounds(instance.edit.at, &text, instance.edit.scale);
        instance.edit.bounds = bounds;
        instance.edit.laid_out_content = text.content.clone();
        place(
            &instance.window,
            bounds,
            instance.edit.scale,
            &text,
            displays,
        );
        true
    }

    /// A caption follows its badge geometry while retaining the live input value.
    pub fn apply_caption_style(&mut self, object: &Object, displays: &[Rect]) -> bool {
        let Some(instance) = self
            .active
            .as_mut()
            .filter(|instance| instance.edit.object == Some(object.id))
        else {
            return false;
        };
        let Extra::Sequence(sequence) = &object.extra else {
            return false;
        };
        instance.edit.original = sequence.caption.clone();
        configure(&instance.window, &sequence.caption, sequence.caption_color);
        let mut visible = object.clone();
        if let Extra::Sequence(sequence) = &mut visible.extra {
            sequence.caption.content = preview(&instance.window).0;
        }
        crate::annotation_sequence::layout(&mut visible);
        let caption = visible.sequence_caption().unwrap();
        let Extra::Text(text) = &caption.extra else {
            unreachable!();
        };
        instance.edit.bounds = caption.bounds;
        instance.edit.at = (caption.bounds.left, caption.bounds.top);
        instance.edit.laid_out_content = text.content.clone();
        place(
            &instance.window,
            caption.bounds,
            instance.edit.scale,
            text,
            displays,
        );
        true
    }

    /// Invalidate access before stopping work and destroying the input component.
    pub fn take(&mut self) -> Option<ClosedEdit> {
        let instance = self.active.take()?;
        let content = instance.window.get_value().to_string();
        let (edit, values) = instance.retire();
        Some(ClosedEdit {
            edit,
            content,
            values,
        })
    }

    pub fn retire_input(&self) {
        if let Some(instance) = &self.active {
            instance.edit.stop();
        }
    }

    pub fn clear(&mut self) {
        if let Some(instance) = self.active.take() {
            drop(instance.retire());
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.clear();
    }
}

thread_local! { static NEXT_INSTANCE: Cell<u64> = const { Cell::new(0) }; }

pub(super) fn next_instance() -> u64 {
    NEXT_INSTANCE.with(|next| {
        let id = next
            .get()
            .checked_add(1)
            .expect("text editor identity exhausted");
        next.set(id);
        id
    })
}

/// Input callbacks can fire while the coordinator is borrowed. Own deferred work
/// independently of that borrow, and invalidate it even if a caller retains the window.
struct InputWork {
    active: Cell<bool>,
    pointer_gate: super::window::PointerGate,
    finishing: Cell<bool>,
    update: slint::Timer,
    finish: slint::Timer,
}

impl Default for InputWork {
    fn default() -> Self {
        Self {
            active: Cell::new(true),
            pointer_gate: super::window::PointerGate::default(),
            finishing: Cell::new(false),
            update: slint::Timer::default(),
            finish: slint::Timer::default(),
        }
    }
}

impl InputWork {
    fn update(&self, action: impl FnOnce() + 'static) {
        if self.active.get() && !self.finishing.get() {
            Self::schedule(&self.update, action);
        }
    }

    fn finish(&self, action: impl FnOnce() + 'static) {
        if self.active.get() && !self.finishing.replace(true) {
            // The finish path reads the latest value and measures unlaid-out text.
            self.update.stop();
            Self::schedule(&self.finish, action);
        }
    }

    fn schedule(timer: &slint::Timer, action: impl FnOnce() + 'static) {
        let mut action = Some(action);
        timer.start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
            if let Some(action) = action.take() {
                action();
            }
        });
    }

    fn retire(&self) {
        self.active.set(false);
        self.update.stop();
        self.finish.stop();
    }
}

/// Callbacks retain the queue, not its owner. Dropping an uninstalled edit or
/// partially consuming its layout must still disable externally retained input.
struct InputLifetime(Rc<InputWork>);
impl Default for InputLifetime {
    fn default() -> Self {
        Self(Rc::new(InputWork::default()))
    }
}
impl Drop for InputLifetime {
    fn drop(&mut self) {
        self.0.retire();
    }
}

pub(super) struct TextEdit {
    pub id: u64,
    pub selection_anchor: Option<usize>,
    pub object: Option<ObjectId>,
    pub at: (f32, f32),
    pub scale: f32,
    pub original: TextAnnotation,
    pub bounds: Bounds,
    pub laid_out_content: String,
    pub caret: CaretBlink,
    input_work: InputLifetime,
    caret_timer: slint::Timer,
    preparation: slint::Timer,
}

impl TextEdit {
    pub fn new(
        id: u64,
        object: Option<ObjectId>,
        at: (f32, f32),
        scale: f32,
        original: TextAnnotation,
        bounds: Bounds,
        laid_out_content: String,
    ) -> Self {
        Self {
            id,
            object,
            at,
            scale,
            original,
            bounds,
            laid_out_content,
            selection_anchor: None,
            caret: CaretBlink { visible: true },
            input_work: InputLifetime::default(),
            caret_timer: slint::Timer::default(),
            preparation: slint::Timer::default(),
        }
    }

    /// Coalesce layout notifications and keep the first completion request.
    /// Callers still validate session and edit identity before applying either action.
    pub fn bind_input(
        &self,
        window: &AnnotationTextInput,
        update: impl Fn() + 'static,
        finish: impl Fn(bool) + 'static,
    ) {
        let update = Rc::new(update);
        let finish = Rc::new(finish);
        let work = self.input_work.0.clone();
        let action = update.clone();
        window.on_content_changed(move |_| {
            let action = action.clone();
            work.update(move || action());
        });
        let work = self.input_work.0.clone();
        window.on_caret_changed(move || {
            let action = update.clone();
            work.update(move || action());
        });
        let work = self.input_work.0.clone();
        let action = finish.clone();
        window.on_done(move |_| {
            let action = action.clone();
            work.finish(move || action(true));
        });
        let work = self.input_work.0.clone();
        let action = finish.clone();
        window.on_cancel(move || {
            let action = action.clone();
            work.finish(move || action(false));
        });
        let work = self.input_work.0.clone();
        window.window().on_close_requested(move || {
            let action = finish.clone();
            work.finish(move || action(true));
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    /// Escape can dismiss a child popup without retiring or disabling this input instance.
    pub fn bind_escape(&self, window: &AnnotationTextInput, escape: impl Fn() + 'static) {
        let work = self.input_work.0.clone();
        let escape = Rc::new(escape);
        window.on_escape_requested(move || {
            if work.active.get() && !work.finishing.get() {
                let action = escape.clone();
                InputWork::schedule(&work.finish, move || action());
            }
        });
    }

    /// Native teardown can lag editor retirement if another handle retains the
    /// component. Guard dispatch with instance lifetime, not weak liveness alone.
    pub fn pointer_sink(&self, window: &AnnotationTextInput) -> crate::bridge::PopupPointerSink {
        let weak = window.as_weak();
        let work = self.input_work.0.clone();
        Rc::new(move |input| {
            if work.active.get()
                && work.pointer_gate.allows()
                && let Some(window) = weak.upgrade()
            {
                crate::bridge::dispatch_pointer_input(window.window(), input);
            }
        })
    }

    pub fn pause_pointer(&self) -> super::window::PointerPause {
        self.input_work.0.pointer_gate.pause()
    }

    /// An absent target is a new draft; a missing committed target is not a new draft.
    pub fn index(&self, session: &Session) -> Option<usize> {
        self.object.and_then(|id| session.object_index(id))
    }

    pub fn reset_caret(&mut self, tick: impl FnMut() + 'static) {
        self.caret.reset();
        self.caret_timer
            .start(slint::TimerMode::Repeated, Duration::from_millis(500), tick);
    }

    #[cfg(test)]
    pub fn caret_running(&self) -> bool {
        self.caret_timer.running()
    }

    pub fn prepare_after(&self, delay: Duration, prepare: impl FnMut() + 'static) {
        self.preparation
            .start(slint::TimerMode::SingleShot, delay, prepare);
    }

    pub fn stop(&self) {
        self.input_work.0.retire();
        self.preparation.stop();
        self.caret_timer.stop();
    }
}

/// Canvas feedback has its own clock because the native TextInput is transparent.
pub(super) struct CaretBlink {
    pub visible: bool,
}
impl CaretBlink {
    pub fn reset(&mut self) {
        self.visible = true;
    }
    pub fn tick(&mut self, held: bool) -> bool {
        let previous = self.visible;
        self.visible = held || !self.visible;
        self.visible != previous
    }
}

/// Match the native input typography to the shared canvas layout. Value and
/// selection are intentionally not written during a parameter update.
pub(super) fn configure(window: &AnnotationTextInput, text: &TextAnnotation, color: [u8; 3]) {
    window.set_text_size(text.font_size);
    window.set_font_name(text.font_family.clone().into());
    window.set_bold(text.bold);
    window.set_italic(text.italic);
    window.set_text_padding(crate::annotation_text::text_padding(text, 1.));
    window.set_ink(slint::Color::from_rgb_u8(color[0], color[1], color[2]));
}

/// Matches TextInput's insertion semantics without adding composition to the model.
pub(super) fn preview(editor: &AnnotationTextInput) -> (String, std::ops::Range<usize>) {
    crate::annotation_text::composition_preview(
        &editor.get_value(),
        &editor.get_preedit(),
        editor.get_cursor_offset().max(0) as usize,
    )
}

/// Bounds the native surface to the desktop, while TextInput keeps full logical layout.
pub(super) fn place(
    editor: &AnnotationTextInput,
    bounds: Bounds,
    scale: f32,
    text: &TextAnnotation,
    displays: &[Rect],
) {
    let viewport = viewport(bounds, displays);
    editor.window().set_position(slint::PhysicalPosition::new(
        viewport.left.floor() as i32,
        viewport.top.floor() as i32,
    ));
    let native_scale = editor.window().scale_factor().max(0.1);
    editor.set_text_size(text.font_size * scale / native_scale);
    editor.set_text_padding(crate::annotation_text::text_padding(text, scale) / native_scale);
    editor.set_content_x((bounds.left - viewport.left.floor()) / native_scale);
    editor.set_content_y((bounds.top - viewport.top.floor()) / native_scale);
    editor.set_layout_width(bounds.width() / native_scale);
    editor.set_layout_height(bounds.height() / native_scale);
    editor.window().set_size(slint::PhysicalSize::new(
        (viewport.right.ceil() - viewport.left.floor()).max(1.) as u32,
        (viewport.bottom.ceil() - viewport.top.floor()).max(1.) as u32,
    ));
}

pub(super) fn viewport(bounds: Bounds, displays: &[Rect]) -> Bounds {
    let Some(first) = displays.first() else {
        return bounds;
    };
    let desktop = displays.iter().fold(*first, |mut area, screen| {
        area.left = area.left.min(screen.left);
        area.top = area.top.min(screen.top);
        area.right = area.right.max(screen.right);
        area.bottom = area.bottom.max(screen.bottom);
        area
    });
    let left = bounds
        .left
        .max(desktop.left as f32)
        .min(desktop.right as f32 - 1.);
    let top = bounds
        .top
        .max(desktop.top as f32)
        .min(desktop.bottom as f32 - 1.);
    Bounds {
        left,
        top,
        right: bounds.right.min(desktop.right as f32).max(left + 1.),
        bottom: bounds.bottom.min(desktop.bottom as f32).max(top + 1.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    fn make_edit() -> TextEdit {
        let text = TextAnnotation {
            content: String::new(),
            font_family: "Arial".into(),
            font_size: 22.,
            bold: false,
            italic: false,
            outline: None,
            background: None,
            rotation: 0.,
            linked_arrow: None,
            layout_scale: 1.,
            linked_arrow_control: None,
        };
        TextEdit::new(
            next_instance(),
            None,
            (0., 0.),
            1.,
            text,
            Bounds::from_corners((0., 0.), (30., 30.)),
            String::new(),
        )
    }

    fn closed_edit(target: Option<ObjectId>, content: &str) -> ClosedEdit {
        let mut edit = make_edit();
        edit.object = target;
        edit.laid_out_content = content.into();
        ClosedEdit {
            edit,
            content: content.into(),
            values: target
                .is_none()
                .then(|| super::super::parameters::defaults(8)),
        }
    }

    #[test]
    fn settlement_distinguishes_cancel_missing_target_and_empty_caption() {
        let target = ObjectId::default();
        assert!(closed_edit(None, "new").resolve(false, None).is_none());
        assert!(
            closed_edit(Some(target), "existing")
                .resolve(true, None)
                .is_none()
        );
        assert!(closed_edit(None, " \n").resolve(true, None).is_none());
        assert!(
            closed_edit(Some(target), " ")
                .resolve(true, Some(Kind::Text))
                .is_none()
        );
        let Some(EditChange::Caption {
            target: actual,
            text,
        }) = closed_edit(Some(target), "").resolve(true, Some(Kind::Sequence))
        else {
            panic!("empty caption must still clear the target");
        };
        assert_eq!(actual, target);
        assert!(text.content.is_empty());
    }

    #[test]
    fn settlement_preserves_target_layout_and_draft_parameters() {
        let target = ObjectId::default();
        let existing = closed_edit(Some(target), "中文说明");
        let bounds = existing.edit.bounds;
        let Some(EditChange::Text {
            target: actual,
            text,
            bounds: actual_bounds,
        }) = existing.resolve(true, Some(Kind::Text))
        else {
            panic!("existing text must update its own target");
        };
        assert_eq!(actual, target);
        assert_eq!(actual_bounds, bounds);
        assert_eq!(text.content, "中文说明");

        let mut draft = closed_edit(None, "new text");
        let values = draft.values.as_mut().unwrap();
        values.color_index = 8;
        values.custom_color = "#123456".into();
        let Some(EditChange::New(object)) = draft.resolve(true, None) else {
            panic!("draft must produce a new text object");
        };
        assert_eq!(object.bounds, bounds);
        assert_eq!(object.style.color, [0x12, 0x34, 0x56]);
        let Extra::Text(text) = object.extra else {
            panic!("text payload");
        };
        assert_eq!(text.content, "new text");
    }

    #[test]
    fn style_sync_preserves_instance_selection_and_runtime_composition() {
        use i_slint_core::input::{InternalKeyEvent, KeyEvent, KeyEventType};
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let displays = [Rect {
            left: -300,
            top: -100,
            right: 700,
            bottom: 500,
        }];
        for scale in [1., 1.25] {
            for caption in [false, true] {
                let window = AnnotationTextInput::new().unwrap();
                window.set_value("A中B".into());
                window.show().unwrap();
                window.invoke_focus_input();
                let mut session = Session::default();
                let mut object = crate::annotation_sequence::tests::badge(scale);
                crate::annotation_sequence::layout(&mut object);
                assert!(session.add(object.clone()));
                object.id = session.objects[0].id;
                let mut edit = make_edit();
                edit.scale = scale;
                edit.at = (-100., 80.);
                edit.object = caption.then_some(object.id);
                let id = edit.id;
                let mut owner = State::default();
                owner.open(
                    window.clone_strong(),
                    edit,
                    (!caption).then(|| super::super::parameters::defaults(8)),
                );
                let before = super::super::parameters::defaults(8);
                let mut after = before.clone();
                after.color_index = 8;
                after.custom_color = "#123456".into();
                let mut next = owner.edit().unwrap().original.clone();
                next.font_size = 31.5;
                next.bold = true;
                next.italic = true;
                after.text_size = 32;
                if let Extra::Sequence(sequence) = &mut object.extra {
                    sequence.caption = next.clone();
                    sequence.caption_color = [0x12, 0x34, 0x56];
                }
                let update = |owner: &mut State| {
                    if caption {
                        owner.apply_caption_style(&object, &displays)
                    } else {
                        owner.apply_text_style(
                            &next,
                            [0x12, 0x34, 0x56],
                            &before,
                            &after,
                            &displays,
                        )
                    }
                };
                window.invoke_place_selection(1, 4);
                assert!(update(&mut owner));
                assert_eq!(
                    (window.get_anchor_offset(), window.get_cursor_offset()),
                    (1, 4)
                );
                assert_eq!(window.get_value(), "A中B");
                assert_eq!(owner.edit().unwrap().id, id);
                assert!(window.get_bold() && window.get_italic());
                assert_eq!(
                    window.get_ink(),
                    slint::Color::from_rgb_u8(0x12, 0x34, 0x56)
                );
                window.invoke_place_selection(1, 1);
                window
                    .window()
                    .dispatch_event(WindowEvent::internal(InternalKeyEvent {
                        event_type: KeyEventType::UpdateComposition,
                        preedit_text: "拼音".into(),
                        preedit_selection: Some(0..6),
                        ..Default::default()
                    }));
                assert_eq!(window.get_preedit(), "拼音");
                let selection = (window.get_anchor_offset(), window.get_cursor_offset());
                assert!(update(&mut owner));
                assert_eq!(window.get_preedit(), "拼音");
                assert_eq!(window.get_value(), "A中B");
                assert_eq!(
                    (window.get_anchor_offset(), window.get_cursor_offset()),
                    selection
                );
                assert_eq!(owner.edit().unwrap().laid_out_content, preview(&window).0);
                let mut key_event = KeyEvent::default();
                key_event.text = "字".into();
                window
                    .window()
                    .dispatch_event(WindowEvent::internal(InternalKeyEvent {
                        event_type: KeyEventType::CommitComposition,
                        key_event,
                        ..Default::default()
                    }));
                assert_eq!(window.get_value(), "A字中B");
                assert!(window.get_preedit().is_empty());
                assert_eq!(owner.edit().unwrap().id, id);
                if caption {
                    assert!(session.add(object.clone()));
                    let original = owner.edit().unwrap().original.clone();
                    assert!(!owner.apply_caption_style(&session.objects[1], &displays));
                    assert_eq!(owner.edit().unwrap().original, original);
                }
                assert!(update(&mut owner));
                if scale == 1.25 {
                    let mut visible = if caption {
                        object.clone()
                    } else {
                        Object {
                            erased: Vec::new(),
                            id: Default::default(),
                            kind: Kind::Text,
                            bounds: owner.edit().unwrap().bounds,
                            style: lexift_core::domain::annotation::Style {
                                color: [0x12, 0x34, 0x56],
                                ..Default::default()
                            },
                            points: Vec::new(),
                            extra: Extra::Text(owner.edit().unwrap().original.clone()),
                        }
                    };
                    match &mut visible.extra {
                        Extra::Text(text) => text.content = window.get_value().to_string(),
                        Extra::Sequence(sequence) => {
                            sequence.caption.content = window.get_value().to_string()
                        }
                        _ => unreachable!(),
                    }
                    if caption {
                        crate::annotation_sequence::layout(&mut visible);
                    }
                    let mut rendered = Session::default();
                    assert!(rendered.add(visible));
                    rendered.selected = None;
                    let monitor = Rect {
                        left: -200,
                        top: 0,
                        right: 440,
                        bottom: 300,
                    };
                    let pixels =
                        crate::annotation_render::render(&rendered, monitor, scale, None, None)
                            .unwrap();
                    let mut canvas =
                        tiny_skia::Pixmap::new(pixels.width(), pixels.height()).unwrap();
                    canvas.fill(tiny_skia::Color::from_rgba8(190, 211, 225, 255));
                    canvas.draw_pixmap(
                        0,
                        0,
                        pixels.as_ref(),
                        &tiny_skia::PixmapPaint::default(),
                        tiny_skia::Transform::identity(),
                        None,
                    );
                    let kind = if caption { "caption" } else { "text" };
                    let mut pixels = canvas.width().to_le_bytes().to_vec();
                    pixels.extend_from_slice(&canvas.height().to_le_bytes());
                    pixels.extend_from_slice(canvas.data());
                    std::fs::write(
                        format!("../../target/annotation-editor-style-{kind}-125.rgba"),
                        pixels,
                    )
                    .unwrap();
                }
                owner.clear();
                assert!(
                    !update(&mut owner),
                    "retired instance must not accept style updates"
                );
            }
        }
    }

    #[test]
    fn uninstalled_editor_drop_retires_queued_and_retained_input() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        for finishing in [false, true] {
            let window = AnnotationTextInput::new().unwrap();
            let retained = window.clone_strong();
            let mut edit = make_edit();
            let queue = Rc::downgrade(&edit.input_work.0);
            edit.bind_input(
                &window,
                || panic!("aborted construction must not update"),
                |_| panic!("aborted construction must not finish"),
            );
            edit.reset_caret(|| panic!("dropped edit must not tick"));
            edit.prepare_after(Duration::from_millis(16), || {
                panic!("dropped edit must not prepare")
            });
            retained.invoke_content_changed("queued".into());
            if finishing {
                retained.invoke_done("queued".into());
            }
            // Mimic preparation failing before State::open; no explicit stop.
            drop(edit);
            assert!(!queue.upgrade().unwrap().active.get());
            retained.invoke_content_changed("late".into());
            retained.invoke_caret_changed();
            retained.invoke_cancel();
            retained.invoke_done("late".into());
            clock.set(clock.get() + Duration::from_secs(1));
            slint::platform::update_timers_and_animations();
            drop(window);
            assert!(
                queue.upgrade().is_some(),
                "retained component still owns disabled callbacks"
            );
            drop(retained);
            assert!(
                queue.upgrade().is_none(),
                "queue releases with the last component"
            );
        }
    }

    #[test]
    fn pointer_bridge_stops_dispatching_to_retained_retired_component() {
        use crate::bridge::PopupPointerInput;
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform;
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
        let window = AnnotationTextInput::new().unwrap();
        window.window().set_size(slint::LogicalSize::new(200., 80.));
        window.show().unwrap();
        let calls = Rc::new(Cell::new(0));
        let observed = calls.clone();
        window.on_pointer(move |_, _, _| observed.set(observed.get() + 1));
        let edit = make_edit();
        let sink = edit.pointer_sink(&window);
        let click = || {
            sink(PopupPointerInput::Moved { x: 20., y: 20. });
            sink(PopupPointerInput::LeftPressed { x: 20., y: 20. });
            sink(PopupPointerInput::LeftReleased { x: 20., y: 20. });
        };
        click();
        assert!(calls.get() > 0, "live bridge must dispatch to Slint");
        let before = calls.get();
        let paused = edit.pause_pointer();
        click();
        assert_eq!(calls.get(), before, "teardown pause rejects editor input");
        drop(paused);
        click();
        assert!(calls.get() > before, "surviving editor resumes dispatch");
        let before = calls.get();
        let paused = edit.pause_pointer();
        edit.stop();
        drop(paused);
        click();
        assert_eq!(
            calls.get(),
            before,
            "ending a pause cannot reactivate a retired editor"
        );
        let next = make_edit();
        let next_sink = next.pointer_sink(&window);
        next_sink(PopupPointerInput::LeftPressed { x: 30., y: 20. });
        next_sink(PopupPointerInput::LeftReleased { x: 30., y: 20. });
        assert!(
            calls.get() > before,
            "new instance can reuse component in this fixture"
        );
        let before = calls.get();
        drop(next);
        next_sink(PopupPointerInput::Moved { x: 40., y: 20. });
        next_sink(PopupPointerInput::LeftPressed { x: 40., y: 20. });
        next_sink(PopupPointerInput::LeftReleased { x: 40., y: 20. });
        click();
        assert_eq!(calls.get(), before);
        window.hide().unwrap();
    }

    #[test]
    fn component_and_draft_retire_together_on_take_replace_and_owner_drop() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let mut owner = State::default();
        assert_eq!(owner.edit().map(|edit| edit.id), None);
        let window = AnnotationTextInput::new().unwrap();
        let weak = window.as_weak();
        let mut edit = make_edit();
        edit.original.content = "草稿中文abc".into();
        window.set_value(edit.original.content.clone().into());
        window.invoke_place_selection(3, 6);
        edit.reset_caret(|| panic!("retired instance must not tick"));
        edit.prepare_after(Duration::from_millis(16), || {
            panic!("retired instance must not prepare")
        });
        let id = edit.id;
        owner.open(window, edit, Some(AnnotationValues::default()));
        assert_eq!(owner.edit().map(|edit| edit.id), Some(id));
        let (window, edit) = owner.parts_mut().unwrap();
        edit.original.font_size = 40.;
        window.set_text_size(40.);
        assert_eq!(edit.id, id);
        assert_eq!(window.get_value(), "草稿中文abc");
        assert_eq!(
            (window.get_anchor_offset(), window.get_cursor_offset()),
            (3, 6)
        );
        let retired = owner.take().unwrap();
        assert_eq!(owner.edit().map(|edit| edit.id), None);
        assert_eq!(retired.content, "草稿中文abc");
        assert_eq!(retired.edit.original.content, "草稿中文abc");
        assert!(!retired.edit.caret_running());
        assert!(owner.window().is_none() && owner.edit().is_none());
        assert!(weak.upgrade().is_none());
        let window = AnnotationTextInput::new().unwrap();
        let replaced = window.as_weak();
        owner.open(window, make_edit(), Some(AnnotationValues::default()));
        assert_ne!(owner.edit().map(|edit| edit.id), Some(id));
        owner
            .edit()
            .unwrap()
            .prepare_after(Duration::from_millis(16), || {
                panic!("replaced instance must not prepare")
            });
        let window = AnnotationTextInput::new().unwrap();
        let last = window.as_weak();
        owner.open(window, make_edit(), Some(AnnotationValues::default()));
        assert!(replaced.upgrade().is_none());
        owner
            .edit()
            .unwrap()
            .prepare_after(Duration::from_millis(16), || {
                panic!("dropped owner must not prepare")
            });
        drop(owner);
        assert!(last.upgrade().is_none());
        clock.set(Duration::from_millis(1000));
        slint::platform::update_timers_and_animations();
        // Keep the extracted draft alive: stopping its work cannot rely on its destructor.
        assert_eq!(retired.edit.id, id);
    }

    #[test]
    fn input_notifications_coalesce_and_retired_windows_cannot_schedule_work() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        };
        let updates = Rc::new(RefCell::new(Vec::new()));
        let finishes = Rc::new(RefCell::new(Vec::new()));
        let mut owner = State::default();
        let window = AnnotationTextInput::new().unwrap();
        let retained = window.clone_strong();
        let edit = make_edit();
        let weak = window.as_weak();
        let recorded = updates.clone();
        let completed = finishes.clone();
        edit.bind_input(
            &window,
            move || {
                let input = weak.upgrade().unwrap();
                recorded.borrow_mut().push((
                    input.get_value().to_string(),
                    input.get_anchor_offset(),
                    input.get_cursor_offset(),
                ));
            },
            move |commit| completed.borrow_mut().push(commit),
        );
        owner.open(window, edit, Some(AnnotationValues::default()));
        for text in ["中", "中文abc"] {
            retained.set_value(text.into());
            retained.invoke_content_changed(text.into());
            retained.invoke_caret_changed();
        }
        retained.invoke_place_selection(3, 6);
        advance();
        assert_eq!(*updates.borrow(), vec![("中文abc".into(), 3, 6)]);
        retained.invoke_content_changed("queued".into());
        let closed = owner.take().unwrap();
        assert_eq!(closed.content, "中文abc");
        // Native teardown may lag retirement while another handle still exists.
        retained.invoke_content_changed("stale".into());
        retained.invoke_caret_changed();
        retained.invoke_done("stale".into());
        retained.invoke_cancel();
        advance();
        assert_eq!(updates.borrow().len(), 1);
        assert!(finishes.borrow().is_empty());

        let next = AnnotationTextInput::new().unwrap();
        let edit = make_edit();
        let recorded = updates.clone();
        let completed = finishes.clone();
        edit.bind_input(
            &next,
            move || recorded.borrow_mut().push(("unexpected".into(), 0, 0)),
            move |commit| completed.borrow_mut().push(commit),
        );
        next.invoke_content_changed("pending".into());
        next.invoke_cancel();
        next.invoke_done("duplicate".into());
        next.invoke_caret_changed();
        advance();
        assert_eq!(*finishes.borrow(), vec![false]);
        assert_eq!(updates.borrow().len(), 1);
        owner.open(next, edit, Some(AnnotationValues::default()));
        let next = AnnotationTextInput::new().unwrap();
        let edit = make_edit();
        let recorded = updates.clone();
        edit.bind_input(
            &next,
            move || recorded.borrow_mut().push(("unexpected".into(), 0, 0)),
            |_| panic!("destroyed editor must not finish"),
        );
        let held = next.clone_strong();
        owner.open(next, edit, Some(AnnotationValues::default()));
        held.invoke_caret_changed();
        drop(owner);
        held.invoke_content_changed("stale".into());
        held.invoke_done("stale".into());
        advance();
        assert_eq!(updates.borrow().len(), 1);
    }

    #[test]
    fn editor_owns_and_cancels_preparation_and_caret_work() {
        use slint::platform::{Platform, WindowAdapter};
        struct TestClock(Rc<Cell<Duration>>);
        impl Platform for TestClock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                unreachable!("instance scheduling does not create a window")
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestClock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = |milliseconds| {
            clock.set(clock.get() + Duration::from_millis(milliseconds));
            slint::platform::update_timers_and_animations();
        };
        let mut edit = make_edit();
        let work = Rc::new(RefCell::new(Vec::new()));
        for value in [1, 2] {
            let work = work.clone();
            edit.prepare_after(Duration::from_millis(16), move || {
                work.borrow_mut().push(value)
            });
        }
        let ticks = Rc::new(Cell::new(0));
        let result = ticks.clone();
        edit.reset_caret(move || result.set(result.get() + 1));
        advance(20);
        assert_eq!(*work.borrow(), vec![2]);
        advance(500);
        assert_eq!(ticks.get(), 1);
        edit.prepare_after(Duration::from_millis(16), || {
            panic!("stopped editor must not prepare its window")
        });
        edit.stop();
        advance(1000);
        assert_eq!(ticks.get(), 1);
        edit.prepare_after(Duration::from_millis(16), || {
            panic!("destroyed editor must not prepare its window")
        });
        let result = ticks.clone();
        edit.reset_caret(move || result.set(result.get() + 1));
        drop(edit);
        advance(1000);
        assert_eq!(ticks.get(), 1);
    }
}
