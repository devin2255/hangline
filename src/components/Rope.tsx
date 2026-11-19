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
  landing?: number;
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
      d="M0 -8 L1.7 -2.5 L7.6 -2.5 L2.9 1.1 L4.6 6.6 L0 3.4 L-4.6 6.6 L-2.9 1.1 L-7.6 -2.5 L-1.7 -2.5 Z"
      fill={color}
    />
  );
}

/** Brass loop. The ornament group origin is the knot, and the charm hangs below it. */
function CharmLoop({ metal }: { metal: string }) {
  return (
    <path
      d="M0 0.4 C0.1 -1.6 1.8 -1.8 1.8 -0.4"
      fill="none"
      stroke={metal}
      strokeWidth="0.9"
      strokeLinecap="round"
    />
  );
}

/**
 * Hanging charms. The knot is the origin; every shape hangs downward from a small brass loop.
 * Bodies keep their own enamel colour. Brass is only the fitting.
 */
function OrnamentShape({ ornament, metal }: { ornament: HolidayOrnament; metal: string }) {
  const { kind, color } = ornament;
  const deep = shade(color, -26);
  const lit = shade(color, 36);
  switch (kind) {
    case "bauble":
      return (
        <g>
          <CharmLoop metal={metal} />
          <rect x="-1.7" y="1.2" width="3.4" height="1.8" rx="0.5" fill={metal} />
          <circle cy="9.4" r="6.7" fill={deep} />
          <circle cy="9.2" r="6.1" fill={color} />
          <path
            d="M-3.6 6.2 A4.6 4.6 0 0 1 0.6 5.2"
            fill="none"
            stroke={lit}
            strokeWidth="1"
            strokeLinecap="round"
          />
        </g>
      );
    case "star":
      return (
        <g>
          <CharmLoop metal={metal} />
          <g transform="translate(0 9)">
            <StarPath color={deep} size={12.2} />
            <StarPath color={color} size={10.4} />
          </g>
        </g>
      );
    case "bell":
      return (
        <g>
          <CharmLoop metal={metal} />
          <rect x="-1.5" y="1.1" width="3" height="1.5" rx="0.4" fill={metal} />
          <path d="M0 2.6 C-6.4 3.4 -7.2 10.2 -6.2 12.4 H6.2 C7.2 10.2 6.4 3.4 0 2.6 Z" fill={deep} />
          <path d="M0 3.2 C-5.4 4 -6.2 10 -5.4 11.8 H5.4 C6.2 10 5.4 4 0 3.2 Z" fill={color} />
          <path d="M-5.6 11.8 H5.6" stroke={metal} strokeWidth="1.3" strokeLinecap="round" />
          <circle cy="13.6" r="1.35" fill={metal} />
          <path d="M-2.2 5.4 Q0 4.2 2.2 5.6" fill="none" stroke={lit} strokeWidth="0.7" strokeLinecap="round" />
        </g>
      );
    case "snow":
      return (
        <g stroke={color} strokeLinecap="round" transform="translate(0 8)">
          <path d="M0 -6.4 V6.4 M-5.5 -3.2 L5.5 3.2 M5.5 -3.2 L-5.5 3.2" strokeWidth="1.15" />
          <path d="M0 -6.4 L-1.5 -4.4 M0 -6.4 L1.5 -4.4 M0 6.4 L-1.5 4.4 M0 6.4 L1.5 4.4" strokeWidth="0.75" />
        </g>
      );
    case "pinecone":
      return (
        <g>
          <CharmLoop metal={metal} />
          <ellipse cx="0" cy="10.2" rx="4.6" ry="7.2" fill={deep} />
          <ellipse cx="0" cy="10" rx="4.1" ry="6.6" fill={color} />
          <path
            d="M-3.2 6.2 Q0 7.6 3.2 6.2 M-3.6 9.2 Q0 10.8 3.6 9.2 M-3.2 12.2 Q0 13.6 3.2 12.2"
            fill="none"
            stroke={deep}
            strokeWidth="0.7"
            strokeLinecap="round"
          />
        </g>
      );
    case "heart":
      return (
        <g>
          <CharmLoop metal={metal} />
          <path
            d="M0 15.2 C-8.4 9.2 -8.2 3.2 -4.2 3.2 C-2.2 3.2 0 4.8 0 6.2 C0 4.8 2.2 3.2 4.2 3.2 C8.2 3.2 8.4 9.2 0 15.2 Z"
            fill={deep}
          />
          <path
            d="M0 14.4 C-7.4 8.8 -7.2 3.8 -3.6 3.8 C-1.8 3.8 0 5.2 0 6.4 C0 5.2 1.8 3.8 3.6 3.8 C7.2 3.8 7.4 8.8 0 14.4 Z"
            fill={color}
          />
          <path d="M-2.6 6.2 C-3.8 5 -4.6 5.4 -4 6.6" fill="none" stroke={lit} strokeWidth="0.7" strokeLinecap="round" />
        </g>
      );
    case "lantern":
      return (
        <g>
          <path d="M0 0 V2.2" stroke={metal} strokeWidth="0.8" />
          <path d="M-2.6 2.4 H2.6" stroke={metal} strokeWidth="1.15" strokeLinecap="round" />
          <path d="M-4.6 4.2 C-6.6 8 -6.6 14 -4.6 17.2 H4.6 C6.6 14 6.6 8 4.6 4.2 Z" fill={deep} />
          <path d="M-4.1 4.6 C-5.8 8.2 -5.8 13.6 -4.1 16.6 H4.1 C5.8 13.6 5.8 8.2 4.1 4.6 Z" fill={color} />
          <path d="M-4.4 4.4 H4.4 M-4.4 16.8 H4.4" stroke={metal} strokeWidth="1.15" strokeLinecap="round" />
          <path d="M-1.6 5.2 V16 M1.6 5.2 V16" stroke={lit} strokeOpacity="0.55" strokeWidth="0.45" />
          <path d="M0 17.6 V20.4" stroke={metal} strokeWidth="0.6" />
          <path d="M-1.3 20.4 H1.3" stroke={metal} strokeWidth="0.6" strokeLinecap="round" />
        </g>
      );
    case "firework":
      return (
        <g stroke={color} strokeLinecap="round" transform="translate(0 8)">
          <circle r="1.15" fill={color} stroke="none" />
          <path d="M0 -7.2 V-2.4 M0 2.4 V7.2 M-7.2 0 H-2.4 M2.4 0 H7.2" strokeWidth="0.9" />
          <path d="M-5 -5 L-2.2 -2.2 M2.2 2.2 L5 5 M5 -5 L2.2 -2.2 M-2.2 2.2 L-5 5" strokeWidth="0.7" />
        </g>
      );
    case "mooncake":
      return (
        <g>
          <CharmLoop metal={metal} />
          <circle cy="9.2" r="6.4" fill={deep} />
          <circle cy="9" r="5.8" fill={color} />
          <circle cy="9" r="3.3" fill="none" stroke={lit} strokeWidth="0.7" />
          <circle cy="9" r="1.1" fill={lit} fillOpacity="0.85" />
        </g>
      );
    case "pumpkin":
      return (
        <g>
          <path d="M-0.7 1.6 C-0.2 -0.2 1.4 -0.2 1.6 1.8" fill="none" stroke={shade(metal, -18)} strokeWidth="1" strokeLinecap="round" />
          <ellipse cx="-2.4" cy="9.4" rx="3.3" ry="5.6" fill={deep} />
          <ellipse cx="2.4" cy="9.4" rx="3.3" ry="5.6" fill={deep} />
          <ellipse cy="9.6" rx="3.1" ry="6" fill={color} />
          <path d="M-1.2 5.2 V14.2 M1.2 5.2 V14.2" stroke={deep} strokeOpacity="0.45" strokeWidth="0.45" />
        </g>
      );
    case "bat":
      return (
        <g transform="translate(0 7)">
          <path
            d="M0 -1.2 C-2.4 -4.6 -6.4 -3.4 -8.2 0.2 C-5.6 0.6 -4 2.4 -2.4 1.6 C-0.8 3.2 0.8 3.2 2.4 1.6 C4 2.4 5.6 0.6 8.2 0.2 C6.4 -3.4 2.4 -4.6 0 -1.2 Z"
            fill={color}
          />
          <circle cx="-1.3" cy="-0.2" r="0.45" fill={lit} />
          <circle cx="1.3" cy="-0.2" r="0.45" fill={lit} />
        </g>
      );
    case "flag":
      return (
        <g>
          <path d="M0 0 V16" stroke={metal} strokeWidth="0.9" strokeLinecap="round" />
          <path d="M0.6 1.6 C6 2.2 8.4 3.6 8.6 5.6 C6.2 6.4 3.2 6.2 0.6 7.2 Z" fill={color} />
          <path d="M0.6 2.2 C4.6 2.6 6.4 3.4 6.8 4.6" fill="none" stroke={lit} strokeWidth="0.45" strokeLinecap="round" />
        </g>
      );
    case "moon":
      return (
        <g>
          <CharmLoop metal={metal} />
          <path
            d="M2.4 3.2 A6.6 6.6 0 1 0 2.4 15.2 A5 5 0 1 1 2.4 3.2 Z"
            fill={color}
          />
          <path
            d="M1.2 5.4 A4.2 4.2 0 0 0 -1.6 8.2"
            fill="none"
            stroke={lit}
            strokeWidth="0.7"
            strokeLinecap="round"
          />
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
      <rect x="-2.1" y="-1.5" width="4.2" height="3" rx="0.9" fill="#C8A96A" />
      <rect x="-2.1" y="-0.3" width="4.2" height="0.7" fill="#8A6B3A" opacity="0.8" />
      <path d="M-0.5 1.4 C-1.4 5.2 -1.8 9 -1.1 12.4" fill="none" stroke={color} strokeWidth="0.7" strokeLinecap="round" opacity="0.8" />
      <path d="M0.5 1.4 C1.5 5.4 1.9 9.2 1.2 12.6" fill="none" stroke={color} strokeWidth="0.55" strokeLinecap="round" opacity="0.55" />
      <circle r="0.9" fill={accent} />
    </g>
  );
}

export function Rope({ width, sag, style, color, holiday = null, landing = 0 }: RopeProps) {
  const safeWidth = Math.max(1, Math.round(width));
  const safeSag = Number.isFinite(sag) ? Math.max(0, sag) : 0;
  const spec = holiday && holiday !== "christmas" ? HOLIDAY_ROPE_SPECS[holiday] : null;
  const height = Math.max(1, Math.ceil(ROPE_TOP + safeSag + (holiday ? 84 : 8)));
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
              <feGaussianBlur stdDeviation="2.4" />
            </filter>
            <filter id={holidayBokeh} x="-100%" y="-100%" width="300%" height="300%">
              <feGaussianBlur stdDeviation="2.6" />
            </filter>
          </>
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

      {holiday === "christmas" && <ChristmasGarland width={safeWidth} sag={safeSag} landing={landing} />}
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
            strokeDasharray="0.7 5.6"
          />

          {/* Cord-woven motifs: small hand-drawn details between the hanging pieces. */}
          <g className="rope-sprigs">
            {spec.motifs.map((motif, index) => {
              const point = holidayRopePoint(safeWidth, safeSag, motif.t);
              const sideOffset = motif.side === "top" ? -3.2 : motif.side === "bottom" ? 3.2 : 0;
              const scale = (motif.scale ?? 0.8) * 0.62;
              return (
                <g
                  key={`motif-${index}`}
                  transform={`translate(${round(point.x)}, ${round(point.y + sideOffset)}) scale(${scale})`}
                  opacity="0.72"
                >
                  <MotifShape motif={motif} />
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
                transform={`translate(${round(x)}, ${round(y + 4.5)}) scale(${scale})`}
              >
                <line x1="0" y1="-4.5" x2="0" y2="0" className="rope-light__wire" />
                <circle r="2.6" className="rope-light__halo" style={{ fill: spec.light }} />
                <circle r="1.15" className="rope-light__bulb" style={{ fill: spec.light }} />
              </g>
            );
          })}

          {/* Hanging ornaments: the generous, readable layer of the garland. */}
          {spec.ornaments.map((ornament, index) => {
            const { x, y } = holidayRopePoint(safeWidth, safeSag, ornament.t);
            const cx = round(x);
            const top = round(y);
            const hang = Math.round((ornament.hang ?? 18) * 0.5);
            const value = (ornament.scale ?? 1) * 0.78;
            return (
              <g
                key={`ornament-${index}`}
                className="rope-ornament"
                style={{ animationDelay: `${index * 150}ms` }}
              >
                <line x1={cx} y1={top + 1} x2={cx} y2={top + hang} className="rope-ornament__string" />
                <circle cx={cx} cy={top + 1.1} r="0.9" className="rope-ornament__knot" />
                <g transform={`translate(${cx}, ${top + hang}) scale(${value})`}>
                  <OrnamentShape ornament={ornament} metal={spec.metal} />
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
