# Windows resize performance diagnostics

The Task Manager capture from an Intel integrated GPU showed Lexift at 82.3% of GPU 0's 3D engine while the CPU entry was 0.3%. The value returned to normal a few seconds after mouse release. This identifies interactive resizing as the workload to compare; it does not identify whether frame count, GPU work per frame, or driver presentation is dominant.

## Resize paint policy

The current build includes FemtoVG and Slint's software renderer. On Windows, startup selects software rendering for a single active Intel display and FemtoVG otherwise; `SLINT_BACKEND=winit-software` or `winit-femtovg` overrides the automatic choice before startup. A running window cannot switch renderers. Native move and resize remain under Windows/Winit control, with no app-side paint gate. The software path retains the opaque background-band fill and release-time paint repair described below; the FemtoVG path does not install them. The cross-DPI drag case on the software path still needs hardware validation. See `devdoc/translation-popup.md` for the current flow.

## Renderer comparison

Run `powershell -ExecutionPolicy Bypass -File scripts/build-renderer-diagnostics.ps1` from the repository. It copies the current working tree into `target/renderer-diagnostics/workspace`, switches only that copy's Slint renderer feature, and builds independent Release executables under `target/renderer-diagnostics/{skia-opengl,femtovg,software}/lexift.exe`. Diagnostic builds bypass the automatic startup choice so each executable uses its single compiled renderer. The repository's manifests and existing uncommitted changes remain untouched.

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
