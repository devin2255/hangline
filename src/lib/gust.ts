/**
 * Pure gust scheduling for the breeze (design spec §3.3 / §4.6 B, Tendedero `Line.swift:304-310`).
 *
 * The panel owns one timer chain: each tick waits `nextGustDelay(random())` (7–16 s), increments a
 * counter, and every card derives its own amplitude and 0–350 ms delay from that counter change.
 * Keeping the three mappings pure (unit-interval random in, token range out) makes the ranges unit
 * testable and keeps `Math.random()` at the call sites.
 */

import { GUST } from "./motion";

/** Clamps a random sample to `[0, 1]`; non-finite values fall back to 0 so callers never emit NaN. */
function unit(rand: number): number {
  if (!Number.isFinite(rand)) return 0;
  return Math.min(1, Math.max(0, rand));
}

/** Time until the next gust in ms: `GUST.minIntervalMs + rand * (max − min)` (7000–16000). */
export function nextGustDelay(rand: number): number {
  return GUST.minIntervalMs + unit(rand) * (GUST.maxIntervalMs - GUST.minIntervalMs);
}

/** Gust amplitude in degrees: `GUST.minAmplitudeDeg + rand * (max − min)` (1.6–3.4). */
export function gustAmplitude(rand: number): number {
  return GUST.minAmplitudeDeg + unit(rand) * (GUST.maxAmplitudeDeg - GUST.minAmplitudeDeg);
}

/** Per-card breeze delay in ms: `rand * GUST.maxDelayMs` (0–350). */
export function gustDelay(rand: number): number {
  return unit(rand) * GUST.maxDelayMs;
}
