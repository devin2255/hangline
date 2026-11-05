/**
 * Critically-ish damped spring integrator for the panel reveal animation (design spec §3.3).
 *
 * Semi-implicit Euler: velocity is integrated first, then position uses the *new* velocity. With
 * the spec parameters (`stiffness 224`, `damping 25`, ζ ≈ 0.84) this reaches the target in about
 * 0.42 s and never diverges for frame times up to [`MAX_DT`]. The step is clamped so a stalled tab
 * (or a debugger pause) can never launch the panel off-screen: a huge `dt` behaves like one
 * 30 fps frame instead of exploding the state.
 *
 * The panel drives `translateY` with this on every `requestAnimationFrame`; `tick` itself is pure
 * so the convergence/stability contract is unit-tested without a browser.
 */

/** One integrator sample: position (px) and velocity (px/s). */
export interface SpringState {
  /** Current offset relative to the target, in pixels. */
  x: number;
  /** Current velocity, in pixels per second. */
  v: number;
}

/** Largest integration step, in seconds (30 fps); longer frames are clamped to this. */
export const MAX_DT = 1 / 30;

/**
 * Advances the spring one frame.
 *
 * @param state - Current position/velocity.
 * @param target - Target position (the spring always pulls towards it).
 * @param stiffness - Spring constant `k` (224 for the reveal animation).
 * @param damping - Damping coefficient `c` (25 for the reveal animation).
 * @param dt - Frame time in seconds; non-positive values leave the state untouched and values
 *   above [`MAX_DT`] are clamped.
 * @returns The new position/velocity (a new object; the input is not mutated).
 */
export function tick(
  state: SpringState,
  target: number,
  stiffness: number,
  damping: number,
  dt: number,
): SpringState {
  if (!(dt > 0) || !Number.isFinite(dt)) {
    return { x: state.x, v: state.v };
  }
  const step = Math.min(dt, MAX_DT);
  // a = -k·(x - target) - c·v; velocity first, then position with the new velocity.
  const acceleration = -stiffness * (state.x - target) - damping * state.v;
  const v = state.v + acceleration * step;
  const x = state.x + v * step;
  return { x, v };
}
