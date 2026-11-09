/**
 * Tests for the card pointer gesture state machine (Task 12).
 *
 * Contract: a single click must wait the system double-click interval before copying, so the first
 * click of a double-click never pollutes the clipboard; the second click/double-click cancels the
 * pending copy and opens the file instead (design spec §3.1).
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  HOLD_CLICK_GUARD_MS,
  PRIMARY_POINTER_BUTTON,
  createCardGesture,
  isSecondaryPress,
  shouldRunCardClick,
  shouldStartCardPress,
  shouldSuppressCardClick,
} from "./cardGesture";

const DOUBLE_CLICK_MS = 500;

function setup() {
  const onCopy = vi.fn();
  const onOpen = vi.fn();
  const gesture = createCardGesture({
    doubleClickMs: DOUBLE_CLICK_MS,
    onCopy,
    onOpen,
  });
  return { gesture, onCopy, onOpen };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("createCardGesture", () => {
  it("defers a single click's copy until the double-click interval has passed", () => {
    vi.useFakeTimers();
    const { gesture, onCopy, onOpen } = setup();

    gesture.click(1);
    expect(onCopy).not.toHaveBeenCalled();

    vi.advanceTimersByTime(DOUBLE_CLICK_MS - 1);
    expect(onCopy).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(onCopy).toHaveBeenCalledTimes(1);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("cancels the pending copy and opens on a double-click", () => {
    vi.useFakeTimers();
    const { gesture, onCopy, onOpen } = setup();

    gesture.click(1);
    gesture.click(2);
    gesture.doubleClick();
    vi.advanceTimersByTime(DOUBLE_CLICK_MS * 2);

    expect(onCopy).not.toHaveBeenCalled();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("waits for the dblclick before opening after a second click", () => {
    vi.useFakeTimers();
    const { gesture, onCopy, onOpen } = setup();

    gesture.click(1);
    gesture.click(2);
    expect(onOpen).not.toHaveBeenCalled();
    vi.advanceTimersByTime(DOUBLE_CLICK_MS);
    expect(onCopy).not.toHaveBeenCalled();
    expect(onOpen).not.toHaveBeenCalled();

    gesture.doubleClick();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("cancel prevents a pending copy", () => {
    vi.useFakeTimers();
    const { gesture, onCopy } = setup();

    gesture.click(1);
    gesture.cancel();
    vi.advanceTimersByTime(DOUBLE_CLICK_MS);

    expect(onCopy).not.toHaveBeenCalled();
  });

  it("allows another single click after the interval already copied", () => {
    vi.useFakeTimers();
    const { gesture, onCopy } = setup();

    gesture.click(1);
    vi.advanceTimersByTime(DOUBLE_CLICK_MS);
    expect(onCopy).toHaveBeenCalledTimes(1);

    gesture.click(1);
    vi.advanceTimersByTime(DOUBLE_CLICK_MS);
    expect(onCopy).toHaveBeenCalledTimes(2);
  });

  it("uses the supplied interval for the copy deadline", () => {
    vi.useFakeTimers();
    const onCopy = vi.fn();
    const gesture = createCardGesture({
      doubleClickMs: 900,
      onCopy,
      onOpen: vi.fn(),
    });

    gesture.click(1);
    vi.advanceTimersByTime(899);
    expect(onCopy).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onCopy).toHaveBeenCalledTimes(1);
  });
});

describe("right-button / menu gesture arbitration", () => {
  it("treats every non-primary pointer button as secondary (context menu, middle click)", () => {
    expect(PRIMARY_POINTER_BUTTON).toBe(0);
    expect(isSecondaryPress(0)).toBe(false);
    expect(isSecondaryPress(1)).toBe(true);
    expect(isSecondaryPress(2)).toBe(true);
    expect(isSecondaryPress(3)).toBe(true);
  });

  it("lets a primary press start on an idle, revealed card", () => {
    expect(shouldStartCardPress({ button: 0, locked: false })).toBe(true);
    expect(shouldStartCardPress({ button: 0, locked: false, paused: false, dragging: false })).toBe(
      true,
    );
  });

  it("never starts a press for a right click, an open menu, a paused panel or a native drag", () => {
    expect(shouldStartCardPress({ button: 2, locked: false })).toBe(false);
    expect(shouldStartCardPress({ button: 1, locked: false })).toBe(false);
    expect(shouldStartCardPress({ button: 0, locked: true })).toBe(false);
    expect(shouldStartCardPress({ button: 0, locked: false, paused: true })).toBe(false);
    expect(shouldStartCardPress({ button: 0, locked: false, dragging: true })).toBe(false);
  });

  it("suppresses the click/double-click copy-open path only while the card is locked", () => {
    expect(shouldRunCardClick(false)).toBe(true);
    expect(shouldRunCardClick(true)).toBe(false);
  });
});

describe("shouldSuppressCardClick", () => {
  it("suppresses every click while the long-press latch is held", () => {
    expect(shouldSuppressCardClick(true, 10_000, null)).toBe(true);
    expect(shouldSuppressCardClick(true, 10_000, 1_000)).toBe(true);
  });

  it("suppresses the click that follows a hold during the post-fire guard only", () => {
    expect(shouldSuppressCardClick(false, 1_000, 1_000)).toBe(true);
    expect(shouldSuppressCardClick(false, 1_000 + HOLD_CLICK_GUARD_MS - 1, 1_000)).toBe(true);
    expect(HOLD_CLICK_GUARD_MS).toBeGreaterThanOrEqual(500);
  });

  it("lets a deliberate later click through once the guard has elapsed", () => {
    expect(shouldSuppressCardClick(false, 1_000 + HOLD_CLICK_GUARD_MS, 1_000)).toBe(false);
    expect(shouldSuppressCardClick(false, 9_000, 1_000)).toBe(false);
  });

  it("does not suppress when no hold fired or the clock is not finite", () => {
    expect(shouldSuppressCardClick(false, 1_000, null)).toBe(false);
    expect(shouldSuppressCardClick(false, Number.NaN, 1_000)).toBe(false);
    expect(shouldSuppressCardClick(false, 1_000, Number.POSITIVE_INFINITY)).toBe(false);
    // A clock that went backwards cannot prove the guard is still active.
    expect(shouldSuppressCardClick(false, 500, 1_000)).toBe(false);
  });

  it("accepts a longer guard for a slow system double-click interval", () => {
    expect(shouldSuppressCardClick(false, 1_800, 1_000, 900)).toBe(true);
    expect(shouldSuppressCardClick(false, 1_900, 1_000, 900)).toBe(false);
    // A non-finite custom guard falls back to the pinned default instead of never expiring.
    expect(shouldSuppressCardClick(false, 1_000 + HOLD_CLICK_GUARD_MS, 1_000, Number.NaN)).toBe(
      false,
    );
  });
});
