import { describe, expect, it } from "vitest";

import {
  buildSweaterSvg,
  normalizePalette,
  curatedSweaterPalette,
  isSweaterPattern,
  patternForItem,
  patternForItemWithOverride,
  stableHash,
  sweaterFrameDataUrl,
  SWEATER_PATTERNS,
} from "./sweater";

const palette = ["#8FA98F", "#D9A55C", "#E8DFCC"];

describe("sweater frame generator", () => {
  it("picks a stable pattern for a card id", () => {
    expect(patternForItem("abc")).toBe(patternForItem("abc"));
    const seen = new Set<string>();
    for (let index = 0; index < 64; index += 1) {
      seen.add(patternForItem(`item-${index}`));
    }
    expect(seen.size).toBeGreaterThan(2);
    for (const pattern of seen) expect(SWEATER_PATTERNS).toContain(pattern);
  });

  it("exposes the three curated palettes and rejects stale values", () => {
    expect(curatedSweaterPalette("pine")).toHaveLength(3);
    expect(curatedSweaterPalette("frost")).toHaveLength(3);
    expect(curatedSweaterPalette("berry")).toHaveLength(3);
    expect(curatedSweaterPalette("auto")).toBeNull();
    expect(curatedSweaterPalette("not-a-palette")).toBeNull();
  });

  it("prefers a valid per-card chart override and ignores stale slugs", () => {
    const itemId = "override-item";
    expect(patternForItemWithOverride(itemId, "snowflake")).toBe("snowflake");
    expect(patternForItemWithOverride(itemId, "not-a-chart")).toBe(patternForItem(itemId));
    expect(patternForItemWithOverride(itemId, null)).toBe(patternForItem(itemId));
    expect(isSweaterPattern("checks")).toBe(true);
    expect(isSweaterPattern("nope")).toBe(false);
    expect(isSweaterPattern(7)).toBe(false);
  });

  it("hashes deterministically without negative numbers", () => {
    expect(stableHash("hangline")).toBe(stableHash("hangline"));
    expect(stableHash("hangline")).toBeGreaterThanOrEqual(0);
    expect(Number.isInteger(stableHash("hangline"))).toBe(true);
  });

  it("normalizes malformed palettes to three colours", () => {
    expect(normalizePalette(null)).toHaveLength(3);
    expect(normalizePalette(["#ABCDEF"])).toHaveLength(3);
    expect(normalizePalette(["#ABCDEF", "not-a-colour", "#123456"])).toEqual([
      "#ABCDEF",
      "#123456",
      "#E8DFCC",
    ]);
  });

  it("keeps the screenshot colours in the generated SVG", () => {
    const svg = buildSweaterSvg(palette, "checks", "classic");
    expect(svg).toContain("<svg");
    expect(svg).toContain("#8FA98F");
    expect(svg).toContain("#D9A55C");
    expect(svg).toContain("#E8DFCC");
    expect(svg).toContain("fill-rule=\"evenodd\"");
    expect(svg).toContain("patterns".slice(0, 0) + "pattern");
  });

  it("changes the frame thickness per width setting", () => {
    const slim = buildSweaterSvg(palette, "stripe", "slim");
    const chunky = buildSweaterSvg(palette, "stripe", "chunky");
    expect(slim).not.toBe(chunky);
    expect(slim.length).toBeGreaterThan(200);
    expect(chunky.length).toBeGreaterThan(200);
  });

  it("produces a data URL the browser can use as a background image", () => {
    const url = sweaterFrameDataUrl(palette, "snowflake", "classic");
    expect(url.startsWith("data:image/svg+xml,")).toBe(true);
    expect(decodeURIComponent(url.slice("data:image/svg+xml,".length))).toContain("<svg");
  });
});
