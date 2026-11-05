import { describe, expect, it } from "vitest";
import { DRAG_THRESHOLD_DP, exceedsDragThreshold } from "./dragThreshold";

describe("exceedsDragThreshold", () => {
  it("keeps a small pointer jitter as a click", () => {
    expect(exceedsDragThreshold(0, 0)).toBe(false);
    expect(exceedsDragThreshold(DRAG_THRESHOLD_DP, 0)).toBe(false);
    expect(exceedsDragThreshold(3, 2)).toBe(false);
  });

  it("starts a drag after travelling strictly more than the threshold", () => {
    expect(exceedsDragThreshold(DRAG_THRESHOLD_DP + 1, 0)).toBe(true);
    expect(exceedsDragThreshold(0, DRAG_THRESHOLD_DP + 1)).toBe(true);
  });

  it("measures radial distance rather than one axis", () => {
    // sqrt(3^2 + 3^2) ≈ 4.24 dp > 4 dp.
    expect(exceedsDragThreshold(3, 3)).toBe(true);
    // sqrt(4^2 + 0.5^2) ≈ 4.03 dp > 4 dp.
    expect(exceedsDragThreshold(4, 0.5)).toBe(true);
  });
});
