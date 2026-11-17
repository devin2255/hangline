/**
 * v1.6.0 Sweater Season: deterministic knitted frames for screenshot cards.
 *
 * Pure SVG generation (no DOM/canvas) so the panel can cache one data URL per card and the logic is
 * testable under Vitest. The frame overlays the card's outer edge; the screenshot is never resized
 * and the card's hit rect / rope geometry stay unchanged.
 *
 * The band is stockinette, not a flat print. Each stitch is two round yarn legs — a cast
 * shadow, a darker rim, the yarn colour, and a same-hue highlight on the upper shoulder —
 * sitting on a deeper wool ground. Colourwork only changes the yarn colour. A few loose
 * fibres lie on top. Rows nest, and a stitch is a little wider than it is tall.
 */

import { CARD_RADIUS } from "./layout";
import type { SweaterPalette } from "./types";

export type SweaterPattern = "chevron" | "checks" | "snowflake" | "stripe" | "fairisle";

export type SweaterWidth = "slim" | "classic" | "chunky";

/** Default drawing viewBox used by tests; the panel passes the card's real CSS size. */
export const SWEATER_VIEWBOX = { width: 300, height: 200 } as const;

/** Curated palettes for the "Sweater palette" tray submenu (order = main, contrast, neutral). */
export const CURATED_SWEATER_PALETTES: Record<
  Exclude<SweaterPalette, "auto">,
  readonly [string, string, string]
> = {
  pine: ["#2F4A38", "#C9A05C", "#EFE3CC"],
  frost: ["#8FA6B2", "#DDE7EA", "#3D4A55"],
  berry: ["#7E2F3B", "#D9B45C", "#F1E4D2"],
};

/** Returns a curated palette copy for a known slug, or null for `auto`/stale values. */
export function curatedSweaterPalette(palette: unknown): string[] | null {
  if (palette === "pine" || palette === "frost" || palette === "berry") {
    return [...CURATED_SWEATER_PALETTES[palette]];
  }
  return null;
}

/**
 * Nominal band thickness in CSS pixels. Wide enough for two or three stockinette rows;
 * very short cards clamp it so the photograph keeps the middle of the frame.
 */
export const SWEATER_THICKNESS: Record<SweaterWidth, number> = {
  slim: 12,
  classic: 15,
  chunky: 20,
};

/** Stitch rows across the band. Fewer, larger stitches read as yarn; more rows read as fabric. */
const SWEATER_ROWS: Record<SweaterWidth, number> = {
  slim: 2,
  classic: 2,
  chunky: 3,
};

export const SWEATER_PATTERNS: readonly SweaterPattern[] = [
  "chevron",
  "checks",
  "snowflake",
  "stripe",
  "fairisle",
] as const;

const FALLBACK_PALETTE = ["#8FA98F", "#D9A55C", "#E8DFCC"] as const;

const HEX = /^#[0-9a-f]{6}$/i;

/** Small, stable string hash (FNV-1a 32-bit) used to pick a pattern per card id. */
export function stableHash(value: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/** Picks one of the five knitted charts from the card id; stable for the life of the card. */
export function patternForItem(itemId: string): SweaterPattern {
  return SWEATER_PATTERNS[stableHash(itemId) % SWEATER_PATTERNS.length];
}

/** True for a chart slug the renderer knows (persisted overrides are validated against this). */
export function isSweaterPattern(value: unknown): value is SweaterPattern {
  return typeof value === "string" && SWEATER_PATTERNS.includes(value as SweaterPattern);
}

/**
 * Chart for a card: an explicit per-card override wins only when it is a known slug; otherwise the
 * automatic stable hash applies.
 */
export function patternForItemWithOverride(itemId: string, override?: string | null): SweaterPattern {
  return isSweaterPattern(override) ? override : patternForItem(itemId);
}

/** Normalises a palette to exactly three valid hex colours (invalid entries fall back). */
export function normalizePalette(palette: readonly string[] | null | undefined): [string, string, string] {
  const valid = (palette ?? []).filter((color) => HEX.test(color)).slice(0, 3);
  if (valid.length >= 3) return [valid[0], valid[1], valid[2]];
  if (valid.length === 2) return [valid[0], valid[1], FALLBACK_PALETTE[2]];
  if (valid.length === 1) return [valid[0], FALLBACK_PALETTE[1], FALLBACK_PALETTE[2]];
  return [FALLBACK_PALETTE[0], FALLBACK_PALETTE[1], FALLBACK_PALETTE[2]];
}

function esc(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * Colourwork charts. One cell is one stitch.
 * 0 = neutral ground yarn, 1 = contrast, 2 = main. Every chart uses all three so a
 * palette sampled from the screenshot stays visible on every edge.
 */
const KNIT_CHARTS: Record<SweaterPattern, readonly (readonly (0 | 1 | 2)[])[]> = {
  checks: [
    [1, 1, 0, 0],
    [1, 1, 0, 0],
    [2, 2, 1, 1],
    [2, 2, 1, 1],
  ],
  stripe: [
    [1, 1, 1, 1, 1, 1],
    [1, 1, 1, 1, 1, 1],
    [2, 2, 2, 2, 2, 2],
    [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0],
    [2, 2, 2, 2, 2, 2],
  ],
  chevron: [
    [0, 0, 1, 2, 2, 1, 0, 0],
    [0, 1, 2, 1, 1, 2, 1, 0],
    [1, 2, 1, 0, 0, 1, 2, 1],
    [2, 1, 0, 1, 1, 0, 1, 2],
  ],
  snowflake: [
    [0, 0, 1, 0, 0, 1, 0, 0],
    [0, 1, 2, 1, 1, 2, 1, 0],
    [1, 2, 1, 0, 0, 1, 2, 1],
    [0, 1, 0, 2, 2, 0, 1, 0],
    [1, 2, 1, 0, 0, 1, 2, 1],
    [0, 1, 2, 1, 1, 2, 1, 0],
  ],
  fairisle: [
    [1, 1, 1, 1, 1, 1, 1, 1],
    [2, 0, 2, 0, 2, 0, 2, 0],
    [0, 1, 0, 2, 0, 1, 0, 2],
    [0, 0, 1, 2, 2, 1, 0, 0],
    [0, 1, 0, 2, 0, 1, 0, 2],
    [2, 0, 2, 0, 2, 0, 2, 0],
  ],
};

/** Deterministic wobble in [0, 1). Same inputs always return the same stitch. */
function knitNoise(a: number, b: number): number {
  const value = Math.sin(a * 12.9898 + b * 78.233) * 43758.5453;
  return value - Math.floor(value);
}

function mod(value: number, size: number): number {
  return ((value % size) + size) % size;
}

function round2(value: number): string {
  return (Math.round(value * 100) / 100).toString();
}

/**
 * Brightness of a yarn. `lighten` pushes toward a brighter version of the same hue
 * and never toward white, so a solid patch stays one colour.
 */
function yarnShade(hex: string, shade: number, lighten = 0): string {
  const parsed = Number.parseInt(hex.slice(1), 16);
  const red = ((parsed >> 16) & 255) / 255;
  const green = ((parsed >> 8) & 255) / 255;
  const blue = (parsed & 255) / 255;
  const cap = Math.max(red, green, blue, 0.05);
  const factor = Math.min(shade * (1 + lighten), 1 / cap);
  const channel = (value: number) => Math.min(255, Math.round(value * factor * 255));
  return `#${((channel(red) << 16) | (channel(green) << 8) | channel(blue)).toString(16).padStart(6, "0")}`;
}

/**
 * Two round yarn legs that lean into a V. A soft cast shadow sits under the pair,
 * then each leg is a darker rim, the yarn colour, and a highlight on the upper shoulder.
 */
function stitchLegs(stw: number, sth: number, rim: string, body: string, shine: string): string {
  const gap = stw * 0.22;
  const tilt = 28;
  const leg = (side: number, fillRim: string, fillBody: string, fillShine: string | null) => {
    const shine =
      fillShine === null
        ? ""
        : `<ellipse cx="${round2(side * stw * 0.04)}" cy="${round2(-sth * 0.34)}" rx="${round2(stw * 0.045)}" ry="${round2(sth * 0.13)}" fill="${fillShine}"/>`;
    return `<g transform="translate(${round2(side * gap)} ${round2(sth * 0.02)}) rotate(${side * tilt})">
      <ellipse cx="0" cy="0" rx="${round2(stw * 0.16)}" ry="${round2(sth * 0.58)}" fill="${fillRim}"/>
      <ellipse cx="0" cy="${round2(-sth * 0.02)}" rx="${round2(stw * 0.105)}" ry="${round2(sth * 0.46)}" fill="${fillBody}"/>
      ${shine}
    </g>`;
  };
  const shadow = [-1, 1].map((side) => leg(side, "#1c140e", "#1c140e", null)).join("");
  const yarn = [-1, 1].map((side) => leg(side, rim, body, shine)).join("");
  return `<g transform="translate(${round2(stw * 0.035)} ${round2(sth * 0.1)})" opacity="0.38">${shadow}</g>${yarn}`;
}

function fibreMarkup(tileW: number, tileH: number, color: string): string {
  const count = Math.max(5, Math.min(16, Math.round((tileW * tileH) / 70)));
  const lines: string[] = [];
  for (let index = 0; index < count; index += 1) {
    const x = knitNoise(index, 11) * tileW;
    const y = knitNoise(index, 13) * tileH;
    const direction = knitNoise(index, 17) * Math.PI * 2;
    const length = 1.1 + knitNoise(index, 19) * 2.4;
    lines.push(
      `<line x1="${round2(x)}" y1="${round2(y)}" x2="${round2(x + Math.cos(direction) * length)}" y2="${round2(y + Math.sin(direction) * length)}"/>`,
    );
  }
  return `<g stroke="${color}" stroke-opacity="0.55" stroke-width="0.45" stroke-linecap="round">${lines.join("")}</g>`;
}

/**
 * Builds the SVG frame for one card.
 *
 * `palette` is `[main, contrast, neutral]` from the Rust thumbnail sampler (or a safe fallback).
 * The output is deterministic for the same inputs, so the panel can cache it per card.
 */
export function buildSweaterSvg(
  palette: readonly string[] | null | undefined,
  pattern: SweaterPattern,
  width: SweaterWidth,
  size: { width: number; height: number } = SWEATER_VIEWBOX,
): string {
  const [main, contrast, neutral] = normalizePalette(palette);
  const nominal = SWEATER_THICKNESS[width] ?? SWEATER_THICKNESS.classic;
  const vw = Math.max(Math.round(nominal * 3), Math.round(size.width));
  const vh = Math.max(Math.round(nominal * 3), Math.round(size.height));
  const thickness = Math.min(nominal, Math.max(10, Math.min(vw, vh) * 0.24));
  const radius = Math.min(CARD_RADIUS, Math.min(vw, vh) / 2 - 1);
  const inner = Math.max(2, radius - thickness);
  const outerPath = `M${radius},0 H${vw - radius} A${radius},${radius} 0 0 1 ${vw},${radius} V${vh - radius} A${radius},${radius} 0 0 1 ${vw - radius},${vh} H${radius} A${radius},${radius} 0 0 1 0,${vh - radius} V${radius} A${radius},${radius} 0 0 1 ${radius},0 Z`;
  const innerPath = `M${thickness + inner},${thickness} H${vw - thickness - inner} A${inner},${inner} 0 0 1 ${vw - thickness},${thickness + inner} V${vh - thickness - inner} A${inner},${inner} 0 0 1 ${vw - thickness - inner},${vh - thickness} H${thickness + inner} A${inner},${inner} 0 0 1 ${thickness},${vh - thickness - inner} V${thickness + inner} A${inner},${inner} 0 0 1 ${thickness + inner},${thickness} Z`;

  const wantedRows = SWEATER_ROWS[width] ?? SWEATER_ROWS.classic;
  const courses = thickness < 16 ? Math.min(2, wantedRows) : wantedRows;
  const sth = thickness / courses;
  const stw = sth * 1.32;
  const chart = KNIT_CHARTS[pattern] ?? KNIT_CHARTS.chevron;
  const cols = chart[0].length;
  const rows = chart.length;
  const tileW = cols * stw;
  const tileH = rows * sth;
  const yarns = [neutral, contrast, main];
  const id = `sweater-${stableHash(`${main}${contrast}${neutral}${pattern}${width}${vw}x${vh}`).toString(36)}`;
  const symbols = yarns
    .map((color, index) => {
      const rim = yarnShade(color, 0.68);
      const shine = yarnShade(color, 1, 0.16);
      return `<g id="${id}-y${index}">${stitchLegs(stw, sth, rim, esc(color), shine)}</g>`;
    })
    .join("");

  const stitches: string[] = [];
  for (let row = rows; row >= -1; row -= 1) {
    for (let col = -1; col <= cols; col += 1) {
      const wrappedCol = mod(col, cols);
      const wrappedRow = mod(row, rows);
      const yarn = chart[wrappedRow][wrappedCol];
      const jx = (knitNoise(wrappedCol, wrappedRow) - 0.5) * stw * 0.05;
      const jy = (knitNoise(wrappedCol + 3, wrappedRow + 1) - 0.5) * sth * 0.06;
      const opacity = (0.92 + knitNoise(wrappedCol + 8, wrappedRow + 5) * 0.08).toFixed(2);
      const cx = (col + 0.5) * stw + jx;
      const cy = (row + 0.5) * sth + jy;
      stitches.push(
        `<use href="#${id}-y${yarn}" transform="translate(${round2(cx)} ${round2(cy)})" opacity="${opacity}"/>`,
      );
    }
  }

  const ground = yarnShade(neutral, 0.46);
  const rimLight = yarnShade(neutral, 1, 0.12);

  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${vw} ${vh}" preserveAspectRatio="none" role="presentation" focusable="false">
  <defs>
    ${symbols}
    <pattern id="${id}-knit" patternUnits="userSpaceOnUse" width="${round2(tileW)}" height="${round2(tileH)}">
      <rect width="${round2(tileW)}" height="${round2(tileH)}" fill="${ground}"/>
      ${stitches.join("")}
      ${fibreMarkup(tileW, tileH, rimLight)}
    </pattern>
  </defs>
  <path fill-rule="evenodd" fill="${ground}" d="${outerPath} ${innerPath}"/>
  <path fill-rule="evenodd" fill="url(#${id}-knit)" d="${outerPath} ${innerPath}"/>
  <path fill="none" stroke="#2B2219" stroke-opacity="0.42" stroke-width="1.2" d="${innerPath}"/>
  <path fill="none" stroke="${rimLight}" stroke-opacity="0.75" stroke-width="1.05" d="${outerPath}"/>
  <path fill="none" stroke="#2B2219" stroke-opacity="0.2" stroke-width="0.7" d="${outerPath}"/>
</svg>`;
}

/** A data URL suitable for a CSS `background-image`. */
export function sweaterFrameDataUrl(
  palette: readonly string[] | null | undefined,
  pattern: SweaterPattern,
  width: SweaterWidth,
  size?: { width: number; height: number },
): string {
  return `data:image/svg+xml,${encodeURIComponent(buildSweaterSvg(palette, pattern, width, size))}`;
}
