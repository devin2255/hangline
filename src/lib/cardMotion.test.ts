import { describe, expect, it } from "vitest";
import {
  cardEvent,
  cardTransforms,
  createCardMotion,
  settledCardMotion,
  stepCardMotion,
  type CardMotionState,
  type CardMotionTargets,
} from "./cardMotion";
import { TILT } from "./motion";

const REST: CardMotionTargets = { hovered: false, pressed: false };
const HOVER: CardMotionTargets = { hovered: true, pressed: false };
const PRESS: CardMotionTargets = { hovered: false, pressed: true };

function run(
  state: CardMotionState,
  targets: CardMotionTargets,
  frames: number,
  dt = 1 / 60,
): CardMotionState {
  let current = state;
  for (let index = 0; index < frames; index += 1) {
    current = stepCardMotion(current, targets, dt);
  }
  return current;
}

describe("card arrival", () => {
  it("starts 46 dp above, 16° tilted, and settles", () => {
    let s = createCardMotion(false);
    s = cardEvent(s, { type: "arrive" }, 0);
    expect(cardTransforms(s).rotateDeg).toBe(16);
    expect(cardTransforms(s).offsetYDp).toBeCloseTo(-46, 0);
    s = run(s, REST, 240);
    expect(cardTransforms(s).offsetYDp).toBeGreaterThan(-0.6);
    expect(cardTransforms(s).rotateDeg).toBeLessThan(0.6);
    expect(settledCardMotion(s)).toBe(true);
  });
});

describe("breeze and nudges", () => {
  it("peaks near the amplitude then returns to rest", () => {
    let s = createCardMotion(false);
    s = cardEvent(s, { type: "arrive" }, 0);
    s = run(s, REST, 120);
    s = cardEvent(s, { type: "breeze", amplitudeDeg: 3, delayMs: 0 }, 2000);
    let peak = 0;
    for (let i = 0; i < 120; i += 1) {
      s = stepCardMotion(s, REST, 1 / 60);
      peak = Math.max(peak, Math.abs(cardTransforms(s).rotateDeg));
    }
    expect(peak).toBeGreaterThan(2.2);
    expect(peak).toBeLessThan(3.6);
  });
  it("copied/land nudges are small and settle", () => {
    let s = cardEvent(createCardMotion(false), { type: "copied" }, 0);
    s = run(s, REST, 180);
    expect(Math.abs(cardTransforms(s).rotateDeg)).toBeLessThan(0.5);
  });
  it("waits for the breeze delay before swaying", () => {
    let s = cardEvent(createCardMotion(false), { type: "breeze", amplitudeDeg: 3, delayMs: 500 }, 0);
    s = run(s, REST, 10);
    expect(cardTransforms(s).rotateDeg).toBe(0);
    s = run(s, REST, 21);
    expect(Math.abs(cardTransforms(s).rotateDeg)).toBeGreaterThan(0.1);
  });
  it("starts a small 2.2° sway on land", () => {
    let s = cardEvent(createCardMotion(false), { type: "land" }, 0);
    expect(cardTransforms(s).rotateDeg).toBe(0);
    let peak = 0;
    for (let i = 0; i < 90; i += 1) {
      s = stepCardMotion(s, REST, 1 / 60);
      peak = Math.max(peak, Math.abs(cardTransforms(s).rotateDeg));
    }
    expect(peak).toBeGreaterThan(2.0);
    expect(peak).toBeLessThan(2.4);
  });
});

describe("nudge continuity (P2-2 review)", () => {
  it("continues from an in-flight arrival swing instead of popping", () => {
    let s = cardEvent(createCardMotion(false), { type: "arrive" }, 0);
    // Frame 39 of the arrival: the 16° swing is still well in flight (~-3°).
    s = run(s, REST, 39);
    const before = cardTransforms(s).rotateDeg;
    expect(Math.abs(before)).toBeGreaterThan(1);

    s = cardEvent(s, { type: "copied" }, 1000);
    s = stepCardMotion(s, REST, 1 / 60);
    const after = cardTransforms(s).rotateDeg;
    // One frame of the "out" ease may only move along its own curve, never jump to ~0°:
    // travel * (1 - (1 - dt/300ms)³).
    const curveStep =
      Math.abs(TILT.copyDeg - before) * (1 - Math.pow(1 - 1 / 60 / 0.3, 3));
    expect(Math.abs(after - before)).toBeLessThanOrEqual(curveStep + 0.01);
    expect(Math.abs(after - before)).toBeLessThanOrEqual(1);

    s = run(s, REST, 300);
    expect(Math.abs(cardTransforms(s).rotateDeg)).toBeLessThan(0.5);
    expect(settledCardMotion(s)).toBe(true);
  });
});

describe("hover/press scale", () => {
  it("overshoots above 1.035 (under-damped) then settles", () => {
    let s = createCardMotion(false);
    let max = 1;
    for (let i = 0; i < 60; i += 1) {
      s = stepCardMotion(s, HOVER, 1 / 60);
      max = Math.max(max, cardTransforms(s).scale);
    }
    expect(max).toBeGreaterThan(1.035);
    expect(cardTransforms(s).scale).toBeCloseTo(1.035, 2);
  });
  it("press reaches 0.95 over 450 ms", () => {
    let s = createCardMotion(false);
    for (let i = 0; i < 27; i += 1) {
      s = stepCardMotion(s, PRESS, 1 / 60);
    }
    expect(cardTransforms(s).scale).toBeLessThan(0.98);
    expect(cardTransforms(s).scale).toBeGreaterThan(0.95);
  });
  it("settles once the press tween finishes, even while still pressed", () => {
    let s = createCardMotion(false);
    // 26 frames: the tween is still running (elapsed < 450 ms) and the scale is not home yet.
    for (let i = 0; i < 26; i += 1) {
      s = stepCardMotion(s, PRESS, 1 / 60);
    }
    expect(settledCardMotion(s)).toBe(false);
    // 28 frames: the tween has completed and the scale sits exactly on 0.95.
    for (let i = 0; i < 2; i += 1) {
      s = stepCardMotion(s, PRESS, 1 / 60);
    }
    expect(cardTransforms(s).scale).toBe(0.95);
    expect(settledCardMotion(s)).toBe(true);
  });
});

describe("reduced motion", () => {
  it("snaps to rest instantly", () => {
    const s = cardEvent(createCardMotion(true), { type: "arrive" }, 0);
    expect(cardTransforms(s)).toEqual({ rotateDeg: 0, offsetYDp: 0, scale: 1 });
    const next = stepCardMotion(s, HOVER, 1 / 60);
    expect(cardTransforms(next).scale).toBe(1.035);
  });
  it("snaps on a reduced-motion event and can resume", () => {
    let s = cardEvent(createCardMotion(false), { type: "arrive" }, 0);
    s = run(s, REST, 10);
    s = cardEvent(s, { type: "reduced-motion", value: true }, 500);
    expect(cardTransforms(s)).toEqual({ rotateDeg: 0, offsetYDp: 0, scale: 1 });
    const pressed = stepCardMotion(s, PRESS, 1 / 60);
    expect(cardTransforms(pressed).scale).toBe(0.95);

    const resumed = cardEvent(pressed, { type: "reduced-motion", value: false }, 600);
    expect(resumed.reducedMotion).toBe(false);
    // A breeze event while reduced motion is on is ignored rather than queued.
    const reduced = cardEvent(
      createCardMotion(true),
      { type: "breeze", amplitudeDeg: 3, delayMs: 0 },
      0,
    );
    expect(reduced.breeze).toBeNull();
  });
});

describe("stability and purity", () => {
  it("stays finite and settles when a frame is much longer than MAX_DT", () => {
    let s = cardEvent(createCardMotion(false), { type: "arrive" }, 0);
    for (let i = 0; i < 300; i += 1) {
      s = stepCardMotion(s, REST, 1);
      const transforms = cardTransforms(s);
      expect(Number.isFinite(transforms.rotateDeg)).toBe(true);
      expect(Number.isFinite(transforms.offsetYDp)).toBe(true);
      expect(Number.isFinite(transforms.scale)).toBe(true);
    }
    expect(Math.abs(cardTransforms(s).offsetYDp)).toBeLessThan(0.5);
    expect(settledCardMotion(s)).toBe(true);
  });
  it("does not mutate the state it is given", () => {
    const start = createCardMotion(false);
    const arrival = cardEvent(start, { type: "arrive" }, 0);
    expect(start.rotation).toBe(0);
    expect(start.offsetY).toBe(0);
    expect(arrival).not.toBe(start);

    const stepped = stepCardMotion(arrival, HOVER, 1 / 60);
    expect(stepped).not.toBe(arrival);
    expect(arrival.rotation).toBe(16);
    expect(arrival.scale).toBe(1);

    const breeze = cardEvent(arrival, { type: "breeze", amplitudeDeg: 3, delayMs: 0 }, 500);
    expect(arrival.breeze).toBeNull();
    expect(breeze.lastEventAtMs).toBe(500);
  });
});
