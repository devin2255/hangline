import { describe, expect, it } from "vitest";
import { MAX_DT, tick } from "./spring";

/** The reveal animation parameters from design spec §3.3. */
const STIFFNESS = 224;
const DAMPING = 25;
/** Panel height (medium, 210 dp) + the 12 dp overshoot from the spec. */
const TUCKED = -222;
const TARGET = 0;

describe("spring", () => {
  it("converges within 700 ms at stiffness 224 / damping 25", () => {
    let state = { x: TUCKED, v: 0 };
    const dt = 1 / 60;
    for (let elapsed = 0; elapsed < 700; elapsed += dt * 1000) {
      state = tick(state, TARGET, STIFFNESS, DAMPING, dt);
    }
    expect(Math.abs(state.x - TARGET)).toBeLessThanOrEqual(0.5);
    expect(Math.abs(state.v)).toBeLessThan(50);
  });

  it("does not diverge (bounded overshoot on the way to the target)", () => {
    let state = { x: TUCKED, v: 0 };
    let maxAbs = 0;
    for (let i = 0; i < 120; i += 1) {
      state = tick(state, TARGET, STIFFNESS, DAMPING, 1 / 60);
      expect(Number.isFinite(state.x)).toBe(true);
      expect(Number.isFinite(state.v)).toBe(true);
      maxAbs = Math.max(maxAbs, Math.abs(state.x));
    }
    // A damped spring may overshoot a little, but never by more than the initial displacement.
    expect(maxAbs).toBeLessThanOrEqual(Math.abs(TUCKED) * 1.05);
    expect(Math.abs(state.x - TARGET)).toBeLessThanOrEqual(0.5);
  });

  it("stays stable when a frame is much longer than MAX_DT", () => {
    let state = { x: 100, v: 0 };
    for (let i = 0; i < 300; i += 1) {
      state = tick(state, TARGET, STIFFNESS, DAMPING, 1);
      expect(Number.isFinite(state.x)).toBe(true);
      expect(Number.isFinite(state.v)).toBe(true);
    }
    expect(Math.abs(state.x - TARGET)).toBeLessThanOrEqual(0.5);
  });

  it("clamps dt to MAX_DT (one huge frame cannot jump further than one clamped step)", () => {
    const start = { x: TUCKED, v: 0 };
    const huge = tick(start, TARGET, STIFFNESS, DAMPING, 10);
    const clamped = tick(start, TARGET, STIFFNESS, DAMPING, MAX_DT);
    expect(huge).toEqual(clamped);
  });

  it("keeps the state unchanged for a non-positive dt", () => {
    const state = { x: 12, v: -3 };
    expect(tick(state, TARGET, STIFFNESS, DAMPING, 0)).toEqual(state);
    expect(tick(state, TARGET, STIFFNESS, DAMPING, -0.5)).toEqual(state);
  });
});
