/**
 * Tests for the layered back-out reducer (W1.4).
 *
 * Esc and right-click share this reducer, so these cases define the user-visible order: text entry
 * → crop mode → non-default tool → the existing dirty-confirming cancel. The reducer only decides;
 * a caller applies the state and performs the effect.
 */

import { describe, expect, it } from "vitest";
import { backout, DEFAULT_TOOL, type BackoutState } from "./editorBackout";

/** A clean editor with only the default tool selected. */
function state(overrides: Partial<BackoutState> = {}): BackoutState {
  return { textEditing: false, cropMode: false, tool: DEFAULT_TOOL, dirty: false, ...overrides };
}

describe("backout priority", () => {
  it("cancels an open text entry before anything else", () => {
    const result = backout(state({ textEditing: true, cropMode: true, tool: "crop", dirty: true }));
    expect(result.effect).toEqual({ type: "consumed" });
    expect(result.state.textEditing).toBe(false);
    // Lower layers are untouched by the first step: the next step decides them.
    expect(result.state.cropMode).toBe(true);
    expect(result.state.tool).toBe("crop");
  });

  it("leaves crop mode before falling back to the default tool", () => {
    const result = backout(state({ cropMode: true, tool: "crop", dirty: true }));
    expect(result.effect).toEqual({ type: "consumed" });
    expect(result.state.cropMode).toBe(false);
    expect(result.state.tool).toBe(DEFAULT_TOOL);
    // The already-applied crop is part of the annotation state, not of this reducer: `dirty` stays.
    expect(result.state.dirty).toBe(true);
  });

  it("returns a non-default tool to the default tool", () => {
    const tools = ["highlight", "mosaic", "blur", "number", "text", "rect", "arrow"] as const;
    for (const tool of tools) {
      const result = backout(state({ tool }));
      expect(result.effect).toEqual({ type: "consumed" });
      expect(result.state.tool).toBe(DEFAULT_TOOL);
    }
  });

  it("asks before discarding when only the default tool is left and work is unsaved", () => {
    expect(backout(state({ dirty: true }))).toEqual({
      state: state({ dirty: true }),
      effect: { type: "confirm-discard" },
    });
  });

  it("cancels the editor when there is nothing left and nothing to lose", () => {
    expect(backout(state())).toEqual({ state: state(), effect: { type: "cancel" } });
  });

  it("never treats an empty-but-open text entry as a change to confirm", () => {
    // Layer 1 consumes the step regardless of `dirty`; the component closes the textarea, it never
    // runs the discard confirmation for typed-but-uncommitted text.
    const result = backout(state({ textEditing: true, dirty: false }));
    expect(result.state.textEditing).toBe(false);
    expect(result.effect).toEqual({ type: "consumed" });
  });
});

describe("backout ladder", () => {
  it("walks text → crop → tool → confirm-discard for a fully loaded editor", () => {
    let current = state({ textEditing: true, cropMode: true, tool: "crop", dirty: true });
    const effects: string[] = [];

    for (let step = 0; step < 4; step += 1) {
      const result = backout(current);
      effects.push(result.effect.type);
      current = result.state;
    }

    expect(effects).toEqual(["consumed", "consumed", "confirm-discard", "confirm-discard"]);
    expect(current.tool).toBe(DEFAULT_TOOL);
    expect(current.cropMode).toBe(false);
    expect(current.textEditing).toBe(false);
  });

  it("walks tool → cancel for a clean editor with a non-default tool", () => {
    let current = state({ tool: "mosaic" });
    const effects: string[] = [];
    for (let step = 0; step < 2; step += 1) {
      const result = backout(current);
      effects.push(result.effect.type);
      current = result.state;
    }
    expect(effects).toEqual(["consumed", "cancel"]);
  });

  it("does not mutate the input state", () => {
    const original = state({ textEditing: true, dirty: true });
    backout(original);
    expect(original).toEqual(state({ textEditing: true, dirty: true }));
  });
});
