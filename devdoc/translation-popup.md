# Translation Popup sessions

The selection translation UI is a frameless, always-on-top Slint card managed by `lexift-ui`. Windows applies `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` before showing the card, then removes `WS_EX_NOACTIVATE` with an `SWP_NOACTIVATE` frame refresh immediately after the show completes. This keeps focus in the source application during passive display.

Slint creates the native HWND for a newly allocated top-level window on a later Winit event-loop turn. Passive preparation therefore reports `Pending` while the handle is unavailable. `PopupRegistry` retains the latest show request and retries after yielding to the event loop, then at 16 ms intervals for up to one second. Hiding or closing the session invalidates the pending generation. A temporary missing handle is not logged as a platform failure and never requires a second hotkey press.

Each Popup HWND installs one replaceable Win32 pointer bridge. It consumes mouse movement, leave, left-button, capture, and wheel messages before Winit and forwards them with `slint::Window::dispatch_event_with_result`. Client coordinates remain physical in `lexift-platform`; `lexift-ui` divides them by the live Slint scale factor, including wheel deltas, before dispatch. The first press uses a temporary `AttachThreadInput` connection to activate and focus the Popup, sends a position update before the press, and captures the mouse until release.

The HWND property `Lexift.PopupInputBridge` owns the bridge pointer and identifies an existing installation without relying on the unavailable `GetWindowSubclass` export. Reused HWNDs replace only the stored handler and reset transient pointer state. `WM_NCDESTROY` removes the property and subclass before releasing the bridge.

An unpinned window remains visible for passive reading, then closes on the next pointer press outside its bounds or on a subsequent foreground-window change. The initial foreground window is recorded when the popup is shown so passive display never dismisses it immediately. Pinning unregisters this monitor; pinned windows remain visible and always on top.

## Core ownership

`lexift-core` owns `PopupSessionId` and `PopupSessionState`. A session contains source and translated text, its user-selected source language, per-window target language, provider-detected source language, translation task, pin state, feedback, and speech state. `None` for the selected source language means automatic detection. The active unpinned session is reused by later selections and resets its source choice to automatic detection for every new capture. Pinning detaches it from the active slot, so the next selection creates another session while the pinned session keeps its source choice. Task IDs are checked per session, and results for superseded or closed sessions are ignored.

Changing either popup language only changes that session and immediately starts a new translation with the complete source/target context. It does not write Settings. An explicit source choice is preferred for source speech; automatic mode uses the provider-detected language. Copy and speech requests are explicit Core commands executed by adapters.

## UI and native lifecycle

`PopupRegistry` runs on the Slint event-loop thread and maps session IDs to `TranslationPopup` windows. It creates and destroys windows, applies Core snapshots, positions new windows by the selection anchor, offsets them from visible pinned windows, and clamps resized windows to the monitor work area. Each window attaches its own session ID to callbacks.

The card keeps source, language, and result sections visible. Source and result content grow within capped regions and scroll independently; the editable source area follows the text cursor and shows its vertical scrollbar only when needed. The compact language row reserves a fixed center swap area and divides the remaining width equally between source and target selectors. Provider detection remains session state for speech selection but is not rendered as a separate label.

The source card border also acts as an eight-direction resize handle for the whole Popup. Slint provides directional cursors, short animated edge accents, and curved corner accents that follow the card's rounded border instead of forming square L markers. It then delegates the actual operation to the native window sizing loop. Windows reports live client sizes back to the registry; vertical resize deltas are assigned to the source card while the language and translation sections keep their minimum usable height. Each session records its last manual width, height, source height, and automatic-content baseline. Later source or result growth can extend that manual floor, while shorter content never undoes the user's size. The record is discarded when the session closes or its window slot is reused.

Both language selectors share one reusable `PopupLanguageMenuWindow` owned by the selected session's Popup HWND. The menu records whether its owner is the source or target selector, aligns to that control, shows up to ten rows, flips above the selector or reduces its scrollable height when the monitor work area is constrained, and never changes the translation card's size. Opening one side closes the other. Source options are automatic detection plus the reduced set accepted by DeepL as explicit source languages; target options retain their regional variants. Popup and Settings menus share the same menu typography tokens and snap scroll offsets to stable coordinates to keep Windows text rendering crisp across DPI scales.

While the language menu is open, dismissal monitoring moves from the parent Popup to the menu HWND. A click inside the menu selects a language for the owning session; a click elsewhere closes only the menu and then restores the parent's unpinned dismissal watch. Dragging, hiding, closing, or reusing the owning Popup also closes the menu and clears its scroll and pending native-window state. The menu uses the same delayed HWND preparation, native pointer bridge, DPI conversion, and tool-window ownership as translation windows.

Windows supplies passive/interactive style changes, native custom-title-bar dragging, outside-interaction monitoring, and a dedicated STA SAPI worker. The worker selects a matching installed voice by language when available, falls back to the system voice, replaces the current utterance on a new request, and reports completion or failure back to Core.

Popup sessions are intentionally in-memory only and end when their windows close or the process exits.
