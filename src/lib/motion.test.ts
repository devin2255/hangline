import { describe, expect, it } from "vitest";
import {
  CARD,
  FLY,
  FALL,
  SPRINGS,
  TILT,
  GUST,
  EDGE_FADE,
  springFromResponse,
  easeInOutCubic,
  smoothstep,
  fallEase,
} from "./motion";

describe("springFromResponse", () => {
  it("reproduces the Tendedero reveal spring (response .42, damping .82)", () => {
    const s = springFromResponse(0.42, 0.82);
    expect(s.stiffness).toBeCloseTo(223.8, 0);
    expect(s.damping).toBeCloseTo(24.5, 0);
  });
  it("reproduces the item reorder spring (response .55, damping .78)", () => {
    const s = springFromResponse(0.55, 0.78);
    expect(s.stiffness).toBeCloseTo(130.5, 0);
    expect(s.damping).toBeCloseTo(17.8, 0);
  });
});

describe("P2 tokens", () => {
  it("uses the Tendedero fly-in and fall curves", () => {
    expect(FLY.durationMs).toBe(650);
    expect(FLY.arcDp).toBe(30);
    expect(FLY.chromeStart).toBe(0.35);
    expect(FLY.handoffMs).toBe(160);
    expect(FLY.fadeMs).toBe(160);
    expect(FALL.durationMs).toBe(550);
    expect(FALL.dropDp).toBe(520);
  });
  it("keeps the reference tilt/gust/edge-fade numbers", () => {
    expect(TILT.maxDeg).toBe(2.5);
    expect(TILT.arriveDeg).toBe(16);
    expect(TILT.landDeg).toBe(2.2);
    expect(TILT.copyDeg).toBe(3);
    expect(GUST.minIntervalMs).toBe(7000);
    expect(GUST.maxIntervalMs).toBe(16000);
    expect(GUST.maxDelayMs).toBe(350);
    expect(GUST.minAmplitudeDeg).toBe(1.6);
    expect(GUST.maxAmplitudeDeg).toBe(3.4);
    expect(EDGE_FADE.widthDp).toBe(90);
    expect(CARD.arriveOffsetDp).toBe(-46);
    expect(SPRINGS.reveal.stiffness).toBeCloseTo(224, 0);
    expect(SPRINGS.reveal.damping).toBeCloseTo(25, 0);
    expect(SPRINGS.item.stiffness).toBeCloseTo(130.5, 1);
    expect(SPRINGS.item.damping).toBeCloseTo(17.8, 0);
    // arriveY = springFromResponse(0.42, 0.72), hover = springFromResponse(0.3, 0.6).
    expect(SPRINGS.arriveY.stiffness).toBeCloseTo(223.8, 1);
    expect(SPRINGS.arriveY.damping).toBeCloseTo(21.5, 1);
    expect(SPRINGS.hover.stiffness).toBeCloseTo(438.6, 1);
    expect(SPRINGS.hover.damping).toBeCloseTo(25.1, 1);
    expect(SPRINGS.arriveSwing.stiffness).toBe(46);
    expect(SPRINGS.arriveSwing.damping).toBe(2.6);
    expect(SPRINGS.breeze.stiffness).toBe(38);
    expect(SPRINGS.breeze.damping).toBe(2.4);
  });
});

describe("curves", () => {
  it("easeInOutCubic is symmetric and monotonic", () => {
    expect(easeInOutCubic(0)).toBe(0);
    expect(easeInOutCubic(1)).toBe(1);
    expect(easeInOutCubic(0.5)).toBeCloseTo(0.5);
    expect(easeInOutCubic(0.3) + easeInOutCubic(0.7)).toBeCloseTo(1);
  });
  it("easeInOutCubic has the cubic shape, clamps, and is monotonic in 0.05 steps", () => {
    expect(easeInOutCubic(0.25)).toBeCloseTo(0.0625);
    expect(easeInOutCubic(0.75)).toBeCloseTo(0.9375);
    expect(easeInOutCubic(-1)).toBe(0);
    expect(easeInOutCubic(2)).toBe(1);
    let previous = easeInOutCubic(0);
    for (let step = 1; step <= 20; step += 1) {
      const current = easeInOutCubic(step * 0.05);
      expect(current).toBeGreaterThanOrEqual(previous);
      previous = current;
    }
  });
  it("smoothstep clamps outside its window", () => {
    expect(smoothstep(0.2, 0.35, 1)).toBe(0);
    // (0.7 - 0.35) / (1 - 0.35) = 0.53846…; 3t² - 2t³ = 0.55758… (the brief's
    // 0.5 expectation is the midpoint at x = 0.675, so check both here).
    expect(smoothstep(0.675, 0.35, 1)).toBeCloseTo(0.5);
    expect(smoothstep(0.7, 0.35, 1)).toBeCloseTo(0.5576, 4);
    expect(smoothstep(1.4, 0.35, 1)).toBe(1);
  });
  it("fallEase is cubic", () => {
    expect(fallEase(0.5)).toBeCloseTo(0.125);
    expect(fallEase(1)).toBe(1);
  });
});
