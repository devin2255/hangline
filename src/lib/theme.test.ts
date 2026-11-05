import { describe, expect, it } from "vitest";
import { resolveTheme, themeFromLocation, themeSettingFromLocation } from "./theme";

describe("resolveTheme", () => {
  it("honours explicit light/dark settings regardless of the system preference", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("light", false)).toBe("light");
    expect(resolveTheme("dark", true)).toBe("dark");
    expect(resolveTheme("dark", false)).toBe("dark");
  });

  it("follows the system preference for system/missing/invalid settings", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme(undefined, true)).toBe("dark");
    expect(resolveTheme(null, false)).toBe("light");
    expect(resolveTheme("sepia", true)).toBe("dark");
  });
});

describe("themeFromLocation", () => {
  it("reads the explicit theme query passed by Rust", () => {
    expect(themeFromLocation("?window=capture&theme=light", false)).toBe("light");
    expect(themeFromLocation("?window=shortcut&target=capture&theme=dark", true)).toBe("dark");
  });

  it("falls back to the system query when the setting is system/absent", () => {
    expect(themeFromLocation("?theme=system", true)).toBe("dark");
    expect(themeFromLocation("?theme=system", false)).toBe("light");
    expect(themeFromLocation("", true)).toBe("dark");
    expect(themeFromLocation("", false)).toBe("light");
    expect(themeFromLocation("?theme=sepia", true)).toBe("dark");
  });
});

describe("themeSettingFromLocation", () => {
  it("returns the raw setting for the shared theme hook", () => {
    expect(themeSettingFromLocation("?window=capture&theme=light")).toBe("light");
    expect(themeSettingFromLocation("?window=shortcut&target=capture&theme=dark")).toBe("dark");
    expect(themeSettingFromLocation("?theme=system")).toBe("system");
    expect(themeSettingFromLocation("")).toBe("system");
    expect(themeSettingFromLocation("?theme=sepia")).toBe("system");
  });
});
