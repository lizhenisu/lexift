//! Native synthetic desktop and excluded feedback. Every HWND is created and destroyed
//! by its message-loop owner, independently of capture and encoding threads.
use lexift_core::domain::geometry::Rect;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::*,
};

pub struct Scene {
    pub dynamic: Arc<AtomicBool>,
    pub clicks: Arc<AtomicUsize>,
    pub wheel: Arc<AtomicUsize>,
    pub drag: Arc<AtomicUsize>,
    pub action: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    pub windows: serde_json::Value,
}
struct Owned(
    HWND,
    #[expect(dead_code, reason = "RAII counter retires after DestroyWindow")]
    super::resources::Lease,
);
unsafe extern "system" fn fixture_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const AtomicUsize;
        if !state.is_null() && message == WM_LBUTTONUP {
            let x = (l.0 as u32 & 0xffff) as i16 as i32;
            if (0..400).contains(&x) {
                (*state).store((x / 100 + 1) as usize, Ordering::Release);
                return LRESULT(0);
            }
        }
        if !state.is_null() && message == WM_COMMAND && ((w.0 >> 16) & 0xffff) != 0 {
            return LRESULT(0);
        }
        if !state.is_null() && (message == WM_COMMAND || message == WM_CLOSE) {
            let action = if message == WM_CLOSE { 4 } else { w.0 & 0xffff };
            if (1..=4).contains(&action) {
                (*state).store(action, Ordering::Release);
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn color(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}
unsafe fn fill(dc: HDC, rect: RECT, rgb: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(rgb);
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
}
fn window(r: RECT, overlay: bool, caption: PCWSTR) -> Result<Owned> {
    unsafe {
        let h = CreateWindowExW(
            (if overlay {
                WS_EX_TOPMOST
            } else {
                WINDOW_EX_STYLE(0)
            }) | (if overlay {
                WS_EX_NOACTIVATE
            } else {
                WINDOW_EX_STYLE(0)
            }) | WS_EX_TOOLWINDOW
                | if overlay {
                    WS_EX_LAYERED | WS_EX_TRANSPARENT
                } else {
                    WINDOW_EX_STYLE(0)
                },
            w!("LexiftRecordingGateFixture"),
            caption,
            WS_POPUP,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            None,
            None,
        )?;
        let owned = Owned(h, super::resources::Lease::new(&super::resources::WINDOWS));
        super::super::window_shell_policy::install(
            h,
            Some(if overlay {
                super::super::window_shell_policy::Activation::Passive
            } else {
                super::super::window_shell_policy::Activation::Interactive
            }),
        )
        .map_err(|e| Error::new(E_FAIL, e.to_string()))?;
        if overlay {
            SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA)?;
            SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE)?;
        }
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        Ok(owned)
    }
}

impl Scene {
    /// Real input through the excluded hint must reach the underlying source.
    pub fn check_input(&self, roi: Rect) {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        unsafe {
            let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let point = POINT {
                x: roi.left + 650,
                y: roi.top + 220,
            };
            assert_eq!(
                WindowFromPoint(point).0 as usize,
                self.windows["source"].as_u64().unwrap() as usize
            );
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor).unwrap();
            let foreground = HWND(self.windows["source"].as_u64().unwrap() as *mut _);
            SetCursorPos(point.x, point.y).unwrap();
            let input = |flags| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            assert_eq!(
                SendInput(
                    &[input(MOUSEEVENTF_LEFTDOWN), input(MOUSEEVENTF_LEFTUP)],
                    std::mem::size_of::<INPUT>() as i32
                ),
                2
            );
            let deadline = Instant::now() + Duration::from_secs(1);
            while self.clicks.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(
                self.clicks.load(Ordering::Acquire) > 0,
                "Excluded hint blocked source input"
            );
            let move_event = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 8,
                        dy: 0,
                        dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_MOVE_NOCOALESCE,
                        ..Default::default()
                    },
                },
            };
            let wheel_event = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        mouseData: 120,
                        dwFlags: MOUSEEVENTF_WHEEL,
                        ..Default::default()
                    },
                },
            };
            assert_eq!(
                SendInput(
                    &[
                        input(MOUSEEVENTF_LEFTDOWN),
                        move_event,
                        input(MOUSEEVENTF_LEFTUP),
                        wheel_event
                    ],
                    std::mem::size_of::<INPUT>() as i32
                ),
                4
            );
            let deadline = Instant::now() + Duration::from_secs(1);
            while (self.wheel.load(Ordering::Acquire) == 0
                || self.drag.load(Ordering::Acquire) == 0)
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(
                self.wheel.load(Ordering::Acquire) > 0,
                "Feedback blocked wheel input"
            );
            assert!(
                self.drag.load(Ordering::Acquire) > 0,
                "Feedback blocked drag input"
            );
            assert_eq!(
                GetForegroundWindow(),
                foreground,
                "Feedback changed the source foreground after explicit input"
            );
            SetCursorPos(cursor.x, cursor.y).unwrap();
            SetThreadDpiAwarenessContext(old);
        }
    }
    pub fn start(roi: Rect, inside: bool) -> Self {
        let initial_foreground = unsafe { GetForegroundWindow() };
        let wheel = Arc::new(AtomicUsize::new(0));
        let wheel_count = wheel.clone();
        let drag = Arc::new(AtomicUsize::new(0));
        let drag_count = drag.clone();
        let dynamic = Arc::new(AtomicBool::new(true));
        let action = Arc::new(AtomicUsize::new(0));
        let commands = action.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let clicks = Arc::new(AtomicUsize::new(0));
        let d = dynamic.clone();
        let s = stop.clone();
        let c = clicks.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::spawn(move || unsafe {
            let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // STATIC repaints its background after direct GDI drawing and invalidates
            // the known-pixel oracle. A class without a background brush retains it.
            let class = WNDCLASSW {
                lpfnWndProc: Some(fixture_proc),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
                lpszClassName: w!("LexiftRecordingGateFixture"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let result = (|| -> Result<_> {
                let width = roi.right - roi.left;
                let height = roi.bottom - roi.top;
                let source = window(
                    RECT {
                        left: roi.left,
                        top: roi.top,
                        right: roi.right,
                        bottom: roi.bottom,
                    },
                    false,
                    w!("Lexift recording gate synthetic source"),
                )?;
                let control_y = if inside {
                    roi.top + 200
                } else {
                    roi.bottom + 8
                };
                let monitor = MonitorFromPoint(
                    POINT {
                        x: roi.left,
                        y: roi.top,
                    },
                    MONITOR_DEFAULTTONEAREST,
                );
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                GetMonitorInfoW(monitor, &mut info).ok()?;
                let control_y = control_y.clamp(info.rcWork.top, info.rcWork.bottom - 40);
                let control = window(
                    RECT {
                        left: roi.left + 100,
                        top: control_y,
                        right: roi.left + 500,
                        bottom: control_y + 40,
                    },
                    true,
                    w!("Recording gate | pause / stop / cancel"),
                )?;
                SetWindowLongPtrW(control.0, GWLP_USERDATA, Arc::as_ptr(&commands) as isize);
                let style = GetWindowLongPtrW(control.0, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    control.0,
                    GWL_EXSTYLE,
                    style & !(WS_EX_TRANSPARENT.0 as isize),
                );
                for (index, label) in [w!("Pause"), w!("Resume"), w!("Stop"), w!("Cancel")]
                    .into_iter()
                    .enumerate()
                {
                    CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("BUTTON"),
                        label,
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                        index as i32 * 100,
                        0,
                        100,
                        40,
                        Some(control.0),
                        Some(HMENU((index + 1) as *mut _)),
                        None,
                        None,
                    )?;
                }
                let hint = window(
                    RECT {
                        left: roi.left + 550,
                        top: roi.top + 200,
                        right: roi.left + 850,
                        bottom: roi.top + 240,
                    },
                    true,
                    w!("Excluded passive recording hint"),
                )?;
                let mut borders = Vec::new();
                for r in [
                    RECT {
                        left: roi.left,
                        top: roi.top,
                        right: roi.right,
                        bottom: roi.top + 3,
                    },
                    RECT {
                        left: roi.left,
                        top: roi.bottom - 3,
                        right: roi.right,
                        bottom: roi.bottom,
                    },
                    RECT {
                        left: roi.left,
                        top: roi.top,
                        right: roi.left + 3,
                        bottom: roi.bottom,
                    },
                    RECT {
                        left: roi.right - 3,
                        top: roi.top,
                        right: roi.right,
                        bottom: roi.bottom,
                    },
                ] {
                    borders.push(window(r, true, w!("Recording range"))?);
                }
                let dc = GetDC(Some(source.0));
                fill(
                    dc,
                    RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    },
                    color(32, 52, 76),
                );
                for (i, rgb) in [color(220, 50, 70), color(45, 180, 90), color(50, 100, 220)]
                    .into_iter()
                    .enumerate()
                {
                    fill(
                        dc,
                        RECT {
                            left: 100 + i as i32 * 250,
                            top: 350,
                            right: 300 + i as i32 * 250,
                            bottom: 550,
                        },
                        rgb,
                    );
                }
                SetTextColor(dc, color(245, 245, 245));
                SetBkMode(dc, TRANSPARENT);
                for row in 0..12 {
                    let text: Vec<u16> = format!("ROW {row:03}  SYNTHETIC RECORDING / 123 ABC")
                        .encode_utf16()
                        .collect();
                    TextOutW(dc, 64, 600 + row * 28, &text).ok()?;
                }
                ReleaseDC(Some(source.0), dc);
                for h in [&control, &hint] {
                    let dc = GetDC(Some(h.0));
                    fill(
                        dc,
                        RECT {
                            left: 0,
                            top: 0,
                            right: 400,
                            bottom: 40,
                        },
                        color(255, 0, 255),
                    );
                    ReleaseDC(Some(h.0), dc);
                }
                for h in &borders {
                    let dc = GetDC(Some(h.0));
                    fill(
                        dc,
                        RECT {
                            left: 0,
                            top: 0,
                            right: width,
                            bottom: height,
                        },
                        color(0, 255, 255),
                    );
                    ReleaseDC(Some(h.0), dc);
                }
                let handles = serde_json::json!({"source":source.0.0 as usize,"control":control.0.0 as usize,"hint":hint.0.0 as usize,"borders":borders.iter().map(|b|b.0.0 as usize).collect::<Vec<_>>(),"dpi":GetDpiForWindow(source.0),"inside":inside,"bounds":[roi.left,roi.top,roi.right,roi.bottom],"feedback_noactivate":true,"feedback_capture_excluded":true});
                tx.send(Ok(handles)).unwrap();
                let start = Instant::now();
                let mut last = u32::MAX;
                while !s.load(Ordering::Acquire) {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        if msg.message == WM_MOUSEWHEEL && msg.hwnd == source.0 {
                            wheel_count.fetch_add(1, Ordering::Relaxed);
                        }
                        if msg.message == WM_MOUSEMOVE
                            && msg.hwnd == source.0
                            && msg.wParam.0 & 1 != 0
                        {
                            drag_count.fetch_add(1, Ordering::Relaxed);
                        }
                        if msg.message == WM_LBUTTONDOWN && msg.hwnd == source.0 {
                            c.fetch_add(1, Ordering::Relaxed);
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let counter = if d.load(Ordering::Relaxed) {
                        ((start.elapsed().as_nanos() * 60 / 1_000_000_000) % 65535) as u32 + 1
                    } else {
                        0
                    };
                    if counter != last {
                        let dc = GetDC(Some(source.0));
                        for bit in 0..16 {
                            let v = if counter & (1 << bit) != 0 { 255 } else { 0 };
                            fill(
                                dc,
                                RECT {
                                    left: 64 + bit * 32,
                                    top: 64,
                                    right: 96 + bit * 32,
                                    bottom: 96,
                                },
                                color(v, v, v),
                            );
                        }
                        ReleaseDC(Some(source.0), dc);
                        last = counter;
                    }
                    thread::sleep(Duration::from_millis(4));
                }
                drop(borders);
                drop(hint);
                drop(control);
                drop(source);
                Ok(())
            })();
            if let Err(e) = result {
                let _ = tx.send(Err(e.to_string()));
            }
            SetThreadDpiAwarenessContext(old);
        });
        let windows = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("fixture ready")
            .expect("fixture setup");
        assert_eq!(
            unsafe { GetForegroundWindow() },
            initial_foreground,
            "Hidden preparation stole foreground"
        );
        Self {
            wheel,
            drag,
            action,
            dynamic,
            clicks,
            stop,
            thread: Some(thread),
            windows,
        }
    }
}
impl Drop for Scene {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

pub fn marker(bgra: &[u8], w: usize) -> u32 {
    (0..16).fold(0, |id, bit| {
        id | ((u32::from(bgra[(80 * w + 80 + bit * 32) * 4] > 128)) << bit)
    })
}

/// Owned native button input; uses real pointer events, never foreground forcing.
pub fn click_control(handle: usize, button: usize) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    unsafe {
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = (|| -> Result<()> {
            let hwnd = HWND(handle as *mut _);
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect)?;
            let point = POINT {
                x: rect.left + (button as i32 - 1) * 100 + 50,
                y: rect.top + 20,
            };
            let hit = WindowFromPoint(point);
            if hit != hwnd && GetAncestor(hit, GA_ROOT) != hwnd {
                return Err(Error::new(
                    E_FAIL,
                    format!(
                        "Control input hit another window: control={:?}; hit={:?}; bounds={rect:?}; point={point:?}",
                        hwnd, hit
                    ),
                ));
            }
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor)?;
            SetCursorPos(point.x, point.y)?;
            let event = |flags| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            assert_eq!(
                SendInput(
                    &[event(MOUSEEVENTF_LEFTDOWN), event(MOUSEEVENTF_LEFTUP)],
                    std::mem::size_of::<INPUT>() as i32
                ),
                2
            );
            SetCursorPos(cursor.x, cursor.y)?;
            Ok(())
        })();
        SetThreadDpiAwarenessContext(old);
        result
    }
}

/// Restore the user's pointer after the dedicated cursor fixture.
pub struct Pointer {
    original: POINT,
    last: u64,
    dpi: DPI_AWARENESS_CONTEXT,
}
impl Pointer {
    pub fn new() -> Self {
        let dpi =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let mut original = POINT::default();
        unsafe {
            GetCursorPos(&mut original).unwrap();
        }
        Self {
            original,
            last: u64::MAX,
            dpi,
        }
    }
    pub fn tick(&mut self, roi: Rect, ms: u64) {
        let index = ms / 200;
        if index != self.last {
            unsafe {
                SetCursorPos(roi.left + 1100 + (index % 2) as i32 * 80, roi.top + 800).unwrap();
            }
            self.last = index;
        }
    }
}
impl Drop for Pointer {
    fn drop(&mut self) {
        let _ = unsafe { SetCursorPos(self.original.x, self.original.y) };
        unsafe {
            SetThreadDpiAwarenessContext(self.dpi);
        }
    }
}
