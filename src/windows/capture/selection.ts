/**
 * Selection math for the freeze-frame capture overlay (Task 16, design spec §3.2 A / §3.9).
 *
 * The overlay window covers the whole virtual desktop in physical pixels, while its DOM lays out
 * in CSS pixels at one `devicePixelRatio` (WebView2 applies one zoom/scale per window, even when
 * the window spans monitors with different DPI). The frozen frame is rendered at
 * `physical / dpr` CSS pixels, so a pointer position in CSS pixels converts back to exactly one
 * physical point with [`toPhysicalRect`] — no assumption of 100% scaling, and negative virtual
 * origins (a monitor left of / above the primary) work unchanged.
 *
 * Everything here is pure and unit-tested in `selection.test.ts`; the component only wires DOM
 * events to these helpers.
 */

/** A point in CSS pixels relative to the overlay window's client origin (top-left = 0,0). */
export interface SelectionPoint {
  x: number;
  y: number;
}

/** A rectangle in CSS pixels with a non-negative width/height. */
export interface CssRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A rectangle in virtual-desktop physical pixels (Rust `geometry::PhysicalRect` shape). */
export interface PhysicalSelectionRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A physical size in pixels (the frozen frame dimensions from Rust). */
export interface PhysicalSize {
  w: number;
  h: number;
}

/** A point in virtual-desktop physical pixels (Rust `capture-ready.pressedAt`). */
export interface PhysicalPoint {
  x: number;
  y: number;
}

/**
 * How long the overlay waits for the first input after a borrowed press (ms).
 *
 * Wave 1.6 ("空窗补按"): the hotkey was pressed while the left button was already down, so Rust
 * hands the physical press point to the overlay instead of replaying a `pointerdown` the overlay
 * never received. If no `pointermove`/`pointerup` arrives within this window, the user already
 * released the button outside the overlay; the borrowed selection is dropped so no stale rectangle
 * is left on the frozen frame.
 */
export const PRESSED_SELECTION_DROP_MS = 150;

/**
 * Converts a virtual-desktop physical point into overlay CSS pixels, clamped to the viewport.
 *
 * `origin` is the captured virtual-screen origin (`virtualRect.x/y`, negative on a monitor left of
 * / above the primary) and `scale` the physical-pixels-per-CSS-pixel factor
 * ([`viewportScale`]). The result is clamped to `[0, viewportWidth] × [0, viewportHeight]`: the
 * borrowed press point can sit on another monitor edge, and a selection start outside the frozen
 * frame would render off-screen and fail the "real drag" check unpredictably.
 *
 * Malformed inputs never throw: a non-finite point becomes 0, a non-positive scale becomes 1 and a
 * non-positive/unmeasurable viewport clamps only at the lower edge.
 */
export function physicalPointToCss(
  point: PhysicalPoint,
  origin: PhysicalPoint,
  scale: number,
  viewportWidth: number,
  viewportHeight: number,
): SelectionPoint {
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
  const rawX = (point.x - origin.x) / safeScale;
  const rawY = (point.y - origin.y) / safeScale;
  const clampAxis = (value: number, max: number): number => {
    if (!Number.isFinite(value)) return 0;
    const lower = Math.max(value, 0);
    if (Number.isFinite(max) && max > 0) return Math.min(lower, max);
    return lower;
  };
  return {
    x: clampAxis(rawX, viewportWidth),
    y: clampAxis(rawY, viewportHeight),
  };
}

/**
 * Whether the overlay should drop a borrowed press selection.
 *
 * `elapsedMs` is the time since the borrowed press was applied, `sawInput` whether any
 * pointermove/pointerup was observed for it. A finished drag is never dropped; the drop only covers
 * the "the button was already released before the overlay was ready" case, where no further input
 * will ever arrive.
 */
export function shouldDropPressedSelection(elapsedMs: number, sawInput: boolean): boolean {
  if (sawInput) return false;
  if (!Number.isFinite(elapsedMs)) return false;
  if (elapsedMs < 0) return false;
  return elapsedMs >= PRESSED_SELECTION_DROP_MS;
}

/**
 * Smallest selection (CSS pixels) that is a real drag, not an accidental click.
 *
 * A pointerdown/up without movement must never save a 1x1 screenshot: at 150% 2 CSS px are 3
 * device pixels, so this only rejects clicks and jitter, not a deliberate tiny capture.
 */
export const MIN_SELECTION_CSS = 2;

/** Normalizes a drag from `start` to `end` into a positive-size rectangle. */
export function normalizeSelection(start: SelectionPoint, end: SelectionPoint): CssRect {
  return {
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    w: Math.abs(end.x - start.x),
    h: Math.abs(end.y - start.y),
  };
}

/**
 * Clamps a CSS rectangle to the overlay viewport.
 *
 * Pointer capture keeps delivering moves after the pointer leaves the window, so coordinates can
 * be negative or larger than the viewport. The saved rectangle must stay inside the frozen frame;
 * the display uses the same clamped rectangle the user will get.
 */
export function clampSelection(rect: CssRect, viewportWidth: number, viewportHeight: number): CssRect {
  const left = Math.min(Math.max(rect.x, 0), viewportWidth);
  const top = Math.min(Math.max(rect.y, 0), viewportHeight);
  const right = Math.min(Math.max(rect.x + rect.w, 0), viewportWidth);
  const bottom = Math.min(Math.max(rect.y + rect.h, 0), viewportHeight);
  return {
    x: left,
    y: top,
    w: Math.max(0, right - left),
    h: Math.max(0, bottom - top),
  };
}

/** Returns whether `selection` is a real drag that may be confirmed and saved. */
export function isSelectionLargeEnough(
  selection: CssRect,
  minimum: number = MIN_SELECTION_CSS,
): boolean {
  return selection.w >= minimum && selection.h >= minimum;
}

/**
 * Converts one CSS-pixel selection into the virtual-desktop physical rectangle Rust crops from.
 *
 * `dpr` is the overlay window's `devicePixelRatio`; `origin` is the virtual screen origin
 * (`virtualRect.x/y`, negative when a monitor sits left of / above the primary). Each **edge** is
 * rounded independently (`left = round(x·s)`, `right = round((x+w)·s)`, width = right − left; same
 * for y/h), so adjacent selections do not drift and a 1 CSS px drag at 150% yields the exactly
 * covered device span instead of an independently rounded width.
 */
export function toPhysicalRect(
  selection: CssRect,
  dpr: number,
  origin: SelectionPoint,
): PhysicalSelectionRect {
  const scale = dpr > 0 ? dpr : 1;
  const left = origin.x + Math.round(selection.x * scale);
  const top = origin.y + Math.round(selection.y * scale);
  const right = origin.x + Math.round((selection.x + selection.w) * scale);
  const bottom = origin.y + Math.round((selection.y + selection.h) * scale);
  return {
    x: left,
    y: top,
    w: Math.max(0, right - left),
    h: Math.max(0, bottom - top),
  };
}

/**
 * The frozen frame's CSS size: the whole physical virtual screen scaled by the window scale.
 *
 * This is what the `<img>` is stretched to; the overlay viewport should have the same size (Rust
 * sized the window to the virtual screen in physical pixels).
 */
export function frozenFrameCssSize(physical: PhysicalSize, scale: number): { width: number; height: number } {
  const safeScale = scale > 0 ? scale : 1;
  return { width: physical.w / safeScale, height: physical.h / safeScale };
}

/**
 * The actual physical-pixels-per-CSS-pixel factor of the overlay viewport.
 *
 * `window.devicePixelRatio` is usually the answer, but WebView2 can report it inconsistently for a
 * window that spans mixed-DPI monitors (observed: `devicePixelRatio === 1` while a 3840 px physical
 * window had a 2560 CSS viewport, i.e. the real factor was 1.5). Rust sized the window to the
 * virtual screen in physical pixels, so the physical width ÷ the CSS viewport width is the factor
 * the pointer coordinates and the frozen image actually use. Falls back to `devicePixelRatio` when
 * the viewport is not measurable yet.
 */
export function viewportScale(
  physicalWidth: number,
  viewportWidth: number,
  fallbackDpr: number,
): number {
  if (physicalWidth > 0 && viewportWidth > 0) {
    const ratio = physicalWidth / viewportWidth;
    if (Number.isFinite(ratio) && ratio > 0) return ratio;
  }
  return fallbackDpr > 0 ? fallbackDpr : 1;
}
