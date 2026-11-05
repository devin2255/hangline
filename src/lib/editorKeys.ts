/**
 * Keyboard mapping shared by the markup and trim editor windows (design spec §3.7).
 *
 * The editors only accept a small, fixed set of shortcuts, so the mapping is a pure function:
 * `undo` is Ctrl+Z, `redo` is Ctrl+Shift+Z or Ctrl+Y, `save` is Ctrl+S, and `cancel` is Escape.
 * Matching is case-insensitive because `KeyboardEvent.key` reports an uppercase letter while
 * Shift is held (e.g. `"Z"` for Ctrl+Shift+Z).
 *
 * The `ctrl` parameter means "the platform's command modifier": callers pass
 * `event.ctrlKey || event.metaKey`, so the same mapping also covers Cmd on macOS keyboards.
 * Everything else, including a bare Z/Y/S, returns null and is left to the focused control.
 */

/** Editor action produced by a keyboard event. */
export type EditorCommand = "undo" | "redo" | "save" | "cancel";

/**
 * Maps one keydown to an editor action.
 *
 * `shift` only changes the meaning of Ctrl+Z (redo); it is deliberately ignored for Ctrl+S and
 * Ctrl+Y so an accidentally held Shift does not swallow a save or redo.
 */
export function editorKeys(key: string, ctrl: boolean, shift: boolean): EditorCommand | null {
  const normalized = key.toLowerCase();
  if (ctrl) {
    if (normalized === "z") return shift ? "redo" : "undo";
    if (normalized === "y") return "redo";
    if (normalized === "s") return "save";
  }
  if (normalized === "escape") return "cancel";
  return null;
}
