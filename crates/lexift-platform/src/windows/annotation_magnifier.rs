//! Native live magnifier controls; their content stays outside the annotation document.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use lexift_core::{
    Error, Result,
    ports::magnifier::{MagnifierPort, MagnifierSpec, WindowToken},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, RECT},
        Graphics::Gdi::{
            CreateEllipticRgn, RDW_INVALIDATE, RDW_NOERASE, RDW_UPDATENOW, RedrawWindow,
            SetWindowRgn,
        },
        UI::{
            Magnification::{
                MAGTRANSFORM, MW_FILTERMODE_EXCLUDE, MagGetWindowSource, MagInitialize,
                MagSetWindowFilterList, MagSetWindowSource, MagSetWindowTransform, MagUninitialize,
                WC_MAGNIFIER,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE,
                SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
                SetWindowPos, WS_CHILD, WS_CLIPCHILDREN, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
            },
        },
    },
    core::w,
};

pub(crate) struct WindowsMagnifierPort;

const LIVE_REFRESH_IDLE: Duration = Duration::from_millis(100);
const WARMUP_REFRESHES: u8 = 2;
const WARMUP_ALPHA: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visibility {
    Preview,
    Warming(u8),
    Visible,
}

impl Visibility {
    fn new(preview: bool) -> Self {
        if preview {
            Self::Preview
        } else {
            Self::Warming(0)
        }
    }

    fn synced(self, preview: bool, changed: bool) -> Self {
        match (preview, self) {
            (true, _) => Self::Preview,
            (false, Self::Preview) => Self::Warming(0),
            (false, Self::Warming(_)) if changed => Self::Warming(0),
            _ => self,
        }
    }

    fn refreshed(self) -> Self {
        match self {
            Self::Warming(count) if count + 1 >= WARMUP_REFRESHES => Self::Visible,
            Self::Warming(count) => Self::Warming(count + 1),
            state => state,
        }
    }
}

struct View {
    host: HWND,
    control: HWND,
    applied: Option<AppliedView>,
    visibility: Visibility,
    last_change: Instant,
}
struct Manager {
    initialized: bool,
    views: HashMap<u64, View>,
    overlays: Vec<WindowToken>,
}

thread_local! { static MANAGER: RefCell<Manager> = RefCell::new(Manager { initialized: false, views: HashMap::new(), overlays: Vec::new() }); }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ScreenRect {
    fn from_bounds(bounds: lexift_core::domain::annotation::Bounds) -> Self {
        Self {
            left: bounds.left.floor() as i32,
            top: bounds.top.floor() as i32,
            right: bounds.right.ceil() as i32,
            bottom: bounds.bottom.ceil() as i32,
        }
    }

    fn width(self) -> i32 {
        (self.right - self.left).max(1)
    }

    fn height(self) -> i32 {
        (self.bottom - self.top).max(1)
    }

    fn as_win32(self) -> RECT {
        RECT {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppliedView {
    output: ScreenRect,
    source: ScreenRect,
    zoom_bits: u32,
    ellipse: bool,
    excluded: Vec<isize>,
}

impl AppliedView {
    fn new(spec: MagnifierSpec, hosts: &[HWND], overlays: &[WindowToken]) -> Self {
        let mut excluded = hosts.iter().map(|host| host.0 as isize).collect::<Vec<_>>();
        excluded.extend(overlays.iter().map(|token| token.0));
        excluded.sort_unstable();
        excluded.dedup();
        Self {
            output: ScreenRect::from_bounds(spec.output),
            source: ScreenRect::from_bounds(spec.source),
            zoom_bits: spec.zoom.clamp(1., 8.).to_bits(),
            ellipse: spec.ellipse,
            excluded,
        }
    }
}

/// Separates geometry, source, and filtering changes so a drag never resets an
/// otherwise stable magnifier control.
#[derive(Debug, PartialEq, Eq)]
struct UpdatePlan {
    host: bool,
    control_size: bool,
    region: bool,
    transform: bool,
    source: bool,
    filter: bool,
}

impl UpdatePlan {
    fn between(previous: Option<&AppliedView>, next: &AppliedView) -> Self {
        let size_changed = previous.is_none_or(|old| {
            old.output.width() != next.output.width() || old.output.height() != next.output.height()
        });
        Self {
            host: previous.is_none_or(|old| old.output != next.output),
            control_size: size_changed,
            region: previous.is_some_and(|old| old.ellipse != next.ellipse)
                || (next.ellipse && size_changed),
            transform: previous.is_none_or(|old| old.zoom_bits != next.zoom_bits),
            // Changing the control size or transform also changes how the
            // unchanged source rectangle maps into the control. Reapply it
            // before painting or Windows can briefly show a different area.
            source: previous.is_none_or(|old| old.source != next.source)
                || size_changed
                || previous.is_none_or(|old| old.zoom_bits != next.zoom_bits),
            filter: previous.is_none_or(|old| old.excluded != next.excluded),
        }
    }

    fn changed(&self) -> bool {
        self.host
            || self.control_size
            || self.region
            || self.transform
            || self.source
            || self.filter
    }

    fn needs_paint(&self) -> bool {
        self.control_size || self.region || self.transform || self.source || self.filter
    }
}

fn refresh_due(elapsed: Duration) -> bool {
    elapsed >= LIVE_REFRESH_IDLE
}

fn hwnd(token: WindowToken) -> HWND {
    HWND(token.0 as *mut core::ffi::c_void)
}

fn create_view(preview: bool) -> Result<View> {
    let host = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("Lexift Magnifier"),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(|e| Error::new(format!("Could not create magnifier host: {e}")))?;
    if let Err(e) =
        unsafe { SetLayeredWindowAttributes(host, COLORREF(0), WARMUP_ALPHA, LWA_ALPHA) }
    {
        let _ = unsafe { DestroyWindow(host) };
        return Err(Error::new(format!("Could not prepare magnifier host: {e}")));
    }
    let control = match unsafe {
        CreateWindowExW(
            Default::default(),
            WC_MAGNIFIER,
            w!(""),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            1,
            1,
            Some(host),
            None,
            None,
            None,
        )
    } {
        Ok(control) => control,
        Err(e) => {
            let _ = unsafe { DestroyWindow(host) };
            return Err(Error::new(format!(
                "Could not create magnifier control: {e}"
            )));
        }
    };
    Ok(View {
        host,
        control,
        applied: None,
        visibility: Visibility::new(preview),
        last_change: Instant::now(),
    })
}

fn update_view(
    view: &mut View,
    spec: MagnifierSpec,
    hosts: &[HWND],
    overlays: &[WindowToken],
) -> Result<()> {
    let next = AppliedView::new(spec, hosts, overlays);
    let plan = UpdatePlan::between(view.applied.as_ref(), &next);
    let visibility = view.visibility.synced(spec.preview, plan.needs_paint());
    if view.visibility == Visibility::Visible && visibility == Visibility::Preview {
        unsafe { SetLayeredWindowAttributes(view.host, COLORREF(0), WARMUP_ALPHA, LWA_ALPHA) }
            .map_err(|e| Error::new(format!("Could not conceal magnifier preview: {e}")))?;
    }
    if !plan.changed() {
        view.visibility = visibility;
        return Ok(());
    }
    let first_show = view.applied.is_none();
    let width = next.output.width();
    let height = next.output.height();
    if plan.control_size {
        unsafe {
            SetWindowPos(
                view.control,
                None,
                0,
                0,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
            )
        }
        .map_err(|e| Error::new(format!("Could not size magnifier: {e}")))?;
    }
    if plan.region {
        let region = next
            .ellipse
            .then(|| unsafe { CreateEllipticRgn(0, 0, width, height) });
        if unsafe { SetWindowRgn(view.host, region, true) } == 0 {
            return Err(Error::new("Could not clip magnifier shape"));
        }
    }
    if plan.transform {
        let mut transform = MAGTRANSFORM { v: [0.; 9] };
        transform.v[0] = f32::from_bits(next.zoom_bits);
        transform.v[4] = transform.v[0];
        transform.v[8] = 1.;
        if !unsafe { MagSetWindowTransform(view.control, &mut transform) }.as_bool() {
            return Err(Error::new("Could not update magnifier zoom"));
        }
    }
    if plan.filter {
        // The native view samples desktop content only. Lexift marks are
        // composited separately, so the canvas and every host stay excluded.
        let mut excluded = next
            .excluded
            .iter()
            .map(|token| HWND(*token as *mut core::ffi::c_void))
            .collect::<Vec<_>>();
        if !unsafe {
            MagSetWindowFilterList(
                view.control,
                MW_FILTERMODE_EXCLUDE,
                excluded.len() as i32,
                excluded.as_mut_ptr(),
            )
        }
        .as_bool()
        {
            return Err(Error::new(
                "Could not filter annotation windows from magnifier",
            ));
        }
    }
    if plan.source && !unsafe { MagSetWindowSource(view.control, next.source.as_win32()) }.as_bool()
    {
        return Err(Error::new("Could not update live magnifier source"));
    }
    if plan.host {
        let flags = if first_show {
            SWP_NOACTIVATE | SWP_SHOWWINDOW
        } else if plan.control_size {
            SWP_NOACTIVATE | SWP_NOZORDER
        } else {
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE
        };
        unsafe {
            SetWindowPos(
                view.host,
                first_show.then_some(HWND_TOPMOST),
                next.output.left,
                next.output.top,
                width,
                height,
                flags,
            )
        }
        .map_err(|e| Error::new(format!("Could not place magnifier: {e}")))?;
    }
    if plan.needs_paint()
        && !unsafe {
            RedrawWindow(
                Some(view.control),
                None,
                None,
                RDW_INVALIDATE | RDW_NOERASE | RDW_UPDATENOW,
            )
        }
        .as_bool()
    {
        return Err(Error::new("Could not repaint magnifier"));
    }
    view.applied = Some(next);
    view.visibility = visibility;
    view.last_change = Instant::now();
    Ok(())
}

impl Manager {
    fn refresh(&mut self) -> Result<()> {
        for view in self.views.values_mut() {
            let Some(applied) = &view.applied else {
                continue;
            };
            // Geometry updates already repainted this frame. The timer only
            // refreshes stationary content and must not race with a drag.
            if matches!(view.visibility, Visibility::Visible)
                && !refresh_due(view.last_change.elapsed())
            {
                continue;
            }
            if !unsafe { MagSetWindowSource(view.control, applied.source.as_win32()) }.as_bool() {
                return Err(Error::new("Could not refresh live magnifier"));
            }
            if !unsafe {
                RedrawWindow(
                    Some(view.control),
                    None,
                    None,
                    RDW_INVALIDATE | RDW_NOERASE | RDW_UPDATENOW,
                )
            }
            .as_bool()
            {
                return Err(Error::new("Could not repaint live magnifier"));
            }
            if matches!(view.visibility, Visibility::Warming(_)) {
                let mut actual = RECT::default();
                if !unsafe { MagGetWindowSource(view.control, &mut actual) }.as_bool()
                    || actual != applied.source.as_win32()
                {
                    return Err(Error::new(
                        "Magnifier source did not match the requested area",
                    ));
                }
                let next = view.visibility.refreshed();
                if next == Visibility::Visible {
                    unsafe { SetLayeredWindowAttributes(view.host, COLORREF(0), 255, LWA_ALPHA) }
                        .map_err(|e| Error::new(format!("Could not reveal magnifier: {e}")))?;
                }
                view.visibility = next;
            }
        }
        Ok(())
    }
    fn clear(&mut self) {
        for (_, view) in self.views.drain() {
            let _ = unsafe { DestroyWindow(view.host) };
        }
        if self.initialized {
            let _ = unsafe { MagUninitialize() };
            self.initialized = false;
        }
        self.overlays.clear();
    }
    fn sync(&mut self, specs: &[MagnifierSpec], overlays: &[WindowToken]) -> Result<()> {
        let ids: HashSet<u64> = specs.iter().map(|v| v.id).collect();
        let previous_count = self.views.len();
        self.views.retain(|id, view| {
            if ids.contains(id) {
                true
            } else {
                let _ = unsafe { DestroyWindow(view.host) };
                false
            }
        });
        if specs.is_empty() {
            self.clear();
            return Ok(());
        }
        if !self.initialized {
            if !unsafe { MagInitialize() }.as_bool() {
                return Err(Error::new("Windows magnification is unavailable"));
            }
            self.initialized = true;
        }
        let mut created = false;
        for spec in specs {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.views.entry(spec.id) {
                entry.insert(create_view(spec.preview)?);
                created = true;
            }
        }
        let hosts: Vec<_> = self.views.values().map(|v| v.host).collect();
        for spec in specs {
            update_view(
                self.views.get_mut(&spec.id).unwrap(),
                *spec,
                &hosts,
                overlays,
            )?;
        }
        // The transparent canvas must remain above native content so its edit handles and
        // hit testing continue to work. Owned tool windows follow the canvas in z-order.
        if created || self.views.len() != previous_count || self.overlays != overlays {
            for token in overlays {
                let _ = unsafe {
                    SetWindowPos(
                        hwnd(*token),
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    )
                };
            }
            self.overlays = overlays.to_vec();
        }
        Ok(())
    }
}

impl MagnifierPort for WindowsMagnifierPort {
    fn sync(&self, views: &[MagnifierSpec], overlays: &[WindowToken]) -> Result<()> {
        MANAGER.with(|manager| manager.borrow_mut().sync(views, overlays))
    }
    fn refresh(&self) -> Result<()> {
        MANAGER.with(|manager| manager.borrow_mut().refresh())
    }
    fn clear(&self) {
        MANAGER.with(|manager| manager.borrow_mut().clear());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppliedView {
        AppliedView {
            output: ScreenRect {
                left: 300,
                top: 100,
                right: 500,
                bottom: 250,
            },
            source: ScreenRect {
                left: -100,
                top: 50,
                right: 100,
                bottom: 200,
            },
            zoom_bits: 1.5f32.to_bits(),
            ellipse: false,
            excluded: vec![10, 20],
        }
    }

    #[test]
    fn native_updates_only_apply_changed_fields() {
        let current = state();
        let first = UpdatePlan::between(None, &current);
        assert!(
            first.host && first.control_size && first.transform && first.source && first.filter
        );
        assert!(!first.region);
        assert!(!UpdatePlan::between(Some(&current), &current).changed());

        let mut next = current.clone();
        next.output.left += 30;
        next.output.right += 30;
        let move_output = UpdatePlan::between(Some(&current), &next);
        assert!(move_output.host);
        assert!(!move_output.control_size);
        assert!(!move_output.needs_paint());

        let mut next = current.clone();
        next.output.right += 30;
        let resize_output = UpdatePlan::between(Some(&current), &next);
        assert!(resize_output.host && resize_output.control_size);
        assert!(resize_output.source);
        assert!(!resize_output.transform && !resize_output.filter);

        let mut next = current.clone();
        next.source.left -= 20;
        let move_source = UpdatePlan::between(Some(&current), &next);
        assert!(move_source.source && move_source.needs_paint());
        assert!(!move_source.host && !move_source.control_size && !move_source.filter);

        let mut next = current.clone();
        next.zoom_bits = 2.0f32.to_bits();
        let zoom = UpdatePlan::between(Some(&current), &next);
        assert!(zoom.transform && zoom.source);
        assert!(!zoom.host);

        let mut next = current.clone();
        next.ellipse = true;
        assert!(UpdatePlan::between(Some(&current), &next).region);
        next = current.clone();
        next.excluded.push(30);
        let filter = UpdatePlan::between(Some(&current), &next);
        assert!(filter.filter);
        assert!(!filter.host && !filter.source);
    }

    #[test]
    fn excluded_windows_are_a_set_and_refresh_waits_for_drag_to_stop() {
        use lexift_core::domain::annotation::Bounds;

        let spec = MagnifierSpec {
            id: 0,
            preview: false,
            source: Bounds::from_corners((-100., 50.), (100., 200.)),
            output: Bounds::from_corners((300., 100.), (500., 250.)),
            zoom: 1.5,
            ellipse: false,
            antialias: true,
        };
        let hosts = [HWND(20 as *mut core::ffi::c_void)];
        let state = AppliedView::new(spec, &hosts, &[WindowToken(30), WindowToken(10)]);
        let reordered = AppliedView::new(spec, &hosts, &[WindowToken(10), WindowToken(30)]);
        assert_eq!(state.excluded, vec![10, 20, 30]);
        assert!(!UpdatePlan::between(Some(&state), &reordered).changed());
        assert!(!refresh_due(Duration::from_millis(99)));
        assert!(refresh_due(Duration::from_millis(100)));
    }

    #[test]
    fn preview_commit_and_latest_source_require_two_refreshes() {
        let mut first = Visibility::new(true);
        let mut second = Visibility::new(false);
        assert_eq!(first, Visibility::Preview);
        assert_eq!(first.refreshed(), Visibility::Preview);
        first = first.synced(false, false);
        assert_eq!(first, Visibility::Warming(0));
        first = first.refreshed();
        assert_eq!(first, Visibility::Warming(1));
        first = first.synced(false, true);
        assert_eq!(first, Visibility::Warming(0));
        first = first.refreshed().refreshed();
        assert_eq!(first, Visibility::Visible);
        assert_eq!(first.synced(false, true), Visibility::Visible);
        assert_eq!(second.refreshed(), Visibility::Warming(1));
        second = second.refreshed().refreshed();
        assert_eq!(second, Visibility::Visible);
    }

    #[test]
    fn recreating_after_cancel_or_close_starts_concealed() {
        let visible = Visibility::new(false).refreshed().refreshed();
        assert_eq!(visible, Visibility::Visible);
        assert_eq!(visible.synced(true, false), Visibility::Preview);
        assert_eq!(Visibility::new(true), Visibility::Preview);
        assert_eq!(Visibility::new(false), Visibility::Warming(0));
    }
}
