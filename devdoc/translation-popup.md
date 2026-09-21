# Translation Popup sessions

The selection translation UI is a frameless, always-on-top Slint card managed by `lexift-ui`. Windows applies `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` before showing the card, then removes `WS_EX_NOACTIVATE` with an `SWP_NOACTIVATE` frame refresh immediately after the show completes. This keeps focus in the source application during passive display.

Slint creates the native HWND for a newly allocated top-level window on a later Winit event-loop turn. Passive preparation therefore reports `Pending` while the handle is unavailable. `PopupRegistry` retains the latest show request and retries after yielding to the event loop, then at 16 ms intervals for up to one second. Hiding or closing the session invalidates the pending generation. A temporary missing handle is not logged as a platform failure and never requires a second hotkey press.

Each Popup HWND installs one replaceable Win32 pointer bridge. It consumes mouse movement, leave, left-button, capture, and wheel messages before Winit and forwards them to `slint::Window::try_dispatch_event`. Client coordinates remain physical in `lexift-platform`; `lexift-ui` divides them by the live Slint scale factor, including wheel deltas, before dispatch. The first press uses a temporary `AttachThreadInput` connection to activate and focus the Popup, sends a position update before the press, and captures the mouse until release.

The HWND property `Lexift.PopupInputBridge` owns the bridge pointer and identifies an existing installation without relying on the unavailable `GetWindowSubclass` export. Reused HWNDs replace only the stored handler and reset transient pointer state. `WM_NCDESTROY` removes the property and subclass before releasing the bridge.

An interacted unpinned window is eligible for automatic dismissal only after it has been observed as the foreground window once; a later loss of foreground closes it. This prevents activation timing from dismissing the popup during its first click. Pinned windows remain visible and always on top.

## Core ownership

`lexift-core` owns `PopupSessionId` and `PopupSessionState`. A session contains source and translated text, its per-window target language, detected source language, translation task, pin state, feedback, and speech state. The active unpinned session is reused by later selections. Pinning detaches it from the active slot, so the next selection creates another session. Task IDs are checked per session, and results for superseded or closed sessions are ignored.

Changing a popup target language only changes that session and immediately starts a new translation. It does not write Settings. Copy and speech requests are explicit Core commands executed by adapters.

## UI and native lifecycle

`PopupRegistry` runs on the Slint event-loop thread and maps session IDs to `TranslationPopup` windows. It creates and destroys windows, applies Core snapshots, positions new windows by the selection anchor, offsets them from visible pinned windows, and clamps resized windows to the monitor work area. Each window attaches its own session ID to callbacks.

The card keeps source, language, and result sections visible. Source and result content grow within capped regions, the result scrolls independently, and the target-language menu stays inside the same HWND with an upward or downward viewport chosen from available space.

Windows supplies passive/interactive style changes, native custom-title-bar dragging, foreground detection, and a dedicated STA SAPI worker. The worker selects a matching installed voice by language when available, falls back to the system voice, replaces the current utterance on a new request, and reports completion or failure back to Core.

Popup sessions are intentionally in-memory only and end when their windows close or the process exits.
