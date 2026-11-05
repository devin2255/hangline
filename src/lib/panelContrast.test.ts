import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { composite, contrastRatio, contrastOver } from "./color";

/**
 * P2-8 contrast guard.
 *
 * The test reads the **real CSS token values** (no duplicated literals) and evaluates every
 * small-UI text token against worst-case backdrops: the panel sits on a transparent desktop, so
 * the hint/pill/circle material can be over pure white or pure black; capture labels float over the
 * frozen frame; the shortcut dialog paints its own opaque surface.
 */

const PANEL_CSS = new URL("../windows/panel/Panel.css", import.meta.url);
const FLY_CSS = new URL("../windows/fly/Fly.css", import.meta.url);
const CAPTURE_CSS = new URL("../windows/capture/Capture.css", import.meta.url);
const SHORTCUT_CSS = new URL("../windows/shortcut/Shortcut.css", import.meta.url);

function readCss(url: URL): string {
  return readFileSync(url, "utf8");
}

/** Extracts `--name: value;` declarations from one selector block. */
export function tokensOf(css: string, selector: string): Map<string, string> {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(`${escaped}\\s*\\{`).exec(css);
  if (!match) throw new Error(`selector not found: ${selector}`);
  const start = match.index + match[0].length;
  const end = css.indexOf("}", start);
  if (end < 0) throw new Error(`unterminated block: ${selector}`);
  const block = css.slice(start, end);
  const tokens = new Map<string, string>();
  const declaration = /--([\w-]+)\s*:\s*([^;]+);/g;
  for (let item = declaration.exec(block); item; item = declaration.exec(block)) {
    tokens.set(item[1], item[2].trim().replace(/\s+/g, " "));
  }
  return tokens;
}

function token(tokens: Map<string, string>, name: string): string {
  const value = tokens.get(name);
  if (!value) throw new Error(`missing token --${name}`);
  return value;
}

/** WCAG contrast of `fg` over `bg` composited on an opaque `under` layer. */
function fgOnBg(fg: string, bg: string, under: string): number {
  return contrastOver(fg, bg, under);
}

describe("panel small-UI contrast from real CSS tokens (P2-8)", () => {
  const css = readCss(PANEL_CSS);
  const themes: Array<[string, Map<string, string>]> = [
    ["dark", tokensOf(css, ".panel")],
    ["light", tokensOf(css, '.panel[data-theme="light"]')],
  ];

  it("keeps the hint text >= 4.5:1 over pure white and pure black", () => {
    for (const [name, tokens] of themes) {
      const text = token(tokens, "hint-text");
      const bg = token(tokens, "hint-bg");
      for (const under of ["#ffffff", "#000000"]) {
        const ratio = fgOnBg(text, bg, under);
        expect(ratio, `${name} hint over ${under} = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("keeps the Copied pill text >= 4.5:1 over pure white and pure black", () => {
    for (const [name, tokens] of themes) {
      const text = token(tokens, "pill-text");
      const bg = token(tokens, "pill-bg");
      for (const under of ["#ffffff", "#000000"]) {
        const ratio = fgOnBg(text, bg, under);
        expect(ratio, `${name} pill over ${under} = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("keeps the play/cross glyph >= 4.5:1 over pure white and pure black", () => {
    for (const [name, tokens] of themes) {
      const glyph = token(tokens, "fallback-text");
      const circle = token(tokens, "glass-circle-bg");
      for (const under of ["#ffffff", "#000000"]) {
        const ratio = fgOnBg(glyph, circle, under);
        expect(ratio, `${name} glyph over ${under} = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("keeps the fly material tokens identical to the panel's dark material", () => {
    const panelDark = themes[0][1];
    const flyDark = tokensOf(readCss(FLY_CSS), 'html[data-window="fly"]');
    for (const shared of [
      "glass-fill-top",
      "glass-fill-bottom",
      "glass-tint",
      "glass-border",
      "photo-bg",
    ]) {
      expect(token(flyDark, shared), `fly ${shared}`).toBe(token(panelDark, shared));
    }
  });
});

describe("capture overlay label contrast from real CSS tokens (P2-8)", () => {
  const css = readCss(CAPTURE_CSS);
  const themes: Array<[string, Map<string, string>]> = [
    ["dark", tokensOf(css, 'html[data-window="capture"]')],
    ["light", tokensOf(css, 'html[data-window="capture"][data-theme="light"]')],
  ];

  it("keeps the dimension label readable over a frozen frame (label bg over black/white)", () => {
    for (const [name, tokens] of themes) {
      const text = token(tokens, "capture-label-text");
      const bg = token(tokens, "capture-label-bg");
      for (const under of ["#ffffff", "#000000"]) {
        const background = composite(bg, under);
        const ratio = contrastRatio(composite(text, background), background);
        expect(ratio, `${name} label over ${under} = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});

describe("shortcut dialog contrast from real CSS tokens (P2-8)", () => {
  const css = readCss(SHORTCUT_CSS);
  const themes: Array<[string, Map<string, string>]> = [
    ["dark", tokensOf(css, 'html[data-window="shortcut"]')],
    ["light", tokensOf(css, 'html[data-window="shortcut"][data-theme="light"]')],
  ];

  it("keeps body/muted/primary text >= 4.5:1 on the dialog surface", () => {
    for (const [name, tokens] of themes) {
      const surface = token(tokens, "shortcut-window-bg");
      for (const [field, value] of [
        ["text", token(tokens, "shortcut-text")],
        ["muted", token(tokens, "shortcut-muted")],
      ] as const) {
        const ratio = contrastRatio(value, surface);
        expect(ratio, `${name} ${field} on surface = ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      }
      const primaryRatio = contrastRatio(
        token(tokens, "shortcut-primary-text"),
        token(tokens, "shortcut-primary"),
      );
      expect(
        primaryRatio,
        `${name} primary button = ${primaryRatio.toFixed(2)}`,
      ).toBeGreaterThanOrEqual(4.5);
      // Hover swaps to a brighter/darker fill; the label must stay AA there too (P2-8 residual).
      const hoverRatio = contrastRatio(
        token(tokens, "shortcut-primary-text"),
        token(tokens, "shortcut-primary-hover"),
      );
      expect(
        hoverRatio,
        `${name} primary button hover = ${hoverRatio.toFixed(2)}`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });
});
