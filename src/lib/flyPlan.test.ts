import { describe, expect, it } from "vitest";
import type { FlyAnimation } from "./types";
import { flyRenderKey, mergeFlyAnimations, type FlyRenderEntry } from "./flyPlan";

function animation(overrides: Partial<FlyAnimation> = {}): FlyAnimation {
  return {
    id: "a",
    kind: "fly",
    imagePath: "C:\\shots\\a.png",
    imageVersion: 0,
    from: { x: 0, y: 0, w: 120, h: 80 },
    to: { x: 900, y: 45, w: 144, h: 112 },
    tiltDeg: 1.5,
    durationMs: 650,
    targetScale: 1,
    ...overrides,
  };
}

function entry(payload: FlyAnimation): FlyRenderEntry {
  return { key: flyRenderKey(payload), animation: payload };
}

describe("mergeFlyAnimations", () => {
  it("reuses the existing copy for an identical re-emitted payload", () => {
    // Rust re-emits the whole active list whenever another animation starts: the running copy must
    // keep its rAF (no restart, no key change).
    const first = animation();
    const previous = [entry(first)];
    const merged = mergeFlyAnimations(previous, [animation()], new Set());
    expect(merged).toHaveLength(1);
    expect(merged[0].key).toBe(previous[0].key);
    expect(merged[0].animation).toBe(first);
  });

  it("replaces a same-id copy when the payload changes kind (fly handoff -> fall)", () => {
    // The P2 backlog defect: a take-down within the 160 ms handoff fade reused the finished fly
    // payload, so the fall never rendered and its timeout removed the id entirely.
    const flying = animation();
    const fading = new Set(["a"]);
    const merged = mergeFlyAnimations(
      [entry(flying)],
      [animation({ kind: "fall", durationMs: 550, from: { x: 900, y: 45, w: 144, h: 112 } })],
      fading,
    );
    expect(merged).toHaveLength(1);
    expect(merged[0].animation.kind).toBe("fall");
    expect(merged[0].animation.durationMs).toBe(550);
    expect(merged[0].key).not.toBe(flyRenderKey(flying));
  });

  it("replaces a same-id copy when the geometry changes", () => {
    const previous = [entry(animation())];
    const moved = animation({ to: { x: 700, y: 45, w: 144, h: 112 } });
    const merged = mergeFlyAnimations(previous, [moved], new Set());
    expect(merged).toHaveLength(1);
    expect(merged[0].key).not.toBe(previous[0].key);
    expect(merged[0].animation).toBe(moved);
  });

  it("keeps a fading copy that no incoming animation supersedes", () => {
    const fadingFly = animation({ id: "a" });
    const other = animation({ id: "b" });
    const merged = mergeFlyAnimations(
      [entry(fadingFly), entry(other)],
      [animation({ id: "b" })],
      new Set(["a"]),
    );
    expect(merged.map((item) => item.animation.id)).toEqual(["b", "a"]);
    expect(merged[1].animation).toBe(fadingFly);
  });

  it("drops a fading copy superseded by a new animation of the same id", () => {
    const fadingFly = animation({ id: "a" });
    const merged = mergeFlyAnimations(
      [entry(fadingFly)],
      [animation({ id: "a", kind: "fall", durationMs: 550 })],
      new Set(["a"]),
    );
    expect(merged).toHaveLength(1);
    expect(merged[0].animation.kind).toBe("fall");
  });

  it("drops a copy that is neither active nor fading", () => {
    const merged = mergeFlyAnimations([entry(animation())], [], new Set());
    expect(merged).toEqual([]);
  });

  it("plans a duplicate id only once", () => {
    const merged = mergeFlyAnimations([], [animation(), animation()], new Set());
    expect(merged).toHaveLength(1);
  });

  it("preserves the incoming order", () => {
    const merged = mergeFlyAnimations(
      [],
      [animation({ id: "a" }), animation({ id: "b" }), animation({ id: "c" })],
      new Set(),
    );
    expect(merged.map((item) => item.animation.id)).toEqual(["a", "b", "c"]);
  });
});
