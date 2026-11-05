import { describe, expect, it } from "vitest";
import { resolveHitRectScale } from "./hitRectScale";

describe("hit-rect DPI scale", () => {
  it("adopts the first observed device pixel ratio", () => {
    expect(resolveHitRectScale(null, 1.5)).toEqual({ scale: 1.5, refreshed: true });
    expect(resolveHitRectScale(null, 1)).toEqual({ scale: 1, refreshed: true });
  });

  it("keeps the cache while the device pixel ratio still matches", () => {
    expect(resolveHitRectScale(1.5, 1.5)).toEqual({ scale: 1.5, refreshed: false });
    expect(resolveHitRectScale(2, 2)).toEqual({ scale: 2, refreshed: false });
  });

  it("refreshes on an equal-logical-size cross-DPI move", () => {
    // 1920 CSS px at 100% is 1920 physical px; the same CSS viewport at 200% is 3840 physical px, so
    // no resize event may fire while the DPR doubles. The cache must follow the browser's ratio.
    expect(resolveHitRectScale(1, 2)).toEqual({ scale: 2, refreshed: true });
    expect(resolveHitRectScale(2, 1)).toEqual({ scale: 1, refreshed: true });
    expect(resolveHitRectScale(1, 1.5)).toEqual({ scale: 1.5, refreshed: true });
    expect(resolveHitRectScale(1.5, 1.25)).toEqual({ scale: 1.25, refreshed: true });
  });

  it("keeps a usable scale for a corrupt device pixel ratio", () => {
    expect(resolveHitRectScale(1.5, Number.NaN)).toEqual({ scale: 1.5, refreshed: false });
    expect(resolveHitRectScale(1.5, 0)).toEqual({ scale: 1.5, refreshed: false });
    expect(resolveHitRectScale(1.5, -2)).toEqual({ scale: 1.5, refreshed: false });
    expect(resolveHitRectScale(null, Number.NaN)).toEqual({ scale: 1, refreshed: false });
    expect(resolveHitRectScale(0, Number.NaN)).toEqual({ scale: 1, refreshed: false });
  });
});
