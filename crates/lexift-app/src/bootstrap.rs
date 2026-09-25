use std::sync::Arc;

use i_slint_backend_winit::{WinitWindowAccessor, winit::window::ResizeDirection};
use lexift_core::ports::instance::InstanceStatus;
use lexift_core::ports::tray::{TrayHandler, TrayPort};

use crate::{StartupMode, controller::AppController, lifecycle, wiring::AppServices};

pub(crate) fn run(startup_mode: StartupMode) -> Result<(), Box<dyn std::error::Error>> {
    lexift_observability::init();
    lifecycle::on_start();

    #[cfg(not(feature = "m1-demo"))]
    let platform = lexift_platform::PlatformCapabilities::new();
    #[cfg(feature = "m1-demo")]
    let platform = lexift_platform::PlatformCapabilities::mock();
    let instance = platform.instance();
    if let Some(instance) = &instance
        && instance.acquire()? == InstanceStatus::AlreadyRunning
    {
        tracing::info!("another Lexift instance was activated");
        lifecycle::on_exit();
        return Ok(());
    }

    let services = AppServices::for_current_build(platform)?;
    let initial_state = services
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let screen_for_popup = services.screen.clone();
    let ui = lexift_ui::Ui::new(
        &initial_state,
        cfg!(feature = "m1-demo"),
        |window| match lexift_platform::configure_passive_tool_window(&window.window_handle()) {
            Ok(lexift_platform::PassiveToolWindowPreparation::Ready) => {
                lexift_ui::PassiveWindowPreparation::Ready
            }
            Ok(lexift_platform::PassiveToolWindowPreparation::Pending) => {
                lexift_ui::PassiveWindowPreparation::Pending
            }
            Err(error) => {
                tracing::warn!(%error, "passive window activation policy is unavailable");
                lexift_ui::PassiveWindowPreparation::Failed
            }
        },
        lexift_ui::WindowLifecycleCallbacks::new(
            |window, sink| match lexift_platform::enable_tool_window_interaction(
                &window.window_handle(),
                Box::new(move |event| {
                    let event = match event {
                        lexift_platform::PopupPointerEvent::Moved { x, y } => {
                            lexift_ui::PopupPointerInput::Moved { x, y }
                        }
                        lexift_platform::PopupPointerEvent::Exited => {
                            lexift_ui::PopupPointerInput::Exited
                        }
                        lexift_platform::PopupPointerEvent::LeftPressed { x, y } => {
                            lexift_ui::PopupPointerInput::LeftPressed { x, y }
                        }
                        lexift_platform::PopupPointerEvent::LeftReleased { x, y } => {
                            lexift_ui::PopupPointerInput::LeftReleased { x, y }
                        }
                        lexift_platform::PopupPointerEvent::Scrolled {
                            x,
                            y,
                            delta_x,
                            delta_y,
                        } => lexift_ui::PopupPointerInput::Scrolled {
                            x,
                            y,
                            delta_x,
                            delta_y,
                        },
                        lexift_platform::PopupPointerEvent::DismissRequested => {
                            lexift_ui::PopupPointerInput::DismissRequested
                        }
                        lexift_platform::PopupPointerEvent::NativeTopResizeRequested => {
                            lexift_ui::PopupPointerInput::NativeTopResizeRequested
                        }
                        lexift_platform::PopupPointerEvent::Resized { width, height } => {
                            lexift_ui::PopupPointerInput::Resized { width, height }
                        }
                        lexift_platform::PopupPointerEvent::ResizeFinished { width, height } => {
                            lexift_ui::PopupPointerInput::ResizeFinished { width, height }
                        }
                    };
                    sink(event);
                }),
            ) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "tool window interaction could not be enabled");
                    false
                }
            },
            |window| match lexift_platform::activate_user_requested_window(&window.window_handle())
            {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "user-requested window activation failed");
                    false
                }
            },
            |window| match lexift_platform::begin_window_drag(&window.window_handle()) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "window drag failed");
                    false
                }
            },
            |window, edge, native_hit_test| {
                let platform_edge = popup_resize_platform_edge(edge);
                match lexift_platform::prepare_window_resize_tracking(
                    &window.window_handle(),
                    platform_edge,
                ) {
                    Ok(true) => {}
                    Ok(false) => return false,
                    Err(error) => {
                        tracing::warn!(%error, "popup resize tracking could not be prepared");
                        return false;
                    }
                }
                if native_hit_test {
                    // DefWindowProc will enter the sizing loop for the real HTTOP press.
                    return true;
                }
                let direction = popup_resize_direction(edge);
                match window.with_winit_window(|winit_window| {
                    winit_window.drag_resize_window(direction)
                }) {
                    Some(Ok(())) => true,
                    Some(Err(error)) => {
                        if let Err(cancel_error) =
                            lexift_platform::cancel_window_resize_tracking(&window.window_handle())
                        {
                            tracing::warn!(%cancel_error, "popup resize tracking could not be cancelled");
                        }
                        tracing::warn!(%error, "Winit could not start popup resize");
                        false
                    }
                    None => {
                        if let Err(error) =
                            lexift_platform::cancel_window_resize_tracking(&window.window_handle())
                        {
                            tracing::warn!(%error, "popup resize tracking could not be cancelled");
                        }
                        tracing::warn!("Winit window is unavailable for popup resize");
                        false
                    }
                }
            },
            |window, enabled| match lexift_platform::set_popup_dismissal(
                &window.window_handle(),
                enabled,
            ) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, enabled, "popup automatic dismissal could not be updated");
                    false
                }
            },
            |child, owner| match lexift_platform::attach_tool_window(
                &child.window_handle(),
                &owner.window_handle(),
            ) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "tool window owner could not be attached");
                    false
                }
            },
            || match lexift_platform::trim_process_working_set() {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "idle resident memory trim failed");
                    false
                }
            },
        )
        .with_popup_work_area(move |point| {
            screen_for_popup
                .as_ref()?
                .work_area_for_point(point)
                .ok()
        })
        .with_resize_background(|window, color_rgb| {
            match lexift_platform::configure_resize_background(&window.window_handle(), color_rgb) {
                Ok(()) => true,
                Err(error) => {
                    tracing::debug!(%error, "resize background fill could not be installed");
                    false
                }
            }
        })
        .with_window_paint_repair(|window, repair| {
            match lexift_platform::configure_window_geometry_repair(
                &window.window_handle(),
                Box::new(move || repair()),
            ) {
                Ok(()) => true,
                Err(error) => {
                    tracing::debug!(%error, "window paint repair could not be installed");
                    false
                }
            }
        })
        .with_translation_popup_corners(|window| {
            match lexift_platform::configure_translation_popup_corners(&window.window_handle()) {
                lexift_platform::PopupCornerMode::NativeRounded => {
                    lexift_ui::PopupCornerMode::NativeDwm
                }
                lexift_platform::PopupCornerMode::OpaqueSquare => {
                    lexift_ui::PopupCornerMode::OpaqueSquare
                }
                lexift_platform::PopupCornerMode::SlintRounded => {
                    lexift_ui::PopupCornerMode::SlintRounded
                }
            }
        }),
    )?;
    let controller = Arc::new(
        AppController::new(
            services.runtime.handle().clone(),
            Arc::clone(&services.state),
            services.selection.clone(),
            services.screen.clone(),
            Arc::clone(&services.translator),
            Arc::clone(&services.settings_store),
            Arc::new(ui.handle()),
        )
        .with_runtime_manager(Arc::clone(&services.runtime_manager))
        .with_credential_management(
            Arc::clone(&services.credential_store),
            Arc::clone(&services.credential_reference),
            services.clipboard.clone(),
        )
        .with_speech(services.speech.clone()),
    );
    if let Some(speech) = &services.speech {
        use lexift_core::ports::speech::SpeechEventHandler;
        let controller_for_speech = Arc::downgrade(&controller);
        let handler: SpeechEventHandler = Arc::new(move |event| {
            if let Some(controller) = controller_for_speech.upgrade() {
                controller.dispatch(lexift_core::AppEvent::PopupSpeechStateChanged {
                    session_id: event.session_id,
                    source: event.source,
                    speaking: event.speaking,
                    error: event.error,
                });
            }
        });
        speech.set_event_handler(handler);
    }
    if let Some(instance) = &instance {
        let controller_for_activation = Arc::clone(&controller);
        if let Err(error) = instance.set_activation_handler(Arc::new(move || {
            controller_for_activation.dispatch(lexift_core::AppEvent::MainWindowRequested);
        })) {
            tracing::warn!(%error, "second-launch activation is unavailable");
        }
    }
    ui.on_event(
        {
            let controller = Arc::clone(&controller);
            move |event| controller.dispatch(event)
        },
        {
            let screen = services.screen.clone();
            move || {
                let screen = screen.as_ref()?;
                let cursor = screen.cursor_position().ok()?;
                let work_area = screen.work_area_for_point(cursor).ok()?;
                Some((cursor, work_area))
            }
        },
    );
    if let Err(error) = services
        .runtime_manager
        .start_hotkey(controller.translate_hotkey_handler())
    {
        tracing::warn!(%error, "global translate hotkey is unavailable");
    }
    if let Err(error) = services.runtime_manager.reconcile_autostart() {
        tracing::warn!(%error, "launch-at-login registration could not be reconciled");
    }
    let tray_registered = register_tray(services.tray.as_ref(), controller.tray_handler());
    ui.set_background_mode(tray_registered);
    controller.dispatch(lexift_core::AppEvent::Started);

    let result = ui
        .run(startup_mode == StartupMode::Interactive)
        .map_err(Into::into);

    drop(controller);
    drop(ui);
    drop(services);
    lifecycle::on_exit();
    result
}

fn register_tray(tray: Option<&Arc<dyn TrayPort>>, handler: TrayHandler) -> bool {
    let Some(tray) = tray else {
        return false;
    };
    match tray.register(handler) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "system tray is unavailable; closing the main window will exit");
            false
        }
    }
}

fn popup_resize_platform_edge(
    edge: lexift_ui::PopupResizeEdge,
) -> lexift_platform::PopupResizeEdge {
    match edge {
        lexift_ui::PopupResizeEdge::Left => lexift_platform::PopupResizeEdge::Left,
        lexift_ui::PopupResizeEdge::Right => lexift_platform::PopupResizeEdge::Right,
        lexift_ui::PopupResizeEdge::Top => lexift_platform::PopupResizeEdge::Top,
        lexift_ui::PopupResizeEdge::Bottom => lexift_platform::PopupResizeEdge::Bottom,
        lexift_ui::PopupResizeEdge::TopLeft => lexift_platform::PopupResizeEdge::TopLeft,
        lexift_ui::PopupResizeEdge::TopRight => lexift_platform::PopupResizeEdge::TopRight,
        lexift_ui::PopupResizeEdge::BottomLeft => lexift_platform::PopupResizeEdge::BottomLeft,
        lexift_ui::PopupResizeEdge::BottomRight => lexift_platform::PopupResizeEdge::BottomRight,
    }
}

fn popup_resize_direction(edge: lexift_ui::PopupResizeEdge) -> ResizeDirection {
    match edge {
        lexift_ui::PopupResizeEdge::Left => ResizeDirection::West,
        lexift_ui::PopupResizeEdge::Right => ResizeDirection::East,
        lexift_ui::PopupResizeEdge::Top => ResizeDirection::North,
        lexift_ui::PopupResizeEdge::Bottom => ResizeDirection::South,
        lexift_ui::PopupResizeEdge::TopLeft => ResizeDirection::NorthWest,
        lexift_ui::PopupResizeEdge::TopRight => ResizeDirection::NorthEast,
        lexift_ui::PopupResizeEdge::BottomLeft => ResizeDirection::SouthWest,
        lexift_ui::PopupResizeEdge::BottomRight => ResizeDirection::SouthEast,
    }
}

#[cfg(test)]
mod tests {
    use lexift_core::{Error, Result};

    use super::*;

    struct FailingTray;

    impl TrayPort for FailingTray {
        fn register(&self, _handler: TrayHandler) -> Result<()> {
            Err(Error::new("tray unavailable"))
        }
    }

    #[test]
    fn tray_registration_failure_keeps_background_mode_disabled() {
        let tray: Arc<dyn TrayPort> = Arc::new(FailingTray);
        assert!(!register_tray(Some(&tray), Arc::new(|_| {})));
        assert!(!register_tray(None, Arc::new(|_| {})));
    }

    #[test]
    fn popup_resize_edges_map_to_winit_directions() {
        use ResizeDirection as Direction;
        use lexift_ui::PopupResizeEdge as Edge;

        for (edge, expected) in [
            (Edge::Left, Direction::West),
            (Edge::Right, Direction::East),
            (Edge::Top, Direction::North),
            (Edge::Bottom, Direction::South),
            (Edge::TopLeft, Direction::NorthWest),
            (Edge::TopRight, Direction::NorthEast),
            (Edge::BottomLeft, Direction::SouthWest),
            (Edge::BottomRight, Direction::SouthEast),
        ] {
            assert_eq!(popup_resize_direction(edge), expected);
        }
    }
}
