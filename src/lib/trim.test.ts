import { describe, expect, it } from "vitest";
import {
  MIN_SELECTION_MS,
  canSaveTrim,
  clampEnd,
  clampStart,
  formatTrimTime,
  selectionMs,
  shouldCloseAfterSave,
  snapToStep,
  trimRefusal,
} from "./trim";

describe("trim slider clamping", () => {
  it("snaps to the 10 ms slider grid", () => {
    expect(snapToStep(0)).toBe(0);
    expect(snapToStep(9.9)).toBe(0);
    expect(snapToStep(10)).toBe(10);
    expect(snapToStep(1499.9)).toBe(1490);
    expect(snapToStep(-5)).toBe(0);
    expect(snapToStep(Number.NaN)).toBe(0);
    expect(snapToStep(Number.POSITIVE_INFINITY)).toBe(0);
  });

  it("never lets the start slider cross the 110 ms minimum below the end", () => {
    // 1500 - 110 = 1390 is the highest legal start.
    expect(clampStart(1450, 1500, 3000)).toBe(1390);
    expect(clampStart(1390, 1500, 3000)).toBe(1390);
    expect(clampStart(1400, 1500, 3000)).toBe(1390);
    expect(clampStart(-20, 1500, 3000)).toBe(0);
    expect(clampStart(100, 1500, 3000)).toBe(100);
  });

  it("never lets the end slider come closer than 110 ms to the start or past duration", () => {
    expect(clampEnd(1000, 1000, 3000)).toBe(1110);
    // 1100 - 1000 = 100 is exactly the invalid boundary; the grid minimum pushes it to 1110.
    expect(clampEnd(1100, 1000, 3000)).toBe(1110);
    expect(clampEnd(1110, 1000, 3000)).toBe(1110);
    expect(clampEnd(5000, 1000, 3000)).toBe(3000);
    expect(clampEnd(2500, 1000, 3000)).toBe(2500);
  });

  it("clamps the end to the slider maximum even when the duration is off-grid", () => {
    // 2999 floors to 2990 on the 10 ms grid; the backend still validates the exact range.
    expect(clampEnd(2999, 0, 2999)).toBe(2990);
    expect(clampStart(3000, 2999, 2999)).toBe(2880);
  });

  it("keeps a degenerate clip unsaveable instead of overflowing the sliders", () => {
    // The end simply pins to the (sub-minimum) duration; `canSaveTrim` keeps Save disabled.
    expect(clampEnd(0, 0, 50)).toBe(50);
    expect(clampEnd(-10, 0, 50)).toBe(50);
    expect(canSaveTrim(0, 0, 50)).toBe(false);
    expect(canSaveTrim(0, 100, 3000)).toBe(false);
    expect(canSaveTrim(0, 110, 3000)).toBe(true);
    expect(canSaveTrim(0, 101, 3000)).toBe(true);
    expect(canSaveTrim(0, 102, 3000)).toBe(true);
    expect(canSaveTrim(0, 3001, 3000)).toBe(false);
    expect(canSaveTrim(-10, 1000, 3000)).toBe(false);
    expect(canSaveTrim(0, 1000, 1000)).toBe(true);
  });

  it("matches the backend's exclusive 100 ms minimum", () => {
    // The backend rejects `end - start == 100`; the grid minimum is therefore 110 ms.
    expect(MIN_SELECTION_MS).toBe(110);
    expect(canSaveTrim(0, 100, 3000)).toBe(false);
    expect(selectionMs(1000, 2500)).toBe(1500);
    expect(selectionMs(2500, 1000)).toBe(0);
  });
});

describe("trim refusal mirrors the backend caps", () => {
  it("refuses files over the byte limit and durations over the time limit", () => {
    const maxBytes = 2 * 1024 * 1024 * 1024;
    const maxDurationMs = 60 * 60 * 1000;
    expect(trimRefusal(maxBytes, 3000, maxBytes, maxDurationMs)).toBeNull();
    expect(trimRefusal(maxBytes + 1, 3000, maxBytes, maxDurationMs)).toBe("tooLarge");
    expect(trimRefusal(maxBytes, 3000, maxBytes, maxDurationMs)).toBeNull();
    expect(trimRefusal(1, maxDurationMs + 1, maxBytes, maxDurationMs)).toBe("tooLong");
    expect(trimRefusal(1, maxDurationMs, maxBytes, maxDurationMs)).toBeNull();
    // Size is checked first, matching `validate_trim`.
    expect(trimRefusal(maxBytes + 1, maxDurationMs + 1, maxBytes, maxDurationMs)).toBe("tooLarge");
  });
});

describe("trim save close guard", () => {
  it("closes only when the window still shows the saved item", () => {
    expect(shouldCloseAfterSave("item-a", "item-a")).toBe(true);
    // A retarget that raced the export must not close the window showing the new item.
    expect(shouldCloseAfterSave("item-a", "item-b")).toBe(false);
    expect(shouldCloseAfterSave("", "")).toBe(false);
    expect(shouldCloseAfterSave("item-a", "")).toBe(false);
  });
});

describe("trim time labels", () => {
  it("formats mm:ss.S", () => {
    expect(formatTrimTime(0)).toBe("00:00.0");
    expect(formatTrimTime(1500)).toBe("00:01.5");
    expect(formatTrimTime(3000)).toBe("00:03.0");
    expect(formatTrimTime(61_250)).toBe("01:01.3");
    expect(formatTrimTime(3_600_000)).toBe("60:00.0");
  });

  it("renders non-finite and negative offsets as zero", () => {
    expect(formatTrimTime(-5)).toBe("00:00.0");
    expect(formatTrimTime(Number.NaN)).toBe("00:00.0");
  });
});
