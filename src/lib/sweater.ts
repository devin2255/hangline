/**
 * v1.6.0 Sweater Season: deterministic knitted frames for screenshot cards.
 *
 * Pure SVG generation (no DOM/canvas) so the panel can cache one data URL per card and the logic is
 * testable under Vitest. The frame overlays the card's outer edge; the screenshot is never resized
 * and the card's hit rect / rope geometry stay unchanged.
 */

export type SweaterPattern = "chevron" | "checks" | "snowflake" | "stripe" | "fairisle";

export type SweaterWidth = "slim" | "classic" | "chunky";

/** Default drawing viewBox used by tests; the panel passes the card's real CSS size. */
export const SWEATER_VIEWBOX = { width: 300, height: 200 } as const;

/** Frame thickness in CSS pixels for each setting (the SVG is generated at the card size). */
export const SWEATER_THICKNESS: Record<SweaterWidth, number> = {
  slim: 6,
  classic: 9,
  chunky: 12,
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

function cellPattern(pattern: SweaterPattern, main: string, contrast: string, neutral: string): string {
  switch (pattern) {
    case "checks":
      return `
        <rect width="28" height="24" fill="${neutral}"/>
        <rect x="0" y="0" width="14" height="12" fill="${contrast}"/>
        <rect x="14" y="12" width="14" height="12" fill="${main}"/>
        <rect x="14" y="0" width="14" height="12" fill="${neutral}"/>
        <rect x="0" y="12" width="14" height="12" fill="${neutral}"/>`;
    case "snowflake":
      return `
        <rect width="32" height="32" fill="${neutral}"/>
        <g stroke="${contrast}" stroke-width="2.2" stroke-linecap="round" fill="none">
          <path d="M16 6 V26 M7 11 L25 21 M25 11 L7 21"/>
          <path d="M16 10 L13 13 M16 10 L19 13 M16 22 L13 19 M16 22 L19 19" stroke="${main}"/>
        </g>`;
    case "stripe":
      return `
        <rect width="48" height="24" fill="${neutral}"/>
        <rect y="0" width="48" height="8" fill="${contrast}"/>
        <rect y="8" width="48" height="4" fill="${main}"/>
        <rect y="12" width="48" height="6" fill="${contrast}" opacity="0.65"/>`;
    case "fairisle":
      return `
        <rect width="40" height="28" fill="${neutral}"/>
        <rect y="0" width="40" height="4" fill="${contrast}"/>
        <rect y="24" width="40" height="4" fill="${contrast}"/>
        <path d="M0 14 L10 6 L20 14 L30 6 L40 14" fill="none" stroke="${main}" stroke-width="3"/>
        <circle cx="6" cy="20" r="2.6" fill="${contrast}"/>
        <circle cx="20" cy="20" r="2.6" fill="${contrast}"/>
        <circle cx="34" cy="20" r="2.6" fill="${contrast}"/>`;
    case "chevron":
    default:
      return `
        <rect width="36" height="24" fill="${neutral}"/>
        <path d="M0 16 L9 6 L18 16 L27 6 L36 16" fill="none" stroke="${contrast}" stroke-width="8"/>
        <path d="M0 21 L9 11 L18 21 L27 11 L36 21" fill="none" stroke="${main}" stroke-width="3" opacity="0.75"/>`;
  }
}

/** A tiny repeating stockinette texture (highlight + shadow + rib) laid over the chart. */
const STITCH_TEXTURE = `
  <rect width="16" height="12" fill="none"/>
  <path d="M3 3 L8 9 L13 3" fill="none" stroke="#ffffff" stroke-opacity="0.22" stroke-width="2" stroke-linecap="round"/>
  <path d="M3 5 L8 11 L13 5" fill="none" stroke="#000000" stroke-opacity="0.12" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M8 2 V10" stroke="#000000" stroke-opacity="0.08" stroke-width="1"/>`;

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
  const thickness = SWEATER_THICKNESS[width] ?? SWEATER_THICKNESS.classic;
  const vw = Math.max(thickness * 3, Math.round(size.width));
  const vh = Math.max(thickness * 3, Math.round(size.height));
  const radius = Math.max(thickness + 4, Math.min(18, Math.min(vw, vh) * 0.16));
  const inner = Math.max(2, radius - thickness);
  const outerPath = `M${radius},0 H${vw - radius} A${radius},${radius} 0 0 1 ${vw},${radius} V${vh - radius} A${radius},${radius} 0 0 1 ${vw - radius},${vh} H${radius} A${radius},${radius} 0 0 1 0,${vh - radius} V${radius} A${radius},${radius} 0 0 1 ${radius},0 Z`;
  const innerPath = `M${thickness + inner},${thickness} H${vw - thickness - inner} A${inner},${inner} 0 0 1 ${vw - thickness},${thickness + inner} V${vh - thickness - inner} A${inner},${inner} 0 0 1 ${vw - thickness - inner},${vh - thickness} H${thickness + inner} A${inner},${inner} 0 0 1 ${thickness},${vh - thickness - inner} V${thickness + inner} A${inner},${inner} 0 0 1 ${thickness + inner},${thickness} Z`;
  const patternMarkup = cellPattern(pattern, main, contrast, neutral);
  const id = `sweater-${stableHash(`${main}${contrast}${neutral}${pattern}${width}`).toString(36)}`;

  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${vw} ${vh}" preserveAspectRatio="none" role="presentation" focusable="false">
  <defs>
    <pattern id="${id}-cells" patternUnits="userSpaceOnUse" width="36" height="32">${patternMarkup}</pattern>
    <pattern id="${id}-stitch" patternUnits="userSpaceOnUse" width="16" height="12">${STITCH_TEXTURE}</pattern>
  </defs>
  <path fill-rule="evenodd" fill="${neutral}" fill-opacity="0.98" d="${outerPath} ${innerPath}"/>
  <path fill-rule="evenodd" fill="url(#${id}-cells)" d="${outerPath} ${innerPath}"/>
  <path fill-rule="evenodd" fill="url(#${id}-stitch)" d="${outerPath} ${innerPath}"/>
  <path fill-rule="evenodd" fill="none" stroke="#2B2219" stroke-opacity="0.35" stroke-width="1.2" d="${outerPath} ${innerPath}"/>
  <path fill="none" stroke="${esc(neutral)}" stroke-opacity="0.55" stroke-width="1.4" d="${innerPath}"/>
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
