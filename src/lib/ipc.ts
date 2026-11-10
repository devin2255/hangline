/**
 * Typed wrappers around the Rust IPC commands (design spec 4.4).
 *
 * Keeping the `invoke` names and argument objects here means components never spell command
 * strings by hand; the payload types come from `types.ts`, which mirrors the serde shapes.
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  AppState,
  CaptureFinishResult,
  EditorStateSnapshot,
  EditorTarget,
  FlyReady,
  Item,
  MonitorInfo,
  PanelGeometry,
  PhysicalRect,
  Settings,
  SettingsKey,
  TakeDownSide,
  TrimTarget,
  UpdateInfo,
} from "./types";

/**
 * Fetches the full application state from Rust.
 *
 * Rust owns the data (spec 4.1); call this once on startup, then follow the `state` event. The
 * promise rejects when running outside Tauri (plain `vite`), which callers treat as "no IPC".
 */
export function getAppState(): Promise<AppState> {
  return invoke<AppState>("get_app_state");
}

/**
 * Fetches the monitors of the virtual desktop (spec 4.4).
 *
 * Pure query: it never moves or resizes the panel. Task 14 moved the geometry side effect into
 * [`syncPanelGeometry`]; `get_monitors` is kept for callers that only need the monitor list.
 */
export function getMonitors(): Promise<MonitorInfo[]> {
  return invoke<MonitorInfo[]>("get_monitors");
}

/**
 * Re-applies the panel window geometry to the monitor the panel is already on and returns the
 * fresh geometry (Task 14).
 *
 * The command deliberately re-fits the window's monitor, never the pointer's: invoking it from a
 * settings action on another display must not teleport the line. Rust is the single geometry
 * owner: `set_setting` calls the same path for `cardSize`/`keepOnLine`, and the reveal controller
 * follows the pointer across monitors (500 ms debounce, only while the panel is visible). The
 * frontend calls this once on startup to resync and to log every monitor plus the Rust-chosen
 * target.
 */
export function syncPanelGeometry(): Promise<PanelGeometry> {
  return invoke<PanelGeometry>("sync_panel_geometry");
}

/** Replaces the card rects used by the 30 Hz click-through hit-test (physical panel pixels). */
export function setHitRects(rects: PhysicalRect[]): Promise<void> {
  return invoke("set_hit_rects", { rects });
}

/** Returns the panel's DPI scale factor (physical pixels per logical pixel). */
export function getScaleFactor(): Promise<number> {
  return invoke<number>("get_scale_factor");
}

/** Reports that the panel's first layout is painted; Rust then shows the hidden window. */
export function reportPanelReady(label: string): Promise<void> {
  return invoke("report_panel_ready", { label });
}

/**
 * Changes one global shortcut (Task 15).
 *
 * Rust persists the accelerator in `settings.captureHotkey`/`toggleHotkey`, re-registers the global
 * shortcut and rolls the previous value back when the combination is owned by another application;
 * the rejection reason is the promise's error message (shown in the shortcut window).
 */
export function setHotkey(
  target: "capture" | "toggle" | "editLatest" | "search",
  accelerator: string,
): Promise<void> {
  return invoke("set_hotkey", { target, accelerator });
}

/** Closes the change-shortcut window from its frontend (Esc/Cancel/Save). */
export function closeShortcutWindow(): Promise<void> {
  return invoke("close_shortcut_window");
}

/**
 * Sets the panel's busy flag (menu/press/drag gesture in flight).
 *
 * Rust must not tuck the line while the user is pressing, has a menu open or is dragging; the
 * flag is cleared on pointerup/cancel/blur. Native drag-out is detected by Rust separately.
 */
export function setPanelBusy(busy: boolean): Promise<void> {
  return invoke("set_panel_busy", { busy });
}

/** Returns the system double-click interval in milliseconds (`GetDoubleClickTime`). */
export function getDoubleClickIntervalMs(): Promise<number> {
  return invoke<number>("get_double_click_interval_ms");
}

/**
 * Copies one card (image: PNG + DIBV5 + HDROP; recording: HDROP only).
 *
 * Resolves after the clipboard was written; the card shows the `Copied` pill on success.
 */
export function copyItem(id: string): Promise<void> {
  return invoke("copy_item", { id });
}

/**
 * Copies one card as a file reference only (`CF_HDROP`), without the image/DIB formats (Wave 1.1).
 *
 * This is the card menu's explicit `Copy as file` row for both kinds; `copy_item` already falls
 * back to it for recordings, but the two rows must stay distinguishable for images.
 */
export function copyItemAsFile(id: string): Promise<void> {
  return invoke("copy_item_as_file", { id });
}

/**
 * Reveals one card's file selected in Explorer (`explorer.exe /select,`), Wave 1.1.
 */
export function showInExplorer(id: string): Promise<void> {
  return invoke("show_in_explorer", { id });
}

/** Opens one card's file with its default application. */
export function openItem(id: string): Promise<void> {
  return invoke("open_item", { id });
}

/**
 * Takes one card off the line.
 *
 * `deleteFile` is only a request: Rust recycles the file only when it belongs to Hangline's own
 * screenshot directory and the item does not come from a watched folder.
 */
export function takeDown(id: string, deleteFile: boolean): Promise<void> {
  return invoke("take_down", { id, deleteFile });
}

/**
 * Takes every card on one side of `id` off the line (Wave 1.1 `take_down_side`).
 *
 * Side take-downs never delete files (`take_down(deleteFile = false)` semantics); Rust persists
 * the batch once and broadcasts one state snapshot. Resolves with the number of removed cards.
 */
export function takeDownSide(id: string, side: TakeDownSide): Promise<number> {
  return invoke<number>("take_down_side", { id, side });
}

/**
 * Starts a native OLE drag-out of `path` (Task 13).
 *
 * Resolves as soon as Rust accepted the drag session; the OLE modal loop runs on its own STA
 * thread and the final effect (copy/move/none) arrives through the `drag-ended` event.
 */
export function startDrag(path: string): Promise<void> {
  return invoke("start_drag", { path });
}

/**
 * Tells Rust that a native drag-out of `id` ended with `effect` (Task 13).
 *
 * Rust waits 600 ms for the shell drop target to settle, then applies the effect-driven policy:
 * `move` with an existing file is re-checked for ~2 s (a slow shell move), any effect whose file
 * disappeared prunes, and `copy`/`link`/`none` with an existing file leave the card alone. Safe to
 * call for every ending, including cancels; cancelling twice is idempotent.
 */
export function afterDrag(id: string, effect: string): Promise<void> {
  return invoke("after_drag", { id, effect });
}

/**
 * Validates, persists and returns the new full state for one setting.
 *
 * `key` must be the camelCase spec 3.6 name; Rust rejects unknown keys and out-of-range values
 * with the validation message in the rejection reason.
 */
export function setSetting<K extends SettingsKey>(key: K, value: Settings[K]): Promise<AppState> {
  return invoke<AppState>("set_setting", { key, value });
}

/**
 * Starts Hangline's own freeze-frame region capture (Task 16, design spec §3.2 A).
 *
 * Rust hides the line if needed, grabs the virtual screen, saves the frozen frame and shows the
 * full-virtual-screen `capture` window. Reentry-safe: a request while a capture is already in
 * flight resolves without opening a second overlay.
 */
export function requestCapture(): Promise<void> {
  return invoke("request_capture");
}

/**
 * Capture-overlay ready handshake: Rust then emits `capture-ready` with the frozen frame payload.
 *
 * Call this once after the `capture-ready` listener is registered; emitting from the handshake
 * avoids losing the event while the overlay webview is still loading.
 */
export function reportCaptureReady(): Promise<void> {
  return invoke("report_capture_ready");
}

/**
 * Confirms the overlay selection and hangs the capture on the line (Task 16, Wave 1.5 contract).
 *
 * `rect` is in virtual-desktop physical pixels (CSS selection × `devicePixelRatio` + the virtual
 * origin from `capture-ready`); Rust clamps it to the frozen frame, saves through the dedupe claim
 * order and destroys the overlay. The result distinguishes a saved capture from the clipboard
 * fallback (`saved = false, copiedFallback = true`); the overlay closes in both cases, so callers
 * only need it for diagnostics.
 */
export function finishCapture(rect: PhysicalRect): Promise<CaptureFinishResult> {
  return invoke<CaptureFinishResult>("finish_capture", { rect });
}

/** Cancels the capture overlay (Esc / right click): no file, no item, line restored. */
export function cancelCapture(): Promise<void> {
  return invoke("cancel_capture");
}

/** Whether a new fly-in can start: the overlay page reported ready and has a free slot. */
export function startFly(
  id: string,
  from: PhysicalRect | null,
  to: PhysicalRect,
  tiltDeg: number,
  imageVersion: number,
): Promise<boolean> {
  return invoke<boolean>("start_fly", { id, from, to, tiltDeg, imageVersion });
}

/**
 * Requests a fall animation for a taken-down card (P2-6 drives this; renderer is P2-5).
 *
 * `imageVersion` is the `updatedAt` the panel last rendered, so a card edited by the markup editor
 * falls with the edited bytes (`assetUrl(imagePath, imageVersion)` in the overlay).
 */
export function startFall(
  id: string,
  imagePath: string,
  rect: PhysicalRect,
  tiltDeg: number,
  imageVersion: number,
): Promise<boolean> {
  return invoke<boolean>("start_fall", { id, imagePath, rect, tiltDeg, imageVersion });
}

/** Tells Rust one overlay animation finished; for a fly-in the panel receives `fly-landed`. */
export function flyDone(id: string): Promise<void> {
  return invoke("fly_done", { id });
}

/**
 * Fly-overlay ready handshake (called by the `?window=fly` page on mount).
 *
 * Rust marks the overlay ready and returns the virtual-screen rectangle in physical pixels; the
 * page uses it for `cssScale = innerWidth / rect.w` and as the physical origin.
 */
export function reportFlyReady(): Promise<FlyReady> {
  return invoke<FlyReady>("report_fly_ready");
}

/**
 * Takes (removes) the capture selection rect that produced item `id` (spec §4.6 C).
 *
 * A `null` result is normal for system screenshots, clipboard items and watched folders: the
 * caller then lets Rust use the monitor-centre fallback origin.
 */
export function takeCaptureOrigin(id: string): Promise<PhysicalRect | null> {
  return invoke<PhysicalRect | null>("take_capture_origin", { id });
}

/**
 * Opens the P3 markup editor for item `id` (long-press opens it in P3-4).
 *
 * Rust validates the item, sizes the single `annotate` window to the image and creates (or
 * re-targets) it; the page loads the pixels through [`getEditorTarget`].
 */
export function openAnnotate(id: string): Promise<void> {
  return invoke("open_annotate", { id });
}

/** Loads the item + ownership + theme the markup editor page renders. */
export function getEditorTarget(id: string): Promise<EditorTarget> {
  return invoke<EditorTarget>("get_editor_target", { id });
}

/**
 * Tells Rust whether the open editor has unsaved changes (P3-4).
 *
 * `open_annotate` must not silently discard a dirty editor: Rust focuses the existing window and
 * ignores the new target until the editor reports clean again (`false`).
 */
export function setEditorDirty(dirty: boolean): Promise<void> {
  return invoke("set_editor_dirty", { dirty });
}

/**
 * Editor ready handshake (P3-4).
 *
 * The page registers its `annotate-target` listener before calling this; Rust returns the last
 * target so one emitted while the page was still loading is replayed instead of lost.
 */
export function reportAnnotateReady(): Promise<string | null> {
  return invoke<string | null>("report_annotate_ready");
}

/**
 * Atomically writes the editor's composited PNG over the item's file.
 *
 * Rust re-validates ownership, size limits and decodes the payload off the main thread; on success
 * the item's width/height/hash/`updatedAt` are updated and the new state is broadcast, which makes
 * the panel reload the card through `assetUrl(path, updatedAt)`.
 */
export function saveAnnotation(id: string, pngBase64: string): Promise<Item> {
  return invoke<Item>("save_annotation", { id, pngBase64 });
}

/** Closes the markup editor window (Esc/Cancel and after a successful save). */
export function closeEditor(): Promise<void> {
  return invoke("close_editor");
}

/**
 * Opens the P3-6 trim window for recording `id` (long-press on a recording card).
 *
 * Rust validates the item, queries the clip duration on a worker thread and creates (or
 * re-targets) the single `trim` window; the page loads `get_trim_target` for its metadata.
 */
export function openTrim(id: string): Promise<void> {
  return invoke("open_trim", { id });
}

/** Loads the item, size/duration limits and theme the trim page renders. */
export function getTrimTarget(id: string): Promise<TrimTarget> {
  return invoke<TrimTarget>("get_trim_target", { id });
}

/**
 * Exports `[startMs, endMs)` of a recording and atomically replaces the original (P3-7).
 *
 * Coarse progress arrives through the `trim-progress` event. On success the new item is also
 * broadcast through `state`, so the panel reloads the card with `assetUrl(path, updatedAt)`.
 */
export function trimVideo(id: string, startMs: number, endMs: number): Promise<Item> {
  return invoke<Item>("trim_video", { id, startMs, endMs });
}

/** Closes the trim window (Esc/Cancel and after a successful export). */
export function closeTrim(): Promise<void> {
  return invoke("close_trim");
}

/**
 * Trim ready handshake (P3-6).
 *
 * The page registers its `trim-target` listener before calling this; Rust returns the last target
 * so one emitted while the page was still loading is replayed instead of lost.
 */
export function reportTrimReady(): Promise<string | null> {
  return invoke<string | null>("report_trim_ready");
}

/**
 * Reads the live annotate/trim window state (P3-4 review follow-up).
 *
 * The panel reconciles its shared busy flag with this after registering the editor listeners, so a
 * panel reload while an editor is open cannot let the line tuck under the editor.
 */
export function getEditorState(): Promise<EditorStateSnapshot> {
  return invoke<EditorStateSnapshot>("get_editor_state");
}

/**
 * Checks the configured updater endpoint once (P5-2, design spec §6.2).
 *
 * `manual = true` always runs and rejects with a human-readable error when the check fails;
 * `manual = false` is the gated daily check (no request while `autoCheckUpdates` is off or the last
 * successful check is younger than 24 h) and resolves `null` quietly on failure. The call only
 * announces an update — nothing is downloaded until [`installUpdate`] is invoked.
 */
export function checkUpdates(manual: boolean): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("check_updates", { manual });
}

/**
 * Downloads and installs the announced update after an explicit user action (P5-2).
 *
 * Download progress arrives through the `update-progress` event. The plugin verifies the update's
 * signature before launching the installer; on Windows the installer restarts the app (the command
 * does not resolve), so a rejected promise is always safe to show as "Update failed".
 */
export function installUpdate(): Promise<void> {
  return invoke("install_update");
}

/**
 * Returns the update announced before this panel registered its listeners (P5-2).
 *
 * The panel calls this once after wiring the update events, so a fast startup check that emitted
 * `update-available` while the webview was still loading is not lost.
 */
export function getPendingUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("get_pending_update");
}

/**
 * Opens the search window (`?window=search`, Wave 2.2).
 *
 * Rust creates (or re-focuses) the single search window; the page reads the item list through the
 * normal `get_app_state`/`state` channel and filters locally — there is no backend search command.
 */
export function openSearch(): Promise<void> {
  return invoke("open_search");
}

/** Closes the search window from its frontend (Esc / Close button). */
export function closeSearch(): Promise<void> {
  return invoke("close_search");
}

/**
 * Replaces one item's note (Wave 2.3 detail editor).
 *
 * Rust persists the note and broadcasts a new `state` snapshot; the search window's store updates
 * from the event, so no local copy is written here.
 */
export function setItemNote(id: string, note: string): Promise<void> {
  return invoke("set_item_note", { id, note });
}

/**
 * Replaces one item's tags (Wave 2.3 detail editor).
 *
 * `tags` is the already-parsed, trimmed array; Rust persists it and broadcasts a new `state`
 * snapshot (the search list's tag chips update from the event).
 */
export function setItemTags(id: string, tags: string[]): Promise<void> {
  return invoke("set_item_tags", { id, tags });
}
