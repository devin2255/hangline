/**
 * Tests for the Copied pill lifetime (Task 12): the pill shown after a successful card copy stays
 * for exactly 1.2 s, and a second copy restarts the timer instead of stacking timers.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { COPIED_PILL_MS, createCopiedPillTimer } from "./pillTimer";

afterEach(() => {
  vi.useRealTimers();
});

describe("createCopiedPillTimer", () => {
  it("hides the pill after 1.2 seconds", () => {
    vi.useFakeTimers();
    const onExpire = vi.fn();
    const timer = createCopiedPillTimer(onExpire);
    timer.restart();

    vi.advanceTimersByTime(COPIED_PILL_MS - 1);
    expect(onExpire).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpire).toHaveBeenCalledTimes(1);
    expect(COPIED_PILL_MS).toBe(1200);
  });

  it("restarts from the latest copy instead of expiring the old deadline", () => {
    vi.useFakeTimers();
    const onExpire = vi.fn();
    const timer = createCopiedPillTimer(onExpire);

    timer.restart();
    vi.advanceTimersByTime(800);
    timer.restart();
    vi.advanceTimersByTime(COPIED_PILL_MS - 1);
    expect(onExpire).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onExpire).toHaveBeenCalledTimes(1);
  });

  it("cancel prevents the expiry callback", () => {
    vi.useFakeTimers();
    const onExpire = vi.fn();
    const timer = createCopiedPillTimer(onExpire);

    timer.restart();
    timer.cancel();
    vi.advanceTimersByTime(COPIED_PILL_MS * 2);

    expect(onExpire).not.toHaveBeenCalled();
  });
});
