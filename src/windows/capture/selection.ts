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
