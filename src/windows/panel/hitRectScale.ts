/**
 * DPI-scale resolution for the panel's physical hit-rect reporting (Task 14 review fix).
 *
 * The webview lays cards out in CSS pixels but Rust compares physical cursor pixels, so every
 * reported rect is multiplied by the window's device scale. The cached scale must follow the window
 * when it crosses a DPI boundary. A `resize` event is not enough: moving a 1920 px-wide window from
 * a 100% monitor to a 3840 px 200% monitor keeps the CSS viewport at 1920×210, so no resize fires
 * while `devicePixelRatio` doubles. Comparing the observed DPR with the cache catches that case.
 */

/** Tolerance for float noise in `devicePixelRatio` (browsers report exact quarter steps). */
const SCALE_EPSILON = 1e-6;

/** Scale for one hit-rect collection and whether the cache had to be refreshed. */
export interface HitRectScale {
  /** Physical pixels per CSS pixel to multiply the rects with. */
  scale: number;
  /** Whether the cached scale was stale and has been replaced by the observed one. */
  refreshed: boolean;
}

/**
 * Resolves the scale for one hit-rect collection.
 *
 * `devicePixelRatio` is the ratio the browser paints with, so a finite positive DPR is authoritative
 * (and counts as `refreshed` whenever it differs from the cache). A corrupt DPR falls back to the
 * cached value, then to 1.0, so a collection always produces a usable scale instead of `NaN` rects.
 */
export function resolveHitRectScale(cached: number | null, devicePixelRatio: number): HitRectScale {
  const observed =
    Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : null;
  if (observed === null) {
    const fallback = cached !== null && Number.isFinite(cached) && cached > 0 ? cached : 1;
    return { scale: fallback, refreshed: false };
  }
  if (
    cached !== null &&
    Number.isFinite(cached) &&
    cached > 0 &&
    Math.abs(cached - observed) <= SCALE_EPSILON
  ) {
    return { scale: cached, refreshed: false };
  }
  return { scale: observed, refreshed: true };
}
