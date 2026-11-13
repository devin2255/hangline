import { describe, expect, it } from "vitest";
import {
  DROP_TOAST_MS,
  dropinPillFor,
  dropToastExpiresAt,
  dropToastFor,
  hoverImageCount,
  isDropToastExpired,
  mergeDropHover,
} from "./dropin";
import type { DropinFeedback, DropinHover } from "./dropin";

/** A result with every count zero; individual tests override the interesting field. */
function result(overrides: { added?: number; duplicates?: number; rejected?: number }) {
  return { added: 0, duplicates: 0, rejected: 0, ...overrides };
}

/** The no-feedback baseline: nothing dragged, no toast. */
const IDLE: DropinFeedback = { dropActive: null, dropToast: null };

describe("dropToastFor: dropin-result → toast", () => {
  it("shows 'Already on the line' when the only skipped images were duplicates", () => {
    expect(dropToastFor(result({ duplicates: 2 }))).toBe("Already on the line");
  });

  it("shows 'Images only' when every dragged item was rejected", () => {
    expect(dropToastFor(result({ rejected: 3 }))).toBe("Images only");
  });

  it("prefers 'Images only' when duplicates and rejected are both present", () => {
    // Both counters are actionable no-op explanations; the rejection wins because it says nothing
    // will appear until the user drags an actual image, while the duplicate only explains the rest.
    expect(dropToastFor(result({ duplicates: 1, rejected: 1 }))).toBe("Images only");
    expect(dropToastFor(result({ duplicates: 5, rejected: 2 }))).toBe("Images only");
  });

  it("shows no toast when at least one card was added", () => {
    // The state broadcast hangs the cards; the panel must not add a second message on top.
    expect(dropToastFor(result({ added: 1 }))).toBeNull();
    expect(dropToastFor(result({ added: 2, duplicates: 1, rejected: 1 }))).toBeNull();
  });

  it("shows no toast for an empty or malformed payload", () => {
    expect(dropToastFor(result({}))).toBeNull();
    expect(dropToastFor(null)).toBeNull();
    expect(dropToastFor(undefined)).toBeNull();
    expect(dropToastFor({ added: Number.NaN, duplicates: -3, rejected: Number.NaN })).toBeNull();
  });
});

describe("hover state merge: dropin-hover", () => {
  it("enters the drop state with the dragged image count", () => {
    const hover = mergeDropHover(null, { images: 3 });
    expect(hover).toEqual({ images: 3 });
  });

  it("keeps the same object for a repeated count so DragOver spam cannot re-render", () => {
    const first = mergeDropHover(null, { images: 2 });
    const repeat = mergeDropHover(first, { images: 2 });
    expect(repeat).toBe(first);
    const changed = mergeDropHover(first, { images: 4 });
    expect(changed).not.toBe(first);
    expect(changed).toEqual({ images: 4 });
  });

  it("sanitizes untrusted counts (missing/NaN/negative/fractional) to a whole number", () => {
    expect(hoverImageCount({ images: 2.9 })).toBe(2);
    expect(hoverImageCount({ images: -4 })).toBe(0);
    expect(hoverImageCount({ images: Number.NaN })).toBe(0);
    expect(hoverImageCount(null)).toBe(0);
    // A malformed payload still proves an external drag is over the panel: highlight the cord.
    expect(mergeDropHover(null, null)).toEqual({ images: 0 });
  });

  it("treats a leave as the end of the drop state (pill falls back to the empty hint)", () => {
    const hovering = mergeDropHover(null, { images: 1 });
    const left: DropinHover | null = null;
    expect(dropinPillFor({ dropActive: hovering, dropToast: null }, 0)).toEqual({
      kind: "dropHint",
      images: 1,
    });
    expect(dropinPillFor({ dropActive: left, dropToast: null }, 0)).toEqual({
      kind: "emptyHint",
    });
  });
});

describe("dropinPillFor: one pill, drop feedback wins", () => {
  it("renders the drop hint while hovering, even over a populated line", () => {
    expect(dropinPillFor({ dropActive: { images: 2 }, dropToast: null }, 5)).toEqual({
      kind: "dropHint",
      images: 2,
    });
  });

  it("prioritizes the drop hint over a toast left from a previous result", () => {
    expect(dropinPillFor({ dropActive: { images: 1 }, dropToast: "Images only" }, 0)).toEqual({
      kind: "dropHint",
      images: 1,
    });
  });

  it("shows the result toast instead of the empty-line hint", () => {
    // Empty line + toast: the capsule slot is shared, and the result wins (spec §8 priority).
    expect(dropinPillFor({ dropActive: null, dropToast: "Already on the line" }, 0)).toEqual({
      kind: "toast",
      toast: "Already on the line",
    });
  });

  it("shows the empty-line hint only on an idle empty line", () => {
    expect(dropinPillFor(IDLE, 0)).toEqual({ kind: "emptyHint" });
    expect(dropinPillFor(IDLE, 3)).toBeNull();
  });
});

describe("result toast lifetime (2 s, pure timestamps)", () => {
  it("expires exactly after DROP_TOAST_MS and not a millisecond earlier", () => {
    expect(DROP_TOAST_MS).toBe(2000);
    const shownAt = 10_000;
    expect(dropToastExpiresAt(shownAt)).toBe(12_000);
    expect(isDropToastExpired(shownAt, 11_999)).toBe(false);
    expect(isDropToastExpired(shownAt, 12_000)).toBe(true);
    expect(isDropToastExpired(shownAt, 14_000)).toBe(true);
  });

  it("supports an injected duration and degrades gracefully on corrupt timestamps", () => {
    expect(dropToastExpiresAt(500, 250)).toBe(750);
    expect(isDropToastExpired(500, 750, 250)).toBe(true);
    // A non-finite start anchors at 0; a negative duration clamps to 0; a non-finite duration
    // falls back to the documented 2 s contract.
    expect(dropToastExpiresAt(Number.NaN, -10)).toBe(0);
    expect(dropToastExpiresAt(Number.NaN, Number.NaN)).toBe(DROP_TOAST_MS);
  });
});
