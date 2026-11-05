import { describe, expect, it } from "vitest";
import { composite, contrastOver, contrastRatio, parseColor, relativeLuminance } from "./color";
import { readCss, token, tokensOf } from "./cssTokens";

describe("color helpers", () => {
  it("parses hex (#rgb/#rrggbb) and rgb/rgba strings", () => {
    expect(parseColor("#fff")).toEqual({ r: 255, g: 255, b: 255, a: 1 });
    expect(parseColor("#14171b")).toEqual({ r: 20, g: 23, b: 27, a: 1 });
    expect(parseColor("rgba(14, 19, 25, 0.86)")).toEqual({ r: 14, g: 19, b: 25, a: 0.86 });
    expect(parseColor("rgb(10 20 30 / 0.5)")).toEqual({ r: 10, g: 20, b: 30, a: 0.5 });
  });

  it("rejects malformed colours (documented throw behaviour)", () => {
    expect(() => parseColor("#12")).toThrow();
    expect(() => parseColor("#zzzzzz")).toThrow();
    expect(() => parseColor("rgb(1, 2)")).toThrow();
    expect(() => parseColor("not-a-colour")).toThrow();
  });

  it("composites translucent tokens over an opaque background", () => {
    expect(composite("rgba(0, 0, 0, 0.5)", "#ffffff")).toBe("#808080");
    expect(composite("#ff0000", "#000000")).toBe("#ff0000");
    expect(composite("rgba(255, 255, 255, 0.5)", "#000000")).toBe("#808080");
  });

  it("computes WCAG luminance and contrast", () => {
    expect(relativeLuminance("#000000")).toBe(0);
    expect(relativeLuminance("#ffffff")).toBeCloseTo(1, 5);
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#ffffff", "#ffffff")).toBeCloseTo(1, 5);
  });

  it("keeps the Copied pill tokens readable over white and black (values parsed from Panel.css)", () => {
    // P2-9 residual: no duplicated colour literals — read the real tokens instead.
    const panelCss = readCss("../windows/panel/Panel.css");
    for (const selector of ['.panel', '.panel[data-theme="light"]']) {
      const tokens = tokensOf(panelCss, selector);
      const text = token(tokens, "pill-text");
      const background = token(tokens, "pill-bg");
      for (const under of ["#ffffff", "#000000"]) {
        expect(
          contrastOver(text, background, under),
          `${selector} pill over ${under}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});
