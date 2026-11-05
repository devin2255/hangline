/**
 * Tests for the keyboard mapping shared by the markup and trim editors (design spec §3.7).
 *
 * Contract: Ctrl+Z undoes, Ctrl+Shift+Z / Ctrl+Y redoes, Ctrl+S saves, Escape cancels, and every
 * other combination is ignored. Matching is case-insensitive because `KeyboardEvent.key` may
 * report an uppercase letter while Shift is held. Callers pass `ctrlKey || metaKey` as `ctrl`.
 */

import { describe, expect, it } from "vitest";
import { editorKeys } from "./editorKeys";

describe("editorKeys", () => {
  it("maps Ctrl+Z to undo", () => {
    expect(editorKeys("z", true, false)).toBe("undo");
  });

  it("maps Ctrl+Z to undo regardless of the key's case", () => {
    expect(editorKeys("Z", true, false)).toBe("undo");
  });

  it("maps Ctrl+Shift+Z to redo", () => {
    expect(editorKeys("z", true, true)).toBe("redo");
    expect(editorKeys("Z", true, true)).toBe("redo");
  });

  it("maps Ctrl+Y to redo", () => {
    expect(editorKeys("y", true, false)).toBe("redo");
    expect(editorKeys("Y", true, false)).toBe("redo");
  });

  it("maps Ctrl+S to save, case-insensitively", () => {
    expect(editorKeys("s", true, false)).toBe("save");
    expect(editorKeys("S", true, false)).toBe("save");
  });

  it("maps Escape to cancel, case-insensitively", () => {
    expect(editorKeys("Escape", false, false)).toBe("cancel");
    expect(editorKeys("ESCAPE", false, false)).toBe("cancel");
  });

  it("does not treat plain z/y/s as editor commands", () => {
    expect(editorKeys("z", false, false)).toBeNull();
    expect(editorKeys("y", false, false)).toBeNull();
    expect(editorKeys("s", false, false)).toBeNull();
  });

  it("does not treat Shift+Z without Ctrl as undo", () => {
    expect(editorKeys("Z", false, true)).toBeNull();
  });

  it("ignores every other key", () => {
    expect(editorKeys("a", false, false)).toBeNull();
    expect(editorKeys("Enter", true, false)).toBeNull();
    expect(editorKeys(" ", false, false)).toBeNull();
    expect(editorKeys("F5", false, false)).toBeNull();
    expect(editorKeys("Delete", false, false)).toBeNull();
    expect(editorKeys("Control", true, false)).toBeNull();
    expect(editorKeys("", false, false)).toBeNull();
  });
});
