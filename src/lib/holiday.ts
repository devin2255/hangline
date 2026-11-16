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

/** localStorage key prefix for the once-per-season Christmas hint (local only, no telemetry). */
const HOLIDAY_PROMPT_PREFIX = "hangline.holidayPrompt";

/**
 * The Christmas season spans New Year, so a January date belongs to the previous year's season.
 * The key keeps the hint once per season instead of once per calendar year.
 */
export function holidaySeasonYear(date: Date): number {
  return date.getMonth() === 0 ? date.getFullYear() - 1 : date.getFullYear();
}

/** localStorage key for "the user has already seen the Christmas hint this season". */
export function holidayPromptKey(date: Date): string {
  return `${HOLIDAY_PROMPT_PREFIX}.christmas.${holidaySeasonYear(date)}`;
}

/**
 * True when a passive hint should be shown at most once: inside the Christmas window, the theme is
 * still off, and this season's hint has not been shown yet. Pure so the date/seen half is testable
 * without a DOM; the localStorage read/write lives in the panel's effect.
 */
export function holidayPromptDue(theme: HolidayTheme | string, date: Date, seen: boolean): boolean {
  return isChristmasWindow(date) && parseHolidayTheme(theme) === "off" && !seen;
}

/** Reads the persisted hint flag; storage failures degrade to "already seen" (never nag). */
export function readHolidayPromptSeen(date: Date): boolean {
  try {
    return window.localStorage.getItem(holidayPromptKey(date)) === "1";
  } catch {
    return true;
  }
}

/** Marks the hint as shown for this season; storage failures are harmless. */
export function markHolidayPromptSeen(date: Date): void {
  try {
    window.localStorage.setItem(holidayPromptKey(date), "1");
  } catch {
    // Private/blocked storage: the in-memory timer still hides the pill for this session.
  }
}
