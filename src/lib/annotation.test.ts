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
  BLUR_RADIUS_MAX_PX,
  BLUR_RADIUS_MIN_PX,
  HIGHLIGHT_ALPHA,
  HIGHLIGHT_COLOR,
  MAX_HISTORY,
  MOSAIC_BLOCK_MAX_PX,
  MOSAIC_BLOCK_MIN_PX,
  NUMBER_RADIUS_MIN_PX,
  addShape,
  blurRadiusPx,
  canRedo,
  canUndo,
  clampCrop,
  createAnnotationState,
  cropOutput,
  fitContain,
  mosaicBlockPx,
  moveCrop,
  nextNumberValue,
  numberRadiusPx,
  numberTextColor,
  redo,
  renderShapes,
  setCrop,
  shapeRect,
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

/** A rect-like shape helper for the rectangle tools (`highlight`, `mosaic`, `blur`). */
function rectShape(tool: Shape["tool"], points: Shape["points"]): Shape {
  return { id: `${tool}-1`, tool, color: "#ff3b30", width: 4, points };
}

/** A number badge with its sequence label. */
function numberShape(label: string): Shape {
  return {
    id: `number-${label}`,
    tool: "number",
    color: "#ff3b30",
    width: 4,
    points: [[50, 50]],
    label,
  };
}

/** A `CanvasImageSource` stand-in with only the natural size the renderer reads. */
function fakeImage(width: number, height: number): CanvasImageSource {
  return { naturalWidth: width, naturalHeight: height } as unknown as CanvasImageSource;
}

interface FakeContext {
  ctx: CanvasRenderingContext2D;
  calls: string[];
  state: Record<string, unknown>;
}

/** Records the renderer's canvas calls and the mutable context state it sets. */
function fakeContext(options: { filter?: boolean } = {}): FakeContext {
  const calls: string[] = [];
  const state: Record<string, unknown> = {
    imageSmoothingEnabled: true,
    globalAlpha: 1,
    filter: options.filter === false ? undefined : "none",
  };
  const target: Record<string, unknown> = {
    save: () => calls.push("save"),
    restore: () => calls.push("restore"),
    beginPath: () => calls.push("beginPath"),
    moveTo: () => undefined,
    lineTo: () => undefined,
    arc: () => calls.push("arc"),
    rect: () => calls.push("rect"),
    clip: () => calls.push("clip"),
    fill: () => calls.push("fill"),
    stroke: () => calls.push("stroke"),
    fillRect: (x: number, y: number, w: number, h: number) =>
      calls.push(`fillRect ${x},${y},${w},${h}`),
    strokeRect: () => calls.push("strokeRect"),
    fillText: (text: string, x: number, y: number) => calls.push(`fillText ${text} ${x},${y}`),
    drawImage: (...args: unknown[]) => calls.push(`drawImage ${args.length}`),
    clearRect: () => undefined,
  };
  for (const key of [
    "strokeStyle",
    "fillStyle",
    "lineWidth",
    "lineCap",
    "lineJoin",
    "font",
    "textAlign",
    "textBaseline",
    "globalAlpha",
    "filter",
    "imageSmoothingEnabled",
  ]) {
    Object.defineProperty(target, key, {
      get: () => state[key],
      set: (value: unknown) => {
        state[key] = value;
        calls.push(`${key} ${String(value)}`);
      },
      enumerable: true,
      configurable: true,
    });
  }
  return { ctx: target as unknown as CanvasRenderingContext2D, calls, state };
}

/** Runs `run` with a minimal `document.createElement("canvas")` (the mosaic scratch canvas). */
function withFakeDocument<T>(run: () => T): T {
  const globals = globalThis as unknown as { document?: unknown };
  const previous = globals.document;
  const scratchContext: Record<string, unknown> = {
    drawImage: () => undefined,
    clearRect: () => undefined,
    imageSmoothingEnabled: false,
  };
  const scratchCanvas: Record<string, unknown> = {
    width: 0,
    height: 0,
    getContext: () => scratchContext,
  };
  scratchContext.canvas = scratchCanvas;
  globals.document = { createElement: () => scratchCanvas };
  try {
    return run();
  } finally {
    globals.document = previous;
  }
}

describe("shapeRect (highlight / mosaic / blur geometry)", () => {
  it("normalizes a drag in either direction into a positive rectangle", () => {
    expect(shapeRect({ points: [[40, 30], [10, 10]] })).toEqual({ x: 10, y: 10, w: 30, h: 20 });
    expect(shapeRect({ points: [[10, 10], [40, 30]] })).toEqual({ x: 10, y: 10, w: 30, h: 20 });
    // A click-sized shape is still a (degenerate) rectangle; the renderers skip it.
    expect(shapeRect({ points: [[10, 10]] })).toEqual({ x: 10, y: 10, w: 0, h: 0 });
  });

  it("rejects missing and non-finite points", () => {
    expect(shapeRect({ points: [] })).toBeNull();
    expect(shapeRect({ points: [[Number.NaN, 0], [10, 10]] })).toBeNull();
    expect(shapeRect({ points: [[0, 0], [Number.POSITIVE_INFINITY, 10]] })).toBeNull();
  });

  it("clamps a pixel-tool rectangle to the base image before sampling", () => {
    const rect = shapeRect(rectShape("mosaic", [[300, 200], [-40, -30]]));
    expect(rect).toEqual({ x: -40, y: -30, w: 340, h: 230 });
    expect(clampCrop(rect as { x: number; y: number; w: number; h: number }, 200, 150)).toEqual({
      x: 0,
      y: 0,
      w: 200,
      h: 150,
    });
  });
});

describe("pixel-tool sizes", () => {
  it("derives the mosaic block size from the stroke width and clamps it", () => {
    expect(mosaicBlockPx(4)).toBe(4);
    expect(mosaicBlockPx(10, 2)).toBe(20);
    expect(mosaicBlockPx(0.5)).toBe(MOSAIC_BLOCK_MIN_PX);
    expect(mosaicBlockPx(500)).toBe(MOSAIC_BLOCK_MAX_PX);
    expect(mosaicBlockPx(Number.NaN)).toBe(MOSAIC_BLOCK_MIN_PX);
  });

  it("derives the blur radius from the stroke width and clamps it", () => {
    expect(blurRadiusPx(4)).toBe(6);
    expect(blurRadiusPx(10, 0.5)).toBe(8);
    expect(blurRadiusPx(1)).toBe(BLUR_RADIUS_MIN_PX);
    expect(blurRadiusPx(1000)).toBe(BLUR_RADIUS_MAX_PX);
  });
});

describe("new tool rendering", () => {
  it("paints a highlight as a 35 % translucent rectangle", () => {
    const { ctx, calls, state } = fakeContext();
    renderShapes(ctx, [rectShape("highlight", [[10, 20], [110, 70]])], 1);
    expect(HIGHLIGHT_ALPHA).toBe(0.35);
    expect(calls).toContain("fillRect 10,20,100,50");
    // The saved PNG calls the same renderer, so this alpha is what the export composites.
    expect(state.globalAlpha).toBe(HIGHLIGHT_ALPHA);
    expect(state.fillStyle).toBe("#ff3b30");
  });

  it("mosaics the base image through a downscaled scratch canvas", () => {
    const { ctx, calls, state } = fakeContext();
    const mosaic = rectShape("mosaic", [[10, 10], [210, 110]]);
    withFakeDocument(() => renderShapes(ctx, [mosaic], 1, fakeImage(1000, 800)));
    expect(calls.some((call) => call.startsWith("imageSmoothingEnabled false"))).toBe(true);
    // One 9-argument drawImage back onto the overlay canvas.
    expect(calls).toContain("drawImage 9");
    expect(state.imageSmoothingEnabled).toBe(true);
  });

  it("blurs by clipping the region and redrawing the base through ctx.filter", () => {
    const { ctx, calls, state } = fakeContext();
    const blur = rectShape("blur", [[0, 0], [100, 50]]);
    renderShapes(ctx, [blur], 1, fakeImage(500, 400));
    expect(calls).toContain("clip");
    expect(calls).toContain("filter blur(6px)");
    expect(calls).toContain("drawImage 5");
    expect(state.filter).toBe("blur(6px)");
  });

  it("falls back to mosaic when the engine supports no canvas filter", () => {
    const { ctx, calls } = fakeContext({ filter: false });
    const blur = rectShape("blur", [[0, 0], [100, 50]]);
    withFakeDocument(() => renderShapes(ctx, [blur], 1, fakeImage(500, 400)));
    expect(calls).not.toContain("clip");
    expect(calls).not.toContain("filter blur(6px)");
    // The mosaic fallback painted (9-argument drawImage) instead of leaving a hole.
    expect(calls).toContain("drawImage 9");
  });

  it("paints nothing for the pixel tools without a base image", () => {
    const { ctx, calls } = fakeContext();
    const shapes = [rectShape("mosaic", [[0, 0], [10, 10]]), rectShape("blur", [[0, 0], [10, 10]])];
    renderShapes(ctx, shapes, 1);
    expect(calls.some((call) => call.startsWith("drawImage"))).toBe(false);
    expect(calls).not.toContain("clip");
  });

  it("paints a number badge with its label in a contrast colour", () => {
    const { ctx, calls, state } = fakeContext();
    renderShapes(ctx, [numberShape("3")], 1);
    expect(calls).toContain("arc");
    expect(calls).toContain("fillText 3 50,50");
    expect(state.textAlign).toBe("center");
    expect(state.fillStyle).toBe("#ffffff");
  });

  it("skips a number badge that carries no label (corrupt/legacy state)", () => {
    const { ctx, calls } = fakeContext();
    const broken: Shape = { id: "n", tool: "number", color: "#ff3b30", width: 4, points: [[0, 0]] };
    renderShapes(ctx, [broken], 1);
    expect(calls).not.toContain("arc");
  });
});

describe("number sequencing", () => {
  it("scales the badge radius with the stroke width", () => {
    expect(numberRadiusPx(2)).toBe(NUMBER_RADIUS_MIN_PX);
    expect(numberRadiusPx(10)).toBe(30);
  });

  it("uses dark digits on pale badge colours and white on dark ones", () => {
    expect(numberTextColor("#ffcc00")).toBe("#111827");
    expect(numberTextColor("#ff3b30")).toBe("#ffffff");
    expect(numberTextColor("not-a-colour")).toBe("#ffffff");
  });

  it("computes the next value from the committed shapes", () => {
    expect(nextNumberValue([])).toBe(1);
    expect(nextNumberValue([numberShape("1"), numberShape("2")])).toBe(3);
    expect(nextNumberValue([numberShape("2"), numberShape("7")])).toBe(8);
    expect(nextNumberValue([numberShape("three")])).toBe(1);
  });

  it("reuses a number after undo instead of skipping it", () => {
    let state = createAnnotationState(800, 600);
    state = addShape(state, numberShape("1"));
    state = addShape(state, numberShape("2"));
    state = addShape(state, numberShape("3"));
    expect(nextNumberValue(state.shapes)).toBe(4);

    state = undo(state);
    expect(nextNumberValue(state.shapes)).toBe(3);
    state = addShape(state, numberShape("3"));
    expect(nextNumberValue(state.shapes)).toBe(4);
  });
});

describe("serialization compatibility", () => {
  it("round-trips the new tools through JSON and still reads legacy shapes", () => {
    const shapes: Shape[] = [
      numberShape("2"),
      { id: "m", tool: "mosaic", color: HIGHLIGHT_COLOR, width: 8, points: [[0, 0], [10, 10]] },
      { id: "h", tool: "highlight", color: HIGHLIGHT_COLOR, width: 4, points: [[0, 0], [10, 10]] },
    ];
    const decoded = JSON.parse(JSON.stringify(shapes)) as Shape[];
    expect(decoded).toEqual(shapes);
    expect(decoded[0].label).toBe("2");

    // State written before W1.3 has no `label`/`text` keys; it must still decode and render.
    const legacy = JSON.parse(
      '{"id":"old","tool":"rect","color":"#ff3b30","width":4,"points":[[0,0],[10,10]]}',
    ) as Shape;
    expect(legacy.label).toBeUndefined();
    expect(nextNumberValue([legacy])).toBe(1);
    const { ctx } = fakeContext();
    expect(() => renderShapes(ctx, [legacy, ...decoded], 1, fakeImage(100, 100))).not.toThrow();
  });
});
