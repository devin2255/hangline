import { describe, expect, it } from "vitest";
import type { PhysicalRect } from "./types";
import type { FallFrameInfo } from "./fallPlan";
import { FALL_STAGGER_MS, planFallSequence } from "./fallPlan";

const rect = (x: number, y: number, w = 144, h = 112): PhysicalRect => ({ x, y, w, h });

function frames(entries: Array<[string, PhysicalRect]>): Map<string, FallFrameInfo> {
  return new Map(
    entries.map(([id, frameRect]) => [
      id,
      {
        rect: frameRect,
        tiltDeg: 1.5,
        path: `C:\\shots\\${id}.png`,
        imageVersion: id === "a" ? 1_700_000_000_900 : 0,
      },
    ]),
  );
}

describe("planFallSequence", () => {
  it("staggers falls by 60 ms in input order", () => {
    const plan = planFallSequence(
      ["a", "b", "c"],
      frames([
        ["a", rect(100, 10)],
        ["b", rect(200, 10)],
        ["c", rect(300, 10)],
      ]),
      1000,
    );
    expect(plan.map((entry) => entry.delayMs)).toEqual([0, FALL_STAGGER_MS, 2 * FALL_STAGGER_MS]);
    expect(plan.map((entry) => entry.startAtMs)).toEqual([
      1000, 1000 + FALL_STAGGER_MS, 1000 + 2 * FALL_STAGGER_MS,
    ]);
    expect(plan.map((entry) => entry.id)).toEqual(["a", "b", "c"]);
    expect(plan[1].rect).toEqual(rect(200, 10));
    expect(plan[1].tiltDeg).toBe(1.5);
    expect(plan[1].path).toBe("C:\\shots\\b.png");
    expect(plan[0].imageVersion).toBe(1_700_000_000_900);
    expect(plan[1].imageVersion).toBe(0);
  });

  it("skips ids without a known frame and keeps the remaining order", () => {
    const plan = planFallSequence(
      ["a", "b"],
      frames([["b", rect(200, 10)]]),
      123,
    );
    expect(plan).toHaveLength(1);
    expect(plan[0].id).toBe("b");
    expect(plan[0].delayMs).toBe(0);
  });

  it("handles an empty removal list and an empty frame map", () => {
    expect(planFallSequence([], frames([]), 0)).toEqual([]);
    expect(planFallSequence(["a"], new Map(), 0)).toEqual([]);
  });

  it("plans a duplicate id only once (first occurrence wins)", () => {
    const plan = planFallSequence(
      ["a", "b", "a"],
      frames([
        ["a", rect(100, 10)],
        ["b", rect(200, 10)],
      ]),
      0,
    );
    expect(plan.map((entry) => entry.id)).toEqual(["a", "b"]);
    expect(plan.map((entry) => entry.delayMs)).toEqual([0, FALL_STAGGER_MS]);
  });

  it("returns a copy of the rect and does not mutate the input frames", () => {
    const input = frames([["a", rect(100, 10)]]);
    const plan = planFallSequence(["a"], input, 0);
    expect(input.size).toBe(1);
    expect(input.get("a")?.rect).toEqual(rect(100, 10));
    expect(plan[0].rect).not.toBe(input.get("a")?.rect);
    plan[0].rect.x = 999;
    expect(input.get("a")?.rect.x).toBe(100);
  });
});

