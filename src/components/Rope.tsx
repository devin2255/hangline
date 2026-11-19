/**
 * The clothesline rope (design spec §3.3, v1.4.0 "quiet luxury" appearance system).
 *
 * A quadratic Bezier whose sag is owned by the panel (`lib/layout.ts` `ropeSag`/`ropePath`): the
 * rope starts almost straight on an empty line and bends further with every visible card, and the
 * panel animates the sag over 220 ms when the count changes.
 *
 * Every layer of every material is always in the SVG and each one keeps a **stable** gradient id
 * (`useId`); switching `style` only flips CSS opacity/stroke rules, so the four materials cross-fade
 * over 180 ms instead of hard-cutting. Colour and theme adaptation live in `Panel.css`.
 *
 * v1.6.2 turns the seasonal layer into a proper garland: layered cord, woven thread, warm light
 * points, cord-woven motifs (pine, blossom, petals, flags, ...), hanging ornaments and hand-tied
 * ribbon ends. The whole SVG stays decorative (`aria-hidden`, `pointer-events: none`), so geometry,
 * hit rects and the click-through contract are untouched.
 */

import { ChristmasGarland } from "./ChristmasGarland";
import { useId } from "react";
import { ROPE_TOP, ropePath } from "../lib/layout";
import { FestivalGarland } from "./FestivalGarland";
import type { HolidayThemeId, RopeColor, RopeStyle } from "../lib/types";

/** v1.6.2 seasonal rope decorations; null keeps the plain line. */
export type RopeHoliday = HolidayThemeId | null;

export interface RopeProps {
  /** Line width in dp (CSS px); the rope spans the full line content. */
  width: number;
  /** Current sag in dp (animated by the panel; 0 = straight). */
  sag: number;
  /** Current material. */
  style: RopeStyle;
  /** Current low-saturation palette key. */
  color: RopeColor;
  /** v1.6.0 seasonal theme; renders decorations and a themed cord without changing geometry. */
  holiday?: RopeHoliday;
  landing?: number;
}

export function Rope({ width, sag, style, color, holiday = null, landing = 0 }: RopeProps) {
  const safeWidth = Math.max(1, Math.round(width));
  const safeSag = Number.isFinite(sag) ? Math.max(0, sag) : 0;
  const height = Math.max(1, Math.ceil(ROPE_TOP + safeSag + (holiday ? 84 : 8)));
  const path = ropePath(safeWidth, safeSag);

  // React ids contain ":" characters, which are awkward inside SVG url(#...) references.
  const uid = useId().replace(/:/g, "");
  const silkGradient = `rope-silk-${uid}`;
  const brassGradient = `rope-brass-${uid}`;
  const auroraGradient = `rope-aurora-${uid}`;
  const auroraGlow = `rope-aurora-glow-${uid}`;

  return (
    <svg
      className="panel__rope"
      data-rope-style={style}
      data-rope-color={color}
      data-rope-holiday={holiday ?? undefined}
      width={safeWidth}
      height={height}
      viewBox={`0 0 ${safeWidth} ${height}`}
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        <linearGradient id={silkGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-stop rope-stop--a" />
          <stop offset="0.5" className="rope-stop rope-stop--b" />
          <stop offset="1" className="rope-stop rope-stop--a" />
        </linearGradient>
        <linearGradient id={brassGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-stop rope-stop--brass-a" />
          <stop offset="0.45" className="rope-stop rope-stop--brass-b" />
          <stop offset="1" className="rope-stop rope-stop--brass-a" />
        </linearGradient>
        <linearGradient id={auroraGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-stop rope-stop--aurora-a" />
          <stop offset="1" className="rope-stop rope-stop--aurora-b" />
        </linearGradient>
        <filter id={auroraGlow} x="-10%" y="-60%" width="120%" height="220%">
          <feGaussianBlur stdDeviation="1.6" />
        </filter>
      </defs>

      {/* Shared depth layer: every material sits on the same soft shadow. */}
      <path className="rope rope--shadow" d={path} />
      {/* Material layers: always mounted, cross-faded by CSS (`opacity` 180 ms). */}
      <path className="rope rope--silk" d={path} stroke={`url(#${silkGradient})`} />
      <path className="rope rope--linen" d={path} />
      <path className="rope rope--linen-texture" d={path} />
      <path className="rope rope--brass" d={path} stroke={`url(#${brassGradient})`} />
      <path
        className="rope rope--aurora-glow"
        d={path}
        stroke={`url(#${auroraGradient})`}
        filter={`url(#${auroraGlow})`}
      />
      <path className="rope rope--aurora-core" d={path} stroke={`url(#${auroraGradient})`} />
      {/* Shared top-light: the single light source above the rope. */}
      <path className="rope rope--highlight" d={path} />

      {holiday === "christmas" && (
        <ChristmasGarland width={safeWidth} sag={safeSag} landing={landing} />
      )}
      {holiday && holiday !== "christmas" && (
        <FestivalGarland holiday={holiday} width={safeWidth} sag={safeSag} landing={landing} />
      )}
    </svg>
  );
}
