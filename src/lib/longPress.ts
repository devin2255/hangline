/**
 * Long-press gesture controller (design spec §3.1/§3.7, Tendedero `GrabArea.swift`).
 *
 * A press on a card becomes a long press only after a 450 ms hold. Moving more than 4 CSS px
 * (the same radial threshold as the drag-out in `src/windows/panel/dragThreshold.ts`) or
 * releasing before the deadline cancels it, so a slow click and a drag never open the editor.
 * The controller is timer-injected through `globalThis.setTimeout`/`clearTimeout` and has no DOM
 * dependency, which lets the tests drive the clock with fake timers.
 *
 * Once the hold fires, `held` stays true and the rest of that press's move/up events are ignored;
 * this is what lets the card swallow the click that follows a long press. The next `pointerDown`
 * re-arms the controller (`held` becomes false again). `pointerUp` never fires the callback by
 * itself, and `cancel()` (unmount, take-down, window blur) drops any pending hold.
 */

/** Handle returned by the global timer (a number in the browser, an object under Node). */
type TimerHandle = ReturnType<typeof globalThis.setTimeout>;

/** Default hold before a press becomes a long press (design spec §3.1). */
export const LONG_PRESS_MS = 450;

/** Pointer travel, in CSS px (dp), that turns the press into a drag and cancels the hold. */
export const LONG_PRESS_MOVE_THRESHOLD_PX = 4;

export interface LongPressOptions {
  /** Hold duration in milliseconds; defaults to {@link LONG_PRESS_MS}. */
  holdMs?: number;
  /** Radial travel that cancels the hold; defaults to {@link LONG_PRESS_MOVE_THRESHOLD_PX}. */
  moveThresholdPx?: number;
  /** Called once when the hold deadline passes without a cancel. */
  onLongPress: () => void;
}

export interface LongPress {
  /** Starts (or restarts) a hold at the pointer's position. */
  pointerDown(x: number, y: number): void;
  /** Cancels the hold once the pointer travels more than the threshold. */
  pointerMove(x: number, y: number): void;
  /** Cancels a pending hold; after it fired it is a no-op. */
  pointerUp(): void;
  /** Drops a pending hold without firing (unmount, take-down, window blur). */
  cancel(): void;
  /** True from the moment the hold fires until the next `pointerDown`. */
  readonly held: boolean;
}

/**
 * Creates one long-press controller, normally per card/pointer target.
 *
 * All state is local: a controller can be cancelled and reused for the next press without
 * allocating a new instance. Timers are read from `globalThis` at call time (not captured at
 * module load) so fake timers installed after import still work.
 */
export function createLongPress({
  holdMs = LONG_PRESS_MS,
  moveThresholdPx = LONG_PRESS_MOVE_THRESHOLD_PX,
  onLongPress,
}: LongPressOptions): LongPress {
  // Non-finite options would make `setTimeout` fire immediately (NaN) or clamp unpredictably
  // (Infinity) and turn the threshold comparison into "never cancels"; fall back to the spec values
  // instead of silently changing the gesture. Negative finite values keep the old clamp to 0.
  const deadlineMs = Number.isFinite(holdMs) ? Math.max(0, holdMs) : LONG_PRESS_MS;
  const threshold = Number.isFinite(moveThresholdPx)
    ? Math.max(0, moveThresholdPx)
    : LONG_PRESS_MOVE_THRESHOLD_PX;
  const thresholdSq = threshold ** 2;
  let timer: TimerHandle | null = null;
  let originX = 0;
  let originY = 0;
  /** A press is in progress and has not fired yet. */
  let pressing = false;
  /** The last press crossed the hold deadline; stays true until the next `pointerDown`. */
  let held = false;

  const stopPending = () => {
    pressing = false;
    if (timer === null) return;
    globalThis.clearTimeout(timer);
    timer = null;
  };

  return {
    pointerDown(x, y) {
      // A second down (e.g. a repeated press before the deadline) starts over from the new point.
      stopPending();
      originX = x;
      originY = y;
      pressing = true;
      held = false;
      timer = globalThis.setTimeout(() => {
        timer = null;
        if (!pressing) return;
        pressing = false;
        held = true;
        onLongPress();
      }, deadlineMs);
    },
    pointerMove(x, y) {
      if (!pressing) return;
      const dx = x - originX;
      const dy = y - originY;
      if (dx * dx + dy * dy > thresholdSq) stopPending();
    },
    pointerUp() {
      if (!pressing) return;
      stopPending();
    },
    cancel: stopPending,
    get held() {
      return held;
    },
  };
}
