# Annotation cursor feedback

The canvas previously used a fixed crosshair. `annotation_cursor.rs` now resolves its Slint cursor from the existing Core hit test and the current gesture:

- Empty canvas: crosshair; movable object: four-way move.
- Handles 0/7: NW–SE; 2/5: NE–SW; 1/6: vertical; 3/4: horizontal.
- An active draw, move or resize keeps its operation's cursor even when leaving the original hit region.
- Mouse mode remains click-through. State changes refresh the canvas properties; tool windows retain their own cursor behavior.

Hover updates do not schedule annotation rasterization. Slint 1.18.1 samples TouchArea's cursor before invoking its pointer callback, so a changed cursor schedules one deferred pointer refresh. It validates the session, last pointer position, current OS pointer and overlapping tool windows before dispatching; unchanged feedback does not schedule another refresh. This also refreshes a stationary cursor after deletion, undo or cancellation without adding a platform API or a polling timer.

## Verification (2026-09-27)

- Unit tests cover eight handle directions, active-gesture precedence, mouse mode, rectangle/ellipse/spotlight hit regions, deletion/undo, DPI scales 1/1.25/1.5/2 and negative screen origins.
- Windows at 1920×1080, 125%: checked actual cursor handles via GetCursorInfo against Windows' standard cursors. All eight hover directions and resize drags passed; moving stayed four-way throughout the drag.
- Checked Escape cancellation, Delete/Undo with canvas focus, mouse mode returning the desktop arrow, returning to drawing, ellipse and spotlight feedback.
- Screenshots under `target/annotation-cursor-review/` capture the live screen and actual system cursor using GetCursorInfo/DrawIconEx. `resizing.png` and `moving.png` show held drags.
- UI tests, workspace build, workspace all-target/all-feature Clippy and Windows Release build were run; logs are `target/annotation-cursor-*.log`.
- Mixed-DPI dual-monitor testing remains unavailable on this single-monitor machine.
