/**
 * Pure layout model for the top panel (design spec §3.3).
 *
 * Every value here is in logical pixels (dp, = CSS px). Rust owns the monitor work area and
 * sizes the window in *physical* pixels; the webview reports its width back as CSS pixels, so
 * capacity/card positions are all computed against that width. Keeping the model pure makes the
 * spec numbers unit-testable without a DOM (see `layout.test.ts`).
 */

import { EDGE_FADE, TILT } from "./motion";
import type { CardSize, Item, KeepOnLine } from "./types";

export type { CardSize } from "./types";

/** Card scale per size (spec §3.3: 0.8 / 1.0 / 1.35). */
const SCALE: Record<CardSize, number> = { small: 0.8, medium: 1, large: 1.35 };

/** Corner radius of a glass card (spec §3.3: 16 dp). */
export const CARD_RADIUS = 16;
/** Glass frame inset between the card edge and the photo (source `Frame.inset`, 4 dp). */
export const CARD_VISUAL_INSET = 4;
/** Gap between two neighbouring cards (spec §3.3: `spacing = cardWidth + 24`). */
export const CARD_GAP = 24;
/** Clothes-pin width and height (spec §3.3: 9×26 dp). */
export const PIN_WIDTH = 9;
export const PIN_HEIGHT = 26;
/** The pin top hangs this far above the rope it is hooked on (source `pinAbove`, 9.5 dp). */
export const PIN_ABOVE = 9.5;
/** The card top overlaps the pin by this much (source `VStack(spacing: -12)`, 12 dp). */
export const CARD_PIN_OVERLAP = 12;
/** Rope baseline from the top of the panel (spec §3.3: `ropeTop = 10`). */
export const ROPE_TOP = 10;
/** Maximum rope sag (spec §3.3: `sag = min(30, panelWidth * 0.018)`). */
export const ROPE_MAX_SAG = 30;
export const ROPE_SAG_RATIO = 0.018;
/** Horizontal margin budget behind the capacity formula (spec §3.3: 200 dp total). */
export const PANEL_SIDE_MARGIN = 100;
export const MIN_CAPACITY = 3;
export const MAX_CAPACITY = 12;

/** Card width in dp for a size setting: 120 / 150 / 202 (spec §3.3). */
export function cardWidth(size: CardSize): number {
  return size === "small" ? 120 : size === "large" ? 202 : 150;
}

/** Maximum photo height in dp for a size setting: 83 / 104 / 140 (spec §3.3). */
export function photoMaxHeight(size: CardSize): number {
  return Math.round(104 * SCALE[size]);
}

/** Total panel height in dp: `photoMaxHeight + 106` → 189 / 210 / 246 (spec §3.3). */
export function panelHeight(size: CardSize): number {
  return photoMaxHeight(size) + 106;
}

/** Distance between the left edges of two neighbouring cards. */
export function cardSpacing(size: CardSize): number {
  return cardWidth(size) + CARD_GAP;
}

/**
 * How many cards fit on a panel of `panelWidth` dp (spec §3.3):
 * `max(3, min(12, floor((panelWidth - 200) / spacing)))`.
 */
export function capacity(panelWidth: number, size: CardSize): number {
  const fits = Math.floor((panelWidth - 2 * PANEL_SIDE_MARGIN) / cardSpacing(size));
  return Math.max(MIN_CAPACITY, Math.min(MAX_CAPACITY, fits));
}

/** Total width of `capacity` cards, from the first left edge to the last right edge. */
function groupWidth(count: number, size: CardSize): number {
  return count <= 0 ? 0 : count * cardWidth(size) + (count - 1) * CARD_GAP;
}

/**
 * Left edge of card `index` in a centred group of `capacity` cards.
 *
 * The group is centred with equal margins when it fits; when the minimum capacity of 3 cards is
 * wider than the panel (very narrow line, large size) it starts at the left edge. The viewport then
 * right-aligns the wider line ([`lineShift`]) so the newest card stays fully visible and only the
 * oldest edge is clipped — there is no native horizontal scroll.
 */
export function cardX(index: number, capacity: number, size: CardSize, panelWidth: number): number {
  const group = groupWidth(capacity, size);
  const left = Math.max(0, (panelWidth - group) / 2);
  return left + index * cardSpacing(size);
}

/**
 * Width of the line content; at least the panel width, wider only when the minimum card group
 * overflows. The viewport right-aligns such a line with [`lineShift`] and clips the oldest edge.
 */
export function lineWidth(panelWidth: number, capacity: number, size: CardSize): number {
  return Math.max(panelWidth, groupWidth(capacity, size));
}

/**
 * Horizontal shift applied to the line inside the (overflow-hidden) viewport.
 *
 * A line that fits is not shifted (`0`). A line wider than the panel is right-aligned (a negative
 * shift), keeping the newest card — the right edge — fully visible and clipping the oldest edge
 * instead of clipping both sides or exposing a native scrollbar.
 */
export function lineShift(panelWidth: number, line: number): number {
  const shift = panelWidth - line;
  return Number.isFinite(shift) ? Math.min(0, shift) : 0;
}

/** A half-open item range: `items.slice(start, end)`. */
export interface VisibleSlice {
  /** Absolute index of the oldest visible item (`items` is oldest first). */
  start: number;
  /** Absolute index one past the newest visible item. */
  end: number;
}

/** Positive integer helpers: the panel must stay usable for degenerate inputs/NaN. */
function positive(value: number): number {
  const truncated = Math.trunc(value);
  return Number.isFinite(truncated) && truncated > 0 ? truncated : 0;
}

/**
 * Number of cards kept on the line ("maxItems" in the task brief): `max(capacity, keep)`.
 *
 * `keepOnLine: "fit"` keeps exactly what fits; `25/50/100` keep more so the wheel can bring older
 * cards in. The capacity always wins over a smaller keep value, so the panel never renders fewer
 * cards than fit.
 */
export function maxItems(keep: KeepOnLine, capacity: number): number {
  const fits = Math.max(1, positive(capacity));
  return keep === "fit" ? fits : Math.max(fits, positive(keep));
}

/** Limits for one visible-window computation (`keep` is already resolved by [`maxItems`]). */
function windowLimits(total: number, keep: number, capacity: number) {
  const count = positive(total);
  const fits = Math.max(1, positive(capacity));
  // The line keeps the newest `retained` items; older ones fell off the line.
  const retained = Math.min(count, Math.max(fits, positive(keep)));
  return { count, fits, retained };
}

/**
 * Highest `offset` (in cards) the viewport can scroll towards older cards.
 *
 * Scrolling is bounded by the kept window, not by the total item count: with 30 items and
 * `keepOnLine: 25`, the 5 oldest items are already off the line. The line can never scroll past
 * its oldest kept card, so the result never causes a negative `start`.
 */
export function maxScrollOffset(total: number, keep: number, capacity: number): number {
  const { fits, retained } = windowLimits(total, keep, capacity);
  return Math.max(0, retained - fits);
}

/**
 * Visible window of the line: the newest cards are anchored on the right, and each scroll step
 * (`offset`, in whole cards) brings one older card in from the left.
 *
 * `offset = 0` shows the newest `capacity` items (`end = total`); `offset = maxScrollOffset(...)`
 * shows the oldest kept items. Item indices stay absolute (`items.slice(start, end)`), so a caller
 * can render directly from the Rust snapshot. When fewer than `capacity` cards are kept, the whole
 * line is returned and the offset is clamped to zero.
 *
 * Note on the plan's Task 14 Step 1 example (`visibleSlice(30, 25, 10, 0) -> {20,25}`): an `end`
 * of 25 would hide the newest 5 items and render only 5 cards at capacity 10. The example mixed
 * absolute indices (`start = total - capacity`) with keep-window coordinates (`end = keep`); the
 * coherent absolute window is `{start: 20, end: 30}` (and `offset 5 -> {15, 25}`).
 */
export function visibleSlice(
  total: number,
  keep: number,
  capacity: number,
  offset: number,
): VisibleSlice {
  const { count, fits, retained } = windowLimits(total, keep, capacity);
  const shifted = Math.min(Math.max(0, Math.trunc(offset) || 0), Math.max(0, retained - fits));
  const end = count - shifted;
  const start = Math.max(count - retained, end - Math.min(fits, retained));
  return { start, end };
}

/**
 * One wheel event as a scroll step: `+1` moves towards older cards, `-1` towards the newest.
 *
 * Only the dominant axis counts (`|deltaY| >= |deltaX| ? deltaY : deltaX`, spec §3.1), so a
 * slightly diagonal trackpad gesture cannot move the line twice per event. The step is always one
 * card regardless of the delta magnitude: the callback owns the offset, the browser's native
 * horizontal scroll is prevented (`preventDefault`), and there is no second scroll model.
 */
export function wheelStep(deltaX: number, deltaY: number): -1 | 0 | 1 {
  const x = Number.isFinite(deltaX) ? deltaX : 0;
  const y = Number.isFinite(deltaY) ? deltaY : 0;
  const delta = Math.abs(y) >= Math.abs(x) ? y : x;
  if (delta === 0) return 0;
  return delta > 0 ? 1 : -1;
}

/** Applies one wheel event to the scroll offset, clamped to `[0, maxOffset]`. */
export function nextScrollOffset(
  offset: number,
  deltaX: number,
  deltaY: number,
  maxOffset: number,
): number {
  const current = Math.max(0, Math.trunc(offset) || 0);
  const limit = Math.max(0, Math.trunc(maxOffset) || 0);
  const next = current + wheelStep(deltaX, deltaY);
  return Math.min(limit, Math.max(0, next));
}

/** Pin top in dp for a rope height: the pin hangs `PIN_ABOVE` above the rope. */
export function pinTop(ropeTopY: number): number {
  return ropeTopY - PIN_ABOVE;
}

/**
 * Card top in dp for a rope height: the card hangs from the same pin, overlapping its bottom by
 * `CARD_PIN_OVERLAP` (source: pin top = ropeY − 9.5, card top = pin top + 14 → ropeY + 4.5).
 */
export function cardTopFromRope(ropeTopY: number): number {
  return ropeTopY - PIN_ABOVE + (PIN_HEIGHT - CARD_PIN_OVERLAP);
}

/** Photo size in dp: fits `cardWidth − 14` × `photoMaxHeight`, preserving the aspect ratio. */
export function photoSize(
  width: number,
  height: number,
  size: CardSize,
): { width: number; height: number } {
  const maxWidth = cardWidth(size) - 14;
  const maxHeight = photoMaxHeight(size);
  // Unknown dimensions (recordings before the first frame, undecodable files) fill the budget.
  if (!(width > 0) || !(height > 0)) return { width: maxWidth, height: maxHeight };
  const scale = Math.min(maxWidth / width, maxHeight / height);
  return { width: width * scale, height: height * scale };
}

/**
 * Visual card size in dp: the photo plus the `CARD_VISUAL_INSET` glass frame on both sides.
 */
export function cardVisualSize(
  item: Pick<Item, "width" | "height">,
  size: CardSize,
): { width: number; height: number } {
  const photo = photoSize(item.width, item.height, size);
  return {
    width: photo.width + 2 * CARD_VISUAL_INSET,
    height: photo.height + 2 * CARD_VISUAL_INSET,
  };
}

/** Final image fallback state of a card: cached preview → original file → tile. */
export type CardImageSource = "thumb" | "original" | "failed";

/**
 * Whether a card should render its `<img>` (P3-5 recording first frames).
 *
 * Recordings only get an image when a preview exists: without a `thumbPath` the URL would point at
 * the mp4/mov file, the `<img>` would always fail, and the card would lose the filename + play-badge
 * fallback. Images keep the existing thumb → original chain.
 */
export function showCardImage(
  kind: Item["kind"],
  hasThumb: boolean,
  imageSource: CardImageSource,
): boolean {
  if (imageSource === "failed") return false;
  return kind !== "recording" || hasThumb;
}

/**
 * Opacity of a card near the horizontal ends of the line (`LineView.swift:78`):
 * `min(1, max(0, min(x, panelWidth − x) / 90))`, with the fade width from the motion token.
 */
export function edgeFadeOpacity(x: number, panelWidth: number): number {
  const fade = EDGE_FADE.widthDp;
  return Math.min(1, Math.max(0, Math.min(x, panelWidth - x) / fade));
}

/**
 * Deterministic card tilt in degrees, in [−`TILT.maxDeg`, +`TILT.maxDeg`].
 *
 * Uses FNV-1a over the opaque item id and quantizes to 0.01°; re-renders must not re-roll the
 * angle (design spec §3.3 "随机倾斜 ±2.5°", anchored to the item rather than `Math.random()`).
 * The bound comes from the shared motion token so the visual and the motion layers cannot drift.
 */
export function tiltFor(id: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < id.length; index += 1) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  const unit = (hash >>> 0) / 0x1_0000_0000;
  return Math.round((unit * 2 - 1) * TILT.maxDeg * 100) / 100;
}

/** Rope sag in dp: `min(30, panelWidth * 0.018)` (spec §3.3). */
export function ropeSag(panelWidth: number): number {
  return Math.min(ROPE_MAX_SAG, Math.max(0, panelWidth * ROPE_SAG_RATIO));
}

/**
 * Rope height at `x` for a quadratic Bezier from `(0, ropeTop)` to `(panelWidth, ropeTop)`.
 *
 * With the control point at `ropeTop + 2*sag` the curve is `ropeY(t) = ropeTop + 4*sag*t*(1-t)`,
 * matching spec §3.3 exactly (midpoint dips by the full `sag`).
 */
export function ropeY(panelWidth: number, x: number): number {
  if (panelWidth <= 0) return ROPE_TOP;
  const t = Math.min(1, Math.max(0, x / panelWidth));
  return ROPE_TOP + 4 * ropeSag(panelWidth) * t * (1 - t);
}

/** SVG path for the rope, with the control point that yields the spec sag curve. */
export function ropePath(panelWidth: number): string {
  const half = round2(panelWidth / 2);
  const controlY = round2(ROPE_TOP + 2 * ropeSag(panelWidth));
  return `M0,${ROPE_TOP} Q${half},${controlY} ${round2(panelWidth)},${ROPE_TOP}`;
}

/** File name shown on the fallback tile (handles both Windows and POSIX separators). */
export function fileNameOf(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  if (slash < 0 || slash === normalized.length - 1) return path;
  return normalized.slice(slash + 1);
}

function round2(value: number): number {
  return Math.round(value * 100) / 100;
}
