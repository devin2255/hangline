/** Tendedero-derived motion/visual tokens (design spec §3.3/§4.6 P2 amendments). */
export interface SpringPreset {
  stiffness: number;
  damping: number;
}

/** Converts SwiftUI `spring(response:dampingFraction:)` into k/c for a unit mass. */
export function springFromResponse(response: number, dampingFraction: number): SpringPreset {
  const omega = (2 * Math.PI) / response;
  return {
    stiffness: Math.round(omega * omega * 10) / 10,
    damping: Math.round(2 * dampingFraction * omega * 10) / 10,
  };
}

export const SPRINGS = {
  reveal: { stiffness: 224, damping: 25 }, // spec-rounded k/c (224/25), response ≈ .42 s, ζ ≈ .835
  item: springFromResponse(0.55, 0.78), // layout reorder
  arriveY: springFromResponse(0.42, 0.72), // card drop-in from -46 dp
  hover: springFromResponse(0.3, 0.6), // hover/press scale
  arriveSwing: { stiffness: 46, damping: 2.6 }, // 16° → 0
  breeze: { stiffness: 38, damping: 2.4 }, // 1.6–3.4° → 0
} as const satisfies Record<string, SpringPreset>;

export const FLY = {
  durationMs: 650,
  arcDp: 30,
  chromeStart: 0.35,
  handoffMs: 160,
  fadeMs: 160,
} as const;
export const FALL = { durationMs: 550, dropDp: 520 } as const;
export const TILT = { maxDeg: 2.5, arriveDeg: 16, landDeg: 2.2, copyDeg: 3 } as const;
export const GUST = {
  minIntervalMs: 7000,
  maxIntervalMs: 16000,
  maxDelayMs: 350,
  minAmplitudeDeg: 1.6,
  maxAmplitudeDeg: 3.4,
} as const;
export const EDGE_FADE = { widthDp: 90 } as const;
export const CARD = { arriveOffsetDp: -46 } as const;

export function easeInOutCubic(x: number): number {
  const t = Math.min(1, Math.max(0, x));
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

export function smoothstep(x: number, a: number, b: number): number {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
}

export function fallEase(x: number): number {
  const t = Math.min(1, Math.max(0, x));
  return t * t * t;
}
