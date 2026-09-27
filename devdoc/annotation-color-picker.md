# Annotation custom color picker

The geometry and spotlight custom swatch opens an on-demand color picker owned by
the parameter panel. It shares the parameter menu's placement, dismissal, theme,
generation guards and window cleanup. Opening the picker only reads the current
color; editing applies opaque RGB through the existing geometry style update.

The picker uses hue/saturation with a separate value slider. HEX accepts six ASCII
hexadecimal digits with an optional `#`. Invalid partial input leaves the current
color unchanged; submitting it shows a localized error. Pointer drags keep the
style transaction open until release, so one drag is one undo operation. Undo also
refreshes the parameter values from the selected object.

## Verification (2026-09-27)

- UI tests: 52 passed, including HSV/RGB round trips, achromatic colors, hue wrap
  and HEX validation.
- Workspace build, all-target/all-feature Clippy with warnings denied, and Windows
  Release build passed.
- Windows Release at 125% scaling: live color changes, HEX input, undo and parameter
  synchronization, Escape dismissal, complete session teardown, and reopening.
- Light/dark rendering and upward opening near the bottom edge checked in actual
  native windows. Original application configuration restored after theme checks.
- Review captures: `target/annotation-color-review/release-light.png`,
  `release-dark.png`, `release-bottom.png`, `release-undo.png`.
- Mixed-DPI dual-monitor operation was not available for manual verification.

No configuration fields, native platform interfaces or dependencies were added.
