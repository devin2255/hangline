/**
 * Pure helpers for the P3-6 trim window (slider clamping, labels, limit refusal).
 *
 * The backend is the only authority (`trim::validate_trim`); these helpers keep the sliders inside
 * the same rules so the page never sends a range the command would reject:
 * `0 ≤ start < end ≤ duration` and `end > start + 100 ms` (equality on 100 is invalid). Sliders
 * move in 10 ms steps, so with integer positions the smallest legal selection is 110 ms.
 */

/** Slider granularity in milliseconds (plan P3-6). */
export const TRIM_STEP_MS = 10;

/** Backend minimum selection in milliseconds, exclusive (`end > start + 100`). */
export const MIN_TRIM_RANGE_MS = 100;

/** Smallest legal selection expressible on a 10 ms grid (`start + 110`). */
export const MIN_SELECTION_MS = MIN_TRIM_RANGE_MS + TRIM_STEP_MS;

/** Rounds `value` down to the slider grid; non-finite values become 0. */
export function snapToStep(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 0;
  return Math.floor(value / TRIM_STEP_MS) * TRIM_STEP_MS;
}

/**
 * Clamps a requested start position so the selection keeps the 110 ms grid minimum below `endMs`.
 *
 * `durationMs` only bounds the result from below (0); the end clamp owns the upper bound.
 */
export function clampStart(nextMs: number, endMs: number, durationMs: number): number {
  const duration = snapToStep(durationMs);
  const end = Math.min(duration, Math.max(0, endMs));
  const maxStart = Math.max(0, end - MIN_SELECTION_MS);
  return Math.min(snapToStep(nextMs), snapToStep(maxStart));
}

/** Clamps a requested end position so it stays at least 110 ms above `startMs` and ≤ duration. */
export function clampEnd(nextMs: number, startMs: number, durationMs: number): number {
  const duration = snapToStep(durationMs);
  const start = Math.max(0, startMs);
  const minEnd = Math.min(duration, start + MIN_SELECTION_MS);
  return Math.max(Math.min(snapToStep(nextMs), duration), minEnd);
}

/** Whether the backend would accept this selection (duration 0 / a sub-110 ms clip cannot). */
export function canSaveTrim(startMs: number, endMs: number, durationMs: number): boolean {
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || !Number.isFinite(durationMs)) {
    return false;
  }
  return (
    startMs >= 0 &&
    endMs <= durationMs &&
    endMs > startMs &&
    endMs - startMs > MIN_TRIM_RANGE_MS
  );
}

/** Selection length in milliseconds (never negative). */
export function selectionMs(startMs: number, endMs: number): number {
  return Math.max(0, endMs - startMs);
}

/**
 * Which hard limit refuses this recording, mirroring `trim::validate_trim` (`>` means over).
 *
 * `null` means the page may offer Save; the backend still re-validates size, duration and range.
 */
export function trimRefusal(
  sizeBytes: number,
  durationMs: number,
  maxBytes: number,
  maxDurationMs: number,
): "tooLarge" | "tooLong" | null {
  if (sizeBytes > maxBytes) return "tooLarge";
  if (durationMs > maxDurationMs) return "tooLong";
  return null;
}

/**
 * Whether a finished export may close the trim window.
 *
 * `trim-target` retargets are ignored while an export is in flight (`savingRef`); this is the
 * second half of the contract for a retarget that still slipped in (for example a queued event
 * delivered right after `trim_video` resolved). Closing the window would hide the item it now
 * shows, so the page stays open instead.
 */
export function shouldCloseAfterSave(savedId: string, currentId: string): boolean {
  return Boolean(savedId) && savedId === currentId;
}

/**
 * Formats a time offset as `mm:ss.S` (e.g. `00:01.5`).
 *
 * Rounds to the nearest 100 ms; non-finite/negative values render as `00:00.0`.
 */
export function formatTrimTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "00:00.0";
  const tenths = Math.round(ms / 100);
  const minutes = Math.floor(tenths / 600);
  const seconds = Math.floor((tenths % 600) / 10);
  const tenth = tenths % 10;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${tenth}`;
}
