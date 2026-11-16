/** v1.6.0 local holiday windows for the rope theme. Pure and timezone-local by design. */

import type { HolidayTheme, HolidayThemeId } from "./types";
import { HOLIDAY_AUTO_PRIORITY, HOLIDAY_THEME_IDS, holidayWindowIncludes } from "./holidayThemes";

/** Resolved decorative theme; `null` means the normal rope. */
export type ActiveHolidayTheme = HolidayThemeId | null;

/** True for 12/01 through 01/06 inclusive (local calendar date). */
export function isChristmasWindow(date: Date = new Date()): boolean {
  return holidayWindowIncludes("christmas", date);
}

/**
 * Resolves the rope theme that should be visible now.
 *
 * - `off`: never;
 * - `auto`: the highest-priority local-date window that contains today;
 * - a specific id: always (manual selection / preview).
 */
export function activeHolidayTheme(
  theme: HolidayTheme | undefined,
  date: Date = new Date(),
): ActiveHolidayTheme {
  if (!theme || theme === "off") return null;
  if (theme === "auto") {
    return HOLIDAY_AUTO_PRIORITY.find((id) => holidayWindowIncludes(id, date)) ?? null;
  }
  return HOLIDAY_THEME_IDS.includes(theme) ? theme : null;
}

/** Stable wire-safe check used when hydrating a persisted setting. */
export function parseHolidayTheme(value: unknown): HolidayTheme {
  if (value === "off" || value === "auto") return value;
  return typeof value === "string" && HOLIDAY_THEME_IDS.includes(value as HolidayThemeId)
    ? (value as HolidayThemeId)
    : "off";
}

/** localStorage key prefix for the once-per-holiday hint (local only, no telemetry). */
const HOLIDAY_PROMPT_PREFIX = "hangline.holidayPrompt";

/**
 * One hint per holiday per calendar year: the auto window resolves the holiday id, so New Year,
 * Spring Festival, Christmas, ... each get their own once-only flag. Calendar-year keying is enough
 * here because each window opens at most once in a year (Christmas's January tail is a new year and
 * therefore a new key, which is acceptable for a passive hint).
 */
export function holidayPromptKey(date: Date): string {
  const id = activeHolidayTheme("auto", date) ?? "none";
  return `${HOLIDAY_PROMPT_PREFIX}.${id}.${date.getFullYear()}`;
}

/**
 * True when a passive hint should be shown at most once: a holiday auto-window is open, the theme
 * is still off, and this holiday's hint has not been shown yet. Pure so the date/seen half is
 * testable without a DOM; the localStorage read/write lives in the panel's effect.
 */
export function holidayPromptDue(theme: HolidayTheme | string, date: Date, seen: boolean): boolean {
  return (
    parseHolidayTheme(theme) === "off" &&
    activeHolidayTheme("auto", date) !== null &&
    !seen
  );
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
