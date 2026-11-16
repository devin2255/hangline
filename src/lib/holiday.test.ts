import { describe, expect, it } from "vitest";

import { activeHolidayTheme, isChristmasWindow, parseHolidayTheme } from "./holiday";

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

  it("parses persisted values conservatively", () => {
    expect(parseHolidayTheme("auto")).toBe("auto");
    expect(parseHolidayTheme("christmas")).toBe("christmas");
    expect(parseHolidayTheme("off")).toBe("off");
    expect(parseHolidayTheme("spring")).toBe("off");
    expect(parseHolidayTheme(null)).toBe("off");
  });
});
