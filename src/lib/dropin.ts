/**
 * Pure state helpers for the v1.5.0 drag-in drop panel feedback
 * (`docs/superpowers/specs/2026-11-13-drag-in-drop-design.md` §8).
 *
 * Rust's OLE drop target emits three panel events: `dropin-hover` (`{ images }`, an external drag
 * carrying images is over the panel), `dropin-leave` (`{}`) and `dropin-result`
 * (`{ added, duplicates, rejected }`, the ingestion outcome). The panel only *renders* feedback
 * from them — click-through, geometry and focus stay untouched — so every decision is kept here as
 * DOM-free, unit-tested state math:
 *
 * - sanitize the untrusted IPC payloads (a hostile/old payload must not break the panel),
 * - merge repeated hover events without re-rendering when the drag count did not change,
 * - pick the toast for a result,
 * - decide the single in-panel pill (`drop hint > result toast > empty-line hint`),
 * - express the 2 s toast lifetime as pure timestamps (no timers in this module).
 */

import type { DropinHoverEvent, DropinResultEvent } from "./types";

/** How long a `dropin-result` toast stays visible (spec §8: about 2 s). */
export const DROP_TOAST_MS = 2000;

/**
 * i18n key of the "nothing new was hung" toast (the key *is* the English text, matching the
 * existing table style). Both keys exist in `src/i18n/en.json` and `src/i18n/zh-Hans.json`.
 */
export type DropToastKey = "Already on the line" | "Images only";

/** Active drop signal: how many external images are over the panel (`images` may be `0`). */
export interface DropinHover {
  images: number;
}

/**
 * The panel's two drop-feedback states merged for the one-pill decision.
 *
 * `dropToast` is an i18n key (the panel translates it with `t()` at render time), so a language
 * switch while the toast is visible still renders the new table's text.
 */
export interface DropinFeedback {
  /** `dropin-hover` state; `null` = no external drag over the panel. */
  dropActive: DropinHover | null;
  /** Result toast currently shown; `null` = hidden. */
  dropToast: DropToastKey | null;
}

/**
 * The single in-panel pill to render, decided by {@link dropinPillFor}.
 *
 * `emptyHint` is the existing "take a screenshot" capsule; the drop states win over it (spec §8:
 * "empty hint and drop hint are mutually exclusive, the drop hint has priority").
 */
export type DropinPill =
  | { kind: "dropHint"; images: number }
  | { kind: "toast"; toast: DropToastKey }
  | { kind: "emptyHint" }
  | null;

/** Non-negative integer from one untrusted count field (`undefined`/`NaN`/negative → `0`). */
function countOf(value: unknown): number {
  const number = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(number)) return 0;
  return Math.max(0, Math.trunc(number));
}

/** Sanitized image count of a `dropin-hover` payload (the IPC payload is untrusted). */
export function hoverImageCount(payload: DropinHoverEvent | null | undefined): number {
  return countOf(payload?.images);
}

/**
 * Merges a `dropin-hover` event into the current drop-active state.
 *
 * Always enters the drop state (the hover event proves an external drag is over the panel, even
 * when the count is unknown/`0`), but keeps the previous object when the count is unchanged:
 * Rust may emit `dropin-hover` on every `DragOver`, and a new object would re-render the panel
 * dozens of times per second for no visual change.
 */
export function mergeDropHover(
  current: DropinHover | null,
  payload: DropinHoverEvent | null | undefined,
): DropinHover {
  const images = hoverImageCount(payload);
  return current !== null && current.images === images ? current : { images };
}

/**
 * The toast a `dropin-result` event should show, or `null` when nothing should appear.
 *
 * `added > 0` needs no toast: the regular `state` broadcast hangs the new cards (spec §8). When
 * nothing was added:
 *
 * - `rejected > 0` → "Images only" (a non-image/video/decode failure is actionable: nothing will
 *   ever appear until the user drags an image);
 * - else `duplicates > 0` → "Already on the line" (the drop was a no-op, the image is already
 *   hanging);
 * - both > 0 → **rejected wins**. Concatenating the two messages would need a third template and a
 *   longer capsule in a 189 dp-high panel, and the rejection is the part the user can act on; the
 *   duplicate half only explains why some of the images did not come back.
 * - all zero (defensive: an empty/malformed payload) → `null`, no toast.
 */
export function dropToastFor(
  result: Partial<DropinResultEvent> | null | undefined,
): DropToastKey | null {
  if (!result) return null;
  if (countOf(result.added) > 0) return null;
  if (countOf(result.rejected) > 0) return "Images only";
  if (countOf(result.duplicates) > 0) return "Already on the line";
  return null;
}

/**
 * The one pill to render: drop hint → result toast → empty-line hint.
 *
 * The drop hint is an instruction ("release to hang"), the toast reports the outcome, and the
 * empty-line hint is the idle placeholder; only one capsule may exist at a time. A populated line
 * has no empty hint (`null`), so the panel only shows something while dropping or right after a
 * drop that added nothing.
 */
export function dropinPillFor(feedback: DropinFeedback, itemCount: number): DropinPill {
  if (feedback.dropActive) {
    return { kind: "dropHint", images: feedback.dropActive.images };
  }
  if (feedback.dropToast) {
    return { kind: "toast", toast: feedback.dropToast };
  }
  const count = Number.isFinite(itemCount) ? itemCount : 0;
  return count > 0 ? null : { kind: "emptyHint" };
}

/**
 * Absolute time (ms) a toast shown at `shownAt` auto-hides.
 *
 * Pure so the 2 s contract is unit-testable without fake timers; the panel computes the timer
 * delay and the expiry guard from this same function.
 */
export function dropToastExpiresAt(shownAt: number, durationMs: number = DROP_TOAST_MS): number {
  const start = Number.isFinite(shownAt) ? shownAt : 0;
  const duration = Number.isFinite(durationMs) ? Math.max(0, durationMs) : DROP_TOAST_MS;
  return start + duration;
}

/** Whether a toast shown at `shownAt` must be hidden by `now` (pure; no timers). */
export function isDropToastExpired(
  shownAt: number,
  now: number,
  durationMs: number = DROP_TOAST_MS,
): boolean {
  return now >= dropToastExpiresAt(shownAt, durationMs);
}
