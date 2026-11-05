/**
 * Pure frame math for the screen-wide fly-in / fall overlay (P2-5, design spec §4.6 B).
 *
 * The overlay page converts physical pixel rectangles to CSS pixels with [`cssScale`]; these
 * functions own the per-frame interpolation so the visual curve is unit-testable without a DOM.
 * All lengths are logical dp (= CSS pixels in this window); the component converts the physical
 * `from`/`to` rectangles before applying them.
 */

import { FALL, FLY, fallEase, smoothstep } from "../../lib/motion";

/** Progress-derived values of one fly-in frame (`k` is already eased by `easeInOutCubic`). */
export interface FlyFrame {
  /** Eased progress (for size/position interpolation). */
  k: number;
  /** Width/height interpolation factor from the capture rect to the final card. */
  widthLerp: number;
  /** Same as `widthLerp`, kept explicit so the component reads as geometry, not magic numbers. */
  heightLerp: number;
  /** Upward arch in dp (`sin(πk) · 30`); the card's top-centre rises by this much. */
  arcDp: number;
  /** Glass frame inset in dp (0 → 4). */
  insetDp: number;
  /** Outer corner radius in dp (0 → 16). */
  radiusDp: number;
  /** Opacity of the glass/chrome/pin layers (`smoothstep(k, 0.35, 1)`). */
  chromeOpacity: number;
}

/** Progress-derived values of one fall frame. */
export interface FallFrame {
  /** `k³`-eased progress, in [0, 1]. */
  ease: number;
  /** Downward drop in dp (0 → 520). */
  dropDp: number;
  /** Constant extra tilt in degrees (0 → 20) added on top of the card's tilt acceleration. */
  extraTiltDeg: number;
  /** Whole-card opacity (1 → 0). */
  opacity: number;
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

/**
 * Interpolates one fly-in frame.
 *
 * `k` must already be `easeInOutCubic(rawProgress)` (design spec §4.6 B: the 0.65 s curve eases the
 * whole interpolation, including the arch). At `k = 0` the overlay shows exactly the capture
 * rectangle (no glass, no radius, no clip); at `k = 1` it matches the panel card (4 dp frame,
 * 16 dp radius, fully opaque chrome) so the handoff cross-fade is seamless.
 */
export function flyFrame(k: number): FlyFrame {
  const progress = clamp01(k);
  return {
    k: progress,
    widthLerp: progress,
    heightLerp: progress,
    arcDp: Math.sin(Math.PI * progress) * FLY.arcDp,
    insetDp: progress * 4,
    radiusDp: progress * 16,
    chromeOpacity: smoothstep(progress, FLY.chromeStart, 1),
  };
}

/**
 * Interpolates one fall frame.
 *
 * The component adds `tilt + tilt · 7 · ease + extraTiltDeg` as the rotation and applies `dropDp`
 * as a downward translate; the item is gone from the line when the animation ends.
 */
export function fallFrame(k: number): FallFrame {
  const progress = clamp01(k);
  const ease = fallEase(progress);
  return {
    ease,
    dropDp: FALL.dropDp * ease,
    extraTiltDeg: 20 * ease,
    opacity: 1 - ease,
  };
}

/**
 * CSS pixels per physical virtual-screen pixel for the overlay window.
 *
 * The overlay window covers the whole virtual screen in physical pixels, while the page lays out in
 * CSS pixels: `innerWidth / virtualRect.w` (≈ 1 / DPR). A resized/unknown window falls back to
 * `1 / devicePixelRatio` (the same CSS-per-physical direction) and finally to 1, so the animation
 * still renders on a partially initialized page instead of producing `NaN` geometry.
 */
export function cssScale(
  innerWidth: number,
  virtualWidth: number,
  devicePixelRatio = 1,
): number {
  if (virtualWidth > 0 && Number.isFinite(virtualWidth) && Number.isFinite(innerWidth)) {
    const scale = innerWidth / virtualWidth;
    if (scale > 0) return scale;
  }
  if (devicePixelRatio > 0 && Number.isFinite(devicePixelRatio)) return 1 / devicePixelRatio;
  return 1;
}

