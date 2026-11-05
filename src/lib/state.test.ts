import { describe, expect, it } from "vitest";
import { shouldApplyState } from "./state";

describe("shouldApplyState", () => {
  it("ignores a snapshot older than the last applied revision", () => {
    expect(shouldApplyState(5, 4)).toBe(false);
    expect(shouldApplyState(1, 0)).toBe(false);
    expect(shouldApplyState(42, 41)).toBe(false);
  });

  it("applies an equal revision (equal revisions carry the same full state)", () => {
    // The initial fetch and the startup `state` event both report revision 0.
    expect(shouldApplyState(0, 0)).toBe(true);
    expect(shouldApplyState(7, 7)).toBe(true);
  });

  it("applies a newer revision", () => {
    expect(shouldApplyState(0, 1)).toBe(true);
    expect(shouldApplyState(7, 12)).toBe(true);
  });

  it("applies the first snapshot from the 'nothing applied yet' sentinel", () => {
    expect(shouldApplyState(-1, 0)).toBe(true);
  });
});
