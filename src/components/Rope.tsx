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

import { useId } from "react";
import { ROPE_TOP, ropePath } from "../lib/layout";
import {
  HOLIDAY_ROPE_SPECS,
  type HolidayCordMotif,
  type HolidayOrnament,
} from "../lib/holidayThemes";
import type { HolidayThemeId, RopeColor, RopeStyle } from "../lib/types";

/** v1.6.2 seasonal rope decorations; null keeps the plain line. */
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

/** Mix a #rrggbb colour toward white (positive) or black (negative); keeps ornament depth cheap. */
function shade(hex: string, percent: number): string {
  const value = hex.replace("#", "");
  if (value.length !== 6) return hex;
  const channels = [0, 2, 4].map((offset) => parseInt(value.slice(offset, offset + 2), 16));
  const target = percent >= 0 ? 255 : 0;
  const ratio = Math.min(1, Math.abs(percent) / 100);
  return `#${channels
    .map((channel) => Math.round(channel + (target - channel) * ratio).toString(16).padStart(2, "0"))
    .join("")}`;
}

function StarPath({ color, size = 8 }: { color: string; size?: number }) {
  const s = size / 8;
  return (
    <path
      transform={`scale(${s})`}
      d="M0 -8 L2.4 -2.6 L8 -2.6 L3.6 1 L5.2 6.4 L0 3.2 L-5.2 6.4 L-3.6 1 L-8 -2.6 L-2.4 -2.6 Z"
      fill={color}
    />
  );
}

/** One hanging ornament. Coordinates are absolute; scale/rotation are applied by the caller. */
function OrnamentShape({ ornament, fillId }: { ornament: HolidayOrnament; fillId: string }) {
  const { kind, color } = ornament;
  switch (kind) {
    case "bauble":
      return (
        <g>
          <circle r="10.4" fill={fillId} />
          <circle r="10.4" fill="none" stroke="#000000" strokeOpacity="0.13" strokeWidth="0.8" />
          <circle cx="-3.1" cy="-3.8" r="3.1" fill="#ffffff" fillOpacity="0.34" />
          <path
            d="M-7.2 -7.4 A10.4 10.4 0 0 0 -7.2 7.4"
            fill="none"
            stroke="#ffffff"
            strokeOpacity="0.2"
            strokeWidth="1.5"
          />
          <rect x="-3.2" y="-13.2" width="6.4" height="4" rx="1.6" className="rope-ornament__cap" />
        </g>
      );
    case "star":
      return <StarPath color={color} size={11} />;
    case "bell":
      return (
        <g>
          <path d="M-8 6 Q-6 -7 0 -8 Q6 -7 8 6 Z" fill={fillId} />
          <path d="M-8 6 Q0 9 8 6" fill="none" stroke="#000000" strokeOpacity="0.16" strokeWidth="1" />
          <circle cy="8.2" r="2.4" fill="#7A5A35" />
          <path d="M-2.8 -4.8 Q0 -7 2.8 -4.8" fill="none" stroke="#FFF4DC" strokeOpacity="0.55" strokeWidth="1.3" />
        </g>
      );
    case "snow":
      return (
        <g stroke={color} strokeWidth="2" strokeLinecap="round">
          <path d="M0 -10 V10 M-8.6 -5 L8.6 5 M8.6 -5 L-8.6 5" />
          <path d="M0 -10 L-3 -6.6 M0 -10 L3 -6.6 M0 10 L-3 6.6 M0 10 L3 6.6" strokeWidth="1.3" />
          <path d="M-8.6 -5 L-4.6 -5.6 M8.6 5 L4.6 5.6 M8.6 -5 L4.6 -5.6 M-8.6 5 L-4.6 5.6" strokeWidth="1.1" />
        </g>
      );
    case "pinecone":
      return (
        <g>
          <ellipse rx="7.2" ry="9.6" fill={color} />
          <ellipse rx="4.4" ry="8.2" fill="#000000" fillOpacity="0.08" />
          <path
            d="M-4.8 -4.4 L0 -1.2 L4.8 -4.4 M-5.4 0 L0 3.4 L5.4 0 M-4.8 4.6 L0 7.6 L4.8 4.6"
            fill="none"
            stroke="#5A3822"
            strokeWidth="1.4"
          />
        </g>
      );
    case "heart":
      return (
        <path
          d="M0 8 C-10.5 0 -10.5 -8 -3.5 -8 C-1.2 -8 0 -6 0 -4.6 C0 -6 1.2 -8 3.5 -8 C10.5 -8 10.5 0 0 8 Z"
          fill={fillId}
        />
      );
    case "lantern":
      return (
        <g>
          <path d="M-2.2 -14 Q0 -17 2.2 -14" fill="none" stroke="#C9A05C" strokeWidth="1.2" />
          <path
            d="M-7.2 -8.4 C-12.8 -4.4 -12.8 7.2 -7.2 11.4 C-3 14.6 3 14.6 7.2 11.4 C12.8 7.2 12.8 -4.4 7.2 -8.4 C3 -11.6 -3 -11.6 -7.2 -8.4 Z"
            fill={fillId}
          />
          <path
            d="M-4.6 -9.2 C-6.4 -4.4 -6.4 7.6 -4.6 12.2 M0 -10.4 V13.4 M4.6 -9.2 C6.4 -4.4 6.4 7.6 4.6 12.2"
            fill="none"
            stroke="#F2C14E"
            strokeOpacity="0.38"
            strokeWidth="0.9"
          />
          <path d="M-5.4 -9.2 H5.4" stroke="#E8C46A" strokeWidth="2.4" strokeLinecap="round" />
          <path d="M-5.4 12.1 H5.4" stroke="#E8C46A" strokeWidth="2.4" strokeLinecap="round" />
          <line x1="0" y1="14" x2="0" y2="20" stroke="#E8C46A" strokeWidth="1.1" />
          <line x1="-2.4" y1="20" x2="2.4" y2="20" stroke="#E8C46A" strokeWidth="1.1" strokeLinecap="round" />
        </g>
      );
    case "firework":
      return (
        <g stroke={color} strokeWidth="1.8" strokeLinecap="round">
          <circle r="2.1" fill={color} stroke="none" />
          <path d="M0 -13 V-5 M0 5 V13 M-13 0 H-5 M5 0 H13" />
          <path d="M-9 -9 L-4 -4 M4 4 L9 9 M9 -9 L4 -4 M-4 4 L-9 9" />
          <path d="M-4.6 -11.2 L-2.2 -7.2 M4.6 11.2 L2.2 7.2 M11.2 -4.6 L7.2 -2.2 M-11.2 4.6 L-7.2 2.2" strokeWidth="1.1" />
        </g>
      );
    case "mooncake":
      return (
        <g>
          <circle r="9.2" fill={fillId} />
          <circle r="9.2" fill="none" stroke="#000000" strokeOpacity="0.12" strokeWidth="0.8" />
          <rect x="-4.6" y="-4.6" width="9.2" height="9.2" rx="2" fill="none" stroke="#F4D98B" strokeWidth="1.3" />
          <path d="M0 -9.2 V-6 M0 6 V9.2 M-9.2 0 H-6 M6 0 H9.2" stroke="#F4D98B" strokeWidth="1.1" />
        </g>
      );
    case "pumpkin":
      return (
        <g>
          <ellipse rx="9.6" ry="8.2" fill={fillId} />
          <ellipse rx="5.4" ry="8" fill="#000000" fillOpacity="0.09" />
          <ellipse rx="2.2" ry="7" fill="#ffffff" fillOpacity="0.08" />
          <path d="M-2.8 -11 Q0 -15 2.8 -11 L2.2 -8 H-2.2 Z" fill="#4F6B3A" />
          <path d="M-8 -2.6 Q-4 0.6 0 0.6 Q4 0.6 8 -2.6" fill="none" stroke="#000000" strokeOpacity="0.14" strokeWidth="1.1" />
        </g>
      );
    case "bat":
      return (
        <path
          d="M0 -2.6 C-3.6 -7.4 -9.6 -5 -12 0 C-8.4 1.2 -6 3.6 -3.6 2.4 C-1.2 4.8 1.2 4.8 3.6 2.4 C6 3.6 8.4 1.2 12 0 C9.6 -5 3.6 -7.4 0 -2.6 Z"
          fill={color}
        />
      );
    case "flag":
      return (
        <g>
          <line x1="-8" y1="-11" x2="-8" y2="10" stroke="#C9A05C" strokeWidth="1.6" />
          <path d="M-6.8 -9.6 H9 V-0.4 H-6.8 Z" fill={fillId} />
          <path d="M0 -6 L1.2 -3.6 L3.8 -3.6 L1.8 -1.8 L2.5 0.8 L0 -0.8 L-2.5 0.8 L-1.8 -1.8 L-3.8 -3.6 L-1.2 -3.6 Z" fill="#F2C14E" />
        </g>
      );
    case "moon":
      return (
        <g>
          <circle r="13.6" fill={color} fillOpacity="0.18" />
          <circle r="12.2" fill={fillId} />
          <circle cx="-4.2" cy="-3.2" r="2.6" fill="#ffffff" fillOpacity="0.28" />
          <circle cx="4.4" cy="3.2" r="3.4" fill="#ffffff" fillOpacity="0.18" />
          <circle cx="2.2" cy="-6.4" r="1.5" fill="#ffffff" fillOpacity="0.22" />
        </g>
      );
  }
}

function MotifShape({ motif }: { motif: HolidayCordMotif }) {
  const { kind, color } = motif;
  switch (kind) {
    case "pine":
      return (
        <g fill="none" stroke={color} strokeWidth="1.35" strokeLinecap="round">
          <path d="M0 0 C2 -3 5 -5 9 -5 M0 0 C-2 -3 -5 -5 -9 -5 M1 -1 C3 -2 5 -3 7 -2 M-1 -1 C-3 -2 -5 -3 -7 -2" />
          <path d="M9 -5 l3 -2 M9 -5 l2.6 2 M-9 -5 l-3 -2 M-9 -5 l-2.6 2" strokeWidth="1.1" />
        </g>
      );
    case "blossom":
      return (
        <g>
          {[0, 72, 144, 216, 288].map((angle) => (
            <ellipse
              key={angle}
              rx="2.5"
              ry="1.7"
              fill={color}
              transform={`rotate(${angle}) translate(0 -3)`}
            />
          ))}
          <circle r="1.5" fill="#E8C46A" />
        </g>
      );
    case "leaf":
      return <path d="M0 0 C6 -5 10 -2 9 4 C4 6 -1 4 0 0 Z" fill={color} fillOpacity="0.9" />;
    case "sparkle":
      return (
        <path
          d="M0 -8 L1.4 -1.4 L8 0 L1.4 1.4 L0 8 L-1.4 1.4 L-8 0 L-1.4 -1.4 Z"
          fill={color}
        />
      );
    case "star":
      return <StarPath color={color} size={6.4} />;
    case "petal":
      return <ellipse rx="2.8" ry="1.3" fill={color} fillOpacity="0.85" transform="rotate(-24)" />;
    case "bat":
      return (
        <path
          d="M0 -1.6 C-2.2 -4.6 -5.8 -3 -7 0 C-4.8 0.6 -3.4 2 -2 1.4 C-0.6 2.8 0.6 2.8 2 1.4 C3.4 2 4.8 0.6 7 0 C5.8 -3 2.2 -4.6 0 -1.6 Z"
          fill={color}
        />
      );
    case "pennant":
      return (
        <g>
          <line x1="0" y1="0" x2="0" y2="4" stroke={color} strokeWidth="1" />
          <path d="M0 4 L7 7 L0 10 Z" fill={color} fillOpacity="0.92" />
        </g>
      );
    case "snow":
      return (
        <g stroke={color} strokeWidth="1.1" strokeLinecap="round" opacity="0.85">
          <path d="M0 -5 V5 M-4.4 -2.5 L4.4 2.5 M4.4 -2.5 L-4.4 2.5" />
        </g>
      );
    case "knot":
      return (
        <g fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round">
          <path d="M0 -4 L5 0 L0 4 L-5 0 Z" />
          <circle r="1.3" fill={color} stroke="none" />
        </g>
      );
    case "heart":
      return (
        <path d="M0 4 C-5.6 0 -5.6 -4.2 -2 -4.2 C-0.7 -4.2 0 -3.2 0 -2.5 C0 -3.2 0.7 -4.2 2 -4.2 C5.6 -4.2 5.6 0 0 4 Z" fill={color} />
      );
  }
}

/** Small brass cord end with two fine silk tails; the right-hand one is mirrored. */
function RibbonBow({
  x,
  y,
  color,
  accent,
  flip = false,
  scale = 0.95,
}: {
  x: number;
  y: number;
  color: string;
  accent: string;
  flip?: boolean;
  scale?: number;
}) {
  return (
    <g
      className="rope-end-bow"
      transform={`translate(${round(x)}, ${round(y)}) scale(${flip ? -scale : scale}, ${scale})`}
    >
      <rect x="-3.4" y="-2.6" width="6.8" height="5" rx="1.8" fill="#C8A96A" />
      <rect x="-3.4" y="-0.5" width="6.8" height="1" fill="#8A6B3A" opacity="0.85" />
      <path d="M-1.2 2.2 C-3.2 8 -4.4 15 -2.8 21 L0 18.2 L1 2.2 Z" fill={color} opacity="0.82" />
      <path d="M1.2 2.2 C3.2 8 4.4 15 2.8 21 L0 18.2 L-1 2.2 Z" fill={color} opacity="0.66" />
      <path d="M-1.6 2.2 C-3 8 -3.8 14 -3 19 M1.6 2.2 C3 8 3.8 14 3 19" fill="none" stroke={accent} strokeOpacity="0.35" strokeWidth="0.45" />
      <circle r="1.4" fill={accent} />
    </g>
  );
}

export function Rope({ width, sag, style, color, holiday = null }: RopeProps) {
  const safeWidth = Math.max(1, Math.round(width));
  const safeSag = Number.isFinite(sag) ? Math.max(0, sag) : 0;
  const spec = holiday ? HOLIDAY_ROPE_SPECS[holiday] : null;
  const height = Math.max(1, Math.ceil(ROPE_TOP + safeSag + (spec ? 84 : 8)));
  const path = ropePath(safeWidth, safeSag);

  // React ids contain ":" characters, which are awkward inside SVG url(#...) references.
  const uid = useId().replace(/:/g, "");
  const silkGradient = `rope-silk-${uid}`;
  const brassGradient = `rope-brass-${uid}`;
  const auroraGradient = `rope-aurora-${uid}`;
  const auroraGlow = `rope-aurora-glow-${uid}`;
  const holidayGradient = `rope-holiday-${uid}`;
  const holidayGlow = `rope-holiday-glow-${uid}`;
  const holidayBokeh = `rope-holiday-bokeh-${uid}`;
  const holidayMetal = `rope-metal-${uid}`;
  const metalFill = `url(#${holidayMetal})`;

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
          <>
            <linearGradient id={holidayGradient} x1="0" y1="0" x2="1" y2="0">
              <stop offset="0" stopColor={spec.cord[0]} />
              <stop offset="0.5" stopColor={spec.cord[1]} />
              <stop offset="1" stopColor={spec.cord[0]} />
            </linearGradient>
            <filter id={holidayGlow} x="-40%" y="-80%" width="180%" height="260%">
              <feGaussianBlur stdDeviation="5.2" />
            </filter>
            <filter id={holidayBokeh} x="-100%" y="-100%" width="300%" height="300%">
              <feGaussianBlur stdDeviation="2.6" />
            </filter>
          </>
        ) : null}
        {spec ? (
          <radialGradient id={holidayMetal} cx="34%" cy="26%" r="82%">
            <stop offset="0" stopColor={shade(spec.metal, 38)} />
            <stop offset="0.5" stopColor={spec.metal} />
            <stop offset="1" stopColor={shade(spec.metal, -32)} />
          </radialGradient>
        ) : null}
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
          {/* Ambient bokeh behind the cord: slow, low-opacity colour dust. */}
          <g className="rope-bokeh" filter={`url(#${holidayBokeh})`}>
            {[
              { t: 0.14, r: 1.8, dy: 14, o: 0.1 },
              { t: 0.42, r: 1.4, dy: 10, o: 0.07 },
              { t: 0.73, r: 1.8, dy: 32, o: 0.09 },
              { t: 0.9, r: 1.2, dy: 18, o: 0.1 },
            ].map((bokeh, index) => {
              const point = holidayRopePoint(safeWidth, safeSag, bokeh.t);
              return (
                <circle
                  key={`bokeh-${index}`}
                  cx={round(point.x)}
                  cy={round(point.y + bokeh.dy)}
                  r={bokeh.r}
                  fill={index % 2 === 0 ? spec.light : spec.thread}
                  opacity={bokeh.o}
                />
              );
            })}
          </g>

          {/* Layered cord: soft glow, gradient core, top-light and a woven thread. */}
          <path
            className="rope rope--holiday-glow"
            d={path}
            stroke={`url(#${holidayGradient})`}
            filter={`url(#${holidayGlow})`}
          />
          <path className="rope rope--holiday" d={path} stroke={`url(#${holidayGradient})`} />
          <path
            className="rope rope--holiday-highlight"
            d={path}
            stroke={spec.light}
            transform="translate(0, -0.85)"
          />
          <path
            className="rope rope--holiday-thread"
            d={path}
            stroke={spec.thread}
            strokeDasharray="1.2 4.4"
          />

          {/* Cord-woven motifs: small hand-drawn details between the hanging pieces. */}
          <g className="rope-sprigs">
            {spec.motifs.map((motif, index) => {
              const point = holidayRopePoint(safeWidth, safeSag, motif.t);
              const sideOffset = motif.side === "top" ? -3.2 : motif.side === "bottom" ? 3.2 : 0;
              const scale = (motif.scale ?? 0.8) * 0.82;
              return (
                <g
                  key={`motif-${index}`}
                  transform={`translate(${round(point.x)}, ${round(point.y + sideOffset)}) scale(${scale})`}
                  opacity="0.9"
                >
                  <MotifShape motif={{ ...motif, color: spec.metal }} />
                </g>
              );
            })}
          </g>

          {/* Warm light points strung below the cord. */}
          {spec.lights.map((t, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, t);
            const scale = index % 3 === 0 ? 1.18 : index % 3 === 1 ? 0.92 : 1;
            return (
              <g
                key={`light-${index}`}
                className="rope-light"
                style={{ animationDelay: `${index * 230}ms` }}
                transform={`translate(${round(x)}, ${round(y + 7)}) scale(${scale})`}
              >
                <line x1="0" y1="-7" x2="0" y2="0" className="rope-light__wire" />
                <circle r="6.6" className="rope-light__halo" style={{ fill: spec.light }} />
                <circle r="1.8" className="rope-light__bulb" style={{ fill: spec.light }} />
              </g>
            );
          })}

          {/* Hanging ornaments: the generous, readable layer of the garland. */}
          {spec.ornaments.map((ornament, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, ornament.t);
            const cx = round(x);
            const top = round(y);
            const hang = (ornament.hang ?? 20 + (index % 3) * 4) + 4;
            const value = (ornament.scale ?? 1) * 1.0;
            return (
              <g
                key={`ornament-${index}`}
                className="rope-ornament"
                style={{ animationDelay: `${index * 150}ms` }}
              >
                <line x1={cx} y1={top} x2={cx} y2={top + hang} className="rope-ornament__string" />
                <circle cx={cx} cy={top + hang} r="1.4" className="rope-ornament__knot" />
                <g transform={`translate(${cx}, ${top + hang}) scale(${value})`}>
                  <OrnamentShape ornament={{ ...ornament, color: spec.metal }} fillId={metalFill} />
                </g>
              </g>
            );
          })}

          {/* Hand-tied ribbon bows anchor both ends of the seasonal garland. */}
          <RibbonBow
            x={safeWidth * 0.046}
            y={holidayRopePoint(safeWidth, safeSag, 0.046).y}
            color={spec.ribbon}
            accent={spec.thread}
          />
          <RibbonBow
            x={safeWidth * 0.954}
            y={holidayRopePoint(safeWidth, safeSag, 0.954).y}
            color={spec.ribbon}
            accent={spec.thread}
            flip
          />
        </g>
      ) : null}
    </svg>
  );
}
