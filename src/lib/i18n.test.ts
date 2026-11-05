import { describe, expect, it } from "vitest";
import { activeLanguageTag, languageForResolvedTag, setResolvedLanguage } from "./i18n";

describe("resolved language mapping", () => {
  it("maps the two Rust-resolved tags to the shipped tables", () => {
    expect(languageForResolvedTag("en")).toBe("en");
    expect(languageForResolvedTag("zh-Hans")).toBe("zh-Hans");
  });

  it("accepts full Chinese locale tags defensively", () => {
    expect(languageForResolvedTag("zh")).toBe("zh-Hans");
    expect(languageForResolvedTag("zh-CN")).toBe("zh-Hans");
    expect(languageForResolvedTag("zh-CN".toLowerCase())).toBe("zh-Hans");
    expect(languageForResolvedTag("ZH-HANT")).toBe("zh-Hans");
  });

  it("falls back to English for unsupported/empty tags", () => {
    expect(languageForResolvedTag("fr")).toBe("en");
    expect(languageForResolvedTag("")).toBe("en");
    expect(languageForResolvedTag("   ")).toBe("en");
  });

  it("switches the active table from the Rust tag and reports it", () => {
    expect(setResolvedLanguage("zh-Hans")).toBe("zh-Hans");
    expect(activeLanguageTag()).toBe("zh-Hans");
    expect(setResolvedLanguage("en")).toBe("en");
    expect(activeLanguageTag()).toBe("en");
    // `navigator.language` is intentionally not consulted: the Rust-resolved tag is authoritative.
    expect(setResolvedLanguage("fr")).toBe("en");
  });
});
