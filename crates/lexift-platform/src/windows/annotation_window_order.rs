//! Session-window ordering without activation or per-frame restacking.

use std::{
    cell::Cell,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{Result, w},
};

const SUBCLASS: usize = 0x4c584f52;
const RECONCILE: u32 = WM_APP + 0x51;
const LOWER_VISUAL: u32 = WM_APP + 0x52;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Canvas,
    Tool { owner: usize },
    Visual,
}
#[derive(Clone, Copy)]
struct Entry {
    hwnd: usize,
    id: usize,
    role: Role,
    pending: bool,
}
impl Entry {
    fn window(self) -> HWND {
        HWND(self.hwnd as _)
    }
}

fn registry() -> &'static Mutex<Vec<Entry>> {
    static REGISTRY: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}
thread_local! { static REORDERING: Cell<bool> = const { Cell::new(false) }; }

fn alive(entry: Entry) -> bool {
    unsafe { GetPropW(entry.window(), w!("Lexift.AnnotationOrder")).0 as usize == entry.id }
}
fn snapshot() -> Vec<Entry> {
    let entries = registry().lock().unwrap().clone();
    entries.into_iter().filter(|entry| alive(*entry)).collect()
}

fn register(hwnd: HWND, role: Role) -> Result<()> {
    let existing = snapshot().into_iter().find(|entry| entry.window() == hwnd);
    if let Some(existing) = existing {
        if existing.role != role
            && let Some(entry) = registry()
                .lock()
                .unwrap()
                .iter_mut()
                .find(|entry| entry.id == existing.id)
        {
            entry.role = role;
        }
        request();
        return Ok(());
    }
    if matches!(role, Role::Tool { .. }) {
        // DWM's show transition reveals live annotations through an otherwise opaque menu.
        // Keep this window-local; do not change the user's system animation preferences.
        let disabled = 1_i32; // Win32 BOOL
        if let Err(error) = unsafe {
            windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_TRANSITIONS_FORCEDISABLED,
                &disabled as *const _ as _,
                std::mem::size_of_val(&disabled) as u32,
            )
        } {
            tracing::debug!(%error, "annotation window transitions could not be disabled");
        }
    }
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    unsafe {
        SetPropW(hwnd, w!("Lexift.AnnotationOrder"), Some(HANDLE(id as _)))?;
        if !SetWindowSubclass(hwnd, Some(subclass), SUBCLASS, id).as_bool() {
            let _ = RemovePropW(hwnd, w!("Lexift.AnnotationOrder"));
            return Err(windows::core::Error::from_thread());
        }
    }
    registry().lock().unwrap().push(Entry {
        hwnd: hwnd.0 as usize,
        id,
        role,
        pending: false,
    });
    request();
    Ok(())
}

pub(super) fn canvas(hwnd: HWND) -> Result<()> {
    register(hwnd, Role::Canvas)?;
    super::annotation_overlay::mark(hwnd)
}
pub(super) fn visual(hwnd: HWND) -> Result<()> {
    register(hwnd, Role::Visual)
}

/// Generic popup owners are untouched; only descendants of registered annotation windows participate.
pub(super) fn tool(hwnd: HWND, owner: HWND) -> Result<()> {
    if let Some(owner) = snapshot()
        .into_iter()
        .find(|entry| entry.window() == owner && !matches!(entry.role, Role::Visual))
    {
        register(hwnd, Role::Tool { owner: owner.id })?;
        // One-shot capture freezes its explicit HWND list. A menu/canvas created
        // after that request must still be excluded before its first show.
        super::annotation_overlay::mark(hwnd)?;
    }
    Ok(())
}

fn post(entry: Entry, message: u32) {
    {
        let mut entries = registry().lock().unwrap();
        let Some(current) = entries.iter_mut().find(|current| current.id == entry.id) else {
            return;
        };
        if current.pending {
            return;
        }
        current.pending = true;
    }
    if unsafe { PostMessageW(Some(entry.window()), message, WPARAM(entry.id), LPARAM(0)) }.is_err()
        && let Some(current) = registry()
            .lock()
            .unwrap()
            .iter_mut()
            .find(|current| current.id == entry.id)
    {
        current.pending = false;
    }
}

fn request() {
    if REORDERING.get() {
        return;
    }
    if let Some(canvas) = snapshot()
        .into_iter()
        .find(|entry| entry.role == Role::Canvas)
    {
        post(canvas, RECONCILE);
    }
}

/// Top-to-bottom native order; no process-wide hooks or enumeration on pointer/frame hot paths.
fn ordered(entries: &[Entry]) -> Vec<Entry> {
    let mut ordered = Vec::new();
    let mut current = unsafe { GetTopWindow(None) }.unwrap_or_default();
    while !current.0.is_null() {
        if let Some(entry) = entries.iter().find(|entry| entry.window() == current)
            && unsafe { IsWindowVisible(current) }.as_bool()
        {
            ordered.push(*entry);
        }
        current = unsafe { GetWindow(current, GW_HWNDNEXT) }.unwrap_or_default();
    }
    ordered
}

fn rank(entry: Entry, entries: &[Entry]) -> usize {
    let mut role = entry.role;
    let mut rank = 1;
    for _ in 0..entries.len() {
        match role {
            Role::Visual => return 0,
            Role::Canvas => return rank,
            Role::Tool { owner } => {
                rank += 1;
                let Some(parent) = entries.iter().find(|parent| parent.id == owner) else {
                    return rank;
                };
                role = parent.role;
            }
        }
    }
    rank
}

fn valid_order(ordered: &[Entry], entries: &[Entry]) -> bool {
    ordered
        .windows(2)
        .all(|pair| rank(pair[0], entries) >= rank(pair[1], entries))
}

fn reconcile() {
    let entries = snapshot();
    let order = ordered(&entries);
    if valid_order(&order, &entries) {
        return;
    }
    REORDERING.set(true);
    let mut tools: Vec<_> = order
        .iter()
        .copied()
        .filter(|entry| matches!(entry.role, Role::Tool { .. }))
        .collect();
    tools.sort_by_key(|entry| (rank(*entry, &entries), entry.id));
    for tool in tools {
        // UI controls are moved on their owning thread; visuals receive their own bounded message.
        if unsafe { GetWindowThreadProcessId(tool.window(), None) }
            == unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
        {
            let result = unsafe {
                SetWindowPos(
                    tool.window(),
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOMOVE | SWP_NOSIZE,
                )
            };
            if let Err(error) = result {
                tracing::warn!(%error, "could not restore annotation tool order");
            }
        }
    }
    REORDERING.set(false);
    let order = ordered(&entries);
    if let Some(last_canvas) = order.iter().rposition(|entry| entry.role == Role::Canvas) {
        for entry in &order[..last_canvas] {
            if entry.role == Role::Visual {
                post(*entry, LOWER_VISUAL);
            }
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    id: usize,
) -> LRESULT {
    // Enforce tier boundaries before User32 changes the order, avoiding a one-frame cover.
    if message == WM_WINDOWPOSCHANGING && lparam.0 != 0 {
        let position = unsafe { &mut *(lparam.0 as *mut WINDOWPOS) };
        if !position.flags.contains(SWP_NOZORDER) && !position.flags.contains(SWP_HIDEWINDOW) {
            let entries = snapshot();
            if let Some(entry) = entries.iter().find(|entry| entry.id == id) {
                let order = ordered(&entries);
                let anchor = match entry.role {
                    Role::Canvas => order
                        .iter()
                        .rfind(|entry| matches!(entry.role, Role::Tool { .. })),
                    Role::Visual => order.iter().rfind(|entry| entry.role == Role::Canvas),
                    Role::Tool { .. } => None,
                };
                if let Some(anchor) = anchor {
                    position.hwndInsertAfter = anchor.window();
                    position.flags |= SWP_NOOWNERZORDER | SWP_NOACTIVATE;
                }
            }
        }
    }
    if message == WM_NCDESTROY {
        registry().lock().unwrap().retain(|entry| entry.id != id);
        unsafe {
            let _ = RemovePropW(hwnd, w!("Lexift.AnnotationOrder"));
            let _ = RemoveWindowSubclass(hwnd, Some(subclass), subclass_id);
        }
        request();
    } else if matches!(message, RECONCILE | LOWER_VISUAL) {
        let entry = {
            let mut entries = registry().lock().unwrap();
            entries
                .iter_mut()
                .find(|entry| entry.id == id && wparam.0 == id)
                .map(|entry| {
                    entry.pending = false;
                    *entry
                })
        };
        if let Some(entry) = entry.filter(|entry| alive(*entry)) {
            if message == RECONCILE && entry.role == Role::Canvas {
                reconcile();
            } else if message == LOWER_VISUAL && entry.role == Role::Visual {
                let entries = snapshot();
                if let Some(canvas) = ordered(&entries)
                    .into_iter()
                    .rfind(|entry| entry.role == Role::Canvas)
                {
                    let result = unsafe {
                        SetWindowPos(
                            hwnd,
                            Some(canvas.window()),
                            0,
                            0,
                            0,
                            0,
                            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOMOVE | SWP_NOSIZE,
                        )
                    };
                    if let Err(error) = result {
                        tracing::warn!(%error, "could not restore annotation visual order");
                    }
                }
                request();
            }
        }
        return LRESULT(0);
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    let order_changed = message == WM_WINDOWPOSCHANGED
        && lparam.0 != 0
        && !unsafe { &*(lparam.0 as *const WINDOWPOS) }
            .flags
            .contains(SWP_NOZORDER);
    if order_changed || matches!(message, WM_ACTIVATE | WM_SHOWWINDOW) {
        request();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::popup;
    use super::{Entry, LOWER_VISUAL, RECONCILE, Role, ordered, snapshot, valid_order};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
    use std::{num::NonZeroIsize, time::Duration};
    use windows::{
        Win32::{
            Foundation::{COLORREF, HWND, POINT, RECT},
            Graphics::Gdi::*,
            UI::WindowsAndMessaging::*,
        },
        core::w,
    };

    struct Window(HWND);
    impl HasWindowHandle for Window {
        fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
            let handle = Win32WindowHandle::new(NonZeroIsize::new(self.0.0 as isize).unwrap());
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
        }
    }
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    fn window(x: i32, y: i32, width: i32, height: i32) -> Window {
        Window(unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Lexift ordering regression"),
                WS_POPUP | WINDOW_STYLE(0x100),
                x,
                y,
                width,
                height,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        })
    }
    fn pump() {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    fn paint(window: &Window, color: COLORREF) {
        unsafe {
            let dc = GetDC(Some(window.0));
            let brush = CreateSolidBrush(color);
            let mut rect = RECT::default();
            GetClientRect(window.0, &mut rect).unwrap();
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush.into());
            ReleaseDC(Some(window.0), dc);
        }
    }

    #[test]
    fn newly_registered_annotation_windows_are_excluded_before_first_show() {
        let canvas = window(0, 0, 40, 40);
        super::canvas(canvas.0).unwrap();
        let toolbar = window(0, 0, 40, 40);
        super::tool(toolbar.0, canvas.0).unwrap();
        let menu = window(0, 0, 40, 40);
        super::tool(menu.0, toolbar.0).unwrap();
        for window in [&canvas, &toolbar, &menu] {
            assert!(super::super::annotation_overlay::contains(window.0));
            assert!(!unsafe { IsWindowVisible(window.0) }.as_bool());
        }
        let normal_owner = window(0, 0, 40, 40);
        let normal_popup = window(0, 0, 40, 40);
        super::tool(normal_popup.0, normal_owner.0).unwrap();
        assert!(!super::super::annotation_overlay::contains(normal_popup.0));
        let retired = menu.0;
        drop(menu);
        assert!(!super::super::annotation_overlay::contains(retired));
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn rapid_menu_reopen_stays_above_other_monitor_canvas() {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let foreground = unsafe { GetForegroundWindow() };
        let canvas = window(100, 100, 300, 220);
        let other_canvas = window(100, 100, 300, 220);
        popup::set_click_through(&canvas, false).unwrap();
        popup::set_click_through(&other_canvas, false).unwrap();
        let toolbar = window(110, 110, 250, 40);
        popup::attach_owner(&toolbar, &canvas).unwrap();
        unsafe {
            let _ = ShowWindow(canvas.0, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(other_canvas.0, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(toolbar.0, SW_SHOWNOACTIVATE);
        }
        for iteration in 0..100 {
            let menu = window(120, 160, 180, 120);
            popup::attach_owner(&menu, &toolbar).unwrap();
            let visual = window(100, 100, 300, 220);
            super::visual(visual.0).unwrap();
            let old_visual_id = snapshot()
                .into_iter()
                .find(|entry| entry.window() == visual.0)
                .unwrap()
                .id;
            unsafe {
                let _ = ShowWindow(menu.0, SW_SHOWNOACTIVATE);
                let _ = ShowWindow(visual.0, SW_SHOWNOACTIVATE);
                SetWindowPos(
                    visual.0,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                )
                .unwrap();
                SetWindowPos(
                    other_canvas.0,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                )
                .unwrap();
            }
            pump();
            assert!(
                valid_order(&ordered(&snapshot()), &snapshot()),
                "invalid tier order at reopen {iteration}"
            );
            paint(&canvas, COLORREF(0x000000ff));
            paint(&other_canvas, COLORREF(0x00ff0000));
            paint(&menu, COLORREF(0x00ffffff));
            unsafe {
                let _ = GdiFlush();
                let _ = windows::Win32::Graphics::Dwm::DwmFlush();
            }
            std::thread::sleep(Duration::from_millis(16));
            pump();
            assert_eq!(
                unsafe { WindowFromPoint(POINT { x: 150, y: 190 }) },
                menu.0,
                "menu hit obscured at reopen {iteration}"
            );
            unsafe {
                let dc = GetDC(None);
                let color = GetPixel(dc, 150, 190);
                ReleaseDC(None, dc);
                assert_eq!(
                    color.0, 0x00ffffff,
                    "menu pixels obscured at reopen {iteration}"
                );
            }
            drop(visual);
            drop(menu);
            assert_eq!(snapshot().len(), 3, "destroyed windows remained registered");
            unsafe {
                PostMessageW(
                    Some(canvas.0),
                    RECONCILE,
                    windows::Win32::Foundation::WPARAM(old_visual_id),
                    windows::Win32::Foundation::LPARAM(0),
                )
                .unwrap();
                PostMessageW(
                    Some(toolbar.0),
                    LOWER_VISUAL,
                    windows::Win32::Foundation::WPARAM(old_visual_id),
                    windows::Win32::Foundation::LPARAM(0),
                )
                .unwrap();
            }
            pump();
        }
        assert_eq!(
            unsafe { GetForegroundWindow() },
            foreground,
            "ordering activated a window"
        );
    }

    #[test]
    fn tiers_follow_owner_depth_without_requiring_monitor_order() {
        let entries = [
            Entry {
                hwnd: 1,
                id: 1,
                role: Role::Canvas,
                pending: false,
            },
            Entry {
                hwnd: 2,
                id: 2,
                role: Role::Canvas,
                pending: false,
            },
            Entry {
                hwnd: 3,
                id: 3,
                role: Role::Visual,
                pending: false,
            },
            Entry {
                hwnd: 4,
                id: 4,
                role: Role::Tool { owner: 1 },
                pending: false,
            },
            Entry {
                hwnd: 5,
                id: 5,
                role: Role::Tool { owner: 4 },
                pending: false,
            },
        ];
        assert!(valid_order(
            &[entries[4], entries[3], entries[1], entries[0], entries[2]],
            &entries
        ));
        assert!(!valid_order(
            &[entries[1], entries[4], entries[3], entries[0], entries[2]],
            &entries
        ));
        assert!(!valid_order(
            &[entries[3], entries[4], entries[1], entries[0], entries[2]],
            &entries
        ));
    }
}
