/**
 * The clothesline rope (design spec §3.3, v1.4.0 "quiet luxury" appearance system).
 *
 * A quadratic Bezier whose sag is owned by the panel (`lib/layout.ts` `ropeSag`/`ropePath`): the
 * rope starts almost straight on an empty line and bends further with every visible card, and the
 * panel animates the sag over 220 ms when the count changes.
 *
 * Every layer of every material is always in the SVG and each one keeps a **stable** gradient id
 * (`useId`); switching `style` only flips CSS opacity/stroke rules, so the four materials cross-fade
 * over 180 ms instead of hard-cutting. Colour and theme adaptation live in `Panel.css`:
 * the panel sets `--rope-palette` from the selected palette and the CSS derives the base/sheen/gold
 * tokens from it with `color-mix` (dark theme mixes toward a warm tint, light theme toward a darker
 * one, so `ink` stays visible on either).
 *
 * The SVG is decorative only: `aria-hidden`, `pointer-events: none` (Panel.css), and the card owns
 * the hit rect. The CSS mask fades both rope ends into the panel edge.
 */

import { useId } from "react";
import { ROPE_TOP, ropePath } from "../lib/layout";
import type { RopeColor, RopeStyle } from "../lib/types";

/** v1.6.0 seasonal rope decorations; null keeps the plain line. */
export type RopeHoliday = "christmas" | null;

/** Ornament anchors along the normalised rope (0..1); cards render above and hide some of them. */
const CHRISTMAS_ORNAMENTS: ReadonlyArray<{
  t: number;
  kind: "bauble" | "bell" | "star" | "snow" | "pinecone";
  color: string;
}> = [
  { t: 0.055, kind: "bauble", color: "#A34B4E" },
  { t: 0.16, kind: "star", color: "#D9B45C" },
  { t: 0.33, kind: "bell", color: "#C9A05C" },
  { t: 0.44, kind: "snow", color: "#E8EEE7" },
  { t: 0.58, kind: "bauble", color: "#3E6B4F" },
  { t: 0.70, kind: "pinecone", color: "#8B5E3C" },
  { t: 0.85, kind: "star", color: "#D9B45C" },
  { t: 0.945, kind: "bauble", color: "#A34B4E" },
];

const CHRISTMAS_LIGHTS = [0.1, 0.24, 0.38, 0.52, 0.66, 0.8, 0.93] as const;

/** Rope Y at normalised t for the quadratic used by `ropePath` (control = top + 2×sag). */
function holidayRopePoint(width: number, sag: number, t: number): { x: number; y: number } {
  return { x: width * t, y: ROPE_TOP + 4 * sag * t * (1 - t) };
}

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
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

export function Rope({ width, sag, style, color, holiday = null }: RopeProps) {
  const safeWidth = Math.max(1, Math.round(width));
  const safeSag = Number.isFinite(sag) ? Math.max(0, sag) : 0;
  const height = Math.max(1, Math.ceil(ROPE_TOP + safeSag + (holiday ? 52 : 8)));
  const path = ropePath(safeWidth, safeSag);

  // React ids contain ":" characters, which are awkward inside SVG url(#...) references.
  const uid = useId().replace(/:/g, "");
  const silkGradient = `rope-silk-${uid}`;
  const brassGradient = `rope-brass-${uid}`;
  const auroraGradient = `rope-aurora-${uid}`;
  const auroraGlow = `rope-aurora-glow-${uid}`;
  const holidayGradient = `rope-holiday-${uid}`;
  const holidayGlow = `rope-holiday-glow-${uid}`;

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
        {/* Silk: along-the-rope horizontal gradient (base → +18% sheen → base). */}
        <linearGradient id={silkGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-stop rope-stop--a" />
          <stop offset="0.5" className="rope-stop rope-stop--b" />
          <stop offset="1" className="rope-stop rope-stop--a" />
        </linearGradient>
        {/* Brass: warm gold gradient (most pronounced on the champagne palette via CSS). */}
        <linearGradient id={brassGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-stop rope-stop--brass-a" />
          <stop offset="0.45" className="rope-stop rope-stop--brass-b" />
          <stop offset="1" className="rope-stop rope-stop--brass-a" />
        </linearGradient>
        {/* Aurora: base → bright core (+25%). */}
        <linearGradient id={auroraGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-stop rope-stop--aurora-a" />
          <stop offset="1" className="rope-stop rope-stop--aurora-b" />
        </linearGradient>
        <filter id={auroraGlow} x="-10%" y="-60%" width="120%" height="220%">
          <feGaussianBlur stdDeviation="1.6" />
        </filter>
        <linearGradient id={holidayGradient} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" className="rope-holiday-stop rope-holiday-stop--a" />
          <stop offset="0.5" className="rope-holiday-stop rope-holiday-stop--b" />
          <stop offset="1" className="rope-holiday-stop rope-holiday-stop--a" />
        </linearGradient>
        <filter id={holidayGlow} x="-40%" y="-80%" width="180%" height="260%">
          <feGaussianBlur stdDeviation="2.4" />
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

      {holiday === "christmas" ? (
        <g className="rope-holiday" aria-hidden="true">
          {/* Themed cord: deep pine with a warm gold thread and a soft glow. */}
          <path className="rope rope--holiday-glow" d={path} stroke={`url(#${holidayGradient})`} filter={`url(#${holidayGlow})`} />
          <path className="rope rope--holiday" d={path} stroke={`url(#${holidayGradient})`} />
          <path className="rope rope--holiday-thread" d={path} />

          {/* Warm fairy lights: tiny bulbs strung below the cord, breathing slowly. */}
          {CHRISTMAS_LIGHTS.map((t, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, t);
            return (
              <g
                key={`light-${index}`}
                className="rope-light"
                style={{ animationDelay: `${index * 260}ms` }}
                transform={`translate(${round(x)}, ${round(y + 7)})`}
              >
                <line x1="0" y1="-7" x2="0" y2="0" className="rope-light__wire" />
                <circle r="5.2" className="rope-light__glow" />
                <circle r="1.9" className="rope-light__bulb" />
              </g>
            );
          })}

          {/* Hanging ornaments: a hand-picked mix of baubles, stars, bells, snowflakes, pinecones. */}
          {CHRISTMAS_ORNAMENTS.map((ornament, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, ornament.t);
            const cx = round(x);
            const top = round(y);
            const hang = 20 + (index % 3) * 5;
            const cy = top + hang + 8;
            return (
              <g
                key={`ornament-${index}`}
                className="rope-ornament"
                style={{ animationDelay: `${index * 170}ms` }}
              >
                <line x1={cx} y1={top} x2={cx} y2={top + hang} className="rope-ornament__string" />
                {ornament.kind === "bauble" ? (
                  <g>
                    <rect x={cx - 3} y={top + hang - 2} width="6" height="4" rx="1.5" className="rope-ornament__cap" />
                    <circle cx={cx} cy={cy} r="8.2" fill={ornament.color} />
                    <circle cx={cx - 2.4} cy={cy - 3} r="2.3" fill="#ffffff" fillOpacity="0.35" />
                    <path
                      d={`M${cx - 5.4} ${cy - 6.2} A8.2 8.2 0 0 0 ${cx - 5.4} ${cy + 6.2}`}
                      fill="none"
                      stroke="#ffffff"
                      strokeOpacity="0.18"
                      strokeWidth="1.4"
                    />
                  </g>
                ) : null}
                {ornament.kind === "star" ? (
                  <g transform={`translate(${cx}, ${cy})`}>
                    <path
                      d="M0 -8 L2.4 -2.6 L8 -2.6 L3.6 1 L5.2 6.4 L0 3.2 L-5.2 6.4 L-3.6 1 L-8 -2.6 L-2.4 -2.6 Z"
                      fill={ornament.color}
                    />
                  </g>
                ) : null}
                {ornament.kind === "bell" ? (
                  <g transform={`translate(${cx}, ${cy})`}>
                    <path d="M-7 6 Q-5 -6 0 -7 Q5 -6 7 6 Z" fill={ornament.color} />
                    <circle cx="0" cy="7" r="2.2" fill="#7A5A35" />
                    <path d="M-2.5 -4 Q0 -6 2.5 -4" fill="none" stroke="#FFF4DC" strokeOpacity="0.5" strokeWidth="1.2" />
                  </g>
                ) : null}
                {ornament.kind === "snow" ? (
                  <g transform={`translate(${cx}, ${cy})`} stroke={ornament.color} strokeWidth="1.8" strokeLinecap="round">
                    <path d="M0 -8 V8 M-7 -4 L7 4 M7 -4 L-7 4" />
                    <path d="M0 -8 L-2.6 -5.4 M0 -8 L2.6 -5.4 M0 8 L-2.6 5.4 M0 8 L2.6 5.4" strokeWidth="1.2" />
                  </g>
                ) : null}
                {ornament.kind === "pinecone" ? (
                  <g transform={`translate(${cx}, ${cy})`}>
                    <ellipse cx="0" cy="0" rx="6.4" ry="8.2" fill={ornament.color} />
                    <path d="M-4.4 -4 L0 -1 L4.4 -4 M-4.8 0 L0 3 L4.8 0 M-4 4 L0 7 L4 4" fill="none" stroke="#5A3822" strokeWidth="1.4" />
                  </g>
                ) : null}
              </g>
            );
          })}

          {/* Pine sprigs and ribbons at the two ends. */}
          <g className="rope-holiday__ends" aria-hidden="true">
            <g transform={`translate(18, ${round(holidayRopePoint(safeWidth, safeSag, 0.02).y)})`}>
              <path d="M0 0 L26 -5 M0 0 L28 5 M0 0 L22 -13 M0 0 L24 13 M6 -1 L18 -9 M6 1 L18 9" className="rope-pine" />
              <path d="M2 -2 Q-8 -8 -12 -1 Q-5 5 2 2 M2 2 Q-8 8 -12 1 Q-5 -5 2 -2" className="rope-bow" />
            </g>
            <g transform={`translate(${round(safeWidth - 18)}, ${round(holidayRopePoint(safeWidth, safeSag, 0.98).y)})`}>
              <path d="M0 0 L-26 -5 M0 0 L-28 5 M0 0 L-22 -13 M0 0 L-24 13 M-6 -1 L-18 -9 M-6 1 L-18 9" className="rope-pine" />
              <path d="M-2 -2 Q8 -8 12 -1 Q5 5 -2 2 M-2 2 Q8 8 12 1 Q5 -5 -2 -2" className="rope-bow" />
            </g>
          </g>
        </g>
      ) : null}
    </svg>
  );
}
