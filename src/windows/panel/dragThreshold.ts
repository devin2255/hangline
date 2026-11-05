/** Pointer travel (dp) that turns a press on a card into a native drag-out. */
export const DRAG_THRESHOLD_DP = 4;

/**
 * Returns true once the pointer has travelled strictly more than {@link DRAG_THRESHOLD_DP}.
 *
 * `dx`/`dy` are CSS pixels (dp). The check is radial, not per-axis, so a diagonal move of
 * (3, 3) dp already starts the drag while a 4 dp jitter on one axis does not.
 */
export function exceedsDragThreshold(dx: number, dy: number): boolean {
  return dx * dx + dy * dy > DRAG_THRESHOLD_DP * DRAG_THRESHOLD_DP;
}
