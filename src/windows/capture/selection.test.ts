/**
 * Pure selection math for the freeze-frame overlay (Task 16).
 *
 * The overlay window covers the whole virtual desktop in physical pixels but lays its content out
 * in CSS pixels at one `devicePixelRatio`; every assertion here is about that conversion staying
 * exact when the drag goes up/left, crosses a 150% monitor or starts at a negative virtual origin.
 */

import { describe, expect, it } from "vitest";
import {
  PRESSED_SELECTION_DROP_MS,
  clampSelection,
  frozenFrameCssSize,
  isSelectionLargeEnough,
  normalizeSelection,
  physicalPointToCss,
  shouldDropPressedSelection,
  toPhysicalRect,
  viewportScale,
} from "./selection";

describe("normalizeSelection", () => {
  it("normalizes drags in every direction to a positive-size rectangle", () => {
    const expected = { x: 40, y: 20, w: 60, h: 60 };
    expect(normalizeSelection({ x: 40, y: 20 }, { x: 100, y: 80 })).toEqual(expected);
    expect(normalizeSelection({ x: 100, y: 80 }, { x: 40, y: 20 })).toEqual(expected);
    expect(normalizeSelection({ x: 100, y: 20 }, { x: 40, y: 80 })).toEqual(expected);
    expect(normalizeSelection({ x: 40, y: 80 }, { x: 100, y: 20 })).toEqual(expected);
  });

  it("keeps a zero-size rectangle for a click without movement", () => {
    expect(normalizeSelection({ x: 10, y: 10 }, { x: 10, y: 10 })).toEqual({
      x: 10,
      y: 10,
      w: 0,
      h: 0,
    });
  });
});

describe("isSelectionLargeEnough", () => {
  it("rejects accidental clicks and hairline drags", () => {
    expect(isSelectionLargeEnough({ x: 0, y: 0, w: 2, h: 2 })).toBe(true);
    expect(isSelectionLargeEnough({ x: 0, y: 0, w: 1, h: 100 })).toBe(false);
    expect(isSelectionLargeEnough({ x: 0, y: 0, w: 100, h: 1 })).toBe(false);
    expect(isSelectionLargeEnough({ x: 0, y: 0, w: 0, h: 0 })).toBe(false);
  });
});

describe("clampSelection", () => {
  it("clamps pointer capture coordinates that leave the window", () => {
    const viewport = { w: 2560, h: 720 };
    expect(clampSelection({ x: -20, y: -10, w: 50, h: 30 }, viewport.w, viewport.h)).toEqual({
      x: 0,
      y: 0,
      w: 30,
      h: 20,
    });
    expect(
      clampSelection({ x: 2500, y: 700, w: 200, h: 100 }, viewport.w, viewport.h),
    ).toEqual({ x: 2500, y: 700, w: 60, h: 20 });
    expect(clampSelection({ x: 3000, y: 0, w: 100, h: 100 }, viewport.w, viewport.h)).toEqual({
      x: 2560,
      y: 0,
      w: 0,
      h: 100,
    });
  });
});

describe("toPhysicalRect", () => {
  it("maps CSS pixels to virtual-desktop physical pixels across a 150% monitor", () => {
    const origin = { x: -1920, y: 0 };
    expect(toPhysicalRect({ x: 0, y: 0, w: 100, h: 50 }, 1.5, origin)).toEqual({
      x: -1920,
      y: 0,
      w: 150,
      h: 75,
    });
    expect(toPhysicalRect({ x: 100, y: 50, w: 20, h: 10 }, 1.5, origin)).toEqual({
      x: -1770,
      y: 75,
      w: 30,
      h: 15,
    });
  });

  it("never assumes 100% and rounds instead of truncating", () => {
    expect(toPhysicalRect({ x: 0, y: 0, w: 10, h: 10 }, 1, { x: 0, y: 0 })).toEqual({
      x: 0,
      y: 0,
      w: 10,
      h: 10,
    });
    // 1 CSS px at 150% is 1.5 physical px. Edges are rounded separately, so the covered device
    // span is round(2 * 1.5) - round(1 * 1.5) = 1 px (not an independently rounded 2 px width).
    expect(toPhysicalRect({ x: 1, y: 1, w: 1, h: 1 }, 1.5, { x: 0, y: 0 })).toEqual({
      x: 2,
      y: 2,
      w: 1,
      h: 1,
    });
    // A 2.0 DPR secondary monitor (200%) works the same way.
    expect(toPhysicalRect({ x: 5, y: 5, w: 5, h: 5 }, 2, { x: -2560, y: -200 })).toEqual({
      x: -2550,
      y: -190,
      w: 10,
      h: 10,
    });
  });

  it("rounds each edge independently so selections do not drift", () => {
    // 3.3 CSS px at 150%: left edge 4.95 -> 5, right edge 8.25 -> 8: a 3 px device span.
    expect(toPhysicalRect({ x: 3.3, y: 0, w: 2.2, h: 10 }, 1.5, { x: 0, y: 0 })).toEqual({
      x: 5,
      y: 0,
      w: 3,
      h: 15,
    });
    // Two adjacent selections must tile the device grid without overlap or gap.
    const left = toPhysicalRect({ x: 100, y: 10, w: 33, h: 20 }, 1.5, { x: -1920, y: 0 });
    const right = toPhysicalRect({ x: 133, y: 10, w: 33, h: 20 }, 1.5, { x: -1920, y: 0 });
    expect(left.x + left.w).toBe(right.x);
    // The dimension label uses the same helper: a normal full rectangle keeps the exact size.
    expect(toPhysicalRect({ x: 0, y: 0, w: 200, h: 100 }, 1.5, { x: 0, y: 0 })).toEqual({
      x: 0,
      y: 0,
      w: 300,
      h: 150,
    });
  });

  it("derives the physical rectangle from an actual CDP/real-mouse drag", () => {
    // CSS (148, 66) size 266x200 at 150% with the left monitor at x = -1920.
    expect(
      toPhysicalRect({ x: 148, y: 66, w: 266, h: 200 }, 1.5, { x: -1920, y: 0 }),
    ).toEqual({ x: -1698, y: 99, w: 399, h: 300 });
  });
});

describe("frozenFrameCssSize", () => {
  it("scales the physical frozen frame down to CSS pixels with the window DPR", () => {
    expect(frozenFrameCssSize({ w: 3840, h: 1080 }, 1.5)).toEqual({ width: 2560, height: 720 });
    expect(frozenFrameCssSize({ w: 3840, h: 1080 }, 1)).toEqual({ width: 3840, height: 1080 });
    expect(frozenFrameCssSize({ w: 2560, h: 1440 }, 2)).toEqual({ width: 1280, height: 720 });
  });

  it("falls back to 100% when the DPR is unavailable", () => {
    expect(frozenFrameCssSize({ w: 800, h: 600 }, 0)).toEqual({ width: 800, height: 600 });
  });
});

describe("viewportScale", () => {
  it("derives the real CSS-to-physical scale from the physical virtual screen and the CSS viewport", () => {
    // Unlocked run: 3840 px physical window, 3840 CSS viewport -> 1.0 (matches devicePixelRatio).
    expect(viewportScale(3840, 3840, 1)).toBe(1);
    // Locked/mixed-DPI run: 3840 px physical window, 2560 CSS viewport -> 1.5 even though WebView2
    // reported devicePixelRatio = 1; using the reported value would crop the wrong region.
    expect(viewportScale(3840, 2560, 1)).toBe(1.5);
    expect(viewportScale(2560, 1280, 2)).toBe(2);
  });

  it("falls back to devicePixelRatio before the viewport is measurable", () => {
    expect(viewportScale(3840, 0, 1.5)).toBe(1.5);
    expect(viewportScale(0, 2560, 1.5)).toBe(1.5);
    expect(viewportScale(3840, 0, 0)).toBe(1);
  });
});

describe("physicalPointToCss (W1.6 空窗补按)", () => {
  it("maps a virtual-desktop physical press point into overlay CSS pixels", () => {
    // 150% monitor with the virtual origin at x = -1920: physical (-1770, 75) is CSS (100, 50).
    expect(
      physicalPointToCss({ x: -1770, y: 75 }, { x: -1920, y: 0 }, 1.5, 2560, 720),
    ).toEqual({ x: 100, y: 50 });
    // 100% overlay: the physical point is already the CSS point.
    expect(physicalPointToCss({ x: 640, y: 480 }, { x: 0, y: 0 }, 1, 3840, 1080)).toEqual({
      x: 640,
      y: 480,
    });
  });

  it("keeps fractional CSS coordinates (no rounding before the drag starts)", () => {
    const css = physicalPointToCss({ x: 5, y: 8 }, { x: 0, y: 0 }, 1.5, 2560, 720);
    expect(css.x).toBeCloseTo(10 / 3, 10);
    expect(css.y).toBeCloseTo(16 / 3, 10);
  });

  it("clamps a press point that lands outside the frozen frame to the viewport", () => {
    // A press on a monitor to the right of the captured window (or a stale rect) cannot start the
    // drag off-screen.
    expect(physicalPointToCss({ x: 5000, y: 9000 }, { x: 0, y: 0 }, 1, 1920, 1080)).toEqual({
      x: 1920,
      y: 1080,
    });
    expect(physicalPointToCss({ x: -100, y: -50 }, { x: 0, y: 0 }, 1, 1920, 1080)).toEqual({
      x: 0,
      y: 0,
    });
  });

  it("survives malformed numbers instead of producing NaN coordinates", () => {
    expect(
      physicalPointToCss(
        { x: Number.NaN, y: Number.POSITIVE_INFINITY },
        { x: 0, y: 0 },
        1.5,
        1920,
        1080,
      ),
    ).toEqual({ x: 0, y: 0 });
    // Invalid scale falls back to 1; a non-measurable viewport clamps only at the lower edge.
    expect(
      physicalPointToCss({ x: 10, y: 20 }, { x: 0, y: 0 }, Number.NaN, Number.NaN, Number.NaN),
    ).toEqual({ x: 10, y: 20 });
    expect(physicalPointToCss({ x: 10, y: 20 }, { x: 0, y: 0 }, 0, 0, 0)).toEqual({
      x: 10,
      y: 20,
    });
  });
});

describe("shouldDropPressedSelection (W1.6 drop rule)", () => {
  it("keeps the borrowed selection while input is still arriving", () => {
    expect(shouldDropPressedSelection(0, true)).toBe(false);
    expect(shouldDropPressedSelection(10_000, true)).toBe(false);
  });

  it("drops it only after the 150 ms window with no input at all", () => {
    expect(shouldDropPressedSelection(PRESSED_SELECTION_DROP_MS - 1, false)).toBe(false);
    expect(shouldDropPressedSelection(PRESSED_SELECTION_DROP_MS, false)).toBe(true);
    expect(shouldDropPressedSelection(PRESSED_SELECTION_DROP_MS + 1000, false)).toBe(true);
    expect(PRESSED_SELECTION_DROP_MS).toBe(150);
  });

  it("never drops on a broken clock (NaN / negative elapsed)", () => {
    expect(shouldDropPressedSelection(Number.NaN, false)).toBe(false);
    expect(shouldDropPressedSelection(Number.POSITIVE_INFINITY, false)).toBe(false);
    expect(shouldDropPressedSelection(-5, false)).toBe(false);
  });
});
