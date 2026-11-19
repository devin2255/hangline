import { describe, expect, it } from "vitest";

import {
  activeHolidayTheme,
  holidayPromptDue,
  isChristmasWindow,
  parseHolidayTheme,
} from "./holiday";
import {
  HOLIDAY_ROPE_SPECS,
  HOLIDAY_THEME_IDS,
  holidayWindowIncludes,
} from "./holidayThemes";

describe("holiday windows", () => {
  const at = (year: number, month: number, day: number) => new Date(year, month - 1, day, 12, 0, 0);

  it("starts on December 1 and ends on January 6 (inclusive)", () => {
    expect(isChristmasWindow(at(2026, 11, 30))).toBe(false);
    expect(isChristmasWindow(at(2026, 12, 1))).toBe(true);
    expect(isChristmasWindow(at(2026, 12, 25))).toBe(true);
    expect(isChristmasWindow(at(2027, 1, 6))).toBe(true);
    expect(isChristmasWindow(at(2027, 1, 7))).toBe(false);
    expect(isChristmasWindow(at(2027, 2, 1))).toBe(false);
  });

  it("resolves off / auto / manual selections", () => {
    const december = at(2026, 12, 10);
    const summer = at(2027, 7, 1);
    expect(activeHolidayTheme("off", december)).toBeNull();
    expect(activeHolidayTheme("auto", summer)).toBeNull();
    expect(activeHolidayTheme("auto", december)).toBe("christmas");
    expect(activeHolidayTheme("christmas", summer)).toBe("christmas");
    expect(activeHolidayTheme(undefined, december)).toBeNull();
  });

  it("covers every holiday's local-date window", () => {
    expect(holidayWindowIncludes("new-year", at(2026, 12, 30))).toBe(true);
    expect(holidayWindowIncludes("new-year", at(2027, 1, 2))).toBe(true);
    expect(holidayWindowIncludes("new-year", at(2027, 1, 3))).toBe(false);
    expect(holidayWindowIncludes("spring-festival", at(2027, 2, 5))).toBe(true);
    expect(holidayWindowIncludes("spring-festival", at(2027, 2, 6))).toBe(true);
    expect(holidayWindowIncludes("spring-festival", at(2027, 2, 12))).toBe(true);
    expect(holidayWindowIncludes("spring-festival", at(2027, 2, 13))).toBe(false);
    expect(holidayWindowIncludes("valentine", at(2026, 2, 12))).toBe(true);
    expect(holidayWindowIncludes("valentine", at(2026, 2, 15))).toBe(true);
    expect(holidayWindowIncludes("mid-autumn", at(2026, 9, 24))).toBe(true);
    expect(holidayWindowIncludes("mid-autumn", at(2026, 9, 25))).toBe(true);
    expect(holidayWindowIncludes("mid-autumn", at(2026, 9, 26))).toBe(true);
    expect(holidayWindowIncludes("mid-autumn", at(2026, 9, 27))).toBe(false);
    expect(holidayWindowIncludes("national-day", at(2026, 9, 30))).toBe(true);
    expect(holidayWindowIncludes("national-day", at(2026, 10, 7))).toBe(true);
    expect(holidayWindowIncludes("halloween", at(2026, 10, 25))).toBe(true);
    expect(holidayWindowIncludes("halloween", at(2026, 11, 1))).toBe(true);
  });

  it("auto-picks the most specific holiday when windows overlap", () => {
    expect(activeHolidayTheme("auto", at(2026, 12, 31))).toBe("new-year");
    expect(activeHolidayTheme("auto", at(2027, 2, 5))).toBe("spring-festival");
    expect(activeHolidayTheme("auto", at(2026, 9, 25))).toBe("mid-autumn");
    expect(activeHolidayTheme("auto", at(2026, 10, 1))).toBe("national-day");
    expect(activeHolidayTheme("auto", at(2026, 2, 14))).toBe("valentine");
    expect(activeHolidayTheme("auto", at(2026, 10, 31))).toBe("halloween");
  });

  it("manually previews every supported holiday", () => {
    const summer = at(2026, 7, 1);
    for (const id of HOLIDAY_THEME_IDS) {
      expect(activeHolidayTheme(id, summer)).toBe(id);
      expect(parseHolidayTheme(id)).toBe(id);
      if (id !== "christmas") {
        expect(HOLIDAY_ROPE_SPECS[id].ornaments.length).toBeGreaterThanOrEqual(6);
      }
    }
    expect(parseHolidayTheme("spring")).toBe("off");
    expect(parseHolidayTheme("not-a-holiday")).toBe("off");
  });

  it("parses persisted values conservatively", () => {
    expect(parseHolidayTheme("auto")).toBe("auto");
    expect(parseHolidayTheme("christmas")).toBe("christmas");
    expect(parseHolidayTheme("off")).toBe("off");
    expect(parseHolidayTheme("spring")).toBe("off");
    expect(parseHolidayTheme(null)).toBe("off");
  });

  it("prompts once per holiday window, and only while the theme is off", () => {
    const christmas = new Date(2026, 11, 10, 12, 0, 0);
    const newYear = new Date(2026, 11, 31, 12, 0, 0);
    const nextJanuary = new Date(2027, 0, 3, 12, 0, 0);
    expect(holidayPromptDue("off", christmas, false)).toBe(true);
    expect(holidayPromptDue("off", christmas, true)).toBe(false);
    expect(holidayPromptDue("auto", christmas, false)).toBe(false);
    expect(holidayPromptDue("christmas", christmas, false)).toBe(false);
    expect(holidayPromptDue("off", newYear, false)).toBe(true);
    expect(holidayPromptDue("off", nextJanuary, false)).toBe(true);
    expect(holidayPromptDue("off", new Date(2026, 10, 30, 12, 0, 0), false)).toBe(false);
  });
});
