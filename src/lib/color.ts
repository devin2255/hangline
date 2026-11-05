/**
 * WCAG colour helpers for the P2-8 contrast checks (pure; used by unit tests and the evidence
 * report). Kept dependency-free and DOM-free.
 */

interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Parses `#rgb`, `#rrggbb`, `rgb(...)`, `rgba(...)`, space/slash or comma syntax, `%` alpha; throws on unsupported input. */
export function parseColor(value: string): Rgba {
  const text = value.trim();
  if (text.startsWith("#")) {
    const hex = text.slice(1);
    if (!/^[0-9a-fA-F]+$/.test(hex)) throw new Error(`unsupported hex colour: ${value}`);
    if (hex.length === 3) {
      return {
        r: parseInt(hex[0] + hex[0], 16),
        g: parseInt(hex[1] + hex[1], 16),
        b: parseInt(hex[2] + hex[2], 16),
        a: 1,
      };
    }
    if (hex.length === 6) {
      return {
        r: parseInt(hex.slice(0, 2), 16),
        g: parseInt(hex.slice(2, 4), 16),
        b: parseInt(hex.slice(4, 6), 16),
        a: 1,
      };
    }
    throw new Error(`unsupported hex colour: ${value}`);
  }
  const match = /^rgba?\((.*)\)$/i.exec(text);
  if (!match) throw new Error(`unsupported colour: ${value}`);
  const raw = match[1]
    .split(/[\s,/]+/)
    .filter(Boolean);
  if (raw.length < 3) throw new Error(`unsupported colour: ${value}`);
  const channels = raw.slice(0, 3).map((part) => {
    const numeric = part.endsWith("%") ? (Number(part.slice(0, -1)) / 100) * 255 : Number(part);
    if (Number.isNaN(numeric)) throw new Error(`unsupported colour: ${value}`);
    return numeric;
  });
  let alpha = 1;
  if (raw.length >= 4) {
    const part = raw[3];
    const numeric = part.endsWith("%") ? Number(part.slice(0, -1)) / 100 : Number(part);
    if (Number.isNaN(numeric)) throw new Error(`unsupported colour: ${value}`);
    alpha = clamp(numeric, 0, 1);
  }
  return {
    r: clamp(Math.round(channels[0]), 0, 255),
    g: clamp(Math.round(channels[1]), 0, 255),
    b: clamp(Math.round(channels[2]), 0, 255),
    a: alpha,
  };
}

function toHex({ r, g, b }: Rgba): string {
  const part = (channel: number) => channel.toString(16).padStart(2, "0");
  return `#${part(clamp(Math.round(r), 0, 255))}${part(clamp(Math.round(g), 0, 255))}${part(clamp(Math.round(b), 0, 255))}`;
}

/** Composites a (possibly translucent) `fg` over an opaque `bg`, returning an opaque hex. */
export function composite(fg: string, bg: string): string {
  const foreground = parseColor(fg);
  const background = parseColor(bg);
  if (background.a !== 1) {
    throw new Error("composite: the background must be opaque");
  }
  const mix = (front: number, back: number) => front * foreground.a + back * (1 - foreground.a);
  return toHex({
    r: mix(foreground.r, background.r),
    g: mix(foreground.g, background.g),
    b: mix(foreground.b, background.b),
    a: 1,
  });
}

/** WCAG relative luminance (0 = black, 1 = white) of an opaque hex colour. */
export function relativeLuminance(hex: string): number {
  const { r, g, b } = parseColor(hex);
  if (parseColor(hex).a !== 1) throw new Error("relativeLuminance: expected an opaque colour");
  const channel = (value: number) => {
    const srgb = value / 255;
    return srgb <= 0.03928 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG contrast ratio between two opaque colours (≥4.5 is the AA text target). */
export function contrastRatio(a: string, b: string): number {
  const first = relativeLuminance(a);
  const second = relativeLuminance(b);
  const lighter = Math.max(first, second);
  const darker = Math.min(first, second);
  return (lighter + 0.05) / (darker + 0.05);
}

/**
 * Contrast of `text` (hex/rgb) over a translucent token `token` composited onto `under`.
 *
 * `under` must be opaque; the panel tokens are always layered over either the card material or
 * the desktop background, both of which the report states explicitly.
 */
export function contrastOver(text: string, token: string, under: string): number {
  const background = composite(token, under);
  const foreground = parseColor(text).a < 1 ? composite(text, background) : text;
  return contrastRatio(foreground, background);
}
