/**
 * Pure render-list planning for the fly/fall overlay page (P2-5/P2-6, P4 hardening).
 *
 * Rust keeps at most two active animations and re-emits the **whole** active list on every
 * `fly-animation` event (a new flight, a page reload via `report_fly_ready`, a re-show after a
 * raced idle hide). The page must therefore:
 *
 * - keep the running copy for an identical payload (a re-emit must not restart its rAF curve);
 * - replace the copy when the payload for the same id changed (a take-down `fall` during the
 *   160 ms fly handoff fade is a different animation even though the id matches);
 * - keep a cross-fading fly copy until its own handoff timer finishes, unless the new payload
 *   supersedes it.
 *
 * React keys are derived from the payload ([`flyRenderKey`]), so a replacement remounts the copy
 * and starts the new animation, while a re-emit keeps the same key and the same component instance.
 * Kept pure so the coalescing rules are testable without a browser (P2 backlog "same-id fly/fall
 * render coalescing": a repeated animation for the same id must not drop the final landed state).
 */

import type { FlyAnimation } from "./types";

/** One rendered overlay copy: a payload-derived React key plus the animation it draws. */
export interface FlyRenderEntry {
  /** Stable React key; changes exactly when the payload describes a different animation. */
  key: string;
  /** The payload `FlyCard` animates. */
  animation: FlyAnimation;
}

/**
 * Stable identity of one animation payload.
 *
 * Every field that changes what the overlay draws is part of the key: a taken-down card falling
 * from a new rect (or a re-captured card flying with new bytes) must start a fresh curve, while
 * Rust's full-list re-emits keep the exact same key and do not restart anything.
 */
export function flyRenderKey(animation: FlyAnimation): string {
  const { from, to } = animation;
  return [
    animation.kind,
    animation.id,
    `${from.x},${from.y},${from.w},${from.h}`,
    `${to.x},${to.y},${to.w},${to.h}`,
    animation.tiltDeg,
    animation.imagePath,
    animation.imageVersion,
    animation.durationMs,
    animation.targetScale,
  ].join("|");
}

/**
 * Merges an incoming `fly-animation` payload into the overlay's current render list.
 *
 * - incoming order is preserved (Rust sends a `BTreeMap` order, i.e. deterministic);
 * - a duplicate id in the payload is planned once (first occurrence wins);
 * - an identical payload reuses the previous entry (same key *and* same object, so the copy's rAF
 *   loop and its `done` flag are untouched);
 * - a changed payload for the same id replaces the entry with a new key, which remounts the copy;
 * - a previous copy that is no longer active is kept only while it is cross-fading, and never when
 *   the payload carries a new animation for that id (the new animation owns the id from now on).
 */
export function mergeFlyAnimations(
  previous: FlyRenderEntry[],
  incoming: FlyAnimation[],
  fadingIds: ReadonlySet<string>,
): FlyRenderEntry[] {
  const previousByKey = new Map(previous.map((entry) => [entry.key, entry]));
  const merged: FlyRenderEntry[] = [];
  const seenIds = new Set<string>();
  for (const animation of incoming) {
    if (seenIds.has(animation.id)) continue;
    seenIds.add(animation.id);
    const key = flyRenderKey(animation);
    const existing = previousByKey.get(key);
    // Reuse the previous object (not just the key) so the running copy keeps its state.
    merged.push(existing ?? { key, animation });
  }
  for (const entry of previous) {
    const id = entry.animation.id;
    if (seenIds.has(id)) continue;
    if (!fadingIds.has(id)) continue;
    if (merged.some((candidate) => candidate.key === entry.key)) continue;
    merged.push(entry);
  }
  return merged;
}
