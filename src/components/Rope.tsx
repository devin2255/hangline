/**
 * The clothesline rope (design spec §3.3): a quadratic Bezier with a sag of
 * `min(30, panelWidth * 0.018)` dp, drawn as three strokes so it reads as a twisted cord.
 *
 * The SVG is positioned at the top of the line and spans its full width (which may exceed the
 * viewport when the minimum card group overflows; the viewport clips the oldest edge). A CSS mask
 * fades both ends so the rope does not terminate in a hard edge at the panel border.
 */

import { ROPE_TOP, ropePath, ropeSag } from "../lib/layout";

export interface RopeProps {
  /** Line width in dp (CSS px); the rope spans the full line content. */
  width: number;
}

export function Rope({ width }: RopeProps) {
  const safeWidth = Math.max(1, Math.round(width));
  const height = Math.max(1, Math.ceil(ROPE_TOP + ropeSag(safeWidth) + 8));
  const path = ropePath(safeWidth);

  return (
    <svg
      className="panel__rope"
      width={safeWidth}
      height={height}
      viewBox={`0 0 ${safeWidth} ${height}`}
      aria-hidden="true"
      focusable="false"
    >
      <path className="rope rope--shadow" d={path} />
      <path className="rope rope--core" d={path} />
      <path className="rope rope--highlight" d={path} />
    </svg>
  );
}
