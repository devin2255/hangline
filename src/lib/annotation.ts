/**
 * Pure model of the markup editor (P3-2, design spec §3.7).
 *
 * The React window owns the pointer/canvas plumbing; this module keeps everything that can be
 * reasoned about without a DOM: the shape list, a bounded undo/redo stack (>= 24 steps), the crop
 * rectangle with its output offset, and the display → image-pixel transform. Shapes are stored in
 * **natural image pixels**, so the same data renders identically into the small on-screen canvas
 * (scaled by the component) and into the full-resolution offscreen canvas used for saving.
 *
 * Text is anchored at its top-left point and supports multiple lines; `width` is the stroke width in
 * image pixels and doubles as the text size driver (see [`textFontPx`]).
 */

/** One editor tool (design spec §3.7; crop is a mode, not a drawn shape). */
export type Tool = "pen" | "rect" | "arrow" | "text" | "crop";

/** A tool that produces a persisted shape. */
export type DrawTool = Exclude<Tool, "crop">;

/** One point in natural image pixels (`[x, y]`). */
export type Point = [number, number];

/** One drawn annotation (image-pixel coordinates, never display coordinates). */
export interface Shape {
  /** Unique id (only used as a React key and for tests). */
  id: string;
  tool: DrawTool;
  /** CSS colour string (`#rrggbb`). */
  color: string;
  /** Stroke width in natural image pixels; the text tool derives its font size from it. */
  width: number;
  /** Image-pixel points: pen = the sampled path, rect/arrow = start + current/end, text = anchor. */
  points: Point[];
  /** Multi-line text for the `text` tool. */
  text?: string;
}

/** A crop rectangle in image pixels (normalized: `w`/`h` positive). */
export interface CropRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Maximum number of undo steps (plan P3-2: `MAX_HISTORY = 24`, spec requires >= 20). */
export const MAX_HISTORY = 24;

/** Pen widths in **display** CSS pixels; the component converts them to image pixels. */
export const PEN_WIDTHS = [2, 4, 8] as const;

/** The six markup colours (red, amber, green, blue, purple, white). */
export const PEN_COLORS = [
  "#ff3b30",
  "#ffcc00",
  "#34c759",
  "#0a84ff",
  "#bf5af2",
  "#ffffff",
] as const;

/** Text font size relative to the stroke width (so the three widths offer three text sizes). */
export const TEXT_FONT_SCALE = 6;

/** Smallest text size in image pixels (keeps text readable on small screenshots). */
export const TEXT_FONT_MIN_PX = 12;

/** The crop/paint state of one open editor image (history is part of the state). */
export interface AnnotationState {
  /** Natural image width in pixels. */
  width: number;
  /** Natural image height in pixels. */
  height: number;
  /** Committed shapes, oldest first (render order). */
  shapes: Shape[];
  /** Committed crop, or `null` for the full image. */
  crop: CropRect | null;
  /** Undo snapshots (oldest first, bounded by [`MAX_HISTORY`]). */
  past: Snapshot[];
  /** Redo snapshots (newest last, bounded by [`MAX_HISTORY`]). */
  future: Snapshot[];
}

/** One undo/redo entry: the shapes and crop before/after a mutation. */
interface Snapshot {
  shapes: Shape[];
  crop: CropRect | null;
}

/** The displayed image rectangle inside the editor stage (CSS pixels). */
export interface EditorView {
  displayWidth: number;
  displayHeight: number;
}

/** A rectangle produced by a drag (image pixels, may be unnormalized). */
export interface DragRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The offscreen image region produced by the current crop. */
export interface CropOutput {
  width: number;
  height: number;
  offsetX: number;
  offsetY: number;
}

/** Clamps `value` into `[min, max]` (callers pass finite values). */
function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** `value` when finite, else `fallback`. */
function finite(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

/** Creates an empty editor state for an image of `width`×`height` natural pixels. */
export function createAnnotationState(width: number, height: number): AnnotationState {
  return {
    width: Math.max(1, Math.round(finite(width, 1))),
    height: Math.max(1, Math.round(finite(height, 1))),
    shapes: [],
    crop: null,
    past: [],
    future: [],
  };
}

/** Whether an undo is available. */
export function canUndo(state: AnnotationState): boolean {
  return state.past.length > 0;
}

/** Whether a redo is available. */
export function canRedo(state: AnnotationState): boolean {
  return state.future.length > 0;
}

/** Records the current snapshot and returns the bounded new undo stack. */
function pushHistory(state: AnnotationState): Snapshot[] {
  return [...state.past, { shapes: state.shapes, crop: state.crop }].slice(-MAX_HISTORY);
}

/** Appends one committed shape and clears the redo branch. */
export function addShape(state: AnnotationState, shape: Shape): AnnotationState {
  return {
    ...state,
    shapes: [...state.shapes, shape],
    past: pushHistory(state),
    future: [],
  };
}

/** Restores the previous snapshot; a no-op at the bottom of the stack. */
export function undo(state: AnnotationState): AnnotationState {
  const previous = state.past[state.past.length - 1];
  if (!previous) return state;
  return {
    ...state,
    shapes: previous.shapes,
    crop: previous.crop,
    past: state.past.slice(0, -1),
    future: [...state.future, { shapes: state.shapes, crop: state.crop }].slice(-MAX_HISTORY),
  };
}

/** Re-applies the last undone snapshot; a no-op without a redo branch. */
export function redo(state: AnnotationState): AnnotationState {
  const next = state.future[state.future.length - 1];
  if (!next) return state;
  return {
    ...state,
    shapes: next.shapes,
    crop: next.crop,
    past: [...state.past, { shapes: state.shapes, crop: state.crop }].slice(-MAX_HISTORY),
    future: state.future.slice(0, -1),
  };
}

/** Whether two crop rectangles describe the same region. */
function sameCrop(left: CropRect | null, right: CropRect | null): boolean {
  if (left === null || right === null) return left === right;
  return left.x === right.x && left.y === right.y && left.w === right.w && left.h === right.h;
}

/** Normalizes/clamps a drag rectangle to the image; `null` for degenerate or invalid input. */
export function clampCrop(
  rect: DragRect,
  imageWidth: number,
  imageHeight: number,
): CropRect | null {
  const width = Math.max(0, Math.round(finite(imageWidth, 0)));
  const height = Math.max(0, Math.round(finite(imageHeight, 0)));
  if (width < 1 || height < 1) return null;
  const { x, y, w, h } = rect;
  if (![x, y, w, h].every((value) => Number.isFinite(value))) return null;
  // A drag can go up/left: normalize both axes before clamping.
  const left = clamp(Math.min(x, x + w), 0, width);
  const right = clamp(Math.max(x, x + w), 0, width);
  const top = clamp(Math.min(y, y + h), 0, height);
  const bottom = clamp(Math.max(y, y + h), 0, height);
  const outWidth = right - left;
  const outHeight = bottom - top;
  if (outWidth < 1 || outHeight < 1) return null;
  return { x: left, y: top, w: outWidth, h: outHeight };
}

/** Commits (or clears, with `null`) the crop; unchanged rectangles do not add a history step. */
export function setCrop(state: AnnotationState, rect: CropRect | null): AnnotationState {
  const next = rect === null ? null : clampCrop(rect, state.width, state.height);
  if (sameCrop(next, state.crop)) return state;
  return {
    ...state,
    crop: next,
    past: pushHistory(state),
    future: [],
  };
}

/**
 * Translates a crop rectangle by `dx`/`dy` image pixels, keeping its size.
 *
 * The crop overlay's `cursor: move` affordance uses this while dragging inside the rectangle; the
 * result may extend past the image and is clamped by [`clampCrop`] on release. Non-finite deltas are
 * ignored (a corrupt pointer must not move the selection to NaN).
 */
export function moveCrop(rect: CropRect, dx: number, dy: number): CropRect {
  const offsetX = Number.isFinite(dx) ? dx : 0;
  const offsetY = Number.isFinite(dy) ? dy : 0;
  return { x: rect.x + offsetX, y: rect.y + offsetY, w: rect.w, h: rect.h };
}

/** The region the composited PNG keeps: crop size + its offset inside the natural image. */
export function cropOutput(state: AnnotationState): CropOutput {
  const crop = state.crop ? clampCrop(state.crop, state.width, state.height) : null;
  if (!crop) {
    return { width: state.width, height: state.height, offsetX: 0, offsetY: 0 };
  }
  const offsetX = clamp(Math.round(crop.x), 0, Math.max(0, state.width - 1));
  const offsetY = clamp(Math.round(crop.y), 0, Math.max(0, state.height - 1));
  return {
    width: Math.max(1, Math.min(Math.round(crop.w), state.width - offsetX)),
    height: Math.max(1, Math.min(Math.round(crop.h), state.height - offsetY)),
    offsetX,
    offsetY,
  };
}

/**
 * Maps a pointer position on the displayed image to natural image pixels.
 *
 * The canvas is positioned exactly over the displayed image (`object-fit: contain`), so only a
 * scale (no offset) is involved. Points outside the image are clamped to its edges; a degenerate
 * view or non-finite input maps to the origin.
 */
export function toImagePoint(
  display: { x: number; y: number },
  state: Pick<AnnotationState, "width" | "height">,
  view: EditorView,
): Point {
  const displayWidth = finite(view.displayWidth, 0);
  const displayHeight = finite(view.displayHeight, 0);
  if (displayWidth <= 0 || displayHeight <= 0) return [0, 0];
  const x = finite(display.x, 0);
  const y = finite(display.y, 0);
  return [
    clamp((x * state.width) / displayWidth, 0, state.width),
    clamp((y * state.height) / displayHeight, 0, state.height),
  ];
}

/** Text font size in image pixels for a shape width (the text tool reuses the pen widths). */
export function textFontPx(shapeWidth: number): number {
  return Math.max(TEXT_FONT_MIN_PX, Math.round(finite(shapeWidth, 0) * TEXT_FONT_SCALE));
}

/**
 * Fits an image into a box preserving its aspect ratio (`object-fit: contain` math in CSS pixels).
 *
 * Returns a zero size for a degenerate box/image so the page can show the loading placeholder
 * instead of a 1×1 canvas. The displayed size is rounded to whole CSS pixels; the caller positions
 * the image and the overlay canvas at exactly this rectangle.
 */
export function fitContain(
  imageWidth: number,
  imageHeight: number,
  boxWidth: number,
  boxHeight: number,
): { width: number; height: number } {
  if (
    !Number.isFinite(imageWidth) ||
    !Number.isFinite(imageHeight) ||
    !Number.isFinite(boxWidth) ||
    !Number.isFinite(boxHeight) ||
    imageWidth <= 0 ||
    imageHeight <= 0 ||
    boxWidth <= 0 ||
    boxHeight <= 0
  ) {
    return { width: 0, height: 0 };
  }
  const scale = Math.min(boxWidth / imageWidth, boxHeight / imageHeight);
  return {
    width: Math.max(1, Math.round(imageWidth * scale)),
    height: Math.max(1, Math.round(imageHeight * scale)),
  };
}

/** Command used by the renderer to paint one shape (image pixels × `scale`). */
export type ShapeRenderer = (ctx: CanvasRenderingContext2D, shape: Shape, scale: number) => void;

/** Head endpoint for an arrow (image pixels), for tests/clients that only need the geometry. */
export function arrowHead(from: Point, to: Point, headLength = 18): [Point, Point] {
  const length = Math.max(1, finite(headLength, 18));
  const angle = Math.atan2(to[1] - from[1], to[0] - from[0]);
  const spread = Math.PI / 7;
  return [
    [to[0] - length * Math.cos(angle - spread), to[1] - length * Math.sin(angle - spread)],
    [to[0] - length * Math.cos(angle + spread), to[1] - length * Math.sin(angle + spread)],
  ];
}

/** Paints the pen path: a polyline (single click = a filled dot). */
function renderPen(ctx: CanvasRenderingContext2D, shape: Shape, scale: number): void {
  const [first, ...rest] = shape.points;
  if (!first) return;
  if (shape.points.length === 1) {
    ctx.beginPath();
    ctx.arc(
      first[0] * scale,
      first[1] * scale,
      Math.max(0.5, (shape.width * scale) / 2),
      0,
      Math.PI * 2,
    );
    ctx.fill();
    return;
  }
  ctx.beginPath();
  ctx.moveTo(first[0] * scale, first[1] * scale);
  for (const point of rest) {
    ctx.lineTo(point[0] * scale, point[1] * scale);
  }
  ctx.stroke();
}

/** Paints a rectangle from its two corner points (either drag direction works). */
function renderRect(ctx: CanvasRenderingContext2D, shape: Shape, scale: number): void {
  const start = shape.points[0];
  const end = shape.points[shape.points.length - 1] ?? start;
  if (!start || !end) return;
  const x = Math.min(start[0], end[0]) * scale;
  const y = Math.min(start[1], end[1]) * scale;
  const width = Math.abs(end[0] - start[0]) * scale;
  const height = Math.abs(end[1] - start[1]) * scale;
  ctx.strokeRect(x, y, width, height);
}

/** Paints an arrow shaft plus its two head strokes. */
function renderArrow(ctx: CanvasRenderingContext2D, shape: Shape, scale: number): void {
  const from = shape.points[0];
  const to = shape.points[shape.points.length - 1] ?? from;
  if (!from || !to) return;
  ctx.beginPath();
  ctx.moveTo(from[0] * scale, from[1] * scale);
  ctx.lineTo(to[0] * scale, to[1] * scale);
  ctx.stroke();
  const headLength = Math.max(12, shape.width * 4);
  const [left, right] = arrowHead(from, to, headLength);
  ctx.beginPath();
  ctx.moveTo(left[0] * scale, left[1] * scale);
  ctx.lineTo(to[0] * scale, to[1] * scale);
  ctx.lineTo(right[0] * scale, right[1] * scale);
  ctx.stroke();
}

/** Paints multi-line text with its top-left corner at the anchor. */
function renderText(ctx: CanvasRenderingContext2D, shape: Shape, scale: number): void {
  const anchor = shape.points[0];
  if (!anchor || !shape.text) return;
  const fontSize = textFontPx(shape.width) * scale;
  ctx.font = `${fontSize}px "Segoe UI", system-ui, sans-serif`;
  ctx.textBaseline = "top";
  const lines = shape.text.split(/\r?\n/);
  const lineHeight = fontSize * 1.25;
  lines.forEach((line, index) => {
    ctx.fillText(line, anchor[0] * scale, anchor[1] * scale + index * lineHeight);
  });
}

/** Draws every committed shape at `scale` (1 = natural image pixels, used for saving). */
export function renderShapes(
  ctx: CanvasRenderingContext2D,
  shapes: readonly Shape[],
  scale: number,
): void {
  if (!Number.isFinite(scale) || scale <= 0) return;
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const shape of shapes) {
    ctx.strokeStyle = shape.color;
    ctx.fillStyle = shape.color;
    ctx.lineWidth = Math.max(1, shape.width) * scale;
    switch (shape.tool) {
      case "pen":
        renderPen(ctx, shape, scale);
        break;
      case "rect":
        renderRect(ctx, shape, scale);
        break;
      case "arrow":
        renderArrow(ctx, shape, scale);
        break;
      case "text":
        renderText(ctx, shape, scale);
        break;
    }
  }
  ctx.restore();
}
