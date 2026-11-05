/**
 * Tests for the pure annotation model (P3-2, design spec §3.7).
 *
 * The editor component owns the canvas/pointer plumbing; everything that can be reasoned about
 * without a DOM lives here: the undo/redo stack (>= 24 steps, redo cleared by a new shape), the crop
 * rectangle and its output mapping (offset included), and the display → image-pixel transform that
 * keeps annotations exact when the image is scaled down to the window.
 */

import { describe, expect, it } from "vitest";
import {
  MAX_HISTORY,
  addShape,
  canRedo,
  canUndo,
  clampCrop,
  createAnnotationState,
  cropOutput,
  fitContain,
  moveCrop,
  redo,
  setCrop,
  toImagePoint,
  undo,
  type Shape,
} from "./annotation";

/** One deterministic shape for the history assertions. */
function shape(id: string): Shape {
  return {
    id,
    tool: "rect",
    color: "#ff3b30",
    width: 4,
    points: [
      [10, 10],
      [40, 30],
    ],
  };
}

describe("createAnnotationState", () => {
  it("starts empty with the natural image size and no crop", () => {
    const state = createAnnotationState(4000, 3000);
    expect(state.width).toBe(4000);
    expect(state.height).toBe(3000);
    expect(state.shapes).toEqual([]);
    expect(state.crop).toBeNull();
    expect(canUndo(state)).toBe(false);
    expect(canRedo(state)).toBe(false);
  });

  it("normalizes invalid dimensions instead of producing a broken canvas", () => {
    for (const [width, height] of [
      [0, 0],
      [-10, 5],
      [Number.NaN, 100],
      [100, Number.POSITIVE_INFINITY],
    ]) {
      const state = createAnnotationState(width, height);
      expect(state.width).toBeGreaterThanOrEqual(1);
      expect(state.height).toBeGreaterThanOrEqual(1);
      expect(Number.isInteger(state.width)).toBe(true);
      expect(Number.isInteger(state.height)).toBe(true);
    }
  });
});

describe("undo / redo history", () => {
  it("undoes and redoes one shape at a time", () => {
    let state = createAnnotationState(800, 600);
    state = addShape(state, shape("a"));
    state = addShape(state, shape("b"));
    expect(state.shapes.map((item) => item.id)).toEqual(["a", "b"]);

    state = undo(state);
    expect(state.shapes.map((item) => item.id)).toEqual(["a"]);
    expect(canRedo(state)).toBe(true);

    state = redo(state);
    expect(state.shapes.map((item) => item.id)).toEqual(["a", "b"]);
    expect(canRedo(state)).toBe(false);

    // Undo at the bottom of the stack is a no-op, not an error.
    state = undo(state);
    state = undo(state);
    const bottom = undo(state);
    expect(bottom).toBe(state);
    expect(canUndo(state)).toBe(false);
  });

  it("clears the redo stack when a new shape is added", () => {
    let state = createAnnotationState(800, 600);
    state = addShape(state, shape("a"));
    state = undo(state);
    expect(canRedo(state)).toBe(true);
    state = addShape(state, shape("b"));
    expect(canRedo(state)).toBe(false);
    expect(state.shapes.map((item) => item.id)).toEqual(["b"]);
    // A redo after the cleared history does nothing.
    expect(redo(state)).toBe(state);
  });

  it("keeps at least 24 steps and drops the oldest when the cap is exceeded", () => {
    expect(MAX_HISTORY).toBe(24);
    let state = createAnnotationState(800, 600);
    for (let index = 0; index < MAX_HISTORY + 6; index += 1) {
      state = addShape(state, shape(`s${index}`));
    }
    expect(state.shapes).toHaveLength(MAX_HISTORY + 6);
    for (let index = 0; index < MAX_HISTORY; index += 1) {
      state = undo(state);
    }
    expect(canUndo(state)).toBe(false);
    expect(state.shapes).toHaveLength(6);
    expect(undo(state)).toBe(state);
  });

  it("undoes a crop change too, restoring the previous rectangle", () => {
    let state = createAnnotationState(1000, 800);
    state = setCrop(state, { x: 10, y: 20, w: 300, h: 200 });
    expect(state.crop).toEqual({ x: 10, y: 20, w: 300, h: 200 });
    state = setCrop(state, { x: 100, y: 100, w: 500, h: 400 });
    state = undo(state);
    expect(state.crop).toEqual({ x: 10, y: 20, w: 300, h: 200 });
    state = undo(state);
    expect(state.crop).toBeNull();
  });

  it("does not record a history entry for an unchanged crop", () => {
    let state = createAnnotationState(1000, 800);
    state = setCrop(state, { x: 10, y: 20, w: 300, h: 200 });
    const afterFirst = state;
    state = setCrop(state, { x: 10, y: 20, w: 300, h: 200 });
    expect(state).toBe(afterFirst);
    expect(undo(state).crop).toBeNull();
  });
});

describe("crop", () => {
  it("maps the crop rectangle to output pixels with its offset", () => {
    let state = createAnnotationState(4000, 3000);
    state = setCrop(state, { x: 250, y: 125, w: 1000, h: 750 });
    expect(cropOutput(state)).toEqual({
      width: 1000,
      height: 750,
      offsetX: 250,
      offsetY: 125,
    });
  });

  it("returns the full image when no crop is applied", () => {
    const state = createAnnotationState(1280, 720);
    expect(cropOutput(state)).toEqual({
      width: 1280,
      height: 720,
      offsetX: 0,
      offsetY: 0,
    });
  });

  it("rounds fractional rectangles and clamps them inside the image", () => {
    let state = createAnnotationState(1000, 800);
    state = setCrop(state, { x: 10.4, y: 20.6, w: 300.4, h: 199.5 });
    expect(cropOutput(state)).toEqual({
      width: 300,
      height: 200,
      offsetX: 10,
      offsetY: 21,
    });

    // A rectangle dragged past the right/bottom edge is clamped, never negative/overflowing.
    state = setCrop(state, { x: 900, y: 700, w: 500, h: 500 });
    expect(state.crop).toEqual({ x: 900, y: 700, w: 100, h: 100 });
    expect(cropOutput(state)).toEqual({
      width: 100,
      height: 100,
      offsetX: 900,
      offsetY: 700,
    });

    // A rectangle dragged past the top/left edge is clamped to the origin.
    expect(clampCrop({ x: -40, y: -30, w: 300, h: 200 }, 1000, 800)).toEqual({
      x: 0,
      y: 0,
      w: 260,
      h: 170,
    });
  });

  it("normalizes a rectangle dragged up/left by the user into a positive rectangle", () => {
    // The crop tool records the drag from its start point; `clampCrop` receives the normalized
    // rectangle, so a reverse drag produces the same result as a forward drag.
    expect(clampCrop({ x: 400, y: 300, w: -150, h: -100 }, 1000, 800)).toEqual({
      x: 250,
      y: 200,
      w: 150,
      h: 100,
    });
  });

  it("rejects degenerate rectangles and non-finite input", () => {
    expect(clampCrop({ x: 10, y: 10, w: 0, h: 50 }, 1000, 800)).toBeNull();
    expect(clampCrop({ x: 10, y: 10, w: 50, h: 0.2 }, 1000, 800)).toBeNull();
    expect(clampCrop({ x: Number.NaN, y: 0, w: 50, h: 50 }, 1000, 800)).toBeNull();
    expect(clampCrop({ x: 10, y: 10, w: 50, h: 50 }, 0, 0)).toBeNull();
  });

  it("moves a crop rectangle by its delta, keeping the size, and clamps it on release", () => {
    const rect = { x: 100, y: 200, w: 300, h: 150 };
    expect(moveCrop(rect, 25, -40)).toEqual({
      x: 125,
      y: 160,
      w: 300,
      h: 150,
    });
    // Non-finite pointer deltas must never move the selection to NaN.
    expect(moveCrop(rect, Number.NaN, Number.POSITIVE_INFINITY)).toEqual(rect);
    // The overlay follows the pointer freely; the release clamps it inside the image.
    expect(clampCrop(moveCrop(rect, 850, 500), 1000, 800)).toEqual({
      x: 950,
      y: 700,
      w: 50,
      h: 100,
    });
  });

  it("clears the crop with setCrop(state, null)", () => {
    let state = createAnnotationState(1000, 800);
    state = setCrop(state, { x: 10, y: 20, w: 300, h: 200 });
    state = setCrop(state, null);
    expect(state.crop).toBeNull();
    expect(cropOutput(state)).toEqual({
      width: 1000,
      height: 800,
      offsetX: 0,
      offsetY: 0,
    });
  });
});

describe("toImagePoint", () => {
  it("maps display pixels to natural image pixels for a scaled-down view", () => {
    const state = createAnnotationState(4000, 3000);
    const view = { displayWidth: 800, displayHeight: 600 };
    expect(toImagePoint({ x: 400, y: 300 }, state, view)).toEqual([2000, 1500]);
    expect(toImagePoint({ x: 0, y: 0 }, state, view)).toEqual([0, 0]);
    expect(toImagePoint({ x: 800, y: 600 }, state, view)).toEqual([4000, 3000]);
  });

  it("clamps points that leave the displayed image", () => {
    const state = createAnnotationState(4000, 3000);
    const view = { displayWidth: 800, displayHeight: 600 };
    expect(toImagePoint({ x: -20, y: 700 }, state, view)).toEqual([0, 3000]);
    expect(toImagePoint({ x: 2000, y: -5 }, state, view)).toEqual([4000, 0]);
  });

  it("is the identity for a 1:1 view and tolerates a degenerate view", () => {
    const state = createAnnotationState(640, 480);
    expect(
      toImagePoint({ x: 12, y: 34 }, state, { displayWidth: 640, displayHeight: 480 }),
    ).toEqual([12, 34]);
    expect(toImagePoint({ x: 12, y: 34 }, state, { displayWidth: 0, displayHeight: 0 })).toEqual([
      0, 0,
    ]);
    expect(
      toImagePoint({ x: Number.NaN, y: Number.POSITIVE_INFINITY }, state, {
        displayWidth: 640,
        displayHeight: 480,
      }),
    ).toEqual([0, 0]);
  });
});

describe("fitContain", () => {
  it("fits a landscape image into the stage and keeps its aspect ratio", () => {
    expect(fitContain(4000, 3000, 1000, 800)).toEqual({ width: 1000, height: 750 });
    expect(fitContain(4000, 3000, 800, 700)).toEqual({ width: 800, height: 600 });
    // Height is the limiting dimension.
    expect(fitContain(1600, 1200, 1200, 500)).toEqual({ width: 667, height: 500 });
  });

  it("returns a zero size for a degenerate stage or image", () => {
    expect(fitContain(4000, 3000, 0, 800)).toEqual({ width: 0, height: 0 });
    expect(fitContain(0, 0, 800, 600)).toEqual({ width: 0, height: 0 });
    expect(fitContain(4000, 3000, Number.NaN, 800)).toEqual({ width: 0, height: 0 });
  });
});
