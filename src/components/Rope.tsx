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
 * v1.6.0 adds seasonal cords. A holiday spec supplies the cord/thread/light colours and a small set
 * of local SVG ornaments; geometry, hit rects and the click-through contract never change because
 * the whole SVG is decorative (`aria-hidden`, `pointer-events: none`).
 */

import { useId } from "react";
import { ROPE_TOP, ropePath } from "../lib/layout";
import { HOLIDAY_ROPE_SPECS, type HolidayOrnament } from "../lib/holidayThemes";
import type { HolidayThemeId, RopeColor, RopeStyle } from "../lib/types";

/** v1.6.0 seasonal rope decorations; null keeps the plain line. */
export type RopeHoliday = HolidayThemeId | null;

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

/** One hanging ornament. Coordinates are absolute; scale/rotation are applied inside. */
function OrnamentShape({ ornament }: { ornament: HolidayOrnament }) {
  const { kind, color } = ornament;
  switch (kind) {
    case "bauble":
      return (
        <g>
          <rect x="-3" y="-10" width="6" height="4" rx="1.5" className="rope-ornament__cap" />
          <circle r="8.2" fill={color} />
          <circle cx="-2.4" cy="-3" r="2.3" fill="#ffffff" fillOpacity="0.35" />
          <path
            d="M-5.4 -6.2 A8.2 8.2 0 0 0 -5.4 6.2"
            fill="none"
            stroke="#ffffff"
            strokeOpacity="0.18"
            strokeWidth="1.4"
          />
        </g>
      );
    case "star":
      return (
        <path
          d="M0 -8 L2.4 -2.6 L8 -2.6 L3.6 1 L5.2 6.4 L0 3.2 L-5.2 6.4 L-3.6 1 L-8 -2.6 L-2.4 -2.6 Z"
          fill={color}
        />
      );
    case "bell":
      return (
        <g>
          <path d="M-7 6 Q-5 -6 0 -7 Q5 -6 7 6 Z" fill={color} />
          <circle cy="7" r="2.2" fill="#7A5A35" />
          <path d="M-2.5 -4 Q0 -6 2.5 -4" fill="none" stroke="#FFF4DC" strokeOpacity="0.5" strokeWidth="1.2" />
        </g>
      );
    case "snow":
      return (
        <g stroke={color} strokeWidth="1.8" strokeLinecap="round">
          <path d="M0 -8 V8 M-7 -4 L7 4 M7 -4 L-7 4" />
          <path d="M0 -8 L-2.6 -5.4 M0 -8 L2.6 -5.4 M0 8 L-2.6 5.4 M0 8 L2.6 5.4" strokeWidth="1.2" />
        </g>
      );
    case "pinecone":
      return (
        <g>
          <ellipse rx="6.4" ry="8.2" fill={color} />
          <path
            d="M-4.4 -4 L0 -1 L4.4 -4 M-4.8 0 L0 3 L4.8 0 M-4 4 L0 7 L4 4"
            fill="none"
            stroke="#5A3822"
            strokeWidth="1.4"
          />
        </g>
      );
    case "heart":
      return (
        <path
          d="M0 7 C-9 0 -9 -7 -3 -7 C-1 -7 0 -5 0 -4 C0 -5 1 -7 3 -7 C9 -7 9 0 0 7 Z"
          fill={color}
        />
      );
    case "lantern":
      return (
        <g>
          <line x1="0" y1="-11" x2="0" y2="-7" stroke="#C9A05C" strokeWidth="1.2" />
          <rect x="-7" y="-7" width="14" height="15" rx="6" fill={color} />
          <path d="M-4 -7 V8 M4 -7 V8" stroke="#F2C14E" strokeOpacity="0.45" strokeWidth="0.9" />
          <rect x="-8" y="-9" width="16" height="3" rx="1.2" fill="#E8C46A" />
          <rect x="-8" y="8" width="16" height="3" rx="1.2" fill="#E8C46A" />
          <line x1="0" y1="11" x2="0" y2="16" stroke="#E8C46A" strokeWidth="1.2" />
        </g>
      );
    case "firework":
      return (
        <g stroke={color} strokeWidth="1.7" strokeLinecap="round">
          <path d="M0 -10 V-4 M0 4 V10 M-10 0 H-4 M4 0 H10" />
          <path d="M-7 -7 L-3 -3 M3 3 L7 7 M7 -7 L3 -3 M-3 3 L-7 7" />
          <circle r="1.8" fill={color} stroke="none" />
        </g>
      );
    case "mooncake":
      return (
        <g>
          <circle r="8" fill={color} />
          <rect x="-4" y="-4" width="8" height="8" rx="1.6" fill="none" stroke="#F4D98B" strokeWidth="1.2" />
          <path d="M0 -8 V-5 M0 5 V8 M-8 0 H-5 M5 0 H8" stroke="#F4D98B" strokeWidth="1" />
        </g>
      );
    case "pumpkin":
      return (
        <g>
          <ellipse rx="8.4" ry="7.2" fill={color} />
          <ellipse rx="4.6" ry="7" fill="#000000" fillOpacity="0.08" />
          <path d="M-2.5 -10 Q0 -13 2.5 -10 L2 -7 H-2 Z" fill="#4F6B3A" />
          <path d="M-7 -2 Q-3 1 0 1 Q3 1 7 -2" fill="none" stroke="#000000" strokeOpacity="0.12" strokeWidth="1" />
        </g>
      );
    case "bat":
      return (
        <path
          d="M0 -2 C-3 -6 -8 -4 -10 0 C-7 1 -5 3 -3 2 C-1 4 1 4 3 2 C5 3 7 1 10 0 C8 -4 3 -6 0 -2 Z"
          fill={color}
        />
      );
    case "flag":
      return (
        <g>
          <line x1="-7" y1="-9" x2="-7" y2="9" stroke="#C9A05C" strokeWidth="1.5" />
          <path d="M-6 -8 H8 V0 H-6 Z" fill={color} />
          <path d="M0 -5 L1 -3 L3.2 -3 L1.5 -1.6 L2.1 1 L0 -0.6 L-2.1 1 L-1.5 -1.6 L-3.2 -3 L-1 -3 Z" fill="#F2C14E" />
        </g>
      );
    case "moon":
      return (
        <g>
          <circle r="12" fill={color} />
          <circle cx="-4" cy="-3" r="2.4" fill="#ffffff" fillOpacity="0.25" />
          <circle cx="4" cy="3" r="3.2" fill="#ffffff" fillOpacity="0.16" />
          <circle cx="2" cy="-6" r="1.4" fill="#ffffff" fillOpacity="0.2" />
        </g>
      );
  }
}

export function Rope({ width, sag, style, color, holiday = null }: RopeProps) {
  const safeWidth = Math.max(1, Math.round(width));
  const safeSag = Number.isFinite(sag) ? Math.max(0, sag) : 0;
  const spec = holiday ? HOLIDAY_ROPE_SPECS[holiday] : null;
  const height = Math.max(1, Math.ceil(ROPE_TOP + safeSag + (spec ? 66 : 8)));
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
        {spec ? (
          <linearGradient id={holidayGradient} x1="0" y1="0" x2="1" y2="0">
            <stop offset="0" stopColor={spec.cord[0]} />
            <stop offset="0.5" stopColor={spec.cord[1]} />
            <stop offset="1" stopColor={spec.cord[0]} />
          </linearGradient>
        ) : null}
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

      {spec ? (
        <g className="rope-holiday" aria-hidden="true">
          {/* Themed cord: a soft glow, the cord gradient and a thin contrasting thread. */}
          <path
            className="rope rope--holiday-glow"
            d={path}
            stroke={`url(#${holidayGradient})`}
            filter={`url(#${holidayGlow})`}
          />
          <path className="rope rope--holiday" d={path} stroke={`url(#${holidayGradient})`} />
          <path className="rope rope--holiday-thread" d={path} stroke={spec.thread} />

          {/* Fairy lights: tiny bulbs strung below the cord, breathing slowly. */}
          {spec.lights.map((t, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, t);
            return (
              <g
                key={`light-${index}`}
                className="rope-light"
                style={{ animationDelay: `${index * 260}ms` }}
                transform={`translate(${round(x)}, ${round(y + 7)})`}
              >
                <line x1="0" y1="-7" x2="0" y2="0" className="rope-light__wire" />
                <circle r="5.2" className="rope-light__glow" style={{ fill: spec.light }} />
                <circle r="1.9" className="rope-light__bulb" style={{ fill: spec.light }} />
              </g>
            );
          })}

          {/* Hanging ornaments: shapes and colours come from the theme spec. */}
          {spec.ornaments.map((ornament, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, ornament.t);
            const cx = round(x);
            const top = round(y);
            const hang = 18 + (index % 3) * 5;
            const cy = top + hang;
            const value = ornament.scale ?? 1;
            return (
              <g
                key={`ornament-${index}`}
                className="rope-ornament"
                style={{ animationDelay: `${index * 170}ms` }}
              >
                <line x1={cx} y1={top} x2={cx} y2={top + hang} className="rope-ornament__string" />
                <g transform={`translate(${cx}, ${cy}) scale(${value})`}>
                  <OrnamentShape ornament={ornament} />
                </g>
              </g>
            );
          })}

          {spec.pineBowEnds ? (
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
          ) : null}
        </g>
      ) : null}
    </svg>
  );
}
