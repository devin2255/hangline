/** v1.6.0 local holiday windows for the rope theme. Pure and timezone-local by design. */

export type HolidayTheme = "off" | "auto" | "christmas";

export type ActiveHolidayTheme = "christmas" | null;

const CHRISTMAS = { startMonth: 11, startDay: 1, endMonth: 0, endDay: 6 } as const;

/** True for 12/01 through 01/06 inclusive (local calendar date). */
export function isChristmasWindow(date: Date = new Date()): boolean {
  const month = date.getMonth();
  const day = date.getDate();
  if (month === CHRISTMAS.startMonth) return day >= CHRISTMAS.startDay;
  if (month === CHRISTMAS.endMonth) return day <= CHRISTMAS.endDay;
  return false;
}

/**
 * Resolves the rope theme that should be visible now.
 *
 * - `off`: never;
 * - `auto`: only inside the Christmas window;
 * - `christmas`: always (manual selection / preview).
 */
export function activeHolidayTheme(
  theme: HolidayTheme | undefined,
  date: Date = new Date(),
): ActiveHolidayTheme {
  if (theme === "christmas") return "christmas";
  if (theme === "auto" && isChristmasWindow(date)) return "christmas";
  return null;
}

/** Stable wire-safe check used when hydrating a persisted setting. */
export function parseHolidayTheme(value: unknown): HolidayTheme {
  return value === "auto" || value === "christmas" ? value : "off";
}
