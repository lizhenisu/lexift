# Annotation parameter menus

## Implementation

Slint 1.18.1's Winit backend embeds `PopupWindow` into its parent surface. The geometry parameter strip is only 68 logical pixels high, so its five-row line-style list was clipped.

Parameter ComboBoxes now request an on-demand `AnnotationChoiceWindow`. The window is owned by the parameter panel and uses the existing native menu taskbar policy, pointer bridge, theme and dismissal capabilities. The parameter panel retains its size and position. Geometry styles use SVG paths rather than font glyphs.

Anchors are converted from panel logical coordinates to physical screen coordinates; menu dimensions use the menu's own DPI scale. Placement prefers below, flips above, and clamps to the work area. Two bounded layout passes handle the native DPI update after moving the new window. Session, panel and menu revisions reject stale callbacks.

Only one parameter menu exists at a time. Selection, Escape, external clicks, tool changes, toolbar dragging, language changes and parent destruction dispose of the relevant menu. Clicking its trigger toggles it; interacting with a child menu does not dismiss its panel. Color editing retains its existing implementation.

## Verification (2026-09-27)

- Windows, 1920×1080, 125% scaling: real Release windows rendered all five choices; selected the final style, reopened it, and confirmed its highlight and control preview.
- Selected each style and drew geometry; subsequent style edits also updated the selected object.
- Escape, repeated opening, clicking other parameter controls, switching tools, dragging the toolbar, and closing the annotation session disposed of the menu.
- Bottom-edge placement opened above the control. Magnifier's shared dropdown changed from 1.5× to 4×.
- Light and dark screenshots inspected. Menu native style was `0x198`: `WS_EX_APPWINDOW` absent, `WS_EX_TOOLWINDOW` present; native owner matched the parameter panel.
- Workspace tests: 234 passed, 15 existing ignored tests. UI tests include negative-coordinate edge placement and parameter-field mapping. Workspace build, all-target/all-feature Clippy and Windows Release build passed.
- Mixed-DPI dual monitors and a physically narrow work area are not available on this test machine; those scenarios still require hardware verification.

Actual screenshots and validation logs are in ignored build output `target/annotation-dropdown-review/` and `target/annotation-dropdown-*.log`. User configuration was restored byte-for-byte after the theme test.
