/** v1.6.2 holiday themes shared by the date windows and the rope renderer. */
import type { HolidayThemeId } from "./types";

export interface HolidayOrnament {
  /** Position along the rope (0 = left end, 1 = right end). */
  t: number;
  kind:
    | "bauble"
    | "bell"
    | "star"
    | "snow"
    | "pinecone"
    | "heart"
    | "lantern"
    | "firework"
    | "mooncake"
    | "pumpkin"
    | "bat"
    | "flag"
    | "moon";
  color: string;
  /** Optional visual scale; the longest ornaments use it to stay in proportion. */
  scale?: number;
  /** Optional string length in px; defaults to a small variation by index. */
  hang?: number;
}

/** A small motif woven into the cord itself (not hanging), used to give the garland texture. */
export interface HolidayCordMotif {
  t: number;
  kind:
    | "pine"
    | "blossom"
    | "leaf"
    | "sparkle"
    | "star"
    | "petal"
    | "bat"
    | "pennant"
    | "snow"
    | "knot"
    | "heart";
  color: string;
  scale?: number;
  side?: "top" | "bottom" | "both";
}

export interface HolidayRopeSpec {
  /** Two cord stops (left/right), the thin thread colour and the end-bow ribbon colour. */
  cord: readonly [string, string];
  thread: string;
  ribbon: string;
  /** Fairy-light bulb colour. */
  light: string;
  /** Bulb positions along the rope. */
  lights: readonly number[];
  ornaments: readonly HolidayOrnament[];
  motifs: readonly HolidayCordMotif[];
  /** Sparse, slowly breathing atmosphere particles around the garland. */
  ambient?: { kind: "snow" | "petal" | "heart" | "bat" | "sparkle"; color: string };
}

export const HOLIDAY_THEME_IDS: readonly HolidayThemeId[] = [
  "new-year",
  "spring-festival",
  "valentine",
  "mid-autumn",
  "national-day",
  "halloween",
  "christmas",
];

const LIGHTS = [0.08, 0.21, 0.35, 0.52, 0.68, 0.82, 0.94] as const;

export const HOLIDAY_ROPE_SPECS: Record<HolidayThemeId, HolidayRopeSpec> = {
  "new-year": {
    cord: ["#5E1622", "#B8323D"],
    thread: "#E7C878",
    ribbon: "#D8B467",
    light: "#FFE3A3",
    lights: LIGHTS,
  ambient: { kind: "sparkle", color: "#E7C878" },
    ornaments: [
      { t: 0.065, kind: "firework", color: "#E7C878", scale: 1.4, hang: 22 },
      { t: 0.095, kind: "bauble", color: "#B8323D", scale: 1.1, hang: 30 },
      { t: 0.18, kind: "star", color: "#E7C878", scale: 0.95, hang: 23 },
      { t: 0.30, kind: "firework", color: "#D86A5A", scale: 1.15, hang: 27 },
      { t: 0.70, kind: "bauble", color: "#D8B467", scale: 1.15, hang: 24 },
      { t: 0.82, kind: "firework", color: "#E7C878", scale: 1.05, hang: 20 },
      { t: 0.90, kind: "star", color: "#E7C878", scale: 1.0, hang: 28 },
      { t: 0.955, kind: "bauble", color: "#B8323D", scale: 1.15, hang: 22 },
    ],
    motifs: [
      { t: 0.03, kind: "sparkle", color: "#E7C878", scale: 0.8 },
      { t: 0.13, kind: "sparkle", color: "#E7C878", scale: 0.55, side: "top" },
      { t: 0.24, kind: "star", color: "#E7C878", scale: 0.55 },
      { t: 0.38, kind: "sparkle", color: "#FFE3A3", scale: 0.6, side: "top" },
      { t: 0.62, kind: "sparkle", color: "#FFE3A3", scale: 0.6, side: "top" },
      { t: 0.76, kind: "star", color: "#E7C878", scale: 0.55 },
      { t: 0.87, kind: "sparkle", color: "#E7C878", scale: 0.75 },
      { t: 0.97, kind: "sparkle", color: "#E7C878", scale: 0.8 },
    ],
  },
  "spring-festival": {
    cord: ["#7C171F", "#B8272E"],
    thread: "#EBC46A",
    ribbon: "#C74532",
    light: "#FFD99A",
    lights: LIGHTS,
  ambient: { kind: "petal", color: "#F3D6C2" },
    ornaments: [
      { t: 0.06, kind: "lantern", color: "#C0392B", scale: 1.4, hang: 22 },
      { t: 0.105, kind: "star", color: "#EBC46A", scale: 0.9, hang: 30 },
      { t: 0.17, kind: "lantern", color: "#A91F2A", scale: 1.05, hang: 24 },
      { t: 0.72, kind: "lantern", color: "#C0392B", scale: 1.05, hang: 25 },
      { t: 0.835, kind: "bauble", color: "#EBC46A", scale: 1.0, hang: 20 },
      { t: 0.885, kind: "lantern", color: "#A91F2A", scale: 1.25, hang: 30 },
      { t: 0.945, kind: "lantern", color: "#C0392B", scale: 1.4, hang: 21 },
    ],
    motifs: [
      { t: 0.035, kind: "blossom", color: "#F3D6C2", scale: 0.85 },
      { t: 0.14, kind: "blossom", color: "#F3D6C2", scale: 0.65, side: "top" },
      { t: 0.23, kind: "knot", color: "#EBC46A", scale: 0.75 },
      { t: 0.32, kind: "blossom", color: "#F0C9B4", scale: 0.55, side: "top" },
      { t: 0.66, kind: "blossom", color: "#F0C9B4", scale: 0.55, side: "top" },
      { t: 0.78, kind: "knot", color: "#EBC46A", scale: 0.7 },
      { t: 0.90, kind: "blossom", color: "#F3D6C2", scale: 0.7, side: "top" },
      { t: 0.965, kind: "blossom", color: "#F3D6C2", scale: 0.85 },
    ],
  },
  valentine: {
    cord: ["#6E2439", "#B5526B"],
    thread: "#EBC9B8",
    ribbon: "#C9768B",
    light: "#FFE1E7",
    lights: LIGHTS,
  ambient: { kind: "heart", color: "#EBC9B8" },
    ornaments: [
      { t: 0.065, kind: "heart", color: "#B5526B", scale: 1.5, hang: 22 },
      { t: 0.10, kind: "bauble", color: "#EBC9B8", scale: 0.95, hang: 30 },
      { t: 0.19, kind: "heart", color: "#C9768B", scale: 1.0, hang: 23 },
      { t: 0.30, kind: "heart", color: "#8B3249", scale: 0.8, hang: 28 },
      { t: 0.70, kind: "heart", color: "#8B3249", scale: 0.8, hang: 24 },
      { t: 0.82, kind: "heart", color: "#C9768B", scale: 1.05, hang: 20 },
      { t: 0.90, kind: "bauble", color: "#EBC9B8", scale: 0.95, hang: 28 },
      { t: 0.95, kind: "heart", color: "#B5526B", scale: 1.45, hang: 23 },
    ],
    motifs: [
      { t: 0.04, kind: "blossom", color: "#EBC9B8", scale: 0.7 },
      { t: 0.15, kind: "heart", color: "#C9768B", scale: 0.5, side: "top" },
      { t: 0.25, kind: "leaf", color: "#9C7A62", scale: 0.7, side: "top" },
      { t: 0.38, kind: "petal", color: "#EBC9B8", scale: 0.6 },
      { t: 0.62, kind: "petal", color: "#EBC9B8", scale: 0.6 },
      { t: 0.75, kind: "leaf", color: "#9C7A62", scale: 0.7, side: "top" },
      { t: 0.86, kind: "heart", color: "#C9768B", scale: 0.5, side: "top" },
      { t: 0.96, kind: "blossom", color: "#EBC9B8", scale: 0.7 },
    ],
  },
  "mid-autumn": {
    cord: ["#1D2943", "#3E5274"],
    thread: "#E8C46A",
    ribbon: "#C9A25A",
    light: "#FFE9AE",
    lights: LIGHTS,
  ambient: { kind: "petal", color: "#E8C46A" },
    ornaments: [
      { t: 0.065, kind: "moon", color: "#F4D98B", scale: 1.5, hang: 18 },
      { t: 0.10, kind: "mooncake", color: "#C99A5B", scale: 1.0, hang: 30 },
      { t: 0.19, kind: "lantern", color: "#D9A441", scale: 1.0, hang: 24 },
      { t: 0.30, kind: "star", color: "#F4D98B", scale: 0.8, hang: 20 },
      { t: 0.70, kind: "mooncake", color: "#C99A5B", scale: 1.05, hang: 27 },
      { t: 0.81, kind: "lantern", color: "#D9A441", scale: 1.0, hang: 22 },
      { t: 0.90, kind: "mooncake", color: "#B8874D", scale: 0.95, hang: 28 },
      { t: 0.95, kind: "moon", color: "#F4D98B", scale: 1.5, hang: 18 },
    ],
    motifs: [
      { t: 0.04, kind: "petal", color: "#E8C46A", scale: 0.6 },
      { t: 0.15, kind: "blossom", color: "#E8C46A", scale: 0.75, side: "top" },
      { t: 0.25, kind: "petal", color: "#F4D98B", scale: 0.55 },
      { t: 0.38, kind: "star", color: "#E8C46A", scale: 0.5 },
      { t: 0.62, kind: "star", color: "#E8C46A", scale: 0.5 },
      { t: 0.75, kind: "petal", color: "#F4D98B", scale: 0.55, side: "top" },
      { t: 0.86, kind: "blossom", color: "#E8C46A", scale: 0.7, side: "top" },
      { t: 0.96, kind: "petal", color: "#E8C46A", scale: 0.6 },
    ],
  },
  "national-day": {
    cord: ["#7D171B", "#C0302D"],
    thread: "#F0C75E",
    ribbon: "#D8A945",
    light: "#FFD98F",
    lights: LIGHTS,
  ambient: { kind: "sparkle", color: "#F0C75E" },
    ornaments: [
      { t: 0.065, kind: "flag", color: "#C0302D", scale: 1.25, hang: 22 },
      { t: 0.10, kind: "star", color: "#F0C75E", scale: 1.0, hang: 30 },
      { t: 0.18, kind: "firework", color: "#F0C75E", scale: 1.2, hang: 24 },
      { t: 0.29, kind: "flag", color: "#C0302D", scale: 0.9, hang: 24 },
      { t: 0.70, kind: "star", color: "#F0C75E", scale: 0.9, hang: 20 },
      { t: 0.81, kind: "firework", color: "#E9B94B", scale: 1.1, hang: 28 },
      { t: 0.90, kind: "flag", color: "#C0302D", scale: 1.0, hang: 22 },
      { t: 0.95, kind: "star", color: "#F0C75E", scale: 1.1, hang: 26 },
    ],
    motifs: [
      { t: 0.095, kind: "pennant", color: "#C0302D", scale: 0.85 },
      { t: 0.16, kind: "pennant", color: "#F0C75E", scale: 0.7 },
      { t: 0.23, kind: "star", color: "#F0C75E", scale: 0.65, side: "top" },
      { t: 0.34, kind: "pennant", color: "#C0302D", scale: 0.65 },
      { t: 0.66, kind: "pennant", color: "#D23B2E", scale: 0.65 },
      { t: 0.77, kind: "pennant", color: "#F0C75E", scale: 0.7 },
      { t: 0.865, kind: "pennant", color: "#C0302D", scale: 0.85 },
      { t: 0.97, kind: "star", color: "#F0C75E", scale: 0.65, side: "top" },
    ],
  },
  halloween: {
    cord: ["#2E1E3A", "#54305C"],
    thread: "#E28A37",
    ribbon: "#C46A2B",
    light: "#FFC46B",
    lights: LIGHTS,
  ambient: { kind: "bat", color: "#6B4B78" },
    ornaments: [
      { t: 0.065, kind: "pumpkin", color: "#D2691E", scale: 1.5, hang: 20 },
      { t: 0.105, kind: "bat", color: "#2A1B30", scale: 1.3, hang: 30 },
      { t: 0.19, kind: "star", color: "#E28A37", scale: 0.8, hang: 24 },
      { t: 0.30, kind: "pumpkin", color: "#E8892B", scale: 0.9, hang: 22 },
      { t: 0.70, kind: "bat", color: "#2A1B30", scale: 1.0, hang: 28 },
      { t: 0.82, kind: "pumpkin", color: "#D2691E", scale: 1.05, hang: 22 },
      { t: 0.90, kind: "bat", color: "#2A1B30", scale: 1.15, hang: 25 },
      { t: 0.95, kind: "pumpkin", color: "#E8892B", scale: 1.45, hang: 21 },
    ],
    motifs: [
      { t: 0.13, kind: "bat", color: "#6B4B78", scale: 0.6, side: "top" },
      { t: 0.24, kind: "leaf", color: "#8B5E3C", scale: 0.75, side: "top" },
      { t: 0.36, kind: "star", color: "#E28A37", scale: 0.5 },
      { t: 0.64, kind: "star", color: "#E28A37", scale: 0.5 },
      { t: 0.76, kind: "leaf", color: "#8B5E3C", scale: 0.75, side: "top" },
      { t: 0.87, kind: "bat", color: "#6B4B78", scale: 0.6, side: "top" },
    ],
  },
  christmas: {
    cord: ["#142E22", "#2F5A3A"],
    thread: "#D8B26A",
    ribbon: "#A34B4E",
    light: "#FFE7B0",
    lights: LIGHTS,
  ambient: { kind: "snow", color: "#DDE8E1" },
    ornaments: [
      { t: 0.06, kind: "bauble", color: "#A34B4E", scale: 1.25, hang: 22 },
      { t: 0.10, kind: "star", color: "#D9B45C", scale: 1.0, hang: 31 },
      { t: 0.18, kind: "snow", color: "#E8EEE7", scale: 1.1, hang: 25 },
      { t: 0.29, kind: "bell", color: "#C9A05C", scale: 1.1, hang: 22 },
      { t: 0.70, kind: "pinecone", color: "#8B5E3C", scale: 1.2, hang: 27 },
      { t: 0.81, kind: "bauble", color: "#3E6B4F", scale: 1.2, hang: 20 },
      { t: 0.90, kind: "star", color: "#D9B45C", scale: 1.0, hang: 28 },
      { t: 0.95, kind: "bauble", color: "#A34B4E", scale: 1.25, hang: 23 },
    ],
    motifs: [
      { t: 0.035, kind: "pine", color: "#4F7A58", scale: 0.8 },
      { t: 0.135, kind: "pine", color: "#3E6B4F", scale: 0.65, side: "top" },
      { t: 0.23, kind: "snow", color: "#DDE8E1", scale: 0.6 },
      { t: 0.34, kind: "pine", color: "#4F7A58", scale: 0.55, side: "top" },
      { t: 0.66, kind: "pine", color: "#4F7A58", scale: 0.55, side: "top" },
      { t: 0.77, kind: "snow", color: "#DDE8E1", scale: 0.6 },
      { t: 0.87, kind: "pine", color: "#3E6B4F", scale: 0.65, side: "top" },
      { t: 0.965, kind: "pine", color: "#4F7A58", scale: 0.8 },
    ],
  },
};

/** Lunar festival dates (lunar new year and lunar 8/15) for the local-date windows. */
const LUNAR_FESTIVALS: Record<"spring-festival" | "mid-autumn", Record<number, string>> = {
  "spring-festival": {
    2023: "01-22",
    2024: "02-10",
    2025: "01-29",
    2026: "02-17",
    2027: "02-06",
    2028: "01-26",
    2029: "02-13",
    2030: "02-03",
    2031: "01-23",
    2032: "02-11",
    2033: "01-31",
    2034: "02-19",
    2035: "02-08",
    2036: "01-28",
  },
  "mid-autumn": {
    2023: "09-29",
    2024: "09-17",
    2025: "10-06",
    2026: "09-25",
    2027: "09-15",
    2028: "10-03",
    2029: "09-22",
    2030: "09-12",
    2031: "10-01",
    2032: "09-19",
    2033: "09-08",
    2034: "09-27",
    2035: "09-16",
    2036: "09-05",
  },
};

function monthDayKey(month: number, day: number): number {
  return month * 100 + day;
}

function dateKey(date: Date): number {
  return monthDayKey(date.getMonth() + 1, date.getDate());
}

function inWrappingRange(date: Date, startMonthDay: number, endMonthDay: number): boolean {
  const key = dateKey(date);
  return startMonthDay <= endMonthDay
    ? key >= startMonthDay && key <= endMonthDay
    : key >= startMonthDay || key <= endMonthDay;
}

function atMidnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function dayDistance(from: Date, to: Date): number {
  return Math.round((atMidnight(to).getTime() - atMidnight(from).getTime()) / 86_400_000);
}

function lunarFestival(id: "spring-festival" | "mid-autumn", year: number): Date | null {
  const monthDay = LUNAR_FESTIVALS[id][year];
  if (!monthDay) return null;
  const [month, day] = monthDay.split("-").map(Number);
  return new Date(year, month - 1, day);
}

/** True while `date` falls inside the holiday's local-date window (inclusive). */
export function holidayWindowIncludes(id: HolidayThemeId, date: Date): boolean {
  switch (id) {
    case "new-year":
      return inWrappingRange(date, 1230, 102);
    case "spring-festival": {
      const festival = lunarFestival("spring-festival", date.getFullYear());
      if (!festival) return false;
      const offset = dayDistance(festival, date);
      return offset >= -1 && offset <= 6;
    }
    case "valentine":
      return inWrappingRange(date, 212, 215);
    case "mid-autumn": {
      const festival = lunarFestival("mid-autumn", date.getFullYear());
      if (!festival) return false;
      const offset = dayDistance(festival, date);
      return offset >= -1 && offset <= 1;
    }
    case "national-day":
      return inWrappingRange(date, 930, 1007);
    case "halloween":
      return inWrappingRange(date, 1025, 1101);
    case "christmas":
      return inWrappingRange(date, 1201, 106);
  }
}

/**
 * Auto-selection priority: shorter/lunar festivals win over the long Christmas season, and the
 * New Year window wins over Christmas around 12/30–01/02.
 */
export const HOLIDAY_AUTO_PRIORITY: readonly HolidayThemeId[] = [
  "spring-festival",
  "mid-autumn",
  "new-year",
  "christmas",
  "valentine",
  "national-day",
  "halloween",
];
