import { describe, expect, it } from "vitest";
import {
  acceleratorFromEvent,
  formatAccelerator,
  mainKeyForCode,
  modifierNames,
  parseShortcutTarget,
  type ShortcutKeyEvent,
} from "./capture";

/** A key press with the modifiers given as an object (all off by default). */
function press(code: string, modifiers: Partial<ShortcutKeyEvent> = {}): ShortcutKeyEvent {
  return {
    code,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    ...modifiers,
  };
}

describe("shortcut capture", () => {
  it("maps letters, digits, function keys and named keys to the Rust spelling", () => {
    expect(mainKeyForCode("KeyS")).toBe("S");
    expect(mainKeyForCode("KeyZ")).toBe("Z");
    expect(mainKeyForCode("Digit5")).toBe("5");
    expect(mainKeyForCode("F5")).toBe("F5");
    expect(mainKeyForCode("F24")).toBe("F24");
    expect(mainKeyForCode("Space")).toBe("Space");
    expect(mainKeyForCode("PrintScreen")).toBe("PrintScreen");
    expect(mainKeyForCode("ArrowLeft")).toBe("ArrowLeft");
    // Unsupported codes stay out of the accepted grammar.
    expect(mainKeyForCode("Numpad5")).toBeNull();
    expect(mainKeyForCode("MediaPlayPause")).toBeNull();
    expect(mainKeyForCode("F25")).toBeNull();
    expect(mainKeyForCode("IntlBackslash")).toBeNull();
  });

  it("orders modifiers Ctrl, Alt, Shift, Super", () => {
    expect(modifierNames(press("KeyS", { metaKey: true, shiftKey: true, altKey: true, ctrlKey: true }))).toEqual([
      "Ctrl",
      "Alt",
      "Shift",
      "Super",
    ]);
    expect(modifierNames(press("KeyS"))).toEqual([]);
  });

  it("accepts the default shortcuts", () => {
    expect(acceleratorFromEvent(press("KeyS", { ctrlKey: true, altKey: true }))).toEqual({
      ok: true,
      accelerator: "Ctrl+Alt+S",
    });
    expect(acceleratorFromEvent(press("KeyT", { ctrlKey: true, altKey: true }))).toEqual({
      ok: true,
      accelerator: "Ctrl+Alt+T",
    });
  });

  it("normalizes modifier case/order by construction (fixed canonical order)", () => {
    expect(
      acceleratorFromEvent(press("F9", { altKey: true, ctrlKey: true, shiftKey: true })),
    ).toEqual({ ok: true, accelerator: "Ctrl+Alt+Shift+F9" });
  });

  it("reports a modifier-only press without consuming it", () => {
    expect(acceleratorFromEvent(press("ControlLeft", { ctrlKey: true }))).toEqual({
      ok: false,
      reason: "modifier",
    });
    expect(acceleratorFromEvent(press("ShiftRight", { shiftKey: true, ctrlKey: true }))).toEqual({
      ok: false,
      reason: "modifier",
    });
  });

  it("requires at least two modifiers and one key (the Rust parser rule)", () => {
    expect(acceleratorFromEvent(press("KeyS"))).toEqual({
      ok: false,
      reason: "too-few-modifiers",
    });
    expect(acceleratorFromEvent(press("KeyS", { ctrlKey: true }))).toEqual({
      ok: false,
      reason: "too-few-modifiers",
    });
  });

  it("rejects unsupported main keys", () => {
    expect(acceleratorFromEvent(press("MediaPlayPause", { ctrlKey: true, altKey: true }))).toEqual({
      ok: false,
      reason: "unknown-key",
    });
  });

  it("formats the preview without changing the canonical string", () => {
    expect(formatAccelerator("  Ctrl+Alt+S ")).toBe("Ctrl+Alt+S");
  });
});

describe("parseShortcutTarget", () => {
  it("maps every known target", () => {
    expect(parseShortcutTarget("capture")).toBe("capture");
    expect(parseShortcutTarget("toggle")).toBe("toggle");
    expect(parseShortcutTarget("editLatest")).toBe("editLatest");
    expect(parseShortcutTarget("search")).toBe("search");
  });

  it("falls back to capture for unknown, empty or missing values", () => {
    expect(parseShortcutTarget("nope")).toBe("capture");
    expect(parseShortcutTarget("")).toBe("capture");
    expect(parseShortcutTarget(null)).toBe("capture");
    expect(parseShortcutTarget(undefined)).toBe("capture");
  });
});