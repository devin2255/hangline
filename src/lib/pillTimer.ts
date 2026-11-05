/**
 * Lifetime of the `Copied` pill (design spec §3.1: 1.2 s).
 *
 * The timer is separated from the component so a second copy can restart the same deadline (no
 * stacked timers, no early hide) and so the exact lifetime is unit-tested with fake timers.
 */

/** How long the pill stays visible after a successful copy. */
export const COPIED_PILL_MS = 1200;

/** Restartable one-shot timer; the last `restart` wins. */
export interface CopiedPillTimer {
  /** Shows/keeps the pill for another full lifetime. */
  restart(): void;
  /** Cancels a pending expiry (unmount). */
  cancel(): void;
}

/** Creates a restartable 1.2 s expiry timer for the Copied pill. */
export function createCopiedPillTimer(
  onExpire: () => void,
  delayMs: number = COPIED_PILL_MS,
): CopiedPillTimer {
  let pending: ReturnType<typeof globalThis.setTimeout> | null = null;

  const cancel = () => {
    if (pending === null) return;
    globalThis.clearTimeout(pending);
    pending = null;
  };

  return {
    restart() {
      cancel();
      pending = globalThis.setTimeout(() => {
        pending = null;
        onExpire();
      }, Math.max(0, delayMs));
    },
    cancel,
  };
}
