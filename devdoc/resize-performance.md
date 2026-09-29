# Windows resize performance diagnostics

For the first Alt+A pointer stall and its fix, read [Windows mouse input isolation](mouse-input-isolation.md). The decisive change was moving `WH_MOUSE_LL` off the Slint UI thread; renderer and toolbar-ready timings alone do not measure system mouse responsiveness.

The Task Manager capture from an Intel integrated GPU showed Lexift at 82.3% of GPU 0's 3D engine while the CPU entry was 0.3%. The value returned to normal a few seconds after mouse release. This identifies interactive resizing as the workload to compare; it does not identify whether frame count, GPU work per frame, or driver presentation is dominant.

## Resize paint policy

The current build includes FemtoVG and Slint's software renderer. On Windows, startup selects software rendering by default. On a 125%/100% dual-monitor test machine, cold FemtoVG annotation toolbar construction took about 200 ms; software rendering removed that pause. `SLINT_BACKEND=winit-software` or `winit-femtovg` overrides the automatic choice before startup. A running window cannot switch renderers. Native move and resize remain under Windows/Winit control, with no app-side paint gate. The software path retains the opaque background-band fill and release-time paint repair described below; the FemtoVG path does not install them. The cross-DPI drag case on the software path still needs hardware validation. See `devdoc/translation-popup.md` for the current flow.

## Renderer comparison

For annotation cold open, the UI records one `First annotation open timing` line per process. It separates the event-loop queue, toolbar construction, monitor enumeration, per-monitor canvas construction/preparation/show, first raster, native `UpdateLayeredWindow` presentation, and toolbar show. On the 125%/100% two-screen test machine, FemtoVG took 443 and 466 ms end to end, including 200 and 199 ms of toolbar construction in one UI callback. Software rendering took 187 and 205 ms, with no measurable toolbar construction pause at millisecond resolution. The default software build took 191 ms with a largest measured stage of 16 ms. The remaining wall time includes intentional 16 ms yields between monitor stages. This timing does not by itself establish pointer frame continuity; a requested 120 fps desktop capture produced only about 87 fps on this machine, so pointer smoothness still needs a reliable high-frame-rate capture or direct user observation.

Run `powershell -ExecutionPolicy Bypass -File scripts/build-renderer-diagnostics.ps1` from the repository. It copies the current working tree into `target/renderer-diagnostics/workspace`, switches only that copy's Slint renderer feature, and builds independent Release executables under `target/renderer-diagnostics/{skia-opengl,femtovg,software}/lexift.exe`. Diagnostic builds bypass the automatic startup choice so each executable uses its single compiled renderer. The repository's manifests and existing uncommitted changes remain untouched.

## Annotation cold-open GPU comparison (2026-09-29)

The diagnostic build script now accepts `-SelectedVariants` to build only requested renderers. The UI creates at most one canvas or performs one canvas show/preparation or full blank-frame presentation per 16 ms bootstrap tick. A monitor awaiting native readiness does not block the other monitor. The toolbar appears only after both canvases have received their native first frame. The single cold-open log line includes toolbar construction, native preparation, `show()`, native-show completion, per-screen retries, and the longest whole UI callback. Cancelling invalidates queued ticks through the annotation session generation.

On the 125%/100% dual-monitor machine, ten fresh processes of each renderer were alternated with no concurrent compilation. The Skia OpenGL executable is also copied as `target/annotation-gpu-cold-start/lexift-gpu-skia.exe` for testing; the FemtoVG control is `target/annotation-gpu-cold-start/femtovg/lexift.exe`. All 20 first opens reached `ready`.

| Renderer | First-open readiness range | Longest UI stage | Longest whole callback | Toolbar construction peak | Toolbar `show()` peak |
|---|---:|---:|---:|---:|---:|
| Skia OpenGL | 467–503 ms | 30 ms | 34 ms | <1 ms | 25 ms |
| FemtoVG | 465–523 ms | 194 ms | 194 ms | 194 ms | 8 ms |

The raw per-run log lines and extracted figures are in `target/annotation-gpu-cold-start/cold-open-results.csv`. A 120 fps `gdigrab` request captured only 249 frames in 2.984 s (about 83.4 fps), so this run cannot prove uninterrupted pointer motion at 120 fps. Actual Skia screenshots in the same directory show the toolbar, a drawn rectangle on each monitor, and the toolbar after a quick cancel/reopen. The first cancelled session logged `cancelled`; its later scheduled ticks did not reopen it. The GPU executable is a comparison build; the main workspace Release build still defaults to software rendering.

## Mouse input isolation (2026-09-29)

The root cause, thread ownership rules, lifecycle, validation method, and regression checklist are documented in [Windows mouse input isolation](mouse-input-isolation.md). The user confirmed that the pointer feels smooth after this change.

The selection gesture `WH_MOUSE_LL` hook was installed before Slint's UI loop on the same thread that later created the annotation windows. Windows sends low-level hook callbacks to the installing thread, so the UI's synchronous renderer work could stall system mouse input. The selection hook now has its own message thread and sends gesture events to a separate dispatcher; popup outside-click and foreground hooks also use their own message thread and dispatcher, stopped when the last watched popup closes. No callback takes the Core state lock or performs UI work. Each hook logs its longest callback and dispatch queue delay once when stopped.

With an external process injecting a mouse movement every 5 ms during first Alt+A, ten fresh-process runs per build gave these maximum individual input-call times (raw runs: `target/annotation-gpu-cold-start/pointer-{baseline,new}-{femtovg,skia}.txt`):

| Renderer | Previous UI-thread hook | Dedicated hook thread |
|---|---:|---:|
| FemtoVG | 136–198 ms | 0.6–6.5 ms |
| Skia OpenGL | 9/10 runs: 157–217 ms; one run 0.7 ms | 1.6–5.4 ms |

The outlying Skia baseline run likely missed the blocking interval, so the per-run input measurement must be read alongside the `First annotation open timing` lines rather than interpreted as guaranteed behavior of every start. Desktop drag still dispatched selection start/completion; Alt+X started the popup hook, and an outside click stopped it (`max_callback_us=12`, `max_dispatch_delay_us=20` in that run). Both hook types passed two interactive stop/restart cycles. A requested 120 fps Desktop Duplication capture reached about 94 fps with CPU encoding and about 81 fps with NVENC while the cursor moved, so no 120 fps visual continuity claim is made. The input-call measurements show that the long input blockage observed with the old hook is absent in this controlled test.

On the Intel GPU test machine, quit all Lexift instances before each variant and run one executable at a time. Record CPU model, Intel and NVIDIA adapter names and driver versions, monitor resolution and scale, AC/battery state, and whether Windows assigned the process to GPU 0 or GPU 1. For each renderer, perform the same ten-second slow and fast resize of both popup and Settings at the same window sizes. Capture Task Manager's per-process CPU, GPU percentage and GPU engine during the drag, then after ten seconds idle; note visible lag, empty areas after release, and any appearance changes. Interactive resizing no longer writes a per-resize summary to `%LOCALAPPDATA%\Lexift\logs\lexift.YYYY-MM-DD.log`, so compare the sampler readings only. Use `SLINT_DEBUG_PERFORMANCE=refresh_lazy,overlay` only as an idle-redraw sanity check, not for the benchmark, because the overlay itself changes painting.

The first target is at least a 50% reduction from the reported approximately 82% GPU utilization during the same drag, with a responsive native border and complete final layout. Compare the builds' drag latency, CPU load, visual parity, and idle load on the Intel machine. The sampled GPU counter is per process and does not measure Windows compositor activity.

## Local build and interaction record (2026-09-24)

Before the software-renderer switch, the Skia OpenGL Release exe was 23,919,616 bytes and its NSIS installer was 10,237,463 bytes. The separate FemtoVG and software Release executables were 15,512,064 and 16,026,112 bytes. These are historical measurements of different variants; the installer size here is for the earlier Skia build.

The painted-frame, suppressed-redraw, and synchronous-paint-duration figures collected on the development machine belonged to the paint gate that was removed on 2026-09-25, so they are no longer reproducible. Event routing and final layout were verified there for both windows and are rechecked against the naive path in the same manual pass.

## Intel integrated GPU A/B (2026-09-24)

On a 1920×1080, 125% scale Intel Graphics laptop, a tester ran all three Release builds through the same top-left-corner oscillation: ten 500-pixel out-and-back cycles for popup; Settings used 500 horizontal and 360 vertical pixels to stay on screen. Slow legs took 500 ms. The tester reported that the software build's popup and Settings appearance and drag feel were normal.

| Renderer | Popup slow GPU peak | Settings slow GPU peak | Settings slow CPU peak (% of whole machine) | Settings slow p95 synchronous paint |
|---|---:|---:|---:|---:|
| Skia OpenGL | 70.9% | 85.7% | 0.60% | 4,687 µs |
| FemtoVG | 70.4% | 80.4% | 0.74% | 3,028 µs |
| Software | no non-zero Lexift GPU Engine sample | no non-zero Lexift GPU Engine sample | 0.38% | 2,536 µs |

Skia Settings accepted 74 paints out of 279 size messages while the paint cap was still in place, so the cap worked but its GPU peak remained high. The software build removed the measured Lexift GPU Engine load without a CPU spike in this test, so it became the production renderer. These GPU samples have approximately one-second resolution, and the synthetic motion is harsher than the original screenshot's drag. No same-motion pre-change baseline was captured, so the exact 50% before/after reduction cannot be calculated. Cross-DPI behavior and broader visual parity remain to be checked separately.

## Historical: frame-copy removal (2026-09-25)

`configure_resize_background` used to copy the whole client area into a DIB after every paint and paste it back on a pure move. That copy was the only resize-specific work left after the paint gate was removed, so the process CPU of the installed build was measured with real mouse input before changing it: idle 0 ms over 3 s, title-drag move 0.17 ms per 28 ms step (1126×526), and border resize 1.91 ms per step at 432×336 versus 3.47 ms per step at 1126×526. Timing the same calls in isolation (`CreateCompatibleBitmap` + `BitBlt` + `DeleteObject` against the window DC) cost 1.62 ms per frame at 420×1152 (0.47 ms allocation, 1.15 ms copy) and 0.26 ms at 340×336, so the copy accounted for roughly a fifth of a small resize step and half of a large one.

The copy was removed instead of optimised. It only served a move that keeps the client size, while Windows repaints every step of its own sizing and moving loops, and the Popup and Settings have been opaque DWM composited windows since the per-pixel-alpha fix, so the compositor no longer loses their pixels. An A/B ran the recorded scenarios with `LEXIFT_DEBUG_DISABLE_RESIZE_SNAPSHOT=1` (a temporary switch, removed with the code) and a temporary log line confirmed the switch reached the window subclass. **The claim that the copy was unnecessary was wrong, but not because of the copy: the A/B dragged the window with coordinates that never took it off the display, so it never exercised the case the copy covered.** Removing the copy together with the erase fill (below) is what finally fixed that case.

Per-step process CPU could not resolve the change on the development machine: two runs of the same binary over the same top-edge oscillation (120 steps, 20 ms apart, 436×457 or larger) came out at 3.52/2.73 ms per step for the new build and 3.39/2.47 ms per step for the installed build with the copy still in place, so the run-to-run spread exceeds the expected gain. A paint-heavy case separates them: hovering the pin button in and out 60 times on a full-height Popup cost 3.13/1.82 ms per cycle with the copy and 1.56/0.52 ms per cycle without it, a difference of about 1.3 ms per cycle that matches the measured per-frame copy cost. The step-level saving is therefore real but small next to the renderer's per-step layout and paint work, which remains the dominant cost of an interactive resize.

## Historical: erase fill removal (2026-09-25)

Dragging the Popup or Settings off the screen and back left a blank band in the window's own background colour, matching the part of the window that had been outside the display. The module's `WM_ERASEBKGND` handler was the cause: Windows invalidates the region that returns to the screen, the handler painted the flat background into that update region, and the software renderer only presents its own damage, so the application never repainted the content underneath. The handler dates from the per-pixel-alpha era, when an unpainted pixel showed the desktop; both windows are DWM composited and opaque now, so all it could still do was destroy pixels. It is gone, together with its cached `HRGN`: `WM_ERASEBKGND` falls through to the default handler, and because the window class has no background brush nothing paints there and the pixels survive the move. The size-change protection is unchanged and does not depend on the erase path: the exposed right and bottom bands are still filled once when `WM_SIZE` arrives and again inside the paint cycle before the application presents them.

Matshell, the reference the report came from, runs the same Slint software renderer on Windows (`renderer_mode()` maps unknown values to `software`) and never handles `WM_ERASEBKGND`; it also disables winit's transparent window through `with_transparent(false)`, which is the same outcome as this module's `DwmEnableBlurBehindWindow(FALSE)` call.

## Window geometry repair for software rendering (2026-09-25)

Dragging the Popup or Settings across monitors with different scale factors could leave part of the client area black or stale. Windows applies the new DPI while its modal move loop owns the window, and the size Slint observes does not describe the intermediate geometry, so Slint keeps believing that nothing needs to be painted; the pixels from the old geometry stay on screen, black where the surface was cleared and older content elsewhere, until something else forces a resize.

`configure_resize_background` installs an optional hook into the same per-window subclass: `configure_window_geometry_repair`. `WM_EXITSIZEMOVE` runs it once per interactive move or resize, and the UI marks the whole window dirty through the software renderer, followed by `Window::request_redraw()`. Popup and Settings install these hooks only when software rendering is selected. FemtoVG uses neither hook.

Verification on a 2048x1152 primary at 125% next to a 1920x1080 secondary, dragging the Settings window from one monitor to the other and back: the build without the hook left a large black rectangle, the build with it painted the window completely, same gesture and otherwise identical binaries. The Popup shows the same result. One caution for future debugging: a circular drag that leaves the window partly outside the monitor produces black pixels in a GDI screen capture of the off-screen part, which looks exactly like this artifact but is a capture artifact, not damage. The earlier investigation in this document was misled by that, which is why the trigger was first attributed to the renderer.
