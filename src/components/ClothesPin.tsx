/**
 * The 9×26 dp aluminium clothes pin (design spec §3.3, Tendedero `PeggedView.swift:184-213`).
 *
 * Exact source recipe: a rounded rect (3.5 dp corners) filled with a horizontal four-stop brushed
 * metal gradient, a thin top-light/bottom-dark stroke, and the dark slot the rope passes through
 * 8.5 dp below the top. The drop shadow lives in `Panel.css` (it must render outside the SVG box).
 * Purely decorative: the card owns the hit rect and all pointer handling.
 */

import { useId } from "react";

export interface ClothesPinProps {
  /** Extra class name for positioning (the panel places each pin over its card). */
  className?: string;
}

export function ClothesPin({ className }: ClothesPinProps) {
  // React ids contain ":" characters, which are awkward inside SVG url(#...) references.
  const uid = useId().replace(/:/g, "");
  const metal = `pin-metal-${uid}`;
  const edge = `pin-edge-${uid}`;

  return (
    <svg
      className={className ? `clothes-pin ${className}` : "clothes-pin"}
      width="9"
      height="26"
      viewBox="0 0 9 26"
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        <linearGradient id={metal} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor="#b3b3b3" />
          <stop offset="0.35" stopColor="#ededed" />
          <stop offset="0.65" stopColor="#d1d1d1" />
          <stop offset="1" stopColor="#9e9e9e" />
        </linearGradient>
        <linearGradient id={edge} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#e6e6e6" />
          <stop offset="1" stopColor="#2e2e2e" />
        </linearGradient>
      </defs>
      {/* Body inset by half the stroke so the 0.6 dp edge stays inside the 9×26 box. */}
      <rect
        x="0.3"
        y="0.3"
        width="8.4"
        height="25.4"
        rx="3.2"
        fill={`url(#${metal})`}
        stroke={`url(#${edge})`}
        strokeWidth="0.6"
      />
      {/* The slot the line passes through (5×1.4 at 8.5 dp from the top). */}
      <rect x="2" y="8.5" width="5" height="1.4" rx="0.7" fill="rgba(0, 0, 0, 0.32)" />
    </svg>
  );
}
