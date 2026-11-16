/** v1.6.0 holiday themes shared by the date windows and the rope renderer. */
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
  /** Optional visual scale; the end ornaments use slightly larger copies. */
  scale?: number;
}

export interface HolidayRopeSpec {
  /** Two cord stops (left/right) and the thin thread colour. */
  cord: readonly [string, string];
  thread: string;
  /** Fairy-light bulb colour. */
  light: string;
  /** Bulb positions along the rope. */
  lights: readonly number[];
  ornaments: readonly HolidayOrnament[];
  /** The Christmas pine sprig + ribbon ends are a special case. */
  pineBowEnds?: boolean;
}

const BASE_LIGHTS = [0.1, 0.24, 0.38, 0.52, 0.66, 0.8, 0.93] as const;

export const HOLIDAY_THEME_IDS: readonly HolidayThemeId[] = [
  "new-year",
  "spring-festival",
  "valentine",
  "mid-autumn",
  "national-day",
  "halloween",
  "christmas",
];

export const HOLIDAY_ROPE_SPECS: Record<HolidayThemeId, HolidayRopeSpec> = {
  "new-year": {
    cord: ["#7E1D26", "#C0392B"],
    thread: "#E8C46A",
    light: "#FFD98A",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.055, kind: "firework", color: "#E8C46A", scale: 1.25 },
      { t: 0.16, kind: "star", color: "#F2C14E" },
      { t: 0.31, kind: "bauble", color: "#C0392B" },
      { t: 0.44, kind: "firework", color: "#F2C14E" },
      { t: 0.58, kind: "bauble", color: "#E8C46A" },
      { t: 0.70, kind: "star", color: "#F2C14E" },
      { t: 0.85, kind: "firework", color: "#E8C46A" },
      { t: 0.945, kind: "star", color: "#F2C14E", scale: 1.2 },
    ],
  },
  "spring-festival": {
    cord: ["#8F171F", "#D33A2C"],
    thread: "#F2C14E",
    light: "#FFD27A",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.055, kind: "lantern", color: "#C0392B", scale: 1.3 },
      { t: 0.18, kind: "star", color: "#F2C14E" },
      { t: 0.32, kind: "lantern", color: "#D33A2C" },
      { t: 0.46, kind: "bauble", color: "#F2C14E" },
      { t: 0.60, kind: "lantern", color: "#C0392B" },
      { t: 0.73, kind: "star", color: "#F2C14E" },
      { t: 0.86, kind: "lantern", color: "#D33A2C" },
      { t: 0.945, kind: "lantern", color: "#C0392B", scale: 1.3 },
    ],
  },
  valentine: {
    cord: ["#922C49", "#D77A8F"],
    thread: "#F1D6C8",
    light: "#FFD7E1",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.06, kind: "heart", color: "#C24D69", scale: 1.35 },
      { t: 0.18, kind: "heart", color: "#E9A7B6" },
      { t: 0.32, kind: "bauble", color: "#F1D6C8" },
      { t: 0.46, kind: "heart", color: "#C24D69" },
      { t: 0.60, kind: "star", color: "#E9A7B6" },
      { t: 0.73, kind: "heart", color: "#D77A8F" },
      { t: 0.86, kind: "bauble", color: "#F1D6C8" },
      { t: 0.945, kind: "heart", color: "#C24D69", scale: 1.35 },
    ],
  },
  "mid-autumn": {
    cord: ["#27324C", "#3B4D6B"],
    thread: "#E8C46A",
    light: "#FFE8A3",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.055, kind: "moon", color: "#F4D98B", scale: 1.5 },
      { t: 0.18, kind: "mooncake", color: "#C99A5B" },
      { t: 0.32, kind: "lantern", color: "#D9A441" },
      { t: 0.46, kind: "star", color: "#F4D98B" },
      { t: 0.60, kind: "mooncake", color: "#C99A5B" },
      { t: 0.73, kind: "lantern", color: "#D9A441" },
      { t: 0.86, kind: "mooncake", color: "#C99A5B" },
      { t: 0.945, kind: "moon", color: "#F4D98B", scale: 1.5 },
    ],
  },
  "national-day": {
    cord: ["#8C171A", "#D23B2E"],
    thread: "#F2C14E",
    light: "#FFD27A",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.055, kind: "flag", color: "#C0392B", scale: 1.25 },
      { t: 0.18, kind: "star", color: "#F2C14E" },
      { t: 0.32, kind: "firework", color: "#F2C14E" },
      { t: 0.46, kind: "flag", color: "#D23B2E" },
      { t: 0.60, kind: "star", color: "#F2C14E" },
      { t: 0.73, kind: "firework", color: "#F2C14E" },
      { t: 0.86, kind: "flag", color: "#C0392B" },
      { t: 0.945, kind: "star", color: "#F2C14E", scale: 1.2 },
    ],
  },
  halloween: {
    cord: ["#3B2342", "#5A2D5C"],
    thread: "#F28C28",
    light: "#FFB347",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.055, kind: "pumpkin", color: "#D2691E", scale: 1.35 },
      { t: 0.18, kind: "bat", color: "#2A1B30" },
      { t: 0.32, kind: "star", color: "#F28C28" },
      { t: 0.46, kind: "pumpkin", color: "#E8892B" },
      { t: 0.60, kind: "bat", color: "#2A1B30" },
      { t: 0.73, kind: "pumpkin", color: "#D2691E" },
      { t: 0.86, kind: "bat", color: "#2A1B30" },
      { t: 0.945, kind: "pumpkin", color: "#E8892B", scale: 1.35 },
    ],
  },
  christmas: {
    cord: ["#1E3A2B", "#2F523C"],
    thread: "#C9A05C",
    light: "#FFE7B0",
    lights: BASE_LIGHTS,
    ornaments: [
      { t: 0.055, kind: "bauble", color: "#A34B4E" },
      { t: 0.16, kind: "star", color: "#D9B45C" },
      { t: 0.33, kind: "bell", color: "#C9A05C" },
      { t: 0.44, kind: "snow", color: "#E8EEE7" },
      { t: 0.58, kind: "bauble", color: "#3E6B4F" },
      { t: 0.70, kind: "pinecone", color: "#8B5E3C" },
      { t: 0.85, kind: "star", color: "#D9B45C" },
      { t: 0.945, kind: "bauble", color: "#A34B4E" },
    ],
    pineBowEnds: true,
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
  // A window such as New Year (12/30–01/02) wraps around the calendar year.
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
