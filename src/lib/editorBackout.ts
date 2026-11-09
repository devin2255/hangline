/**
 * Layered back-out decisions for the markup editor (W1.4).
 *
 * Esc and right-click are the two ways to "step back" in the editor, and they share one pure
 * reducer so the two inputs can never drift apart. The layers, highest priority first:
 *
 * 1. **Text entry open** → cancel the text entry. Typed-but-uncommitted text is dropped here, not
 *    committed: backing out must never *add* a shape.
 * 2. **Crop mode** → leave crop mode (the applied crop stays applied; only the draft is dropped).
 * 3. **Any non-default tool selected** → return to the default tool (`pen`).
 * 4. **Nothing left** → the existing cancel flow: a dirty editor asks for confirmation, a clean one
 *    closes immediately.
 *
 * The reducer only decides. Applying the decision (closing the textarea, switching the tool,
 * asking `window.confirm`) stays in the component, which keeps this module DOM-free and testable.
 */

import type { Tool } from "./annotation";

/** The tool "step back" returns to; matches the editor's initial tool. */
export const DEFAULT_TOOL: Tool = "pen";

/** The slice of editor state the back-out decision depends on. */
export interface BackoutState {
  /** A text entry is open (even if it is still empty). */
  textEditing: boolean;
  /** The crop tool is the active mode. */
  cropMode: boolean;
  /** The currently selected tool. */
  tool: Tool;
  /** The editor holds unsaved work (shapes, an applied crop, or typed text). */
  dirty: boolean;
}

/** What the caller must do after one back-out step. */
export type BackoutEffect =
  /** The step was consumed by a layer; apply the returned state. */
  | { type: "consumed" }
  /** Nothing left to back out, and the editor is dirty: ask before discarding. */
  | { type: "confirm-discard" }
  /** Nothing left to back out and nothing to lose: close the editor. */
  | { type: "cancel" };

/** Result of one back-out step: the next state and the effect the caller must perform. */
export interface BackoutResult {
  state: BackoutState;
  effect: BackoutEffect;
}

/**
 * One back-out step.
 *
 * The returned `state` is the layer result for `consumed` and the unchanged input otherwise, so a
 * caller can drive the whole ladder by repeatedly applying the reducer (see the tests).
 */
export function backout(state: BackoutState): BackoutResult {
  if (state.textEditing) {
    return { state: { ...state, textEditing: false }, effect: { type: "consumed" } };
  }
  if (state.cropMode) {
    // Leaving crop mode keeps the committed crop; the caller drops only the draft rectangle.
    return {
      state: { ...state, cropMode: false, tool: DEFAULT_TOOL },
      effect: { type: "consumed" },
    };
  }
  if (state.tool !== DEFAULT_TOOL) {
    return { state: { ...state, tool: DEFAULT_TOOL }, effect: { type: "consumed" } };
  }
  return {
    state,
    effect: state.dirty ? { type: "confirm-discard" } : { type: "cancel" },
  };
}
