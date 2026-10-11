//! Annotation focus workflow; scoped to its owning session.
use super::*;
pub(super) fn focus_origin(lifecycle: &WindowLifecycleCallbacks) -> focus::Origin {
    lifecycle
        .annotation_foreground
        .as_ref()
        .map_or(focus::Origin::Unobserved, |query| {
            focus::Origin::Foreground(query())
        })
}

pub(super) fn current_focus_origin() -> focus::Origin {
    REGISTRY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(focus::Origin::Unobserved, |r| focus_origin(&r.lifecycle))
    })
}

pub(super) fn permits_focus_activation(r: &Registry, origin: focus::Origin) -> bool {
    origin.permits_activation(
        r.lifecycle
            .annotation_foreground
            .as_ref()
            .and_then(|query| query()),
        &capture_exclusions(r),
    )
}

pub(super) fn restore_editor_focus_with_activation(r: &Registry, activation: focus::Activation) {
    restore_editor_focus_from(r, activation, focus_origin(&r.lifecycle));
}

pub(super) fn restore_editor_focus_from(
    r: &Registry,
    activation: focus::Activation,
    origin: focus::Origin,
) {
    if r.toolbar_state.watermark.get().is_some() {
        restore_watermark_focus(r, r.toolbar_state.watermark.revision());
        return;
    }
    let Some(edit) = r.editor.edit() else { return };
    restore_input_focus(
        r,
        focus::Target::Text {
            editor: edit.id,
            panel: r.toolbar_state.panel.revision(),
        },
        activation,
        origin,
    );
}

pub(super) fn restore_watermark_focus(r: &Registry, revision: u64) {
    restore_input_focus(
        r,
        focus::Target::Watermark { editor: revision },
        focus::Activation::Local,
        focus_origin(&r.lifecycle),
    );
}

pub(super) fn restore_input_focus(
    r: &Registry,
    target: focus::Target,
    activation: focus::Activation,
    origin: focus::Origin,
) {
    let request = focus::Request {
        session: r.generation,
        target,
        menu: r.toolbar_state.menus.revision,
        activation,
    };
    let observer = r.lifecycle.annotation_foreground.clone();
    let owners = capture_exclusions(r);
    r.focus.restore(request, move |mut request| {
        if request.activation == focus::Activation::UserRequested
            && !origin.permits_activation(observer.as_ref().and_then(|query| query()), &owners)
        {
            request.activation = focus::Activation::Local;
        }
        if restore_matching_input_focus(request)
            && matches!(request.target, focus::Target::Text { .. })
        {
            schedule_render(request.session);
        }
    });
}

pub(super) fn restore_matching_input_focus(request: focus::Request) -> bool {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(r) = slot.as_mut() else { return false };
        let live = focus::Context {
            session: r.generation,
            panel: r.toolbar_state.panel.revision(),
            menu: r.toolbar_state.menus.revision,
            text_editor: r.editor.edit().map(|edit| edit.id),
            watermark_editor: r
                .toolbar_state
                .watermark
                .get()
                .map(|_| r.toolbar_state.watermark.revision()),
            popup_open: r.toolbar_state.menus.current.is_some() && !choice::allows_text_focus(r),
            controls_busy: choice::input_busy(r)
                || r.parameter_edit.dragging()
                || r.toolbar_state
                    .panel
                    .get()
                    .is_some_and(|p| p.get_parameter_input_active()),
        };
        if !request.matches(&live) {
            return false;
        }
        match request.target {
            focus::Target::Text { .. } => {
                let Some(editor) = r.editor.window() else {
                    return false;
                };
                editor.invoke_focus_input();
                if request.activation == focus::Activation::UserRequested {
                    (r.lifecycle.activate_user_requested_window)(editor.window());
                }
                reset_text_caret(r);
            }
            focus::Target::Watermark { .. } => {
                r.toolbar_state
                    .watermark
                    .get()
                    .unwrap()
                    .invoke_focus_input();
            }
        }
        true
    })
}

#[cfg(test)]
pub(super) fn restore_matching_text_focus(
    generation: u64,
    id: u64,
    panel_revision: u64,
    choice_revision: u64,
) -> bool {
    restore_matching_input_focus(focus::Request {
        session: generation,
        target: focus::Target::Text {
            editor: id,
            panel: panel_revision,
        },
        menu: choice_revision,
        activation: focus::Activation::UserRequested,
    })
}
