/**
 * Tests for the long-press gesture controller (design spec §3.1/§3.7).
 *
 * Contract: a card press becomes a long press only after a 450 ms hold with less than 4 CSS px
 * of pointer travel (matching the drag-out threshold). Moving further, releasing, or cancelling
 * before the deadline must never fire; once it fires, the remaining pointer events of that press
 * are ignored so the release cannot also copy/open, and the next press re-arms the controller.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { LONG_PRESS_MOVE_THRESHOLD_PX, LONG_PRESS_MS, createLongPress } from "./longPress";

function setup(options: { holdMs?: number; moveThresholdPx?: number } = {}) {
  const onLongPress = vi.fn();
  const press = createLongPress({ onLongPress, ...options });
  return { press, onLongPress };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("createLongPress", () => {
  it("pins the spec defaults: 450 ms and a 4 px radial threshold", () => {
    // P3-1 review: the numbers are a product contract (spec §3.1/§3.7), not an implementation
    // detail; a literal test makes an accidental retune fail loudly.
    expect(LONG_PRESS_MS).toBe(450);
    expect(LONG_PRESS_MOVE_THRESHOLD_PX).toBe(4);
  });

  it("fires after the 450 ms default hold, not one millisecond earlier", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    expect(press.held).toBe(false);
    press.pointerDown(100, 100);
    vi.advanceTimersByTime(LONG_PRESS_MS - 1);
    expect(onLongPress).not.toHaveBeenCalled();
    expect(press.held).toBe(false);

    vi.advanceTimersByTime(1);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(press.held).toBe(true);
    expect(vi.getTimerCount()).toBe(0); // the fired hold must not keep a timer alive
  });

  it("falls back to the spec defaults for non-finite options", () => {
    vi.useFakeTimers();
    const nonFiniteHold = setup({ holdMs: Number.NaN });
    nonFiniteHold.press.pointerDown(0, 0);
    vi.advanceTimersByTime(LONG_PRESS_MS - 1);
    expect(nonFiniteHold.onLongPress).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(nonFiniteHold.onLongPress).toHaveBeenCalledTimes(1);

    const nonFiniteThreshold = setup({ moveThresholdPx: Number.POSITIVE_INFINITY });
    nonFiniteThreshold.press.pointerDown(10, 10);
    // A finite 5 px move must still cancel: the invalid option falls back to the real 4 px rule.
    nonFiniteThreshold.press.pointerMove(15, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS);
    expect(nonFiniteThreshold.onLongPress).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("clamps a negative hold to an immediate (still asynchronous) hold", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup({ holdMs: -100 });
    press.pointerDown(0, 0);
    expect(onLongPress).not.toHaveBeenCalled();
    vi.advanceTimersByTime(0);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("uses a custom hold duration when supplied", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup({ holdMs: 900 });

    press.pointerDown(0, 0);
    vi.advanceTimersByTime(899);
    expect(onLongPress).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("cancels on pointerup before the deadline", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    vi.advanceTimersByTime(300);
    press.pointerUp();
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).not.toHaveBeenCalled();
    expect(press.held).toBe(false);
    expect(vi.getTimerCount()).toBe(0); // an early pointerup must clear its pending timer
  });

  it("cancel() prevents a pending hold from firing", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    press.cancel();
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).not.toHaveBeenCalled();
    expect(press.held).toBe(false);
    expect(vi.getTimerCount()).toBe(0); // cancel() must clear its pending timer
  });

  it("does not cancel at exactly the movement threshold", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    press.pointerMove(10 + LONG_PRESS_MOVE_THRESHOLD_PX, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("cancels when movement exceeds the threshold on one axis", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    press.pointerMove(10 + LONG_PRESS_MOVE_THRESHOLD_PX + 0.01, 10);
    expect(vi.getTimerCount()).toBe(0); // a drag must clear the hold timer immediately
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).not.toHaveBeenCalled();
  });

  it("measures movement radially, so a diagonal jitter above 4 px cancels", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    // sqrt(3^2 + 3^2) ≈ 4.24 px > 4 px.
    press.pointerDown(10, 10);
    press.pointerMove(13, 13);
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).not.toHaveBeenCalled();
  });

  it("tolerates movement below the threshold", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    press.pointerMove(12, 12);
    press.pointerMove(11, 11);
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("ignores pointermove and pointerup after firing until the next pointerdown", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);

    // The gesture already won: neither a large move nor the release may restart or duplicate it.
    press.pointerMove(500, 500);
    press.pointerUp();
    vi.advanceTimersByTime(LONG_PRESS_MS * 2);

    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(press.held).toBe(true);
  });

  it("re-arms on the next pointerdown after a hold fired", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS);
    expect(onLongPress).toHaveBeenCalledTimes(1);

    press.pointerDown(20, 20);
    expect(press.held).toBe(false);
    vi.advanceTimersByTime(LONG_PRESS_MS - 1);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);

    expect(onLongPress).toHaveBeenCalledTimes(2);
    expect(press.held).toBe(true);
  });

  it("restarts the deadline when a new pointerdown arrives before the old one fires", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS - 100);
    press.pointerDown(10, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS - 1);
    expect(onLongPress).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("can start a fresh hold after an early release", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerDown(10, 10);
    press.pointerUp();
    press.pointerDown(10, 10);
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("ignores pointermove and pointerup without a pointerdown", () => {
    vi.useFakeTimers();
    const { press, onLongPress } = setup();

    press.pointerMove(50, 50);
    press.pointerUp();
    vi.advanceTimersByTime(LONG_PRESS_MS);

    expect(onLongPress).not.toHaveBeenCalled();
    expect(press.held).toBe(false);
  });
});
