//! Independent parameter menus: Winit's embedded PopupWindow clips at panel bounds.
use super::parameters::palette;
use super::window::InputWork;
use super::{
    REGISTRY, Registry, color, current_focus_origin, focus_origin, parameters,
    restore_editor_focus_from, restore_watermark_focus,
};
use crate::AnnotationValues;
use crate::bridge::{
    PassiveWindowPreparation, PopupDismissCause, PopupPointerInput, PopupPointerSink,
    dispatch_pointer_input as dispatch,
};
use crate::{AnnotationChoiceRequest, AnnotationChoiceWindow};
use lexift_core::domain::geometry::{Point, Rect};
use slint::ComponentHandle;
use slint::Model;
use std::{rc::Rc, time::Duration};

const TEXT_OUTLINE: i32 = -3;
const TEXT_BACKGROUND: i32 = -4;
const OUTLINE_COLOR: i32 = -5;
const BACKGROUND_COLOR: i32 = -6;
const WATERMARK_PRESET: i32 = -7;
const BRUSH_SIZE: i32 = -9;

fn is_color(field: i32) -> bool {
    parameters::ColorTarget::from_field(field).is_some()
}

fn target_color(values: &AnnotationValues, palette: [[u8; 3]; 8], field: i32) -> [u8; 3] {
    parameters::ColorTarget::from_field(field)
        .unwrap_or(parameters::ColorTarget::Body)
        .color(values, palette)
}

fn text_style_mode(field: i32) -> i32 {
    if field == BRUSH_SIZE {
        return 3;
    }
    parameters::TextStyleTarget::from_field(field).map_or(0, |target| target.mode())
}

/// Owns the active popup and the cancellable work associated with its instance.
#[derive(Default)]
pub(super) struct State {
    pub current: Option<Menu>,
    pub revision: u64,
    preparation: slint::Timer,
    placement: slint::Timer,
    transition: slint::Timer,
}

impl State {
    /// Defer one popup-level transition without letting it outlive its owner.
    fn return_after_frame(&self, action: impl FnOnce() + 'static) {
        let mut action = Some(action);
        self.transition.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(16),
            move || {
                if let Some(action) = action.take() {
                    action();
                }
            },
        );
    }

    pub fn retire_input(&self) {
        self.preparation.stop();
        self.placement.stop();
        self.transition.stop();
        if let Some(menu) = &self.current {
            menu.input.retire();
        }
    }

    fn take(&mut self) -> Option<Menu> {
        // A replaced State must not reuse an old menu's revision in the same
        // session and panel. Delayed native callbacks can outlive the component.
        self.revision = super::window::next_revision();
        self.retire_input();
        self.current.take()
    }
    fn matches(&self, revision: u64) -> bool {
        self.revision == revision
            && self
                .current
                .as_ref()
                .is_some_and(|menu| !menu.return_pending)
    }
}

type Configure<'a> = &'a dyn Fn(&slint::Window) -> bool;
type Attach<'a> = &'a dyn Fn(&slint::Window, &slint::Window) -> bool;
type Complete<'a> = &'a dyn Fn(&slint::Window, PopupPointerSink) -> bool;
type Monitor<'a> = &'a dyn Fn(&slint::Window, bool) -> bool;
type WorkArea<'a> = &'a dyn Fn(Point) -> Option<Rect>;

/// Only native capabilities and the resolved parent are needed to present a
/// menu. Parameters, document transactions and editor state stay with callers.
struct Presentation<'a> {
    parent: &'a slint::Window,
    prepare: fn(&slint::Window) -> PassiveWindowPreparation,
    configure: Configure<'a>,
    attach: Attach<'a>,
    complete: Complete<'a>,
    monitor: Monitor<'a>,
    work_area: WorkArea<'a>,
    avoid: Vec<Rect>,
}

enum ShowResult {
    Shown,
    Pending,
    Failed,
}

type Dismissal = Rc<dyn Fn(&slint::Window, bool) -> bool>;

pub(super) struct Menu {
    pub window: AnnotationChoiceWindow,
    input: Rc<InputWork>,
    dismissal: Dismissal,
    request: AnnotationChoiceRequest,
    panel_position: slint::PhysicalPosition,
    dragging: bool,
    return_request: Option<AnnotationChoiceRequest>,
    transition_region: Option<Rect>,
    return_pending: bool,
    watermark_revision: Option<u64>,
    target: Option<super::parameter_edit::Target>,
}

fn target_matches(r: &Registry, menu: &Menu) -> bool {
    menu.target
        .is_none_or(|target| r.parameter_target(target.tool) == target)
}

/// Keep an existing brush popup in sync without reopening it or changing focus.
pub(super) fn sync_brush_size(r: &Registry, size: i32) {
    if let Some(menu) = r.menus.current.as_ref().filter(|menu| {
        menu.request.field == BRUSH_SIZE
            && menu.target.is_some_and(|target| target.tool == 13)
            && target_matches(r, menu)
            && owner(r, menu).is_some()
    }) {
        let mut values = menu.window.get_style_values();
        if values.size != size {
            values.size = size;
            menu.window.set_style_values(values);
        }
    }
}

// Cleanup also runs when the containing session is dropped without a normal close.
impl Drop for Menu {
    fn drop(&mut self) {
        self.input.retire();
        (self.dismissal)(self.window.window(), false);
        let _ = self.window.hide();
    }
}

/// Resolve the menu's actual owner, including its current editor instance.
fn owner<'a>(r: &'a Registry, menu: &Menu) -> Option<&'a slint::Window> {
    if let Some(revision) = menu.watermark_revision {
        r.watermark
            .get()
            .filter(|_| r.watermark.revision() == revision)
            .map(ComponentHandle::window)
    } else {
        r.panel.get().map(ComponentHandle::window)
    }
}

/// Passive style and color controls may coexist with the existing text editor.
pub(super) fn allows_text_focus(r: &Registry) -> bool {
    r.menus.current.as_ref().is_some_and(|m| {
        (text_style_mode(m.request.field) != 0 || is_color(m.request.field))
            && !m.return_pending
            && target_matches(r, m)
    })
}
pub(super) fn input_busy(r: &Registry) -> bool {
    r.menus
        .current
        .as_ref()
        .is_some_and(|m| m.dragging || m.window.get_parameter_input_active())
}

pub(super) fn toggle_style(
    generation: u64,
    panel_revision: u64,
    request: AnnotationChoiceRequest,
    origin: super::focus::Origin,
) {
    let enabled = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.panel.revision() == panel_revision)?;
        let tool = r.panel.get()?.get_tool();
        if !r.parameter_entry_active(tool) {
            return None;
        }
        close_from(r, CloseReason::Replaced, origin);
        let mut values = r.values(tool as usize).clone();
        let enabled = match request.field {
            TEXT_BACKGROUND => {
                values.text_background = !values.text_background;
                values.text_background
            }
            TEXT_OUTLINE => {
                values.text_outline = !values.text_outline;
                values.text_outline
            }
            _ => return None,
        };
        let target = r.parameter_target(tool);
        super::apply_geometry_values(r, target, values.clone());
        if let Some(panel) = r.panel.get() {
            panel.set_values(values);
        }
        r.queue_render();
        if !enabled {
            restore_editor_focus_from(r, super::focus::Activation::UserRequested, origin);
        }
        Some(enabled)
    });
    if enabled == Some(true) {
        open_from(generation, panel_revision, request, origin);
    }
}

fn return_style_focus(generation: u64, panel: u64, revision: u64, origin: super::focus::Origin) {
    REGISTRY.with(|slot| {
        if let Some(r) = slot.borrow().as_ref().filter(|r| {
            valid(r, generation, panel, revision) && allows_text_focus(r) && !input_busy(r)
        }) {
            restore_editor_focus_from(r, super::focus::Activation::UserRequested, origin);
        }
    });
}

pub(super) fn color_dragging(r: &Registry) -> bool {
    r.menus.current.as_ref().is_some_and(|menu| menu.dragging)
}

/// Closing a popup does not always end the user's interaction with its controls.
#[derive(Clone, Copy)]
pub(super) enum CloseReason {
    InteractionFinished,
    ExternalDismissed,
    PresentationFailed,
    Replaced,
    ControlPressed,
    OwnerRetired,
}

impl CloseReason {
    fn restores_input(self) -> bool {
        matches!(
            self,
            Self::InteractionFinished | Self::ExternalDismissed | Self::PresentationFailed
        )
    }
    fn activation(self) -> super::focus::Activation {
        if matches!(self, Self::InteractionFinished) {
            super::focus::Activation::UserRequested
        } else {
            super::focus::Activation::Local
        }
    }
    fn clears_input(self) -> bool {
        matches!(self, Self::OwnerRetired)
    }
}

pub(super) fn close(r: &mut Registry, reason: CloseReason) {
    close_from(r, reason, focus_origin(&r.lifecycle));
}

fn close_from(r: &mut Registry, reason: CloseReason, origin: super::focus::Origin) {
    let _pointer_pauses = r.pause_native_pointer_inputs();
    if let Some(menu) = r.menus.take() {
        // Retire any older restoration before deciding the destination of this close.
        r.focus.cancel();
        let target_valid = target_matches(r, &menu);
        let restore_focus = reason.restores_input() && target_valid;
        let watermark = menu.watermark_revision;
        if menu.watermark_revision == Some(r.watermark.revision())
            && let Some(editor) = r.watermark.get()
        {
            editor.set_presets_open(false);
        }

        if is_color(menu.request.field) || text_style_mode(menu.request.field) != 0 {
            r.finish_style_edit();
        }
        drop(menu);
        if watermark.is_none()
            && let Some(panel) = r.panel.get()
        {
            if restore_focus {
                panel.invoke_release_parameter_input();
            } else if reason.clears_input() || !target_valid {
                panel.set_parameter_input_active(false);
            }
        }
        if restore_focus {
            if let Some(revision) = watermark {
                restore_watermark_focus(r, revision);
            } else {
                restore_editor_focus_from(r, reason.activation(), origin);
            }
        }
    }
    if let Some(panel) = r.panel.get() {
        panel.set_active_choice(-1);
    }
    if r.native_presenter
        .exclusions_changed(&super::capture_exclusions(r))
    {
        r.queue_render();
    }
}

#[cfg(test)]
pub(super) fn dismiss() -> bool {
    dismiss_from(current_focus_origin())
}

pub(super) fn dismiss_from(origin: super::focus::Origin) -> bool {
    let current = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|r| r.menus.current.is_some())
            .map(|r| (r.generation, r.panel.revision(), r.menus.revision))
    });
    if let Some((generation, panel_revision, revision)) = current {
        finish_from(generation, panel_revision, revision, None, origin);
        true
    } else {
        false
    }
}

#[cfg(test)]
pub(super) fn open(generation: u64, panel_revision: u64, request: AnnotationChoiceRequest) {
    open_from(generation, panel_revision, request, current_focus_origin());
}

pub(super) fn open_from(
    generation: u64,
    panel_revision: u64,
    request: AnnotationChoiceRequest,
    origin: super::focus::Origin,
) {
    open_menu(generation, panel_revision, request, None, origin);
}

pub(super) fn open_watermark(
    generation: u64,
    watermark_revision: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut().filter(|r| {
            r.generation == generation
                && r.watermark.revision() == watermark_revision
                && r.watermark.get().is_some()
        }) else {
            return;
        };
        let same = r
            .menus
            .current
            .as_ref()
            .is_some_and(|m| m.watermark_revision == Some(watermark_revision));
        close(
            r,
            if same {
                CloseReason::InteractionFinished
            } else {
                CloseReason::Replaced
            },
        );
        if same {
            return;
        }
        let editor = r.watermark.get().unwrap();
        let request = AnnotationChoiceRequest {
            field: WATERMARK_PRESET,
            options: editor.get_preset_labels(),
            selected: editor.get_selected_preset(),
            x,
            y,
            width,
            height,
            lines: false,
            connectors: false,
        };
        let Some(window) = build_selection_window(&request) else {
            return;
        };
        let (panel_revision, revision) = (r.panel.revision(), r.menus.revision);
        let input = InputWork::new();
        let select_input = input.clone();
        let lifecycle = r.lifecycle.clone();
        bind_selection(
            &window,
            Rc::new(move |index| {
                let origin = focus_origin(&lifecycle);
                select_input
                    .post(move || finish_from(generation, panel_revision, revision, index, origin));
            }),
        );
        editor.set_presets_open(true);
        r.menus.current = Some(Menu {
            input: input.clone(),
            window,
            dismissal: r.lifecycle.set_popup_dismissal.clone(),
            request,
            panel_position: editor.window().position(),
            dragging: false,
            return_request: None,
            transition_region: None,
            return_pending: false,
            watermark_revision: Some(watermark_revision),
            target: None,
        });
        input.post(move || show(generation, panel_revision, revision, 0));
    });
}

pub(super) fn close_watermark(r: &mut Registry) {
    if r.menus
        .current
        .as_ref()
        .is_some_and(|menu| menu.watermark_revision.is_some())
    {
        close(r, CloseReason::OwnerRetired);
    }
}

pub(super) fn dismiss_watermark(generation: u64, watermark_revision: u64) {
    let current = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|r| r.generation == generation && r.watermark.revision() == watermark_revision)
            .and_then(|r| {
                r.menus
                    .current
                    .as_ref()
                    .filter(|m| m.watermark_revision == Some(watermark_revision))
                    .map(|_| (r.panel.revision(), r.menus.revision))
            })
    });
    if let Some((panel_revision, revision)) = current {
        finish(generation, panel_revision, revision, None);
    }
}

/// Shared option-list construction has no parameter or document dependency.
/// Watermark presets use this directly; parameter menus add their own controls.
fn build_selection_window(request: &AnnotationChoiceRequest) -> Option<AnnotationChoiceWindow> {
    let window = AnnotationChoiceWindow::new().ok()?;
    crate::theme::apply(&window);
    window.set_options(request.options.clone());
    window.set_selected(request.selected);
    window.set_lines(request.lines);
    window.set_connectors(request.connectors);
    Some(window)
}

/// Window callbacks emit a choice; the owner validates identity and decides
/// when to apply it, close the popup, or restore focus.
fn bind_selection(window: &AnnotationChoiceWindow, receive: Rc<dyn Fn(Option<i32>)>) {
    let chosen = receive.clone();
    window.on_chosen(move |index| chosen(Some(index)));
    let cancelled = receive.clone();
    window.on_cancel(move || cancelled(None));
    window.window().on_close_requested(move || {
        receive(None);
        slint::CloseRequestResponse::KeepWindowShown
    });
}

/// Typed popup input has no document target; the owner attaches the captured
/// menu identity and validates it before applying an edit.
enum MenuInput {
    Custom {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Style(parameters::Edit),
    Selection(Option<i32>, super::focus::Origin),
    InputFinished(super::focus::Origin),
    Color(ColorEdit),
    Dragging(bool),
}

/// Bind component callbacks without borrowing the Registry. All input remains
/// FIFO and cancellable with this popup's InputWork, including retained components.
fn bind_parameter_input(
    window: &AnnotationChoiceWindow,
    input: Rc<InputWork>,
    field: i32,
    origin: Rc<dyn Fn() -> super::focus::Origin>,
    receive: Rc<dyn Fn(MenuInput)>,
) {
    let queue = input.receiver(receive);
    let finished = queue.clone();
    let at = origin.clone();
    window.on_parameter_input_finished(move || finished(MenuInput::InputFinished(at())));
    let custom = queue.clone();
    window.on_custom_requested(move |x, y, width, height| {
        custom(MenuInput::Custom {
            x,
            y,
            width,
            height,
        });
    });
    let style_target = parameters::TextStyleTarget::from_field(field);
    let style = queue.clone();
    window.on_text_style_edited(move |values| {
        if field == BRUSH_SIZE {
            style(MenuInput::Style(parameters::Edit::BrushSize(values.size)));
        } else if let Some(target) = style_target {
            style(MenuInput::Style(target.edit(&values)));
        }
    });
    let selected = queue.clone();
    bind_selection(
        window,
        Rc::new(move |index| {
            selected(MenuInput::Selection(index, origin()));
        }),
    );
    let hsv = queue.clone();
    window.on_hsv_edited(move |hue, saturation, value| {
        hsv(MenuInput::Color(ColorEdit::Hsv(color::Hsv {
            hue,
            saturation,
            value,
        })));
    });
    let hex = queue.clone();
    window.on_hex_edited(move |text, commit| {
        hex(MenuInput::Color(ColorEdit::Hex(text.to_string(), commit)));
    });
    window.on_color_dragging(move |active| queue(MenuInput::Dragging(active)));
}

/// Construct a menu from an immutable presentation snapshot. This function
/// cannot read document state, apply parameters, or restore editor focus.
fn build_window(
    request: &AnnotationChoiceRequest,
    tool: i32,
    values: &AnnotationValues,
) -> Option<AnnotationChoiceWindow> {
    let mode = text_style_mode(request.field);
    if matches!(mode, 1 | 2)
        && (!matches!(tool, 8 | 10)
            || (mode == 1 && !values.text_outline)
            || (mode == 2 && !values.text_background))
    {
        return None;
    }
    let window = build_selection_window(request)?;
    window.set_sequence_shapes(request.field == 23);
    window.set_disabled_index(if tool == 4 && request.field == 6 {
        1
    } else {
        -1
    });
    window.set_color_mode(is_color(request.field));
    window.set_font_mode(request.field == 12);
    window.set_style_values(values.clone());
    window.set_text_style_mode(mode);
    if is_color(request.field) {
        set_color_controls(
            &window,
            color::Hsv::from_rgb(target_color(values, palette(tool as usize), request.field)),
            true,
        );
    }
    Some(window)
}

fn open_menu(
    generation: u64,
    panel_revision: u64,
    request: AnnotationChoiceRequest,
    return_request: Option<AnnotationChoiceRequest>,
    origin: super::focus::Origin,
) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| r.generation == generation && r.panel.revision() == panel_revision)
        else {
            return;
        };
        let same = r
            .menus
            .current
            .as_ref()
            .is_some_and(|m| m.request.field == request.field);
        // The triggering click can still have a queued dismissal from the old window.
        // Keep its bounds until the new popup has settled, without keeping the window alive.
        let transition_region = r
            .menus
            .current
            .as_ref()
            .filter(|menu| {
                return_request.is_some()
                    || (is_color(menu.request.field) && text_style_mode(request.field) != 0)
            })
            .map(|menu| {
                let p = menu.window.window().position();
                let size = menu.window.window().size();
                Rect {
                    left: p.x,
                    top: p.y,
                    right: p.x + size.width as i32,
                    bottom: p.y + size.height as i32,
                }
            });
        close_from(
            r,
            if same {
                CloseReason::InteractionFinished
            } else {
                CloseReason::Replaced
            },
            origin,
        );
        let mode = text_style_mode(request.field);
        if same || (!is_color(request.field) && mode == 0 && request.options.row_count() == 0) {
            return;
        }
        let Some(panel) = r.panel.get() else { return };
        let values = r.values(panel.get_tool() as usize).clone();
        let Some(window) = build_window(&request, panel.get_tool(), &values) else {
            return;
        };
        let revision = r.menus.revision;
        let input = InputWork::new();
        let lifecycle = r.lifecycle.clone();
        bind_parameter_input(
            &window,
            input.clone(),
            request.field,
            Rc::new(move || focus_origin(&lifecycle)),
            Rc::new(move |event| match event {
                MenuInput::InputFinished(origin) => {
                    return_style_focus(generation, panel_revision, revision, origin)
                }
                MenuInput::Custom {
                    x,
                    y,
                    width,
                    height,
                } => open_style_color(generation, panel_revision, revision, x, y, width, height),
                MenuInput::Style(edit) => {
                    edit_text_style(generation, panel_revision, revision, edit)
                }
                MenuInput::Selection(index, origin) => {
                    finish_from(generation, panel_revision, revision, index, origin)
                }
                MenuInput::Color(edit) => edit_color(generation, panel_revision, revision, edit),
                MenuInput::Dragging(active) => {
                    set_dragging(generation, panel_revision, revision, active)
                }
            }),
        );
        panel.set_active_choice(request.field);
        let target = r.parameter_target(panel.get_tool());
        r.menus.current = Some(Menu {
            input: input.clone(),
            window,
            dismissal: r.lifecycle.set_popup_dismissal.clone(),
            request,
            panel_position: panel.window().position(),
            dragging: false,
            return_request,
            transition_region,
            return_pending: false,
            watermark_revision: None,
            target: Some(target),
        });
        input.post(move || show(generation, panel_revision, revision, 0));
    });
}

/// Convert the style popup's local button anchor into the toolbar's logical coordinates.
fn open_style_color(
    generation: u64,
    panel_revision: u64,
    revision: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    let request = REGISTRY.with(|slot| {
        let slot = slot.borrow();
        let r = slot
            .as_ref()
            .filter(|r| valid(r, generation, panel_revision, revision))?;
        let menu = r.menus.current.as_ref()?;
        let field = match menu.request.field {
            TEXT_OUTLINE => OUTLINE_COLOR,
            TEXT_BACKGROUND => BACKGROUND_COLOR,
            _ => return None,
        };
        let panel = r.panel.get()?;
        let scale = panel.window().scale_factor().max(0.1);
        let popup_scale = menu.window.window().scale_factor().max(0.1);
        let p = menu.window.window().position();
        let origin = panel.window().position();
        let mut request = menu.request.clone();
        request.field = field;
        request.x = (p.x - origin.x) as f32 / scale + x * popup_scale / scale;
        request.y = (p.y - origin.y) as f32 / scale + y * popup_scale / scale;
        request.width = width * popup_scale / scale;
        request.height = height * popup_scale / scale;
        Some((request, menu.request.clone()))
    });
    if let Some((request, parent)) = request {
        open_menu(
            generation,
            panel_revision,
            request,
            Some(parent),
            current_focus_origin(),
        );
    }
}

/// Merge only this popup's parameters; its snapshot must never overwrite another edit.
fn edit_text_style(generation: u64, panel_revision: u64, revision: u64, edit: parameters::Edit) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        if let Some(target) = r.menus.current.as_ref().and_then(|menu| menu.target) {
            r.apply_menu_edit(target, edit);
        }
    });
    return_style_focus(generation, panel_revision, revision, current_focus_origin());
}

enum ColorEdit {
    Hsv(color::Hsv),
    Hex(String, bool),
}

fn set_color_controls(window: &AnnotationChoiceWindow, hsv: color::Hsv, update_text: bool) {
    let rgb = hsv.rgb();
    window.set_hue(hsv.hue);
    window.set_saturation(hsv.saturation);
    window.set_value(hsv.value);
    window.set_preview(slint::Color::from_rgb_u8(rgb[0], rgb[1], rgb[2]));
    let bright = color::Hsv { value: 1., ..hsv }.rgb();
    window.set_bright(slint::Color::from_rgb_u8(bright[0], bright[1], bright[2]));
    if update_text {
        window.set_hex_text(color::hex(rgb).into());
    }
    window.set_invalid(false);
}

fn edit_color(generation: u64, panel_revision: u64, revision: u64, edit: ColorEdit) {
    let committed = matches!(&edit, ColorEdit::Hex(_, true));
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        let menu = r.menus.current.as_ref().unwrap();
        if let Some(edit) = menu.edit_color(edit)
            && let Some(target) = menu.target
        {
            r.apply_menu_edit(target, edit);
        }
    });
    if committed {
        return_style_focus(generation, panel_revision, revision, current_focus_origin());
    }
}

fn set_dragging(generation: u64, panel_revision: u64, revision: u64, active: bool) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        if active {
            r.finish_style_edit();
        }
        r.menus.current.as_mut().unwrap().dragging = active;
    });
    if !active {
        let input = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot
                .as_ref()
                .filter(|r| valid(r, generation, panel_revision, revision))?;
            Some(r.menus.current.as_ref()?.input.clone())
        });
        if let Some(input) = input {
            input.after_inputs(move || {
                REGISTRY.with(|slot| {
                    if let Some(r) = slot.borrow_mut().as_mut().filter(|r| {
                        valid(r, generation, panel_revision, revision) && !color_dragging(r)
                    }) {
                        r.finish_style_edit();
                        if allows_text_focus(r) {
                            restore_editor_focus_from(
                                r,
                                super::focus::Activation::UserRequested,
                                focus_origin(&r.lifecycle),
                            );
                        }
                    }
                });
            });
        }
    }
}

/// Window-side completion decisions have no access to the document or Registry.
enum SelectionEffect {
    None,
    Parameter(parameters::Edit),
    TimePreset(i32),
}
enum ReturnDestination {
    Panel,
    Parent(AnnotationChoiceRequest),
    Watermark(u64),
}
enum CloseAction {
    Immediate,
    Deferred(ReturnDestination),
}
struct Completion {
    effect: SelectionEffect,
    close: CloseAction,
}

fn completion_plan(
    request: &AnnotationChoiceRequest,
    parent: Option<&AnnotationChoiceRequest>,
    watermark: Option<u64>,
    tool: i32,
    index: Option<i32>,
) -> Option<Completion> {
    if let Some(revision) = watermark {
        let effect = index
            .filter(|i| *i >= 0 && (*i as usize) < request.options.row_count())
            .map_or(SelectionEffect::None, SelectionEffect::TimePreset);
        return Some(Completion {
            effect,
            close: CloseAction::Deferred(ReturnDestination::Watermark(revision)),
        });
    }
    let effect = match parameters::Selection::from_menu(
        tool,
        request.field,
        index,
        request.options.row_count(),
    ) {
        parameters::Selection::KeepOpen => return None,
        parameters::Selection::Close => SelectionEffect::None,
        parameters::Selection::Apply(edit) => SelectionEffect::Parameter(edit),
    };
    let close = if let Some(parent) = parent {
        CloseAction::Deferred(ReturnDestination::Parent(parent.clone()))
    } else if is_color(request.field) {
        CloseAction::Deferred(ReturnDestination::Panel)
    } else {
        CloseAction::Immediate
    };
    Some(Completion { effect, close })
}

#[derive(Clone, Copy)]
struct ReturnToken {
    generation: u64,
    panel: u64,
    menu: u64,
    origin: super::focus::Origin,
}

fn finish(generation: u64, panel_revision: u64, revision: u64, index: Option<i32>) {
    let origin = REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(super::focus::Origin::Unobserved, |r| {
                focus_origin(&r.lifecycle)
            })
    });
    finish_from(generation, panel_revision, revision, index, origin);
}

fn finish_from(
    generation: u64,
    panel_revision: u64,
    revision: u64,
    index: Option<i32>,
    origin: super::focus::Origin,
) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        let menu = r.menus.current.as_ref().unwrap();
        let tool = r.panel.get().map_or(0, |panel| panel.get_tool());
        let Some(completion) = completion_plan(
            &menu.request,
            menu.return_request.as_ref(),
            menu.watermark_revision,
            tool,
            index,
        ) else {
            return;
        };
        match completion.effect {
            SelectionEffect::None => {}
            SelectionEffect::Parameter(edit) => {
                if let Some(target) = menu.target {
                    r.apply_menu_edit(target, edit);
                }
            }
            SelectionEffect::TimePreset(index) => {
                r.watermark.get().unwrap().invoke_choose_preset(index)
            }
        }
        match completion.close {
            CloseAction::Immediate => {
                close_from(r, CloseReason::InteractionFinished, origin);
                rearm_panel(r);
            }
            CloseAction::Deferred(destination) => {
                // Native and Slint can observe the same Escape: keep one-level guard until the next frame.
                r.menus.current.as_mut().unwrap().return_pending = true;
                let token = ReturnToken {
                    generation,
                    panel: panel_revision,
                    menu: revision,
                    origin,
                };
                r.menus
                    .return_after_frame(move || finish_return(token, destination));
            }
        }
    });
}

/// Validate once against the live owner, then return a parent request outside the Registry borrow.
fn finish_return(token: ReturnToken, destination: ReturnDestination) {
    let parent = REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut().filter(|r| {
            r.generation == token.generation
                && r.menus.revision == token.menu
                && r.menus.current.as_ref().is_some_and(|menu| {
                    menu.return_pending && target_matches(r, menu) && owner(r, menu).is_some()
                })
                && match &destination {
                    ReturnDestination::Watermark(revision) => r.watermark.revision() == *revision,
                    _ => r.panel.revision() == token.panel,
                }
        })?;
        match destination {
            ReturnDestination::Parent(request) => Some(request),
            ReturnDestination::Panel => {
                close_from(r, CloseReason::InteractionFinished, token.origin);
                rearm_panel(r);
                None
            }
            ReturnDestination::Watermark(_) => {
                close_from(r, CloseReason::InteractionFinished, token.origin);
                None
            }
        }
    });
    if let Some(parent) = parent {
        open_from(token.generation, token.panel, parent, token.origin);
    }
}

fn valid(r: &Registry, generation: u64, panel_revision: u64, revision: u64) -> bool {
    r.generation == generation
        && r.panel.revision() == panel_revision
        && r.menus.matches(revision)
        && r.menus
            .current
            .as_ref()
            .is_some_and(|menu| owner(r, menu).is_some() && target_matches(r, menu))
}

fn show(generation: u64, panel_revision: u64, revision: u64, attempt: u8) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        let menu = r.menus.current.as_ref().unwrap();
        let Some(parent) = owner(r, menu) else { return };
        let weak = menu.window.as_weak();
        let input_work = menu.input.clone();
        let sink = input_work.clone().pointer_sink(move |input| {
            if let PopupPointerInput::DismissRequested { cause } = input {
                input_work.post(move || outside(generation, panel_revision, revision, cause));
            } else if let Some(w) = weak.upgrade() {
                dispatch(w.window(), input);
                if matches!(input, PopupPointerInput::LeftReleased { .. }) {
                    let origin = current_focus_origin();
                    input_work.after_inputs(move || {
                        return_style_focus(generation, panel_revision, revision, origin)
                    });
                }
            }
        });
        let services = Presentation {
            parent,
            prepare: r.prepare,
            configure: &*r.lifecycle.configure_menu_window,
            attach: &*r.lifecycle.attach_tool_window,
            complete: &*r.lifecycle.complete_passive_window_show,
            monitor: &*r.lifecycle.set_popup_dismissal,
            work_area: &*r.lifecycle.popup_work_area,
            avoid: toolbar_rects(r),
        };
        match menu.show(&services, sink) {
            ShowResult::Shown => {
                if r.native_presenter
                    .exclusions_changed(&super::capture_exclusions(r))
                {
                    r.queue_render();
                }
                settle(r, generation, panel_revision, revision, 2);
                if allows_text_focus(r) {
                    restore_editor_focus_from(
                        r,
                        super::focus::Activation::UserRequested,
                        focus_origin(&r.lifecycle),
                    );
                }
            }
            ShowResult::Pending if attempt < 20 => {
                r.menus.preparation.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(16),
                    move || show(generation, panel_revision, revision, attempt + 1),
                );
            }
            _ => close(r, CloseReason::PresentationFailed),
        }
    });
}

// Moving the native menu to a different-DPI monitor updates its scale asynchronously.
fn settle(r: &Registry, generation: u64, panel_revision: u64, revision: u64, remaining: u8) {
    r.menus.placement.start(
        slint::TimerMode::SingleShot,
        Duration::from_millis(16),
        move || {
            REGISTRY.with(|slot| {
                if let Some(r) = slot
                    .borrow_mut()
                    .as_mut()
                    .filter(|r| valid(r, generation, panel_revision, revision))
                {
                    if let Some(menu) = &r.menus.current
                        && let Some(parent) = owner(r, menu)
                        && !menu.place(parent, &*r.lifecycle.popup_work_area, &toolbar_rects(r))
                    {
                        close(r, CloseReason::PresentationFailed);
                        return;
                    }
                    if remaining == 1 {
                        r.menus.current.as_mut().unwrap().transition_region = None;
                    }
                    if remaining > 1 {
                        settle(r, generation, panel_revision, revision, remaining - 1);
                    }
                }
            });
        },
    );
}

impl Menu {
    fn edit_color(&self, edit: ColorEdit) -> Option<parameters::Edit> {
        let target = parameters::ColorTarget::from_field(self.request.field)?;
        let (hsv, normalize) = match edit {
            ColorEdit::Hsv(hsv) => (hsv, true),
            ColorEdit::Hex(text, commit) => {
                let Some(rgb) = color::parse_hex(&text) else {
                    self.window.set_invalid(commit);
                    return None;
                };
                if commit {
                    self.window.set_parameter_input_active(false);
                }
                (color::Hsv::from_rgb(rgb), commit)
            }
        };
        set_color_controls(&self.window, hsv, normalize);
        Some(parameters::Edit::Color(target, hsv.rgb()))
    }

    fn show(&self, services: &Presentation<'_>, sink: PopupPointerSink) -> ShowResult {
        match (services.prepare)(self.window.window()) {
            PassiveWindowPreparation::Pending => ShowResult::Pending,
            PassiveWindowPreparation::Ready => {
                if !self.place(services.parent, services.work_area, &services.avoid) {
                    return ShowResult::Failed;
                }
                let window = self.window.window();
                if !(services.configure)(window) {
                    return ShowResult::Failed;
                }
                if !(services.attach)(window, services.parent) {
                    return ShowResult::Pending;
                }
                if self.window.show().is_err()
                    || !(services.configure)(window)
                    || !(services.complete)(window, sink)
                    || !(services.monitor)(window, true)
                {
                    tracing::warn!("could not show annotation parameter menu");
                    ShowResult::Failed
                } else {
                    if !self.place(services.parent, services.work_area, &services.avoid) {
                        return ShowResult::Failed;
                    }
                    if !(services.attach)(window, services.parent) {
                        let _ = self.window.hide();
                        return ShowResult::Pending;
                    }
                    ShowResult::Shown
                }
            }
            _ => ShowResult::Failed,
        }
    }
    fn place(&self, parent: &slint::Window, work_area: WorkArea<'_>, avoid: &[Rect]) -> bool {
        let menu = self;
        let scale = parent.scale_factor().max(0.1);
        let p = parent.position();
        let q = &menu.request;
        let anchor = Rect {
            left: p.x + (q.x * scale).round() as i32,
            top: p.y + (q.y * scale).round() as i32,
            right: p.x + ((q.x + q.width) * scale).round() as i32,
            bottom: p.y + ((q.y + q.height) * scale).round() as i32,
        };
        let Some(area) = work_area(Point {
            x: anchor.left,
            y: anchor.top,
        }) else {
            return false;
        };
        let menu_scale = menu.window.window().scale_factor().max(0.1);
        let minimum_width = if q.field == BRUSH_SIZE {
            300.
        } else if is_color(q.field) {
            320.
        } else if q.field == TEXT_OUTLINE {
            330.
        } else if q.field == TEXT_BACKGROUND {
            360.
        } else if q.field == 12 {
            300.
        } else if q.field == 23 {
            240.
        } else if q.lines {
            112.
        } else if q.connectors {
            88.
        } else {
            180.
        };
        let desired_width = (q.width * scale).max(minimum_width * menu_scale).ceil() as u32;
        let desired_height = (menu.window.get_menu_height() * menu_scale).ceil() as u32;
        let prefer_above = avoid.first().is_some_and(|main| {
            i64::from(anchor.top) + i64::from(anchor.bottom)
                < i64::from(main.top) + i64::from(main.bottom)
        });
        let Some(placed) = crate::placement::place_annotation_popup(
            anchor,
            (desired_width, desired_height),
            area,
            avoid,
            prefer_above,
            (3. * menu_scale).ceil() as i32,
        ) else {
            tracing::warn!("no unobstructed space for annotation menu");
            return false;
        };
        menu.window
            .set_content_width(desired_width as f32 / menu_scale);
        menu.window
            .window()
            .set_size(slint::PhysicalSize::new(placed.width, placed.height));
        menu.window
            .window()
            .set_position(slint::PhysicalPosition::new(
                placed.position.x,
                placed.position.y,
            ));
        if q.field == 12 {
            menu.window.invoke_center_selection();
        }
        true
    }
}

fn toolbar_rects(r: &Registry) -> Vec<Rect> {
    [
        r.main.get().map(ComponentHandle::window),
        r.panel.get().map(ComponentHandle::window),
    ]
    .into_iter()
    .flatten()
    .map(|w| {
        let p = w.position();
        let size = w.size();
        Rect {
            left: p.x,
            top: p.y,
            right: p.x + size.width as i32,
            bottom: p.y + size.height as i32,
        }
    })
    .collect()
}

/// Use physical coordinates throughout, including negative monitor origins.
#[cfg(test)]
fn menu_position(anchor: Rect, width: i32, height: i32, area: Rect, gap: i32) -> Point {
    let below = anchor.bottom + gap;
    let y = if below + height <= area.bottom {
        below
    } else {
        anchor.top - gap - height
    };
    Point {
        x: anchor
            .left
            .clamp(area.left, (area.right - width).max(area.left)),
        y: y.clamp(area.top, (area.bottom - height).max(area.top)),
    }
}

fn contains(window: &slint::Window, p: Point) -> bool {
    let origin = window.position();
    let size = window.size();
    p.x >= origin.x
        && p.y >= origin.y
        && p.x < origin.x + size.width as i32
        && p.y < origin.y + size.height as i32
}

pub(super) fn cursor_inside(r: &Registry) -> bool {
    r.menus
        .current
        .as_ref()
        .zip((r.lifecycle.toolbar_cursor_position)())
        .is_some_and(|(menu, p)| {
            contains(menu.window.window(), p)
                || menu.transition_region.is_some_and(|area| {
                    p.x >= area.left && p.x < area.right && p.y >= area.top && p.y < area.bottom
                })
                || r.panel
                    .get()
                    .is_some_and(|panel| contains(panel.window(), p))
                || (menu.watermark_revision.is_some()
                    && owner(r, menu).is_some_and(|parent| contains(parent, p)))
        })
}

pub(super) fn rearm_panel(r: &Registry) {
    if let Some(panel) = r.panel.get() {
        (r.lifecycle.set_popup_dismissal)(panel.window(), false);
        (r.lifecycle.set_popup_dismissal)(panel.window(), panel.get_menu());
    }
}

fn outside(generation: u64, panel_revision: u64, revision: u64, cause: PopupDismissCause) {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot
            .as_mut()
            .filter(|r| valid(r, generation, panel_revision, revision))
        else {
            return;
        };
        // A click on the trigger must reach its toggle callback before dismissal.
        let in_panel = r
            .panel
            .get()
            .zip((r.lifecycle.toolbar_cursor_position)())
            .is_some_and(|(p, cursor)| contains(p.window(), cursor));
        let in_editor = allows_text_focus(r)
            && r.editor.window().is_some_and(|editor| {
                let token = (r.lifecycle.annotation_window_token)(editor.window());
                token.is_some()
                    && token
                        == r.lifecycle
                            .annotation_foreground
                            .as_ref()
                            .and_then(|query| query())
            });
        if in_editor && cause != PopupDismissCause::PointerOutside {
            if let Some(menu) = &r.menus.current {
                (r.lifecycle.set_popup_dismissal)(menu.window.window(), false);
                (r.lifecycle.set_popup_dismissal)(menu.window.window(), true);
            }
            return;
        }
        if cause == PopupDismissCause::PointerOutside && (in_panel || cursor_inside(r)) {
            if let Some(menu) = &r.menus.current {
                (r.lifecycle.set_popup_dismissal)(menu.window.window(), false);
                (r.lifecycle.set_popup_dismissal)(menu.window.window(), true);
            }
        } else {
            close(r, CloseReason::ExternalDismissed);
        }
    });
}

pub(super) fn close_if_context_changed(r: &mut Registry) {
    if r.menus.current.as_ref().is_some_and(|m| {
        !target_matches(r, m) || owner(r, m).is_none_or(|p| m.panel_position != p.position())
    }) {
        close(r, CloseReason::OwnerRetired);
    }
}

pub(super) fn watermark_pointer(input: PopupPointerInput) {
    let PopupPointerInput::LeftPressed { x, y } = input else {
        return;
    };
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(menu) = r
            .menus
            .current
            .as_ref()
            .filter(|m| m.watermark_revision.is_some())
        else {
            return;
        };
        let Some(parent) = owner(r, menu) else { return };
        let scale = parent.scale_factor().max(0.1);
        let (x, y) = (x / scale, y / scale);
        let q = &menu.request;
        if x < q.x || x >= q.x + q.width || y < q.y || y >= q.y + q.height {
            close(r, CloseReason::ControlPressed);
        }
    });
}

pub(super) fn panel_pointer(input: PopupPointerInput) {
    let PopupPointerInput::LeftPressed { x, y } = input else {
        return;
    };
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return };
        let Some(menu) = &r.menus.current else { return };
        let scale = r
            .panel
            .get()
            .map_or(1., |p| p.window().scale_factor().max(0.1));
        let (x, y) = (x / scale, y / scale);
        let q = &menu.request;
        if x < q.x || x >= q.x + q.width || y < q.y || y >= q.y + q.height {
            close(r, CloseReason::ControlPressed);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::parameters::{color_to_values, palette, palette_color};
    use super::*;
    use crate::{
        AnnotationPanel,
        annotation::{defaults, init},
        bridge::WindowLifecycleCallbacks,
    };
    use slint::{ModelRc, VecModel};
    use std::cell::{Cell, RefCell};

    #[test]
    fn background_toggle_is_atomic_and_style_popup_allows_only_current_editor_focus() {
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
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let (generation, panel) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.selected[3] = 8;
            r.set_mode(super::super::InteractionMode::Tool(3));
            let window = AnnotationPanel::new().unwrap();
            window.set_tool(8);
            window.set_values(defaults(8));
            r.panel.open(window);
            (r.generation, r.panel.revision())
        });
        super::super::open_text_editor_object(generation, (-100., -100.), None, 1.25);
        let request = AnnotationChoiceRequest {
            field: TEXT_BACKGROUND,
            x: 132.,
            y: 10.,
            width: 38.,
            height: 38.,
            ..Default::default()
        };
        for _ in 0..3 {
            toggle_style(
                generation,
                panel,
                request.clone(),
                super::super::focus::Origin::Unobserved,
            );
            let (old, revision, editor) = REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(r.values(8).text_background);
                assert!(r.panel.get().unwrap().get_values().text_background);
                assert!(allows_text_focus(r));
                (
                    r.menus.current.as_ref().unwrap().window.clone_strong(),
                    r.menus.revision,
                    r.editor.edit().unwrap().id,
                )
            });
            let focus = super::super::focus::Request {
                session: generation,
                target: super::super::focus::Target::Text { editor, panel },
                menu: revision,
                activation: super::super::focus::Activation::Local,
            };
            assert!(super::super::restore_matching_input_focus(focus));
            REGISTRY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .menus
                    .current
                    .as_ref()
                    .unwrap()
                    .window
                    .set_parameter_input_active(true)
            });
            assert!(!super::super::restore_matching_input_focus(focus));
            REGISTRY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .menus
                    .current
                    .as_ref()
                    .unwrap()
                    .window
                    .set_parameter_input_active(false)
            });
            toggle_style(
                generation,
                panel,
                request.clone(),
                super::super::focus::Origin::Unobserved,
            );
            assert!(!super::super::restore_matching_input_focus(focus));
            old.invoke_text_style_edited(defaults(8));
            old.invoke_cancel();
            REGISTRY.with(|slot| {
                let slot = slot.borrow();
                let r = slot.as_ref().unwrap();
                assert!(!r.values(8).text_background);
                assert!(r.menus.current.is_none());
                assert_eq!(r.editor.edit().unwrap().id, editor);
            });
        }
        toggle_style(
            generation,
            panel,
            request,
            super::super::focus::Origin::Unobserved,
        );
        REGISTRY.with(|slot| {
            close(
                slot.borrow_mut().as_mut().unwrap(),
                CloseReason::ExternalDismissed,
            )
        });
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            assert!(slot.as_ref().unwrap().values(8).text_background);
        });
        open_from(
            generation,
            panel,
            AnnotationChoiceRequest {
                field: TEXT_BACKGROUND,
                ..Default::default()
            },
            super::super::focus::Origin::Unobserved,
        );
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            let editor = r.editor.window().unwrap();
            editor.set_value("甲乙丙".into());
            editor.invoke_place_selection(3, 6);
            editor.invoke_escape_requested();
        });
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(r.menus.current.is_none());
            let editor = r.editor.window().unwrap();
            assert_eq!(editor.get_value(), "甲乙丙");
            assert_eq!(editor.get_cursor_offset(), 6);
            assert_eq!(editor.get_anchor_offset(), 3);
            editor.invoke_content_changed("甲乙丙".into());
        });
        slint::platform::update_timers_and_animations();
        open_from(
            generation,
            panel,
            AnnotationChoiceRequest {
                field: BACKGROUND_COLOR,
                ..Default::default()
            },
            super::super::focus::Origin::Unobserved,
        );
        let (window, revision, focus) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert!(allows_text_focus(r));
            let menu = r.menus.current.as_ref().unwrap();
            (
                menu.window.clone_strong(),
                r.menus.revision,
                super::super::focus::Request {
                    session: generation,
                    target: super::super::focus::Target::Text {
                        editor: r.editor.edit().unwrap().id,
                        panel,
                    },
                    menu: r.menus.revision,
                    activation: super::super::focus::Activation::Local,
                },
            )
        });
        assert!(super::super::restore_matching_input_focus(focus));
        window.set_parameter_input_active(true);
        assert!(!super::super::restore_matching_input_focus(focus));
        edit_color(
            generation,
            panel,
            revision,
            ColorEdit::Hex("invalid".into(), true),
        );
        assert!(window.get_parameter_input_active());
        assert!(window.get_invalid());
        edit_color(
            generation,
            panel,
            revision,
            ColorEdit::Hex("#123456".into(), true),
        );
        assert!(!window.get_parameter_input_active());
        assert!(super::super::restore_matching_input_focus(focus));
        set_dragging(generation, panel, revision, true);
        assert!(!super::super::restore_matching_input_focus(focus));
        set_dragging(generation, panel, revision, false);
        assert!(super::super::restore_matching_input_focus(focus));
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let editor = slot.as_ref().unwrap().editor.window().unwrap();
            assert_eq!(editor.get_value(), "甲乙丙");
            assert_eq!(editor.get_cursor_offset(), 6);
            assert_eq!(editor.get_anchor_offset(), 3);
        });
        super::super::shutdown();
    }

    #[test]
    fn brush_slider_wheel_and_spin_outline_survive_incremental_repaints() {
        use slint::platform::{
            Platform, WindowAdapter, WindowEvent,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::cell::{Cell, RefCell};
        struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
                *self.0.borrow_mut() = Some(window.clone());
                Ok(window)
            }
        }
        let adapter = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        for dpi in [1., 1.25] {
            let window = AnnotationChoiceWindow::new().unwrap();
            window.set_text_style_mode(3);
            window.set_content_width(300.);
            let mut values = parameters::defaults(13);
            values.size = 37;
            window.set_style_values(values);
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
            window
                .window()
                .set_size(slint::LogicalSize::new(300., 104.));
            window.show().unwrap();
            let size = window.window().size();
            let width = size.width as usize;
            let mut pixels = vec![slint::Rgb8Pixel::default(); width * size.height as usize];
            let render = |pixels: &mut [slint::Rgb8Pixel]| {
                slint::platform::update_timers_and_animations();
                adapter
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .draw_if_needed(|renderer| {
                        renderer.render(pixels, width);
                    });
            };
            render(&mut pixels);
            let wheel = |x, dx, dy| {
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerScrolled {
                        position: slint::LogicalPosition::new(x, 52.),
                        delta_x: dx,
                        delta_y: dy,
                    })
            };
            let drag_events = Rc::new(Cell::new(0));
            let events = drag_events.clone();
            window.on_color_dragging(move |_| events.set(events.get() + 1));
            wheel(120., 0., 120.);
            assert_eq!(window.get_style_values().size, 38, "track wheel");
            // Thumb center at size 38: 12 + 8 + 174 * 37 / 99.
            wheel(85., 0., -120.);
            assert_eq!(window.get_style_values().size, 37, "thumb wheel");
            wheel(120., 120., 0.);
            assert_eq!(window.get_style_values().size, 37, "horizontal wheel");
            for _ in 0..110 {
                wheel(120., 0., 120.);
            }
            assert_eq!(window.get_style_values().size, 100);
            for _ in 0..110 {
                wheel(120., 0., -120.);
            }
            assert_eq!(window.get_style_values().size, 1);
            assert_eq!(
                drag_events.get(),
                0,
                "wheel must not start a drag transaction"
            );
            let mut values = window.get_style_values();
            values.size = 37;
            window.set_style_values(values);
            render(&mut pixels);
            if let Some(directory) = std::env::var_os("LEXIFT_ANNOTATION_RENDER_DIR") {
                let mut image = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
                image.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
                std::fs::write(
                    std::path::PathBuf::from(directory).join(format!("brush-spin-{dpi}.ppm")),
                    image,
                )
                .unwrap();
            }
            let right = (286. * dpi).ceil() as usize;
            let top = (28. * dpi) as usize;
            let bottom = (58. * dpi).ceil() as usize;
            let edge = |pixels: &[slint::Rgb8Pixel]| {
                (top..bottom)
                    .flat_map(|y| {
                        (right - 2..right + 1).map(move |x| {
                            let p = pixels[y * width + x];
                            [p.r, p.g, p.b]
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let baseline = edge(&pixels);
            let divider_left = (258. * dpi).floor() as usize;
            let divider_right = (259. * dpi).ceil() as usize;
            let divider = |pixels: &[slint::Rgb8Pixel]| {
                (top + 2..bottom - 2)
                    .flat_map(|row| {
                        (divider_left..divider_right).map(move |x| {
                            let p = pixels[row * width + x];
                            [p.r, p.g, p.b]
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let assert_divider = |pixels: &[slint::Rgb8Pixel], state| {
                for row in top + 2..bottom - 2 {
                    assert!(
                        (divider_left..divider_right).any(|x| pixels[row * width + x].g <= 240),
                        "vertical divider missing at dpi {dpi}, row {row}, state {state}"
                    );
                }
            };
            assert_divider(&pixels, "normal");
            let baseline_divider = divider(&pixels);
            let baseline_pixels = pixels.clone();
            for y in [35., 50., 35., 50.] {
                window.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(272., y),
                });
                render(&mut pixels);
                assert_divider(&pixels, "hover");
                for row in top + 7..bottom - 7 {
                    assert!(
                        (right - 2..right).any(|x| pixels[row * width + x].g < 245),
                        "right outline missing at dpi {dpi}, row {row}, initial {:?}, hovered {:?}",
                        (right - 5..right + 3)
                            .map(|x| baseline_pixels[row * width + x].g)
                            .collect::<Vec<_>>(),
                        (right - 5..right + 3)
                            .map(|x| pixels[row * width + x].g)
                            .collect::<Vec<_>>()
                    );
                }
                window.window().dispatch_event(WindowEvent::PointerPressed {
                    position: slint::LogicalPosition::new(272., y),
                    button: slint::platform::PointerEventButton::Left,
                });
                render(&mut pixels);
                assert_divider(&pixels, "pressed");
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position: slint::LogicalPosition::new(272., y),
                        button: slint::platform::PointerEventButton::Left,
                    });
                window.window().dispatch_event(WindowEvent::PointerMoved {
                    position: slint::LogicalPosition::new(20., 80.),
                });
                render(&mut pixels);
                assert_divider(&pixels, "leave");
                assert_eq!(
                    divider(&pixels),
                    baseline_divider,
                    "divider did not recover at dpi {dpi}"
                );
                assert_eq!(
                    edge(&pixels),
                    baseline,
                    "outline did not recover at dpi {dpi}"
                );
            }
        }
    }

    #[test]
    fn parameter_callback_binding_is_fifo_owned_and_captures_focus_origin() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::cell::{Cell, RefCell};
        struct TestPlatform(Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
                *self.0.borrow_mut() = Some(window.clone());
                Ok(window)
            }
        }
        let adapter = Rc::new(RefCell::new(None));
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        assert!(REGISTRY.with(|slot| slot.borrow().is_none()));
        let window = AnnotationChoiceWindow::new().unwrap();
        let mut values = super::super::parameters::defaults(8);
        values.text_outline = true;
        values.text_outline_width = 4;
        window.set_text_style_mode(1);
        window.set_style_values(values.clone());
        window.window().set_size(slint::LogicalSize::new(330., 80.));
        window.show().unwrap();
        let input = InputWork::new();
        let origin = Rc::new(Cell::new(super::super::focus::Origin::Foreground(None)));
        let supplier = origin.clone();
        let received = Rc::new(RefCell::new(Vec::new()));
        let events = received.clone();
        bind_parameter_input(
            &window,
            input.clone(),
            TEXT_OUTLINE,
            Rc::new(move || supplier.get()),
            Rc::new(move |event| {
                assert!(
                    REGISTRY.with(|slot| slot.try_borrow_mut().is_ok()),
                    "input must not reenter the registry borrow"
                );
                let label = match event {
                    MenuInput::Custom {
                        x,
                        y,
                        width,
                        height,
                    } => {
                        assert_eq!((x, y, width, height), (1., 2., 3., 4.));
                        "custom"
                    }
                    MenuInput::Style(parameters::Edit::Outline { width, .. }) => {
                        assert_eq!(width, 9);
                        "style"
                    }
                    MenuInput::Color(ColorEdit::Hsv(hsv)) => {
                        assert_eq!((hsv.hue, hsv.saturation, hsv.value), (120., 0.5, 0.8));
                        "hsv"
                    }
                    MenuInput::Color(ColorEdit::Hex(text, commit)) => {
                        assert_eq!(text, "#123456");
                        assert!(commit);
                        "hex"
                    }
                    MenuInput::Dragging(true) => "drag",
                    MenuInput::Selection(Some(2), at) => {
                        assert_eq!(at, super::super::focus::Origin::Foreground(None));
                        "choice"
                    }
                    MenuInput::Selection(None, _) => "cancel",
                    _ => panic!("unexpected input"),
                };
                events.borrow_mut().push(label);
            }),
        );
        REGISTRY.with(|slot| {
            let _borrow = slot.borrow_mut();
            window.invoke_custom_requested(1., 2., 3., 4.);
            values.text_outline_width = 9;
            window.invoke_text_style_edited(values);
            window.invoke_hsv_edited(120., 0.5, 0.8);
            window.invoke_hex_edited("#123456".into(), true);
            window.invoke_color_dragging(true);
            window.invoke_chosen(2);
            assert!(received.borrow().is_empty());
        });
        origin.set(super::super::focus::Origin::Unobserved);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            *received.borrow(),
            ["custom", "style", "hsv", "hex", "drag", "choice"]
        );
        window.invoke_cancel();
        slint::platform::update_timers_and_animations();
        assert_eq!(received.borrow().last(), Some(&"cancel"));
        // Pending and future callbacks from an externally retained component retire together.
        window.invoke_chosen(2);
        input.retire();
        window.invoke_color_dragging(false);
        slint::platform::update_timers_and_animations();
        assert_eq!(received.borrow().len(), 7);
        if let Some(directory) = std::env::var_os("LEXIFT_ANNOTATION_RENDER_DIR") {
            let size = window.window().size();
            let (width, height) = (size.width as usize, size.height as usize);
            let mut pixels = vec![slint::Rgb8Pixel::default(); width * height];
            window.window().request_redraw();
            assert!(
                adapter
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .draw_if_needed(|renderer| {
                        renderer.render(&mut pixels, width);
                    })
            );
            let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
            ppm.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("parameter-callback-outline.ppm"), ppm).unwrap();
        }
        // A reduced picker viewport must still allow the user to reach HEX.
        window.set_text_style_mode(0);
        window.set_color_mode(true);
        window.set_content_width(320.);
        window.set_parameter_input_active(false);
        window
            .window()
            .set_size(slint::LogicalSize::new(220., 120.));
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(100., 50.),
                delta_x: 0.,
                delta_y: -1000.,
            });
        slint::platform::update_timers_and_animations();
        assert!(!window.get_parameter_input_active());
        for y in (10..110).step_by(5) {
            for event in [
                slint::platform::WindowEvent::PointerPressed {
                    position: slint::LogicalPosition::new(80., y as f32),
                    button: slint::platform::PointerEventButton::Left,
                },
                slint::platform::WindowEvent::PointerReleased {
                    position: slint::LogicalPosition::new(80., y as f32),
                    button: slint::platform::PointerEventButton::Left,
                },
            ] {
                window.window().dispatch_event(event);
            }
            if window.get_parameter_input_active() {
                break;
            }
        }
        assert!(
            window.get_parameter_input_active(),
            "scrolling a clipped picker must expose the HEX input"
        );
        window.hide().unwrap();
    }

    #[test]
    fn menu_presentation_uses_explicit_services_without_document_state() {
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
        assert!(REGISTRY.with(|slot| slot.borrow().is_none()));
        let parent = AnnotationPanel::new().unwrap();
        parent
            .window()
            .set_position(slint::PhysicalPosition::new(-490, 285));
        let calls = RefCell::new(Vec::new());
        let configure = |_: &slint::Window| {
            calls.borrow_mut().push("configure");
            true
        };
        let attach = |_: &slint::Window, owner: &slint::Window| {
            assert_eq!(owner.position(), parent.window().position());
            calls.borrow_mut().push("attach");
            true
        };
        let complete = |_: &slint::Window, sink: PopupPointerSink| {
            calls.borrow_mut().push("complete");
            sink(PopupPointerInput::DismissRequested {
                cause: PopupDismissCause::PointerOutside,
            });
            true
        };
        let monitor = |_: &slint::Window, enabled: bool| {
            assert!(enabled);
            calls.borrow_mut().push("monitor");
            true
        };
        let area = Rect {
            left: -500,
            top: -300,
            right: 500,
            bottom: 300,
        };
        let work_area = |_: Point| Some(area);
        let mut services = Presentation {
            parent: parent.window(),
            prepare: |_| PassiveWindowPreparation::Pending,
            configure: &configure,
            attach: &attach,
            complete: &complete,
            monitor: &monitor,
            work_area: &work_area,
            avoid: Vec::new(),
        };
        let request = AnnotationChoiceRequest {
            field: 23,
            width: 80.,
            height: 27.,
            ..Default::default()
        };
        let mut values = defaults(10);
        values.text_outline = true;
        values.text_outline_width = 7;
        values.custom_color = "#102030".into();
        let window = build_window(&request, 10, &values).unwrap();
        assert_eq!(window.get_style_values(), values);
        assert!(window.get_sequence_shapes());
        assert!(REGISTRY.with(|slot| slot.borrow().is_none()));
        let outline = AnnotationChoiceRequest {
            field: TEXT_OUTLINE,
            ..Default::default()
        };
        assert!(build_window(&outline, 10, &values).is_some());
        values.text_outline = false;
        assert!(build_window(&outline, 10, &values).is_none());
        let menu = Menu {
            input: InputWork::new(),
            window,
            dismissal: Rc::new(|_, enabled| {
                assert!(!enabled);
                true
            }),
            request,
            panel_position: parent.window().position(),
            dragging: false,
            return_request: None,
            transition_region: None,
            return_pending: false,
            watermark_revision: None,
            target: None,
        };
        let dismissed = Rc::new(Cell::new(0));
        let result = dismissed.clone();
        let sink: PopupPointerSink = Rc::new(move |_| result.set(result.get() + 1));
        assert!(matches!(
            menu.show(&services, sink.clone()),
            ShowResult::Pending
        ));
        assert!(calls.borrow().is_empty());
        services.prepare = |_| PassiveWindowPreparation::Ready;
        assert!(matches!(menu.show(&services, sink), ShowResult::Shown));
        assert_eq!(
            *calls.borrow(),
            [
                "configure",
                "attach",
                "configure",
                "complete",
                "monitor",
                "attach"
            ]
        );
        assert_eq!(dismissed.get(), 1);
        let origin = menu.window.window().position();
        let size = menu.window.window().size();
        assert!(origin.x >= area.left && origin.x + size.width as i32 <= area.right);
        assert!(origin.y >= area.top && origin.y + size.height as i32 <= 285);
        let avoid = [
            Rect {
                left: -500,
                top: -40,
                right: 200,
                bottom: 14,
            },
            Rect {
                left: -500,
                top: 22,
                right: 400,
                bottom: 80,
            },
        ];
        parent
            .window()
            .set_position(slint::PhysicalPosition::new(-500, 22));
        values.text_outline = true;
        values.text_background = true;
        for field in [
            -2,
            -3,
            -4,
            -5,
            -6,
            -8,
            2,
            6,
            12,
            14,
            16,
            18,
            19,
            20,
            21,
            23,
            24,
            WATERMARK_PRESET,
        ] {
            let request = AnnotationChoiceRequest {
                field,
                x: 150.,
                y: 8.,
                width: 38.,
                height: 38.,
                options: ModelRc::new(VecModel::from(vec!["choice".into(); 11])),
                ..Default::default()
            };
            let window = build_window(&request, 10, &values).unwrap();
            let popup = Menu {
                input: InputWork::new(),
                window,
                dismissal: Rc::new(|_, _| true),
                request,
                panel_position: parent.window().position(),
                dragging: false,
                return_request: None,
                transition_region: None,
                return_pending: false,
                watermark_revision: None,
                target: None,
            };
            assert!(
                popup.place(parent.window(), &work_area, &avoid),
                "field {field}"
            );
            // MinimalSoftwareWindow does not implement native window positioning;
            // physical disjointness is asserted by the placement matrix tests.
            let size = popup.window.window().size();
            assert!(size.width > 0 && size.height > 0);
            assert!(size.width <= (area.right - area.left) as u32);
            assert!(size.height <= (area.bottom - area.top) as u32);
            if is_color(field) {
                assert!(popup.window.get_content_width() >= 320.);
            }
        }
    }

    #[test]
    fn completion_decisions_are_independent_of_session_and_window_state() {
        let request = AnnotationChoiceRequest {
            field: 12,
            options: ModelRc::new(VecModel::from(vec!["first".into(), "second".into()])),
            ..Default::default()
        };
        let completion = completion_plan(&request, None, None, 8, Some(1)).unwrap();
        assert!(matches!(
            completion.effect,
            SelectionEffect::Parameter(parameters::Edit::Choice(parameters::ChoiceTarget::Font, 1))
        ));
        assert!(matches!(completion.close, CloseAction::Immediate));
        for index in [None, Some(-1), Some(2)] {
            let completion = completion_plan(&request, None, None, 8, index).unwrap();
            assert!(matches!(completion.effect, SelectionEffect::None));
            assert!(matches!(completion.close, CloseAction::Immediate));
        }
        let mut color = request.clone();
        color.field = -2;
        let completion = completion_plan(&color, None, None, 8, None).unwrap();
        assert!(matches!(
            completion.close,
            CloseAction::Deferred(ReturnDestination::Panel)
        ));
        let parent = AnnotationChoiceRequest {
            field: TEXT_OUTLINE,
            ..Default::default()
        };
        let completion = completion_plan(&color, Some(&parent), None, 8, None).unwrap();
        let CloseAction::Deferred(ReturnDestination::Parent(returned)) = completion.close else {
            panic!("color child must return one level")
        };
        assert_eq!(returned.field, TEXT_OUTLINE);
        for index in [None, Some(-1), Some(2)] {
            let completion = completion_plan(&request, Some(&parent), Some(17), 8, index).unwrap();
            assert!(matches!(completion.effect, SelectionEffect::None));
            assert!(matches!(
                completion.close,
                CloseAction::Deferred(ReturnDestination::Watermark(17))
            ));
        }
        let completion = completion_plan(&request, None, Some(17), 8, Some(1)).unwrap();
        assert!(matches!(completion.effect, SelectionEffect::TimePreset(1)));
        let keep_open = AnnotationChoiceRequest {
            field: 6,
            ..request
        };
        assert!(completion_plan(&keep_open, None, None, 4, Some(1)).is_none());
    }

    #[test]
    fn popup_replacement_and_control_press_keep_input_until_interaction_finishes() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let released = Rc::new(Cell::new(0));
        let (generation, panel_revision, panel) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let panel = AnnotationPanel::new().unwrap();
            panel.set_tool(8);
            panel.set_values(defaults(8));
            let count = released.clone();
            let weak = panel.as_weak();
            panel.on_release_parameter_input(move || {
                count.set(count.get() + 1);
                weak.upgrade().unwrap().set_parameter_input_active(false);
            });
            r.panel.open(panel.clone_strong());
            (r.generation, r.panel.revision(), panel)
        });
        let request = AnnotationChoiceRequest {
            field: 1,
            options: ModelRc::new(VecModel::from(vec!["first".into(), "second".into()])),
            ..Default::default()
        };
        open(generation, panel_revision, request.clone());
        let (old, work) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let menu = slot.as_ref().unwrap().menus.current.as_ref().unwrap();
            (menu.window.clone_strong(), menu.input.clone())
        });
        panel.set_parameter_input_active(true);
        open(
            generation,
            panel_revision,
            AnnotationChoiceRequest {
                field: 2,
                ..request.clone()
            },
        );
        assert!(work.retired());
        assert!(panel.get_parameter_input_active());
        assert_eq!(released.get(), 0);
        panel_pointer(PopupPointerInput::LeftPressed { x: 100., y: 100. });
        assert!(REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().menus.current.is_none()));
        assert!(
            panel.get_parameter_input_active(),
            "the pressed control retains its input"
        );
        assert_eq!(released.get(), 0);
        open(generation, panel_revision, request.clone());
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().menus.revision);
        finish(generation, panel_revision, revision, None);
        assert!(!panel.get_parameter_input_active());
        assert_eq!(released.get(), 1);
        open(generation, panel_revision, request);
        panel.set_parameter_input_active(true);
        REGISTRY.with(|slot| slot.borrow_mut().as_mut().unwrap().close_panel());
        assert!(!panel.get_parameter_input_active());
        assert_eq!(
            released.get(),
            1,
            "owner teardown must not focus retired controls"
        );
        old.invoke_chosen(1);
        old.invoke_cancel();
        clock.set(Duration::from_millis(40));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_ref().unwrap();
            assert!(r.panel.get().is_none() && r.menus.current.is_none());
            slot.take();
        });
        assert_eq!(released.get(), 1);
    }

    #[test]
    fn replacing_menu_state_in_the_same_session_rejects_old_callbacks() {
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
        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| false,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let (generation, panel_revision) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            r.panel.open(AnnotationPanel::new().unwrap());
            (r.generation, r.panel.revision())
        });
        let request = AnnotationChoiceRequest {
            field: 1,
            options: ModelRc::new(VecModel::from(vec!["first".into(), "second".into()])),
            ..Default::default()
        };
        open(generation, panel_revision, request.clone());
        let (old_revision, retained) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let old = r.menus.current.as_ref().unwrap().window.clone_strong();
            let revision = r.menus.revision;
            r.menus = State::default();
            (revision, old)
        });
        open(generation, panel_revision, request);
        show(generation, panel_revision, old_revision, 0);
        finish(generation, panel_revision, old_revision, Some(1));
        outside(
            generation,
            panel_revision,
            old_revision,
            PopupDismissCause::PointerOutside,
        );
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_ne!(r.menus.revision, old_revision);
            assert!(r.menus.current.is_some());
            assert_eq!(r.menus.current.as_ref().unwrap().window.get_selected(), 0);
            assert_eq!(r.generation, generation);
            assert_eq!(r.panel.revision(), panel_revision);
            close(r, CloseReason::InteractionFinished);
            slot.take();
        });
        drop(retained);
    }

    #[test]
    fn popup_input_is_ordered_and_retained_component_cannot_queue_after_retirement() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct Clock(Rc<Cell<Duration>>);
        impl Platform for Clock {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(Clock(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let advance = || {
            clock.set(clock.get() + Duration::from_millis(1));
            slint::platform::update_timers_and_animations();
        };
        let work = InputWork::new();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let called = calls.clone();
        work.post(move || called.borrow_mut().push(1));
        let called = calls.clone();
        work.after_inputs(move || called.borrow_mut().push(3));
        let called = calls.clone();
        work.post(move || called.borrow_mut().push(2));
        advance();
        assert_eq!(*calls.borrow(), vec![1, 2]);
        advance();
        assert_eq!(*calls.borrow(), vec![1, 2, 3]);
        let weak = Rc::downgrade(&work);
        work.post(move || weak.upgrade().unwrap().retire());
        work.post(|| panic!("retirement must discard the rest of the same batch"));
        advance();
        work.post(|| panic!("retired dispatcher must not accept new work"));
        assert!(!work.pending());

        init(
            |_| PassiveWindowPreparation::Pending,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let (generation, panel_revision) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let panel = AnnotationPanel::new().unwrap();
            r.selected[3] = 8;
            assert!(r.set_mode(super::super::InteractionMode::Tool(3)));
            panel.set_tool(8);
            panel.set_values(defaults(8));
            r.panel.open(panel);
            (r.generation, r.panel.revision())
        });
        let request = AnnotationChoiceRequest {
            field: -2,
            ..Default::default()
        };
        open(generation, panel_revision, request.clone());
        let (retained, input) = REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let menu = slot.as_ref().unwrap().menus.current.as_ref().unwrap();
            (menu.window.clone_strong(), menu.input.clone())
        });
        // Actual Slint callbacks must not borrow Registry while an input dispatch owns it.
        REGISTRY.with(|slot| {
            let _borrow = slot.borrow_mut();
            retained.invoke_color_dragging(true);
            retained.invoke_hex_edited("#123456".into(), true);
            retained.invoke_hex_edited("#654321".into(), true);
            retained.invoke_color_dragging(false);
        });
        advance();
        REGISTRY.with(|slot| {
            assert_eq!(
                slot.borrow()
                    .as_ref()
                    .unwrap()
                    .values(8)
                    .custom_color
                    .as_str(),
                "#654321"
            )
        });
        // Queue a final old value, then close before it reaches the document.
        retained.invoke_hex_edited("#ABCDEF".into(), true);
        REGISTRY.with(|slot| {
            close(
                slot.borrow_mut().as_mut().unwrap(),
                CloseReason::OwnerRetired,
            )
        });
        open(generation, panel_revision, request);
        retained.invoke_hex_edited("#FFFFFF".into(), true);
        retained.invoke_cancel();
        assert!(input.retired());
        assert!(!input.pending());
        advance();
        advance();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.values(8).custom_color.as_str(), "#654321");
            assert!(
                r.menus.current.is_some(),
                "old cancel cannot close the replacement popup"
            );
            close(r, CloseReason::InteractionFinished);
            assert!(r.set_mode(super::super::InteractionMode::Tool(0)));
            r.panel.get().unwrap().set_tool(0);
            r.panel.get().unwrap().set_values(defaults(0));
            use lexift_core::domain::annotation::{Bounds, Extra, Kind, Object, Style};
            assert!(r.session.add(Object {
                erased: Vec::new(),
                id: Default::default(),
                kind: Kind::Rectangle,
                bounds: Bounds::from_corners((0., 0.), (40., 40.)),
                style: Style::default(),
                points: Vec::new(),
                extra: Extra::None,
            }));
        });
        let baseline =
            REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().session.objects[0].clone());
        open(
            generation,
            panel_revision,
            AnnotationChoiceRequest {
                field: -2,
                ..Default::default()
            },
        );
        let selected = REGISTRY.with(|slot| {
            slot.borrow()
                .as_ref()
                .unwrap()
                .menus
                .current
                .as_ref()
                .unwrap()
                .window
                .clone_strong()
        });
        selected.invoke_color_dragging(true);
        selected.invoke_hex_edited("#123456".into(), true);
        selected.invoke_hex_edited("#654321".into(), true);
        selected.invoke_color_dragging(false);
        advance();
        advance();
        advance();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            assert_eq!(r.session.objects[0].style.color, [0x65, 0x43, 0x21]);
            assert!(r.session.undo());
            assert_eq!(r.session.objects, vec![baseline]);
            assert!(r.session.undo(), "creation remains its own operation");
            assert!(r.session.objects.is_empty());
            assert!(
                !r.session.can_undo(),
                "continuous color changes create one operation"
            );
            close(r, CloseReason::InteractionFinished);
            slot.take();
        });
    }

    #[test]
    fn menu_owner_cancels_work_and_releases_window_monitor_on_drop() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        struct TestPlatform(Rc<Cell<Duration>>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
            fn duration_since_start(&self) -> Duration {
                self.0.get()
            }
        }
        let clock = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(TestPlatform(clock.clone()))).unwrap();
        slint::platform::update_timers_and_animations();
        let released = Rc::new(Cell::new(0));
        let result = released.clone();
        let window = AnnotationChoiceWindow::new().unwrap();
        let weak = window.as_weak();
        window.show().unwrap();
        let mut state = State {
            current: Some(Menu {
                input: InputWork::new(),
                window,
                dismissal: Rc::new(move |_, enabled| {
                    assert!(!enabled);
                    result.set(result.get() + 1);
                    true
                }),
                request: AnnotationChoiceRequest::default(),
                panel_position: slint::PhysicalPosition::new(0, 0),
                dragging: false,
                return_request: None,
                transition_region: None,
                return_pending: false,
                watermark_revision: None,
                target: None,
            }),
            ..State::default()
        };
        let transitions = Rc::new(Cell::new(0));
        let called = transitions.clone();
        state.return_after_frame(move || called.set(called.get() + 1));
        clock.set(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        assert_eq!(transitions.get(), 1);
        state.return_after_frame(|| panic!("closed menu must not return to its parent"));
        for timer in [&state.preparation, &state.placement] {
            timer.start(
                slint::TimerMode::SingleShot,
                Duration::from_secs(60),
                || {
                    panic!("closed menu must not run preparation or placement work");
                },
            );
        }
        let retired = state.take().unwrap();
        assert!(state.revision > 0);
        assert!(
            !state.preparation.running()
                && !state.placement.running()
                && !state.transition.running()
        );
        clock.set(Duration::from_millis(40));
        slint::platform::update_timers_and_animations();
        assert_eq!(released.get(), 0);
        drop(retired);
        assert_eq!(released.get(), 1);
        assert!(weak.upgrade().is_none());

        // Session teardown can discard the owner without calling the explicit close path.
        let window = AnnotationChoiceWindow::new().unwrap();
        let weak = window.as_weak();
        let result = released.clone();
        state.current = Some(Menu {
            input: InputWork::new(),
            window,
            dismissal: Rc::new(move |_, enabled| {
                assert!(!enabled);
                result.set(result.get() + 1);
                true
            }),
            request: AnnotationChoiceRequest::default(),
            panel_position: slint::PhysicalPosition::new(0, 0),
            dragging: false,
            return_request: None,
            transition_region: None,
            return_pending: false,
            watermark_revision: None,
            target: None,
        });
        state.return_after_frame(|| panic!("destroyed menu must not return to its parent"));
        drop(state);
        clock.set(Duration::from_millis(80));
        slint::platform::update_timers_and_animations();
        assert_eq!(released.get(), 2);
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn font_menu_centers_selection_and_keeps_keyboard_navigation_visible() {
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
        for dpi in [1., 1.25] {
            let window = AnnotationChoiceWindow::new().unwrap();
            window.set_options(ModelRc::new(VecModel::from(
                (0..200)
                    .map(|i| format!("Font {i:03}").into())
                    .collect::<Vec<_>>(),
            )));
            window.set_font_mode(true);
            window.set_selected(155);
            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
            window.window().set_size(slint::PhysicalSize::new(
                (300. * dpi) as u32,
                (320. * dpi) as u32,
            ));
            window.show().unwrap();
            slint::platform::update_timers_and_animations();
            window.invoke_center_selection();
            let assert_visible = || {
                let top = 4. + window.get_selected() as f32 * 36.;
                let scroll = -window.get_font_scroll_y();
                assert!(
                    top >= scroll - 1. && top + 36. <= scroll + 318. + 1.,
                    "{top}, {scroll}"
                );
            };
            assert_visible();
            for key in [
                slint::platform::Key::End,
                slint::platform::Key::Home,
                slint::platform::Key::DownArrow,
                slint::platform::Key::UpArrow,
            ] {
                window
                    .window()
                    .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
                slint::platform::update_timers_and_animations();
                assert_visible();
            }
            window.hide().unwrap();
        }
    }

    #[test]
    fn custom_colors_round_trip_and_update_only_their_target() {
        let original = defaults(8);
        let rgb = [37, 83, 149];
        let (index, custom) = color_to_values(palette(8), rgb);
        assert_eq!(index, 8);
        assert_eq!(palette_color(palette(8), index, &custom), rgb);
        for field in [-2, OUTLINE_COLOR, BACKGROUND_COLOR] {
            let mut values = original.clone();
            parameters::Edit::Color(parameters::ColorTarget::from_field(field).unwrap(), rgb)
                .apply(&mut values);
            assert_eq!(target_color(&values, palette(8), field), rgb);
            let mut restored = original.clone();
            if field == OUTLINE_COLOR {
                restored.text_outline_color_index = 8;
                restored.text_outline_custom_color = custom.clone();
            } else if field == BACKGROUND_COLOR {
                restored.text_bg_color_index = 8;
                restored.text_bg_custom_color = custom.clone();
            } else {
                restored.color_index = 8;
                restored.custom_color = custom.clone();
            }
            assert_eq!(values, restored);
        }
    }

    #[test]
    fn text_color_buttons_and_centered_popup_custom_button_are_hittable_at_both_scales() {
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
        for dpi in [1., 1.25] {
            let click = |window: &slint::Window, x: f32, y: f32| {
                dispatch(
                    window,
                    PopupPointerInput::Moved {
                        x: x * dpi,
                        y: y * dpi,
                    },
                );
                dispatch(
                    window,
                    PopupPointerInput::LeftPressed {
                        x: x * dpi,
                        y: y * dpi,
                    },
                );
                dispatch(
                    window,
                    PopupPointerInput::LeftReleased {
                        x: x * dpi,
                        y: y * dpi,
                    },
                );
            };
            for (tool, width, height, x, y) in [
                (8, 780., 58., 748., 29.),
                (8, 640., 106., 282., 72.),
                (9, 1000., 58., 962., 29.),
                (9, 640., 154., 282., 120.),
            ] {
                let panel = AnnotationPanel::new().unwrap();
                panel.set_tool(tool);
                panel.set_values(defaults(tool));
                panel
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                panel
                    .window()
                    .set_size(slint::LogicalSize::new(width, height));
                panel.show().unwrap();
                let requested = Rc::new(Cell::new(0));
                let result = requested.clone();
                panel.on_choice_requested(move |request| result.set(request.field));
                click(panel.window(), x, y);
                assert_eq!(requested.get(), -2, "tool {tool}, width {width}");
            }
            for (mode, width, height, y) in [(1, 330., 80., 57.5), (2, 360., 160., 135.5)] {
                let window = AnnotationChoiceWindow::new().unwrap();
                window.set_text_style_mode(mode);
                window.set_style_values(defaults(8));
                window
                    .window()
                    .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: dpi });
                window
                    .window()
                    .set_size(slint::LogicalSize::new(width, height));
                window.show().unwrap();
                window.set_parameter_input_active(true);
                click(window.window(), (width - 283.) / 2. + 160. + 13.5, y);
                assert!(
                    !window.get_parameter_input_active(),
                    "preset color must release the automatically focused numeric input"
                );
                let anchor = Rc::new(Cell::new(None));
                let result = anchor.clone();
                window.on_custom_requested(move |x, y, w, h| result.set(Some((x, y, w, h))));
                click(window.window(), (width - 283.) / 2. + 256. + 13.5, y);
                let (x, _, w, _) = anchor.get().expect("custom button was clipped");
                let realized_width = window.window().size().width as f32 / dpi;
                assert!(
                    (realized_width - x - w - (x - 256.)).abs() < 0.01,
                    "palette margins differ"
                );
            }
        }
        init(
            |_| PassiveWindowPreparation::Ready,
            WindowLifecycleCallbacks::new(
                |_, _| true,
                |_| true,
                |_| false,
                |_, _, _| false,
                |_, _| true,
                |_, _| true,
                || false,
            ),
        );
        let (generation, panel_revision) = REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            let r = slot.as_mut().unwrap();
            let panel = AnnotationPanel::new().unwrap();
            panel.set_tool(8);
            let mut values = defaults(8);
            values.text_background = true;
            r.replace_values(8, values.clone());
            panel.set_values(values);
            r.panel.open(panel);
            (r.generation, r.panel.revision())
        });
        open(
            generation,
            panel_revision,
            AnnotationChoiceRequest {
                field: TEXT_BACKGROUND,
                ..Default::default()
            },
        );
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().menus.revision);
        open_style_color(generation, panel_revision, revision, 250., 122., 27., 27.);
        let child_revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().menus.revision);
        finish(generation, panel_revision, child_revision, None);
        assert!(dismiss()); // Duplicate native Escape must not close the pending parent.
        std::thread::sleep(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let slot = slot.borrow();
            let r = slot.as_ref().unwrap();
            assert_eq!(
                r.menus.current.as_ref().unwrap().request.field,
                TEXT_BACKGROUND
            );
            assert!(r.panel.get().is_some());
            assert!(r.panel.get().unwrap().get_values().text_background);
        });
        let revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().menus.revision);
        open_style_color(generation, panel_revision, revision, 250., 122., 27., 27.);
        let child_revision = REGISTRY.with(|slot| slot.borrow().as_ref().unwrap().menus.revision);
        finish(generation, panel_revision, child_revision, None);
        REGISTRY.with(|slot| {
            close(
                slot.borrow_mut().as_mut().unwrap(),
                CloseReason::OwnerRetired,
            )
        });
        std::thread::sleep(Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
        REGISTRY.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(
                slot.as_ref().unwrap().menus.current.is_none(),
                "closed chain was resurrected"
            );
            slot.take();
        });
    }

    #[test]
    fn text_style_popup_merges_only_its_own_parameters() {
        let original = defaults(8);
        let mut source = defaults(8);
        source.text_size = 900;
        source.font = 17;
        source.text = "stale content".into();
        source.text_outline = !original.text_outline;
        source.text_background = !original.text_background;
        source.text_outline_width = 9;
        source.text_outline_color_index = 3;
        source.text_outline_custom_color = "#256395".into();
        source.text_bg_opacity = 25;
        source.text_bg_padding = 13;
        source.text_bg_rounding = 19;
        source.text_bg_color_index = 5;
        source.text_bg_custom_color = "#731458".into();
        for mode in [0, 1, 2] {
            let mut values = original.clone();
            let mut expected = original.clone();
            if mode == 1 {
                expected.text_outline_width = 9;
                expected.text_outline_color_index = 3;
                expected.text_outline_custom_color = "#256395".into();
            } else if mode == 2 {
                expected.text_bg_opacity = 25;
                expected.text_bg_padding = 13;
                expected.text_bg_rounding = 19;
                expected.text_bg_color_index = 5;
                expected.text_bg_custom_color = "#731458".into();
            }
            if let Some(target) = parameters::TextStyleTarget::from_field(match mode {
                1 => TEXT_OUTLINE,
                2 => TEXT_BACKGROUND,
                _ => 0,
            }) {
                target.edit(&source).apply(&mut values);
            }
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn text_style_windows_fit_near_negative_screen_edges_at_both_dpi_scales() {
        let area = Rect {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        let anchor = Rect {
            left: -60,
            top: 800,
            right: -20,
            bottom: 840,
        };
        for dpi in [1., 1.25] {
            for (width, height) in [(330., 80.), (360., 160.)] {
                let (w, h) = ((width * dpi) as i32, (height * dpi) as i32);
                let p = menu_position(anchor, w, h, area, (3. * dpi) as i32);
                assert!(p.y + h < anchor.top);
                assert!(p.x >= area.left && p.x + w <= area.right);
                assert!(p.y >= area.top && p.y + h <= area.bottom);
            }
        }
    }
    #[test]
    fn placement_flips_and_clamps_at_work_area_edges() {
        let area = Rect {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        let anchor = Rect {
            left: -100,
            top: 700,
            right: 0,
            bottom: 730,
        };
        assert_eq!(
            menu_position(anchor, 140, 230, area, 4),
            Point { x: -140, y: 466 }
        );
        let anchor = Rect {
            left: -1800,
            top: 0,
            right: -1660,
            bottom: 40,
        };
        assert_eq!(
            menu_position(anchor, 140, 230, area, 4),
            Point { x: -1800, y: 44 }
        );
    }
    #[test]
    fn parameter_selection_updates_only_the_requested_field() {
        let original = defaults(0);
        for (field, index) in [
            (1, 1),
            (2, 4),
            (6, 1),
            (7, 3),
            (12, 2),
            (14, 1),
            (16, 2),
            (17, 1),
            (18, 1),
            (21, 3),
        ] {
            let mut values = original.clone();
            parameters::Edit::choice(field, index)
                .unwrap()
                .apply(&mut values);
            let selected = match field {
                1 | 17 => values.shape,
                2 => values.style,
                6 | 14 | 18 => values.mode,
                7 => values.zoom,
                12 => values.font,
                16 => values.format,
                21 => values.connector_style,
                _ => unreachable!(),
            };
            assert_eq!(selected, index);
            assert_eq!(values.size, original.size);
            assert_eq!(values.color_index, original.color_index);
        }
    }
}
