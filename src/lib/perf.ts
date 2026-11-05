/**
 * P2-9 frame-time ring used by the performance harness (`scripts/p2/perf.mjs`).
 *
 * The panel feeds one `requestAnimationFrame` delta per frame while it is visible; the ring keeps
 * the most recent `capacity` samples so a 10 s measurement never grows memory. All math is pure so
 * the percentile/long-frame rules are unit-testable without a browser.
 *
 * P4: the whole ring (and the motion trace in `trace.ts`) is a **development-only** diagnostic.
 * [`DEV_INSTRUMENTATION`] is `false` in a production build, and the install sites are gated on it
 * so Vite drops the ring, the `window.__HANG_*` globals and this module from the release bundle
 * (`scripts/check-release-instrumentation.mjs` enforces it on every `pnpm build`).
 */

/**
 * Whether the browser-side performance/trace instrumentation is compiled in.
 *
 * `import.meta.env.DEV` is statically replaced by Vite, so every `if (DEV_INSTRUMENTATION)` block
 * and the global installs behind it are removed from the release bundle. The CDP harnesses
 * (`scripts/p2`, `scripts/p3`, `scripts/p4`) only ever run against the Vite dev server used by the
 * debug build, so they keep the full instrumentation.
 */
export const DEV_INSTRUMENTATION = import.meta.env.DEV;


/** A frame slower than this counts as a "long frame" in the DoD table (≈50 fps). */
export const LONG_FRAME_MS = 20;

/** Fixed-size ring of frame durations (ms). */
export interface FrameRing {
  /** Maximum number of samples kept. */
  capacity: number;
  /** Frame durations in ms, newest last; at most `capacity` entries. */
  samples: number[];
}

/** Creates an empty ring (default 600 samples ≈ 10 s at 60 fps). */
export function createFrameRing(capacity = 600): FrameRing {
  return { capacity: Math.max(1, Math.floor(capacity)), samples: [] };
}

/** Appends one frame duration; non-finite or non-positive deltas are ignored. */
export function pushFrame(ring: FrameRing, dtMs: number): void {
  if (!Number.isFinite(dtMs) || dtMs <= 0) return;
  ring.samples.push(dtMs);
  const overflow = ring.samples.length - ring.capacity;
  if (overflow > 0) ring.samples.splice(0, overflow);
}

/** Nearest-rank percentile of a list of numbers (0..1). The list must be non-empty. */
export function percentile(values: number[], fraction: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const clamped = Math.min(1, Math.max(0, fraction));
  const rank = Math.max(0, Math.ceil(clamped * sorted.length) - 1);
  return sorted[rank];
}

/** Aggregated numbers the harness compares against the DoD. */
export interface FrameSummary {
  /** Mean frames per second over the samples. */
  fpsMean: number;
  /** 5th-percentile fps (the slow tail). */
  fpsP5: number;
  /** Worst frame duration in ms. */
  worstMs: number;
  /** Number of frames slower than {@link LONG_FRAME_MS}. */
  longFrames: number;
  /** Longest run of consecutive long frames (a jank burst, not an isolated hiccup). */
  longestBurst: number;
  /** Number of samples the summary is based on. */
  samples: number;
}

/** Summarizes a ring; a `null` ring (release never allocates one) and an empty ring yield zeros. */
export function summarize(ring: FrameRing | null): FrameSummary {
  const values = ring?.samples ?? [];
  const samples = values.length;
  if (samples === 0) {
    return {
      fpsMean: 0,
      fpsP5: 0,
      worstMs: 0,
      longFrames: 0,
      longestBurst: 0,
      samples: 0,
    };
  }
  const total = values.reduce((sum, value) => sum + value, 0);
  const meanMs = total / samples;
  // The tail fps is derived from the slow (high) tail of the frame times.
  const p95Ms = percentile(values, 0.95);
  let longestBurst = 0;
  let burst = 0;
  for (const value of values) {
    if (value > LONG_FRAME_MS) {
      burst += 1;
      longestBurst = Math.max(longestBurst, burst);
    } else {
      burst = 0;
    }
  }
  return {
    fpsMean: meanMs > 0 ? 1000 / meanMs : 0,
    fpsP5: p95Ms > 0 ? 1000 / p95Ms : 0,
    worstMs: Math.max(...values),
    longFrames: values.filter((value) => value > LONG_FRAME_MS).length,
    longestBurst,
    samples,
  };
}

declare global {
  interface Window {
    /** Installed by the panel; the CDP performance harness calls `summarize()`/`reset()`. */
    __HANG_FRAMES__?: {
      summarize: () => FrameSummary;
      reset: () => void;
    };
    /**
     * Installed by the panel (dev builds only): the line-offset sampler the P4-5 harness uses to
     * prove the measured sweep actually moved the line.
     */
    __HANG_OFFSETS__?: {
      /** Starts a fresh trace anchored on the panel's current offset. */
      start: (startOffset: number) => void;
      /** Appends one sample (called once per panel `requestAnimationFrame`). */
      sample: (t: number, offset: number) => void;
      /** Ends the trace and returns its movement summary. */
      stop: () => OffsetMovement;
    };
  }
}

/** One line-offset sample taken while the panel is visible (`t` = `performance.now()`). */
export interface OffsetSample {
  /** Monotonic timestamp in ms (`performance.now()`). */
  t: number;
  /** Panel scroll offset in cards (`0` = newest end). */
  offset: number;
}

/**
 * Movement statistics of one measured scroll window (P4-5).
 *
 * The P4-1 review found the sweep gate asserted `data-max-offset > 0`, i.e. only that the line has
 * scroll *capacity*; a build whose wheel handler silently stopped would still PASS. These numbers
 * come from the panel's own offset, sampled once per frame, so a static window is visible as
 * `moves: 0`.
 */
export interface OffsetMovement {
  /** Number of samples the summary is based on. */
  samples: number;
  /** Covered window length in ms (first to last sample). */
  windowMs: number;
  /** Offset when the trace started (the harness reads `data-offset` before the sweep). */
  startOffset: number;
  /** Offset at the middle of the window. */
  midOffset: number;
  /** Offset at the last sample. */
  endOffset: number;
  /** Highest offset reached during the window (capacity is reported separately). */
  peakOffset: number;
  /** Number of sample-to-sample offset changes (real wheel steps the panel rendered). */
  moves: number;
  /** Moves in the first half of the window. */
  firstHalfMoves: number;
  /** Moves in the second half of the window. */
  secondHalfMoves: number;
  /** Moves per quarter of the window (oldest → newest quarter). */
  quarterMoves: [number, number, number, number];
  /** `true` when the line had moved towards older cards by the middle of the window. */
  movedMidWindow: boolean;
  /**
   * `true` when every quarter of the window contains at least one real offset change.
   *
   * Guarding every quarter (not just the midpoint) is what fails a sweep whose input path stops
   * halfway: the mid-window sample can still be > 0 while no later wheel event does anything.
   */
  movedThroughout: boolean;
}

/** Empty movement summary (short/degenerate traces). */
function emptyMovement(startOffset: number): OffsetMovement {
  return {
    samples: 0,
    windowMs: 0,
    startOffset,
    midOffset: startOffset,
    endOffset: startOffset,
    peakOffset: startOffset,
    moves: 0,
    firstHalfMoves: 0,
    secondHalfMoves: 0,
    quarterMoves: [0, 0, 0, 0],
    movedMidWindow: false,
    movedThroughout: false,
  };
}

/**
 * Summarizes a line-offset trace into the movement numbers the P4-5 gate asserts.
 *
 * Samples are expected in time order (the panel appends one per frame). Only samples with a
 * non-finite timestamp or offset are filtered out: the trace is *not* reordered, and a
 * non-monotonic timestamp is not rejected, so an out-of-order trace would shorten the measured
 * window or move samples between quarters. "A move" is a *change* of the offset between two
 * consecutive samples: a wheel event clamped at the newest end or at `maxOffset` does not count.
 */
export function summarizeOffsets(samples: OffsetSample[]): OffsetMovement {
  const trace = samples.filter(
    (sample) => Number.isFinite(sample.t) && Number.isFinite(sample.offset),
  );
  const startOffset = trace.length > 0 ? trace[0].offset : 0;
  if (trace.length < 2) {
    return trace.length === 1 ? { ...emptyMovement(startOffset), samples: 1 } : emptyMovement(0);
  }
  const first = trace[0];
  const last = trace[trace.length - 1];
  const windowMs = Math.max(0, last.t - first.t);
  if (windowMs <= 0) return { ...emptyMovement(startOffset), samples: trace.length };
  const midTime = first.t + windowMs / 2;
  // Attribute each change to the sample that shows the new value.
  const moves: number[] = [];
  let firstHalfMoves = 0;
  let secondHalfMoves = 0;
  const quarterMoves: [number, number, number, number] = [0, 0, 0, 0];
  let peakOffset = first.offset;
  for (let index = 1; index < trace.length; index += 1) {
    const sample = trace[index];
    peakOffset = Math.max(peakOffset, sample.offset);
    if (sample.offset === trace[index - 1].offset) continue;
    moves.push(sample.t);
    const fraction = Math.min(1, Math.max(0, (sample.t - first.t) / windowMs));
    const quarter = Math.min(3, Math.floor(fraction * 4));
    quarterMoves[quarter] += 1;
    if (sample.t < midTime) firstHalfMoves += 1;
    else secondHalfMoves += 1;
  }
  const mid = trace.find((sample) => sample.t >= midTime) ?? last;
  return {
    samples: trace.length,
    windowMs,
    startOffset,
    midOffset: mid.offset,
    endOffset: last.offset,
    peakOffset,
    moves: moves.length,
    firstHalfMoves,
    secondHalfMoves,
    quarterMoves,
    movedMidWindow: mid.offset > startOffset,
    movedThroughout: quarterMoves.every((count) => count > 0),
  };
}

