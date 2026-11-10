/**
 * Keyboard-event → accelerator capture for the change-shortcut window (Task 15).
 *
 * The produced string must match the Rust `hotkeys::normalize` grammar exactly, otherwise the save
 * would be rejected: modifiers in the fixed order `Ctrl+Alt+Shift+Super`, then one main key spelled
 * as `A`/`5`/`F12`/`Space`/`Tab`/… The Rust-side parser is authoritative; this module only mirrors
 * its accepted key set so the window can show a useful error before calling the backend.
 */

/** The part of a `KeyboardEvent` this module needs (DOM- and test-friendly). */
export interface ShortcutKeyEvent {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

/** Why a key press does not become an accelerator. */
export type CaptureFailure =
  /** Only modifier keys are held down; wait for the main key. */
  | "modifier"
  /** The main key is outside the accelerator grammar (media keys, Numpad, …). */
  | "unknown-key"
  /** Fewer than two modifiers are held (the Rust parser requires at least two). */
  | "too-few-modifiers";

/** The outcome of one key press. */
export type CaptureResult =
  | { ok: true; accelerator: string }
  | { ok: false; reason: CaptureFailure };

/** Named main keys accepted by the Rust `hotkeys::canonical_key` table, by `KeyboardEvent.code`. */
const NAMED_KEYS: Readonly<Record<string, string>> = {
  Space: "Space",
  Tab: "Tab",
  Enter: "Enter",
  NumpadEnter: "Enter",
  Backspace: "Backspace",
  Delete: "Delete",
  Insert: "Insert",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  ArrowUp: "ArrowUp",
  ArrowDown: "ArrowDown",
  ArrowLeft: "ArrowLeft",
  ArrowRight: "ArrowRight",
  PrintScreen: "PrintScreen",
  CapsLock: "CapsLock",
};

/** `KeyboardEvent.code` values that press a modifier only (no main key yet). */
const MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
]);

/** Maps one key press to the Rust main-key spelling, or `null` when unsupported. */
export function mainKeyForCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice("Key".length);
  if (/^Digit[0-9]$/.test(code)) return code.slice("Digit".length);
  // `F1`..`F24` (the parser accepts 1..=24; `F0`/`F25` are not event codes but guard anyway).
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return NAMED_KEYS[code] ?? null;
}

/** The modifier names in the canonical order, for the modifiers currently held. */
export function modifierNames(event: ShortcutKeyEvent): string[] {
  const names: string[] = [];
  if (event.ctrlKey) names.push("Ctrl");
  if (event.altKey) names.push("Alt");
  if (event.shiftKey) names.push("Shift");
  if (event.metaKey) names.push("Super");
  return names;
}

/**
 * Builds the accelerator for one key press.
 *
 * Returns `{ok: false, reason}` instead of throwing so the window can explain what is missing:
 * - a modifier-only press ("keep holding the modifiers…"),
 * - an unsupported main key,
 * - fewer than the two modifiers the Rust parser requires.
 */
export function acceleratorFromEvent(event: ShortcutKeyEvent): CaptureResult {
  if (MODIFIER_CODES.has(event.code)) {
    return { ok: false, reason: "modifier" };
  }
  const mainKey = mainKeyForCode(event.code);
  if (!mainKey) {
    return { ok: false, reason: "unknown-key" };
  }
  const modifiers = modifierNames(event);
  if (modifiers.length < 2) {
    return { ok: false, reason: "too-few-modifiers" };
  }
  return { ok: true, accelerator: [...modifiers, mainKey].join("+") };
}

/** Pretty form for the captured preview: the canonical accelerator (already human-readable). */
export function formatAccelerator(accelerator: string): string {
  return accelerator.trim();
}

/** Which global shortcut a change-shortcut window edits (W1.8 adds the third one). */
export type ShortcutTarget = "capture" | "toggle" | "editLatest" | "search";

/**
 * Parses the `?target=` query parameter / `shortcut-target` payload.
 *
 * Unknown values fall back to the capture shortcut (the historical behaviour): a malformed URL must
 * still open a usable window, and it must never silently edit a different shortcut than the one the
 * tray row claims.
 */
export function parseShortcutTarget(value: string | null | undefined): ShortcutTarget {
  return value === "toggle" || value === "editLatest" || value === "search" ? value : "capture";
}