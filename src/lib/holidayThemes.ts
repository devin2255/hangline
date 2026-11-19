/** v1.6.2 holiday themes shared by the date windows and the rope renderer. */
import type { HolidayThemeId } from "./types";

export const HOLIDAY_THEME_IDS: readonly HolidayThemeId[] = [
  "new-year",
  "spring-festival",
  "valentine",
  "mid-autumn",
  "national-day",
  "halloween",
  "christmas",
];

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
