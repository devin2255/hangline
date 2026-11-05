/**
 * P2-9 lightweight motion trace (development-only, gated by `window.__HANG_TRACE_ON__`).
 *
 * `scripts/p2/compare.mjs` turns the flag on, triggers known animations and reads
 * `window.__HANG_TRACE__` afterwards to build the reproducible P2-visual tables. The flag is off in
 * normal runs, so each `pushTrace` is a single boolean check (no allocation).
 *
 * Samples are intentionally coarse: `{ t, kind, data }` where `t` is `performance.now()` and `data`
 * carries the numbers the comparison needs (panel offset, card swing/scale, fly/fall progress).
 *
 * P4 hardening: the whole module is compiled out of a release build. [`DEV_INSTRUMENTATION`] is
 * statically `false` there, so the early returns below become unconditional and the minifier drops
 * `window.__HANG_TRACE__`/`__HANG_CLEAR_TRACE__` (verified by
 * `scripts/check-release-instrumentation.mjs` on every `pnpm build`). The harnesses only ever run
 * against the Vite dev server of the debug build, where the instrumentation stays complete.
 */

import { DEV_INSTRUMENTATION } from "./perf";

/** Maximum samples kept in the ring (a few seconds of 60 fps animation samples). */
export const TRACE_CAPACITY = 2000;

/** One trace sample. */
export interface TraceSample {
  /** `performance.now()` timestamp. */
  t: number;
  /** Animation/trace channel (`panel`, `card`, `fly`, `fall`). */
  kind: string;
  /** Numeric payload for this channel. */
  data: Record<string, number>;
}

declare global {
  interface Window {
    /** Set by the comparison harness before it triggers animations. */
    __HANG_TRACE_ON__?: boolean;
    /** Trace ring, newest last (installed by {@link installTrace}). */
    __HANG_TRACE__?: TraceSample[];
    /** Clears the trace ring (installed by {@link installTrace}). */
    __HANG_CLEAR_TRACE__?: () => void;
  }
}

let samples: TraceSample[] = [];

/** Whether the harness asked for trace samples in this page (always `false` in a release build). */
export function traceEnabled(): boolean {
  if (!DEV_INSTRUMENTATION) return false;
  return typeof window !== "undefined" && window.__HANG_TRACE_ON__ === true;
}

/** Installs the global ring/clear hook once (called by the panel; dev-only). */
export function installTrace(): void {
  if (!DEV_INSTRUMENTATION) return;
  if (typeof window === "undefined") return;
  window.__HANG_TRACE__ = samples;
  window.__HANG_CLEAR_TRACE__ = () => {
    samples = [];
    window.__HANG_TRACE__ = samples;
  };
}

/** Pushes one sample when tracing is enabled; a no-op otherwise (and in release builds). */
export function pushTrace(kind: string, data: Record<string, number>): void {
  if (!DEV_INSTRUMENTATION) return;
  if (!traceEnabled()) return;
  samples.push({ t: performance.now(), kind, data });
  if (samples.length > TRACE_CAPACITY) samples.splice(0, samples.length - TRACE_CAPACITY);
  if (typeof window !== "undefined") window.__HANG_TRACE__ = samples;
}

/** Returns a copy of the current samples (harness/debug convenience). */
export function traceSamples(): TraceSample[] {
  return samples.map((sample) => ({ ...sample, data: { ...sample.data } }));
}
