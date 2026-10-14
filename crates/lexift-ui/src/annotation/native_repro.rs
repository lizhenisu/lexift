//! Debug-only complete native screenshot roundtrip; uses production Core/App dispatch.
use super::*;
use lexift_core::domain::screenshot::{ScreenshotOutput, ScreenshotPhase};
use std::cell::Cell;
type Probe = Rc<dyn Fn(&slint::Window, &str, bool) -> bool>;
struct Runner {
    timer: slint::Timer,
    probe: Probe,
    result: Rc<Cell<bool>>,
    step: u8,
    started: Instant,
    after: Instant,
    display: Rect,
    old: u64,
    new: u64,
    before: slint::PhysicalPosition,
}
thread_local! {static RUNNER:RefCell<Option<Runner>>=const{RefCell::new(None)};}
pub(crate) fn start(probe: Probe) -> Rc<Cell<bool>> {
    let result = Rc::new(Cell::new(true));
    let now = Instant::now();
    let runner = Runner {
        timer: slint::Timer::default(),
        probe,
        result: result.clone(),
        step: 0,
        started: now,
        after: now,
        display: Rect::default(),
        old: 0,
        new: 0,
        before: slint::PhysicalPosition::new(0, 0),
    };
    runner
        .timer
        .start(slint::TimerMode::Repeated, Duration::from_millis(50), tick);
    RUNNER.with(|s| *s.borrow_mut() = Some(runner));
    result
}
fn ready() -> bool {
    REGISTRY.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|r| r.canvas_state.annotation_ready)
    })
}
fn draw(group: usize, tool: i32, from: (f32, f32), to: (f32, f32)) {
    REGISTRY.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.as_mut().unwrap();
        assert!(r.select_tool_variant(group, tool));
    });
    let scale = REGISTRY.with(|s| {
        s.borrow().as_ref().unwrap().canvas_state.canvases[0]
            .window
            .window()
            .scale_factor()
    });
    for (event, p) in [(0, from), (1, to), (2, to)] {
        canvas_pointer(0, event, p.0 / scale, p.1 / scale);
    }
}
fn tick() {
    let Some(mut runner) = RUNNER.with(|s| s.borrow_mut().take()) else {
        return;
    };
    let done = runner.advance();
    if done {
        runner.timer.stop();
        println!("PIN_REPRO_RESULT success={}", runner.result.get());
        let h = EXPORT_HANDLER.with(|h| h.borrow().clone());
        if let Some(h) = h {
            h(lexift_core::AppEvent::ExitRequested);
        }
    } else {
        RUNNER.with(|s| *s.borrow_mut() = Some(runner));
    }
}
impl Runner {
    fn check_moved(&self, id: u64) {
        let windows = crate::pinned_screenshot::diagnostic_windows();
        let after = windows
            .iter()
            .find(|(n, _)| *n == id)
            .unwrap()
            .1
            .window()
            .position();
        let moved = (after.x - self.before.x).abs() >= 20 && (after.y - self.before.y).abs() >= 20;
        println!(
            "PIN_REAL_DRAG id={id} before={:?} after={after:?} moved={moved}",
            self.before
        );
        self.result.set(self.result.get() && moved);
    }
    fn next(&mut self) {
        self.step += 1;
        self.after = Instant::now();
        println!("PIN_REPRO_STEP {}", self.step);
    }
    fn probe(&self, id: u64, label: &str, click: bool) {
        let windows = crate::pinned_screenshot::diagnostic_windows();
        if let Some((_, window)) = windows.iter().find(|(n, _)| *n == id) {
            if label == "old-after-new-moved" {
                save_frame(window.window(), label);
            }
            let ok = (self.probe)(window.window(), label, click);
            self.result.set(self.result.get() && ok);
        } else {
            println!("PIN_REPRO_MISSING {label}");
            self.result.set(false);
        }
    }
    fn select(&self, second: bool) {
        let x = if second { 750. } else { 80. };
        let y = 80.;
        let state = screenshot::diagnostic_state();
        let b = self.display;
        for input in [
            PopupPointerInput::LeftPressed { x, y },
            PopupPointerInput::Moved {
                x: x + 480.,
                y: y + 360.,
            },
            PopupPointerInput::LeftReleased {
                x: x + 480.,
                y: y + 360.,
            },
        ] {
            crate::screen_interaction::selection::pointer(state.session, b, 1., input);
        }
    }
    fn advance(&mut self) -> bool {
        if self.started.elapsed() > Duration::from_secs(70) {
            println!("PIN_REPRO_TIMEOUT step={}", self.step);
            self.result.set(false);
            return true;
        }
        match self.step {
            0 => {
                if !super::is_open() {
                    toggle(Instant::now());
                    return false;
                }
                if !ready() {
                    return false;
                }
                self.display =
                    REGISTRY.with(|s| s.borrow().as_ref().unwrap().canvas_state.canvases[0].bounds);
                REGISTRY.with(|s| {
                    let s = s.borrow();
                    let r = s.as_ref().unwrap();
                    if let Some(w) = r.toolbar_state.main.get() {
                        w.window().set_position(slint::PhysicalPosition::new(
                            self.display.left + 100,
                            self.display.bottom - 80,
                        ));
                    }
                });
                screenshot::request();
                self.next();
            }
            1 => {
                if crate::screen_interaction::selection::diagnostic_visible() {
                    self.select(false);
                    self.next();
                }
            }
            2 => {
                if ready() && REGISTRY.with(|s| s.borrow().as_ref().unwrap().screenshot.is_some()) {
                    draw(0, 0, (120., 120.), (220., 200.));
                    draw(0, 2, (260., 140.), (440., 300.));
                    draw(5, 11, (160., 260.), (260., 360.));
                    self.next();
                }
            }
            3 => {
                if self.after.elapsed() > Duration::from_millis(800) {
                    screenshot::output(ScreenshotOutput::Pin);
                    self.next();
                }
            }
            4 => {
                if ready() && screenshot::diagnostic_state().phase == ScreenshotPhase::Idle {
                    self.old = crate::pinned_screenshot::diagnostic_windows()
                        .first()
                        .unwrap()
                        .0;
                    self.probe(self.old, "first-restored", false);
                    draw(0, 0, (800., 150.), (1000., 330.));
                    self.next();
                }
            }
            5 => {
                if self.after.elapsed() > Duration::from_millis(800) {
                    self.probe(self.old, "through-drawing", false);
                    screenshot::request();
                    self.next();
                }
            }
            6 => {
                if crate::screen_interaction::selection::diagnostic_visible() {
                    self.select(true);
                    self.next();
                }
            }
            7 => {
                if ready() && REGISTRY.with(|s| s.borrow().as_ref().unwrap().screenshot.is_some()) {
                    screenshot::output(ScreenshotOutput::Pin);
                    self.next();
                }
            }
            8 => {
                if ready() && screenshot::diagnostic_state().phase == ScreenshotPhase::Idle {
                    let windows = crate::pinned_screenshot::diagnostic_windows();
                    self.new = windows.last().unwrap().0;
                    self.before = windows.last().unwrap().1.window().position();
                    self.probe(self.new, "drag-new", false);
                    self.next();
                }
            }
            9 => {
                if self.after.elapsed() > Duration::from_millis(900) {
                    self.check_moved(self.new);
                    self.probe(self.old, "old-after-new-moved", false);
                    self.before = crate::pinned_screenshot::diagnostic_windows()
                        .iter()
                        .find(|(id, _)| *id == self.old)
                        .unwrap()
                        .1
                        .window()
                        .position();
                    self.probe(self.old, "drag-old", false);
                    self.next();
                }
            }
            10 => {
                if self.after.elapsed() > Duration::from_millis(900) {
                    self.check_moved(self.old);
                    self.probe(self.new, "menu-escape", false);
                    self.next();
                }
            }
            11 if self.after.elapsed() > Duration::from_millis(900) => {
                self.probe(self.old, "menu-annotate", false);
                self.next();
            }
            12 if ready() && pin_editor::current().is_some_and(|p| p.0 == self.old) => {
                let initial = REGISTRY.with(|s| {
                    let s = s.borrow();
                    let r = s.as_ref().unwrap();
                    r.document.session.objects.len() == 3
                        && r.document.session.can_undo()
                        && r.toolbar_state.main.get().unwrap().get_pin_edit_mode()
                        && r.canvas_state.canvases.len() == 1
                });
                println!("PIN_EDIT_IMPORTED initial={initial}");
                self.result.set(self.result.get() && initial);
                REGISTRY.with(|s| {
                    s.borrow_mut().as_mut().unwrap().select_tool_variant(0, 0);
                });
                self.probe(self.old, "draw-pin", false);
                self.next();
            }
            12 if self.after.elapsed() > Duration::from_secs(5) => {
                println!(
                    "PIN_EDIT_BOOTSTRAP_TIMEOUT active={:?} ready={} open={}",
                    pin_editor::current(),
                    ready(),
                    is_open()
                );
                self.result.set(false);
                return true;
            }
            13 if self.after.elapsed() > Duration::from_millis(1000) => {
                let changed = REGISTRY
                    .with(|s| s.borrow().as_ref().unwrap().document.session.objects.len() == 4);
                println!("PIN_EDIT_REAL_DRAW changed={changed}");
                self.result.set(self.result.get() && changed);
                undo();
                let undone = REGISTRY
                    .with(|s| s.borrow().as_ref().unwrap().document.session.objects.len() == 3);
                self.result.set(self.result.get() && undone);
                pin_editor::finish_now();
                self.next();
            }
            14 if ready() && !pin_editor::active() => {
                let restored = REGISTRY.with(|s| {
                    let s = s.borrow();
                    let r = s.as_ref().unwrap();
                    r.document.mode.is_mouse() && r.document.session.objects.len() == 1
                });
                println!("PIN_EDIT_DESKTOP_RESTORED restored={restored}");
                self.result.set(self.result.get() && restored);
                self.probe(self.old, "menu-hide", false);
                self.next();
            }
            15 if self.after.elapsed() > Duration::from_millis(900) => {
                let windows = crate::pinned_screenshot::diagnostic_windows();
                let w = &windows.iter().find(|p| p.0 == self.old).unwrap().1;
                let hidden = w.get_annotations_hidden();
                println!("PIN_HIDE hidden={hidden}");
                self.result.set(self.result.get() && hidden);
                save_frame(w.window(), "pin-hidden");
                self.probe(self.old, "menu-hide", false);
                self.next();
            }
            16 if self.after.elapsed() > Duration::from_millis(900) => {
                let windows = crate::pinned_screenshot::diagnostic_windows();
                let w = &windows.iter().find(|p| p.0 == self.old).unwrap().1;
                let shown = !w.get_annotations_hidden();
                println!("PIN_SHOW shown={shown}");
                self.result.set(self.result.get() && shown);
                save_frame(w.window(), "pin-shown");
                self.probe(self.old, "menu-annotate", false);
                self.next();
            }
            17 if ready() && pin_editor::current().is_some_and(|p| p.0 == self.old) => {
                let history = REGISTRY.with(|s| {
                    let s = s.borrow();
                    let r = s.as_ref().unwrap();
                    r.document.session.objects.len() == 3 && r.document.session.can_undo()
                });
                undo();
                let undone = REGISTRY
                    .with(|s| s.borrow().as_ref().unwrap().document.session.objects.len() == 2);
                println!("PIN_EDIT_REOPEN history={history} undo={undone}");
                self.result.set(self.result.get() && history && undone);
                pin_editor::finish_now();
                self.next();
            }
            18 if ready() && !pin_editor::active() => {
                self.probe(self.new, "menu-escape", false);
                self.next();
            }
            19 => {
                if self.after.elapsed() > Duration::from_millis(900) {
                    if crate::pinned_screenshot::diagnostic_windows().len() != 2 {
                        println!("PIN_MENU_ESCAPE_FAILED");
                        self.result.set(false);
                        return true;
                    }
                    self.probe(self.new, "menu-shadow", false);
                    self.next();
                }
            }
            20 if self.after.elapsed() > Duration::from_millis(1100) => {
                let windows = crate::pinned_screenshot::diagnostic_windows();
                let w = &windows.iter().find(|(id, _)| *id == self.new).unwrap().1;
                let ok = !w.get_shadow_enabled();
                println!("PIN_MENU_SHADOW disabled={ok}");
                self.result.set(self.result.get() && ok);
                save_frame(w.window(), "shadow-off");
                self.probe(self.new, "menu-topmost", false);
                self.next();
            }
            21 if self.after.elapsed() > Duration::from_millis(1100) => {
                let windows = crate::pinned_screenshot::diagnostic_windows();
                let w = &windows.iter().find(|(id, _)| *id == self.new).unwrap().1;
                let ok = !w.get_topmost();
                println!("PIN_MENU_TOPMOST disabled={ok}");
                self.result.set(self.result.get() && ok);
                self.probe(self.new, "not-topmost", false);
                w.set_topmost(true);
                w.invoke_option_requested(1, true);
                self.probe(self.new, "close-new", true);
                self.next();
            }
            22 if self.after.elapsed() > Duration::from_millis(900) => {
                self.probe(self.old, "close-old-after-new-closed", true);
                self.next();
            }
            23 if self.after.elapsed() > Duration::from_millis(900) => {
                let remaining = crate::pinned_screenshot::diagnostic_windows().len();
                println!("PIN_REPRO_REMAINING {remaining}");
                if remaining != 0 {
                    self.result.set(false);
                }
                return true;
            }
            _ => {}
        }
        false
    }
}

fn save_frame(window: &slint::Window, label: &str) {
    let Ok(image) = window.take_snapshot() else {
        return;
    };
    let (w, h) = (image.width(), image.height());
    let row = (w * 3).div_ceil(4) * 4;
    let mut bmp = vec![0u8; (54 + row * h) as usize];
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&(54 + row * h).to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&(w as i32).to_le_bytes());
    bmp[22..26].copy_from_slice(&(-(h as i32)).to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    for (i, p) in image.as_slice().iter().enumerate() {
        let at = 54 + (i / w as usize) * row as usize + (i % w as usize) * 3;
        bmp[at..at + 3].copy_from_slice(&[p.b, p.g, p.r]);
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/multi-pin");
    let _ = std::fs::create_dir_all(&directory);
    let _ = std::fs::write(directory.join(format!("native-{label}.bmp")), bmp);
}
