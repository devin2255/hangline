/**
 * Card click/double-click state machine (design spec §3.1).
 *
 * A single click must not copy immediately: the first click of a double-click would otherwise
 * pollute the clipboard. The copy is therefore delayed by the system double-click interval
 * (`get_double_click_interval_ms`). A second click cancels the pending copy, and the browser's
 * `dblclick` opens the file. Keeping this as a pure, timer-injected controller makes the timing
 * rules testable without a DOM.
 */

/** Timeout handle type (a number in the browser, an object under Node's fake timers). */
type TimerHandle = ReturnType<typeof globalThis.setTimeout>;

/**
 * How long after a long press the follow-up click/double-click stays suppressed.
 *
 * A hold fires mid-press, so the click delivered on release must be swallowed. If the user then
 * clicks again quickly (inside the system double-click interval), the browser can turn that second
 * click into a `dblclick` that would open the file; the guard therefore lasts a little longer than
 * the interval and is refreshed by every hold.
 */
export const HOLD_CLICK_GUARD_MS = 700;

/**
 * Whether a `click`/`dblclick` must be swallowed because a press became a long press.
 *
 * `held` is the long-press controller's latch (true from the deadline until the next pointerdown),
 * and `holdFiredAtMs` is the time the hold fired (performance.now() scale). The two together cover
 * both the immediate click on release and a quick second click that would otherwise double-click
 * open the file. The guard is bounded, so a deliberate later click behaves normally.
 */
export function shouldSuppressCardClick(
  held: boolean,
  nowMs: number,
  holdFiredAtMs: number | null,
  guardMs: number = HOLD_CLICK_GUARD_MS,
): boolean {
  if (held) return true;
  if (holdFiredAtMs === null) return false;
  if (!Number.isFinite(nowMs) || !Number.isFinite(holdFiredAtMs)) return false;
  const guard = Number.isFinite(guardMs) ? Math.max(0, guardMs) : HOLD_CLICK_GUARD_MS;
  const elapsed = nowMs - holdFiredAtMs;
  return elapsed >= 0 && elapsed < guard;
}

export interface CardGestureOptions {
  /** System double-click interval in milliseconds (`GetDoubleClickTime`). */
  doubleClickMs: number;
  /** Called after a confirmed single click. */
  onCopy: () => void;
  /** Called for a double-click. */
  onOpen: () => void;
}

export interface CardGesture {
  /**
   * Handles one `click` event.
   *
   * `detail === 1` schedules the delayed copy; `detail >= 2` only cancels the pending copy (the
   * following `dblclick` performs the open, so the action never runs twice).
   */
  click(detail: number): void;
  /** Handles `dblclick`: cancels any pending copy and opens immediately. */
  doubleClick(): void;
  /** Cancels a pending copy (unmount, take-down, window blur). */
  cancel(): void;
}

/**
 * Creates the per-card gesture controller.
 *
 * Every new first click restarts the deadline; the last click wins. `doubleClick` never copies.
 */
export function createCardGesture({
  doubleClickMs,
  onCopy,
  onOpen,
}: CardGestureOptions): CardGesture {
  let pending: TimerHandle | null = null;

  const cancel = () => {
    if (pending === null) return;
    globalThis.clearTimeout(pending);
    pending = null;
  };

  return {
    click(detail) {
      cancel();
      if (detail > 1) return;
      pending = globalThis.setTimeout(() => {
        pending = null;
        onCopy();
      }, Math.max(0, doubleClickMs));
    },
    doubleClick() {
      cancel();
      onOpen();
    },
    cancel,
  };
}
