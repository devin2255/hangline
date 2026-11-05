import { describe, expect, it } from "vitest";
import { FALL, FLY } from "../../lib/motion";
import { cssScale, fallFrame, flyFrame } from "./motion";

describe("fly overlay frame math", () => {
  it("starts as a bare clip-less rectangle and ends as the final glass card", () => {
    const start = flyFrame(0);
    expect(start.widthLerp).toBe(0);
    expect(start.heightLerp).toBe(0);
    expect(start.arcDp).toBeCloseTo(0, 9);
    expect(start.insetDp).toBe(0);
    expect(start.radiusDp).toBe(0);
    expect(start.chromeOpacity).toBe(0);

    const end = flyFrame(1);
    expect(end.widthLerp).toBe(1);
    expect(end.heightLerp).toBe(1);
    expect(end.arcDp).toBeCloseTo(0, 9);
    expect(end.insetDp).toBe(4);
    expect(end.radiusDp).toBe(16);
    expect(end.chromeOpacity).toBe(1);
  });

  it("clamps progress outside [0, 1] and keeps the arc symmetric", () => {
    expect(flyFrame(-1)).toEqual(flyFrame(0));
    expect(flyFrame(2)).toEqual(flyFrame(1));
    expect(flyFrame(0.25).arcDp).toBeCloseTo(flyFrame(0.75).arcDp, 9);
    // sin(pi/2) * 30 dp is the peak of the upward arch.
    expect(flyFrame(0.5).arcDp).toBeCloseTo(FLY.arcDp, 9);
    // Chrome (glass/shadow/pin) fades in only after the source start fraction.
    expect(flyFrame(FLY.chromeStart).chromeOpacity).toBe(0);
    expect(flyFrame(FLY.chromeStart + 0.05).chromeOpacity).toBeGreaterThan(0);
  });

  it("keeps frame values inside the source ranges", () => {
    for (const k of [0, 0.1, 0.25, 0.35, 0.5, 0.75, 0.9, 1]) {
      const frame = flyFrame(k);
      expect(frame.insetDp).toBeGreaterThanOrEqual(0);
      expect(frame.insetDp).toBeLessThanOrEqual(4);
      expect(frame.radiusDp).toBeGreaterThanOrEqual(0);
      expect(frame.radiusDp).toBeLessThanOrEqual(16);
      expect(frame.chromeOpacity).toBeGreaterThanOrEqual(0);
      expect(frame.chromeOpacity).toBeLessThanOrEqual(1);
      expect(frame.arcDp).toBeGreaterThanOrEqual(-0.001);
      expect(frame.arcDp).toBeLessThanOrEqual(FLY.arcDp + 0.001);
    }
  });

  it("falls with monotonically growing drop and shrinking opacity", () => {
    let previousDrop = -1;
    let previousOpacity = 2;
    for (const k of [0, 0.2, 0.4, 0.6, 0.8, 1]) {
      const frame = fallFrame(k);
      expect(frame.dropDp).toBeGreaterThanOrEqual(previousDrop);
      expect(frame.opacity).toBeLessThanOrEqual(previousOpacity);
      previousDrop = frame.dropDp;
      previousOpacity = frame.opacity;
    }
    expect(fallFrame(0).dropDp).toBe(0);
    expect(fallFrame(0).opacity).toBe(1);
    expect(fallFrame(1).dropDp).toBeCloseTo(FALL.dropDp, 9);
    expect(fallFrame(1).opacity).toBe(0);
    // `k³` easing: at the midpoint only an eighth of the drop has happened.
    expect(fallFrame(0.5).dropDp).toBeCloseTo(FALL.dropDp / 8, 9);
    // Constant extra tilt (the +20° term from spec §4.6 B) follows the same `k³` ease as the drop.
    expect(fallFrame(0.5).extraTiltDeg).toBeCloseTo(2.5, 9);
    expect(fallFrame(1).extraTiltDeg).toBeCloseTo(20, 9);
    expect(fallFrame(1).ease).toBe(1);
  });
});

describe("overlay CSS scale", () => {
  it("converts physical virtual-screen pixels to CSS pixels at 100%/150%/200% DPI", () => {
    // 100%: 1920 physical px over 1920 CSS px.
    expect(cssScale(1920, 1920)).toBeCloseTo(1, 9);
    // 150%: the overlay window is 1920 physical px but 1280 CSS px wide.
    expect(cssScale(1280, 1920)).toBeCloseTo(1280 / 1920, 9);
    // 200% (only reachable through a resized window, but the ratio must hold everywhere).
    expect(cssScale(960, 1920)).toBeCloseTo(0.5, 9);
  });

  it("falls back to CSS-per-physical 1/DPR (then 1) when the virtual rect is unknown", () => {
    expect(cssScale(1280, 0, 1.5)).toBeCloseTo(1 / 1.5, 9);
    expect(cssScale(1280, -1920, 1.5)).toBeCloseTo(1 / 1.5, 9);
    expect(cssScale(1280, Number.NaN, 1.5)).toBeCloseTo(1 / 1.5, 9);
    expect(cssScale(1280, 0, 0)).toBe(1);
    expect(cssScale(1280, 0, Number.NaN)).toBe(1);
  });
});

