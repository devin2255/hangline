import { describe, expect, it } from "vitest";
import {
  createFrameRing,
  LONG_FRAME_MS,
  percentile,
  pushFrame,
  summarize,
  summarizeOffsets,
  type OffsetSample,
} from "./perf";

describe("frame ring", () => {
  it("starts empty and summarizes to zeros", () => {
    const ring = createFrameRing(4);
    expect(ring.samples).toEqual([]);
    expect(summarize(ring)).toEqual({
      fpsMean: 0,
      fpsP5: 0,
      worstMs: 0,
      longFrames: 0,
      longestBurst: 0,
      samples: 0,
    });
  });

  it("treats a missing ring as empty (release allocates none)", () => {
    expect(summarize(null)).toEqual({
      fpsMean: 0,
      fpsP5: 0,
      worstMs: 0,
      longFrames: 0,
      longestBurst: 0,
      samples: 0,
    });
  });

  it("keeps only the newest `capacity` frames and ignores invalid deltas", () => {
    const ring = createFrameRing(3);
    for (const dt of [10, 11, 12, 13, 14]) pushFrame(ring, dt);
    expect(ring.samples).toEqual([12, 13, 14]);
    pushFrame(ring, Number.NaN);
    pushFrame(ring, -5);
    pushFrame(ring, 0);
    expect(ring.samples).toEqual([12, 13, 14]);
  });

  it("computes mean fps, slow-tail fps, worst frame, long frames and the longest burst", () => {
    const ring = createFrameRing(10);
    // 8 frames at 60 fps (16.67 ms) + two long ones (25 ms and 40 ms).
    for (let i = 0; i < 8; i += 1) pushFrame(ring, 1000 / 60);
    pushFrame(ring, 25);
    pushFrame(ring, 40);
    const summary = summarize(ring);
    expect(summary.samples).toBe(10);
    expect(summary.longFrames).toBe(2);
    expect(summary.longestBurst).toBe(2);
    expect(summary.worstMs).toBe(40);
    expect(summary.fpsMean).toBeGreaterThan(50);
    expect(summary.fpsMean).toBeLessThan(60);
    // p95 frame time is the 10th (worst) frame here → 25 fps tail.
    expect(summary.fpsP5).toBeCloseTo(1000 / 40, 4);
    expect(LONG_FRAME_MS).toBe(20);
  });

  it("reports the longest consecutive run of long frames, not their total", () => {
    const ring = createFrameRing(20);
    // Long, long, short, long, long, long, short, long → longest burst is 3.
    for (const dt of [25, 30, 16, 21, 40, 22, 16, 50]) pushFrame(ring, dt);
    const summary = summarize(ring);
    expect(summary.longFrames).toBe(6);
    expect(summary.longestBurst).toBe(3);
  });
});

describe("percentile", () => {
  it("uses nearest-rank and clamps the fraction", () => {
    const values = [40, 10, 30, 20];
    expect(percentile(values, 0)).toBe(10);
    expect(percentile(values, 0.5)).toBe(20);
    expect(percentile(values, 0.95)).toBe(40);
    expect(percentile(values, 2)).toBe(40);
    expect(percentile([], 0.5)).toBe(0);
  });

  it("does not mutate the input", () => {
    const values = [3, 1, 2];
    percentile(values, 0.5);
    expect(values).toEqual([3, 1, 2]);
  });
});

/** One (time, offset) sample per animation frame, like the panel's dev-only offset sampler. */
function samples(values: Array<[number, number]>): OffsetSample[] {
  return values.map(([t, offset]) => ({ t, offset }));
}

/**
 * Synthetic trace of the perf-100 sweep: one wheel step every `stepMs`, alternating direction
 * every `block` steps, sampled once per `sampleMs` (the panel's rAF cadence).
 */
function sweepTrace({
  durationMs = 5000,
  sampleMs = 16,
  stepMs = 43,
  block = 40,
  start = 0,
  stopAtMs = Number.POSITIVE_INFINITY,
}: {
  durationMs?: number;
  sampleMs?: number;
  stepMs?: number;
  block?: number;
  start?: number;
  stopAtMs?: number;
} = {}): OffsetSample[] {
  const trace: OffsetSample[] = [];
  let offset = start;
  let events = 0;
  for (let t = 0; t <= durationMs; t += sampleMs) {
    // Apply every wheel step that happened up to this sample time (whole cards, like
    // `nextScrollOffset`); `stopAtMs` freezes the input path mid-window.
    const due = t >= stopAtMs ? events : Math.floor(t / stepMs);
    while (events < due) {
      events += 1;
      const forward = Math.floor((events - 1) / block) % 2 === 0;
      offset = Math.max(0, Math.min(91, offset + (forward ? 1 : -1)));
    }
    trace.push({ t, offset });
  }
  return trace;
}

describe("offset movement summary (P4-5)", () => {
  it("reports real movement for the alternating 5 s sweep", () => {
    const movement = summarizeOffsets(sweepTrace());
    expect(movement.samples).toBeGreaterThan(250);
    expect(movement.windowMs).toBeGreaterThan(4900);
    expect(movement.startOffset).toBe(0);
    expect(movement.midOffset).toBeGreaterThan(0);
    expect(movement.peakOffset).toBeGreaterThanOrEqual(40);
    expect(movement.moves).toBeGreaterThan(80);
    expect(movement.quarterMoves.every((count) => count > 0)).toBe(true);
    expect(movement.movedMidWindow).toBe(true);
    expect(movement.movedThroughout).toBe(true);
  });

  it("fails a static window even when the line has scroll capacity", () => {
    // Regression for the old gate: `data-max-offset` was 91 while the wheel handler was silently
    // dead, so the sweep looked like it had a scroll window without moving a single card.
    const staticTrace = samples(Array.from({ length: 300 }, (_, index) => [index * 16, 0]));
    const movement = summarizeOffsets(staticTrace);
    expect(movement.peakOffset).toBe(0);
    expect(movement.moves).toBe(0);
    expect(movement.movedMidWindow).toBe(false);
    expect(movement.movedThroughout).toBe(false);
  });

  it("fails when wheel dispatch stops halfway through the window", () => {
    // The panel moved in the first half and then the input path froze: the mid-window assertion
    // alone cannot see this, so every quarter of the window must contain real movement.
    const movement = summarizeOffsets(sweepTrace({ stopAtMs: 2400 }));
    expect(movement.midOffset).toBeGreaterThan(0);
    expect(movement.movedMidWindow).toBe(true);
    expect(movement.quarterMoves[0]).toBeGreaterThan(0);
    expect(movement.quarterMoves[1]).toBeGreaterThan(0);
    expect(movement.quarterMoves[2]).toBe(0);
    expect(movement.quarterMoves[3]).toBe(0);
    expect(movement.movedThroughout).toBe(false);
  });

  it("counts only real offset changes", () => {
    // A wheel event that is clamped at the newest end does not move the line: no move, no claim.
    const movement = summarizeOffsets(samples([[0, 0], [16, 0], [32, 0], [48, 1], [64, 1], [80, 2]]));
    expect(movement.moves).toBe(2);
    expect(movement.peakOffset).toBe(2);
    expect(movement.endOffset).toBe(2);
  });

  it("is total on empty and single-sample traces", () => {
    const empty = summarizeOffsets([]);
    expect(empty.samples).toBe(0);
    expect(empty.windowMs).toBe(0);
    expect(empty.movedMidWindow).toBe(false);
    expect(empty.movedThroughout).toBe(false);
    const single = summarizeOffsets(samples([[100, 3]]));
    expect(single.samples).toBe(1);
    expect(single.startOffset).toBe(3);
    expect(single.midOffset).toBe(3);
    expect(single.moves).toBe(0);
    expect(single.movedThroughout).toBe(false);
  });
});
