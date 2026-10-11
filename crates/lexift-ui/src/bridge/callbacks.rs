//! Callbacks lifecycle and view wiring.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupDismissCause {
    Escape,
    PointerOutside,
    ForegroundChanged,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PopupPointerInput {
    RightPressed,
    /// Cancels the canvas gesture before clearing Slint's pressed pointer state.
    LeftCancelled {
        x: f32,
        y: f32,
    },
    Moved {
        x: f32,
        y: f32,
    },
    Exited,
    LeftPressed {
        x: f32,
        y: f32,
    },
    LeftReleased {
        x: f32,
        y: f32,
    },
    Scrolled {
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    },
    DismissRequested {
        cause: PopupDismissCause,
    },
    NativeTopResizeRequested,
    Resized {
        width: f32,
        height: f32,
    },
    ResizeFinished {
        width: f32,
        height: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl PopupResizeEdge {
    pub(super) fn from_index(index: i32) -> Option<Self> {
        Some(match index {
            0 => Self::Left,
            1 => Self::Right,
            2 => Self::Top,
            3 => Self::Bottom,
            4 => Self::TopLeft,
            5 => Self::TopRight,
            6 => Self::BottomLeft,
            7 => Self::BottomRight,
            _ => return None,
        })
    }

    pub(super) fn changes_height(self) -> bool {
        !matches!(self, Self::Left | Self::Right)
    }
}

pub type PopupPointerSink = Rc<dyn Fn(PopupPointerInput)>;
pub(super) type CompletePassiveWindowShow = Rc<dyn Fn(&slint::Window, PopupPointerSink) -> bool>;
pub(super) type ScreenCreation = Rc<dyn Fn(&mut dyn FnMut())>;
pub(super) type PrepareScreenWindow = Rc<
    dyn Fn(
        &slint::Window,
        lexift_core::domain::geometry::Rect,
        Option<&lexift_core::ports::screen_capture::CapturedRegion>,
    ) -> PassiveWindowPreparation,
>;
pub(super) type CompleteRegionFrame = Rc<dyn Fn(&slint::Window) -> bool>;
pub(super) type SetPopupDismissal = Rc<dyn Fn(&slint::Window, bool) -> bool>;
pub(super) type AttachToolWindow = Rc<dyn Fn(&slint::Window, &slint::Window) -> bool>;
pub(super) type BeginWindowResize = Rc<dyn Fn(&slint::Window, PopupResizeEdge, bool) -> bool>;
pub(super) type PopupWorkArea = Rc<dyn Fn(Point) -> Option<Rect>>;
pub(super) type ToolbarCursorPosition = Rc<dyn Fn() -> Option<Point>>;
pub(super) type AnnotationDisplays = Rc<dyn Fn() -> Vec<Rect>>;
pub(super) type AnnotationClickThrough = Rc<dyn Fn(&slint::Window, bool) -> bool>;
pub(super) type AnnotationCornerCursor = Rc<dyn Fn(&slint::Window, bool) -> bool>;
pub(super) type AnnotationBrushCursor = Rc<dyn Fn(&slint::Window, f32) -> bool>;
pub(super) type AnnotationFramePresenter = Rc<dyn Fn(&slint::Window, &[u8], u32, u32) -> bool>;
pub(super) type AnnotationForeground =
    Rc<dyn Fn() -> Option<lexift_core::ports::magnifier::WindowToken>>;
pub(super) type AnnotationWindowToken =
    Rc<dyn Fn(&slint::Window) -> Option<lexift_core::ports::magnifier::WindowToken>>;
pub(super) type AnnotationMagnifierSync =
    Rc<dyn Fn(&[lexift_core::ports::magnifier::MagnifierSpec], &[&slint::Window]) -> bool>;
pub(super) type AnnotationMagnifierRefresh = Rc<dyn Fn() -> bool>;
pub(super) type ConfigureResizeBackground = Rc<dyn Fn(&slint::Window, [u8; 3]) -> bool>;
pub(super) type WindowPaintRepair = Rc<dyn Fn()>;
pub(super) type ConfigureWindowPaintRepair = Rc<dyn Fn(&slint::Window, WindowPaintRepair) -> bool>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassiveWindowPreparation {
    Ready,
    Pending,
    Failed,
}

#[derive(Clone)]
pub struct WindowLifecycleCallbacks {
    pub(super) llm_query: Option<crate::llm_settings::LlmQuery>,
    pub(super) clipboard_history_supported: bool,
    pub(crate) history_dismissal: SetPopupDismissal,
    pub(crate) ocr_dismissal: SetPopupDismissal,
    pub(crate) screen_creation: ScreenCreation,
    pub(crate) prepare_screen_window: PrepareScreenWindow,
    pub(crate) complete_region_frame: CompleteRegionFrame,
    pub(crate) complete_passive_window_show: CompletePassiveWindowShow,
    pub(crate) complete_toolbar_show: CompletePassiveWindowShow,
    pub(super) toolbar_target_current:
        Rc<dyn Fn(lexift_core::domain::selection::SelectionTarget) -> bool>,
    pub(super) configure_translation_popup_corners: fn(&slint::Window) -> PopupCornerMode,
    pub(crate) activate_user_requested_window: fn(&slint::Window) -> bool,
    pub(crate) begin_window_drag: fn(&slint::Window) -> bool,
    pub(super) begin_window_resize: BeginWindowResize,
    pub(crate) popup_work_area: PopupWorkArea,
    pub(crate) toolbar_cursor_position: ToolbarCursorPosition,
    pub(crate) cancel_toolbar_drag_on_escape: Rc<dyn Fn(&slint::Window) -> bool>,
    pub(crate) set_popup_dismissal: SetPopupDismissal,
    pub(crate) attach_tool_window: AttachToolWindow,
    pub(crate) annotation_displays: AnnotationDisplays,
    pub(crate) annotation_click_through: AnnotationClickThrough,
    pub(crate) annotation_corner_cursor: AnnotationCornerCursor,
    pub(crate) annotation_brush_cursor: AnnotationBrushCursor,
    pub(crate) annotation_frame_presenter: AnnotationFramePresenter,
    pub(crate) annotation_magnifier_sync: AnnotationMagnifierSync,
    pub(crate) annotation_magnifier_refresh: AnnotationMagnifierRefresh,
    pub(crate) annotation_screen_capture:
        Option<std::sync::Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>>,
    pub(crate) annotation_export_supported: bool,
    pub(crate) annotation_window_token: AnnotationWindowToken,
    pub(crate) annotation_foreground: Option<AnnotationForeground>,
    pub(crate) annotation_renderer:
        Option<std::sync::Arc<dyn lexift_core::ports::annotation_render::AnnotationRenderPort>>,
    pub(super) trim_process_working_set: fn() -> bool,
    pub(crate) configure_menu_window: Rc<dyn Fn(&slint::Window) -> bool>,
    pub(crate) tray_menu_cancelled: Rc<dyn Fn()>,
    pub(super) configure_resize_background: ConfigureResizeBackground,
    pub(super) configure_window_paint_repair: ConfigureWindowPaintRepair,
    pub(super) configure_app_window_icon: fn(&slint::Window) -> bool,
    pub(super) prepare_app_window: PrepareAppWindow,
    pub(super) open_about_link: Rc<dyn Fn(crate::AboutLink) -> bool>,
}

impl WindowLifecycleCallbacks {
    pub fn with_llm_query(
        mut self,
        query: impl Fn(
            lexift_core::domain::llm::LlmConnection,
            lexift_core::ports::llm::LlmQueryMode,
            crate::LlmQueryCompletion,
        ) -> crate::LlmQueryCancellation
        + 'static,
    ) -> Self {
        self.llm_query = Some(Rc::new(query));
        self
    }

    pub fn new(
        complete_passive_window_show: impl Fn(&slint::Window, PopupPointerSink) -> bool + 'static,
        activate_user_requested_window: fn(&slint::Window) -> bool,
        begin_window_drag: fn(&slint::Window) -> bool,
        begin_window_resize: impl Fn(&slint::Window, PopupResizeEdge, bool) -> bool + 'static,
        set_popup_dismissal: impl Fn(&slint::Window, bool) -> bool + 'static,
        attach_tool_window: impl Fn(&slint::Window, &slint::Window) -> bool + 'static,
        trim_process_working_set: fn() -> bool,
    ) -> Self {
        Self {
            llm_query: None,
            clipboard_history_supported: false,
            history_dismissal: Rc::new(|_, _| false),
            ocr_dismissal: Rc::new(|_, _| false),
            screen_creation: Rc::new(|factory| factory()),
            prepare_screen_window: Rc::new(|_, _, _| PassiveWindowPreparation::Ready),
            complete_region_frame: Rc::new(|_| false),
            complete_passive_window_show: Rc::new(complete_passive_window_show),
            complete_toolbar_show: Rc::new(|_, _| true),
            toolbar_target_current: Rc::new(|_| false),
            configure_translation_popup_corners: |_| PopupCornerMode::SlintRounded,
            activate_user_requested_window,
            begin_window_drag,
            begin_window_resize: Rc::new(begin_window_resize),
            popup_work_area: Rc::new(|_| None),
            toolbar_cursor_position: Rc::new(|| None),
            cancel_toolbar_drag_on_escape: Rc::new(|_| false),
            set_popup_dismissal: Rc::new(set_popup_dismissal),
            attach_tool_window: Rc::new(attach_tool_window),
            annotation_displays: Rc::new(Vec::new),
            annotation_click_through: Rc::new(|_, _| false),
            annotation_corner_cursor: Rc::new(|_, _| false),
            annotation_brush_cursor: Rc::new(|_, _| false),
            annotation_frame_presenter: Rc::new(|_, _, _, _| false),
            annotation_magnifier_sync: Rc::new(|_, _| false),
            annotation_magnifier_refresh: Rc::new(|| false),
            annotation_screen_capture: None,
            annotation_export_supported: false,
            annotation_window_token: Rc::new(|_| None),
            annotation_foreground: None,
            annotation_renderer: None,
            trim_process_working_set,
            configure_menu_window: Rc::new(|_| true),
            tray_menu_cancelled: Rc::new(|| {}),
            configure_resize_background: Rc::new(|_, _| true),
            configure_window_paint_repair: Rc::new(|_, _| true),
            configure_app_window_icon: |_| true,
            prepare_app_window: Rc::new(|_, _| PassiveWindowPreparation::Ready),
            open_about_link: Rc::new(|_| false),
        }
    }

    /// Hidden native preparation and post-render handoff are separate from input dismissal.
    pub fn with_screen_interaction(
        mut self,
        create: impl Fn(&mut dyn FnMut()) + 'static,
        prepare: impl Fn(
            &slint::Window,
            lexift_core::domain::geometry::Rect,
            Option<&lexift_core::ports::screen_capture::CapturedRegion>,
        ) -> PassiveWindowPreparation
        + 'static,
    ) -> Self {
        self.screen_creation = Rc::new(create);
        self.prepare_screen_window = Rc::new(prepare);
        self
    }

    pub fn with_screen_frame_handoff(
        mut self,
        complete_frame: impl Fn(&slint::Window) -> bool + 'static,
    ) -> Self {
        self.complete_region_frame = Rc::new(complete_frame);
        self
    }

    pub fn with_ocr_selection_dismissal(
        mut self,
        callback: impl Fn(&slint::Window, bool) -> bool + 'static,
    ) -> Self {
        self.ocr_dismissal = Rc::new(callback);
        self
    }
    pub fn with_clipboard_history_dismissal(
        mut self,
        callback: impl Fn(&slint::Window, bool) -> bool + 'static,
    ) -> Self {
        self.history_dismissal = Rc::new(callback);
        self
    }
    pub fn with_clipboard_history_support(mut self, supported: bool) -> Self {
        self.clipboard_history_supported = supported;
        self
    }

    pub fn with_about_links(mut self, open: impl Fn(crate::AboutLink) -> bool + 'static) -> Self {
        self.open_about_link = Rc::new(open);
        self
    }

    pub fn with_app_window_preparation(
        mut self,
        callback: impl Fn(&slint::Window, WindowRole) -> PassiveWindowPreparation + 'static,
    ) -> Self {
        self.prepare_app_window = Rc::new(callback);
        self
    }

    /// Prepares floating menus without changing ordinary application windows.
    pub fn with_menu_window_preparation(
        mut self,
        callback: impl Fn(&slint::Window) -> bool + 'static,
    ) -> Self {
        self.configure_menu_window = Rc::new(callback);
        self
    }

    /// Sets the native icon used by taskbar and window-list consumers.
    pub fn with_app_window_icon(mut self, callback: fn(&slint::Window) -> bool) -> Self {
        self.configure_app_window_icon = callback;
        self
    }

    /// Returns focus to the tray only after explicit menu cancellation.
    pub fn with_tray_menu_cancelled(mut self, callback: impl Fn() + 'static) -> Self {
        self.tray_menu_cancelled = Rc::new(callback);
        self
    }

    /// Resolves the monitor work area for the Popup's physical window center.
    pub fn with_popup_work_area(mut self, query: impl Fn(Point) -> Option<Rect> + 'static) -> Self {
        self.popup_work_area = Rc::new(query);
        self
    }

    /// Supplies native monitor bounds and canvas hit-testing without exposing platform APIs to UI.
    pub fn with_annotation_canvas(
        mut self,
        displays: impl Fn() -> Vec<Rect> + 'static,
        click_through: impl Fn(&slint::Window, bool) -> bool + 'static,
        present: impl Fn(&slint::Window, &[u8], u32, u32) -> bool + 'static,
    ) -> Self {
        self.annotation_displays = Rc::new(displays);
        self.annotation_click_through = Rc::new(click_through);
        self.annotation_frame_presenter = Rc::new(present);
        self
    }

    /// Installs the DPI-aware corner-radius cursor only on annotation canvases.
    pub fn with_annotation_corner_cursor(
        mut self,
        cursor: impl Fn(&slint::Window, bool) -> bool + 'static,
    ) -> Self {
        self.annotation_corner_cursor = Rc::new(cursor);
        self
    }

    pub fn with_annotation_brush_cursor(
        mut self,
        cursor: impl Fn(&slint::Window, f32) -> bool + 'static,
    ) -> Self {
        self.annotation_brush_cursor = Rc::new(cursor);
        self
    }

    pub fn with_annotation_magnifier(
        mut self,
        sync: impl Fn(&[lexift_core::ports::magnifier::MagnifierSpec], &[&slint::Window]) -> bool
        + 'static,
        refresh: impl Fn() -> bool + 'static,
    ) -> Self {
        self.annotation_magnifier_sync = Rc::new(sync);
        self.annotation_magnifier_refresh = Rc::new(refresh);
        self
    }

    /// Injects the asynchronous Core capture capability without a UI/platform dependency.
    pub fn with_annotation_screen_capture(
        mut self,
        port: Option<std::sync::Arc<dyn lexift_core::ports::screen_capture::ScreenCapturePort>>,
    ) -> Self {
        self.annotation_screen_capture = port;
        self
    }

    /// Advertises the App-owned export capability without exposing capture or save Ports.
    pub fn with_annotation_export_support(mut self, supported: bool) -> Self {
        self.annotation_export_supported = supported;
        self
    }

    /// Resolve native input surfaces through App without a UI/platform dependency.
    pub fn with_annotation_window_token(
        mut self,
        query: impl Fn(&slint::Window) -> Option<lexift_core::ports::magnifier::WindowToken> + 'static,
    ) -> Self {
        self.annotation_window_token = Rc::new(query);
        self
    }

    /// Observe foreground identity without coupling UI to native window APIs.
    pub fn with_annotation_foreground(
        mut self,
        query: impl Fn() -> Option<lexift_core::ports::magnifier::WindowToken> + 'static,
    ) -> Self {
        self.annotation_foreground = Some(Rc::new(query));
        self
    }

    /// The native adapter is created by App; UI only submits Core scenes.
    pub fn with_annotation_renderer(
        mut self,
        port: Option<
            std::sync::Arc<dyn lexift_core::ports::annotation_render::AnnotationRenderPort>,
        >,
    ) -> Self {
        self.annotation_renderer = port;
        self
    }

    /// Checks the original input target without activating any window.
    pub fn with_toolbar_target_current(
        mut self,
        check: impl Fn(lexift_core::domain::selection::SelectionTarget) -> bool + 'static,
    ) -> Self {
        self.toolbar_target_current = Rc::new(check);
        self
    }

    /// Supplies physical cursor coordinates while the selection toolbar is visible.
    pub fn with_toolbar_cursor_position(
        mut self,
        query: impl Fn() -> Option<Point> + 'static,
    ) -> Self {
        self.toolbar_cursor_position = Rc::new(query);
        self
    }

    /// Cancels captured passive toolbar dragging without activating the toolbar.
    pub fn with_toolbar_drag_cancellation(
        mut self,
        cancel: impl Fn(&slint::Window) -> bool + 'static,
    ) -> Self {
        self.cancel_toolbar_drag_on_escape = Rc::new(cancel);
        self
    }

    /// Adds a best-effort, popup-only native surface setup after its HWND exists.
    pub fn with_translation_popup_corners(
        mut self,
        configure: fn(&slint::Window) -> PopupCornerMode,
    ) -> Self {
        self.configure_translation_popup_corners = configure;
        self
    }

    pub fn with_passive_toolbar_interaction(
        mut self,
        complete: impl Fn(&slint::Window, PopupPointerSink) -> bool + 'static,
    ) -> Self {
        self.complete_toolbar_show = Rc::new(complete);
        self
    }

    /// Adds the software renderer's native resize fill.
    pub fn with_resize_background(
        mut self,
        configure: impl Fn(&slint::Window, [u8; 3]) -> bool + 'static,
    ) -> Self {
        self.configure_resize_background = Rc::new(configure);
        self
    }

    /// Adds the software renderer's full repaint after native movement ends.
    pub fn with_window_paint_repair(
        mut self,
        configure: impl Fn(&slint::Window, WindowPaintRepair) -> bool + 'static,
    ) -> Self {
        self.configure_window_paint_repair = Rc::new(configure);
        self
    }
}
