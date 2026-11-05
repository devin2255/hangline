import { describe, expect, it } from "vitest";
import { GUST } from "./motion";
import { gustAmplitude, gustDelay, nextGustDelay } from "./gust";

describe("gust scheduling", () => {
  it("maps the unit interval to the GUST interval range", () => {
    expect(nextGustDelay(0)).toBe(GUST.minIntervalMs);
    expect(nextGustDelay(1)).toBe(GUST.maxIntervalMs);
    expect(nextGustDelay(0.5)).toBe((GUST.minIntervalMs + GUST.maxIntervalMs) / 2);
    // 7–16 s (spec §4.6 B / design spec 3.3).
    expect(nextGustDelay(0)).toBe(7000);
    expect(nextGustDelay(1)).toBe(16000);
  });

  it("maps the unit interval to the amplitude range", () => {
    expect(gustAmplitude(0)).toBe(GUST.minAmplitudeDeg);
    expect(gustAmplitude(1)).toBe(GUST.maxAmplitudeDeg);
    expect(gustAmplitude(0)).toBe(1.6);
    expect(gustAmplitude(1)).toBe(3.4);
    expect(gustAmplitude(0.5)).toBeCloseTo((1.6 + 3.4) / 2, 6);
  });

  it("spreads the per-card delay over 0–350 ms", () => {
    expect(gustDelay(0)).toBe(0);
    expect(gustDelay(1)).toBe(GUST.maxDelayMs);
    expect(gustDelay(0.5)).toBe(175);
    expect(gustDelay(0.25)).toBe(87.5);
  });

  it("clamps out-of-range and non-finite samples instead of producing NaN", () => {
    expect(nextGustDelay(-1)).toBe(GUST.minIntervalMs);
    expect(nextGustDelay(2)).toBe(GUST.maxIntervalMs);
    expect(gustAmplitude(-0.5)).toBe(GUST.minAmplitudeDeg);
    expect(gustAmplitude(1.5)).toBe(GUST.maxAmplitudeDeg);
    expect(gustDelay(-2)).toBe(0);
    expect(gustDelay(9)).toBe(GUST.maxDelayMs);
    expect(nextGustDelay(Number.NaN)).toBe(GUST.minIntervalMs);
    expect(gustAmplitude(Number.NaN)).toBe(GUST.minAmplitudeDeg);
    expect(gustDelay(Number.NaN)).toBe(0);
  });
});
