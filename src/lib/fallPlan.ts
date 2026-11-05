/**
 * Pure take-down fall planning (P2-6, design spec §4.6 B "坠落").
 *
 * A take-down/remove snapshot can remove several cards at once (tray "take everything down", a drag
 * prune, future batch actions). The overlay accepts at most two concurrent falls, so the panel plans
 * them with a fixed 60 ms stagger: the first fall starts at 0 ms, the second at 60 ms, and the
 * caller stops scheduling once `start_fall` rejects twice in a row (the remaining cards simply
 * disappear).
 *
 * Kept pure so the ordering/stagger/skip rules are unit-testable without a DOM or IPC.
 */

import type { PhysicalRect } from "./types";

/** Delay between two scheduled falls (design plan P2-6: 60 ms stagger). */
export const FALL_STAGGER_MS = 60;

/** One card's last measured rest frame, captured from the rendered panel card. */
export interface FallFrameInfo {
  /** Card rest AABB in physical panel-client pixels (unrotated, unclipped). */
  rect: PhysicalRect;
  /** Deterministic card tilt from `tiltFor(id)`. */
  tiltDeg: number;
  /** Absolute path of the item's file (the overlay renders the original). */
  path: string;
  /**
   * `Item.updatedAt` when the frame was measured (P3): the overlay builds
   * `assetUrl(path, imageVersion)` so a markup-edited card falls with the edited bytes instead of a
   * pre-edit WebView cache entry. `0` for legacy items.
   */
  imageVersion: number;
  // Not addressed in P2-7; P2-9 follow-up: the line's slot edge-fade opacity is not frozen into the
  // frame yet, so a fall starting from the clipped oldest card begins at full opacity in the overlay.
}

/** One planned fall: the panel schedules `start_fall` at `delayMs` after the snapshot. */
export interface PlannedFall {
  /** Opaque item id. */
  id: string;
  /** Delay in ms from the removal snapshot (= `startAtMs - nowMs`). */
  delayMs: number;
  /** Absolute monotonic timestamp (`nowMs + delayMs`) when this fall should start. */
  startAtMs: number;
  /** Card rest AABB in physical panel-client pixels (a copy, never the stored frame object). */
  rect: PhysicalRect;
  /** Deterministic card tilt. */
  tiltDeg: number;
  /** Absolute source file path. */
  path: string;
  /** `updatedAt` of the source file at frame time (asset cache key, 0 when never rewritten). */
  imageVersion: number;
}

/**
 * Plans the fall sequence for the ids removed in one state snapshot.
 *
 * - preserves the input order (oldest → newest as the item list is ordered);
 * - staggers by [`FALL_STAGGER_MS`], using `nowMs` as the scheduling anchor (`startAtMs`);
 * - skips ids without a known frame (never measured/rendered) and duplicate ids;
 * - copies each rect so callers cannot mutate the stored frame by accident.
 */
export function planFallSequence(
  removedIds: string[],
  frames: Map<string, FallFrameInfo>,
  nowMs: number,
): PlannedFall[] {
  const anchor = Number.isFinite(nowMs) ? nowMs : 0;
  const planned: PlannedFall[] = [];
  const seen = new Set<string>();
  for (const id of removedIds) {
    if (seen.has(id)) continue;
    seen.add(id);
    const frame = frames.get(id);
    if (!frame) continue;
    const delayMs = planned.length * FALL_STAGGER_MS;
    planned.push({
      id,
      delayMs,
      startAtMs: anchor + delayMs,
      rect: { ...frame.rect },
      tiltDeg: frame.tiltDeg,
      path: frame.path,
      imageVersion: frame.imageVersion,
    });
  }
  return planned;
}
