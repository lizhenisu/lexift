# Pencil and highlighter

The second annotation group draws editable objects on the live desktop. Pencil
stores a simplified polyline in physical virtual-desktop coordinates; highlighter
stores either two line endpoints or a filled rounded rectangle. The Core document
owns hit testing, geometry edits and undo. UI samples pointer input and rasterizes
at most once per scheduled frame using the existing canvas presenter.

Pencil uses round caps/joins and five continuous dash patterns. Highlighter uses
30% opacity (77/255), composed once per object. Multiply is explicitly unavailable:
the application does not capture the underlying desktop for this feature.

Line endpoints are independently editable. Highlight rectangles expose eight
resize handles and four linked radius handles; the latter edit one shared radius,
clamped to half the shorter side. Pencil resizing transforms points without
changing stroke width. One gesture is one undo entry. Parameters remain ephemeral.

The existing transient menu/color picker infrastructure is shared. Pencil defaults
to red/12/solid; highlighter defaults to green/12/line/translucent, with rectangle
radius zero. Highlighter rectangle width input is disabled. Multiply cannot be
selected by pointer, keyboard or accessibility callbacks.

## Verification

- Core/UI tests cover sampling, point strokes, path hit testing, transforms,
  endpoints, radius limits, drag cancellation, undo, dash rendering and alpha.
- Workspace tests passed: 242 passed, 15 existing ignored integration tests.
- Windows native review captures are in `target/brush-review/`.
- Workspace build and all-target/all-feature Clippy run with warnings denied.
- Windows Release build passed. Native review covered five pencil patterns,
  highlighter endpoint editing, rectangle radius handles, undo, and light/dark
  parameter panels. Narrow parameter layouts use the existing software snapshot
  test (`brush-3-narrow` and `brush-4-narrow`); resizing the native popup externally
  does not reliably resize its Slint layout.
- Multi-monitor drawing was exercised, but mixed-DPI seam alignment and all
  monitor configurations have not been conclusively verified.
- Original configuration was restored after theme review; Release remains running
  in the background. No files were staged or committed.

No new crate, configuration field or platform capability was introduced.
