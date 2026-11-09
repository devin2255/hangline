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

import { relativeLuminance } from "./color";

/**
 * One editor tool (design spec §3.7; crop is a mode, not a drawn shape).
 *
 * `highlight`, `mosaic`, `blur` and `number` are the W1.3 additions. `mosaic`/`blur` are pixel
 * tools: they do not paint a colour, they sample the base image (see [`renderShapes`]). `number`
 * is click-to-place and stores its sequence value in the optional [`Shape.label`].
 */
export type Tool =
  | "pen"
  | "rect"
  | "arrow"
  | "text"
  | "highlight"
  | "mosaic"
  | "blur"
  | "number"
  | "crop";

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
  /**
   * Single-line label for the `number` tool: the drawn sequence value (`"1"`, `"2"`, …).
   *
   * Optional so state written before W1.3 stays readable: an old shape simply has no label, and a
   * `number` shape without one is not rendered (see [`nextNumberValue`] and `renderNumber`).
   */
  label?: string;
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

/** Opacity of the `highlight` fill. The saved PNG uses the same renderer, so it matches exactly. */
export const HIGHLIGHT_ALPHA = 0.35;

/** Amber default for the highlight tool (picked automatically when the tool is selected). */
export const HIGHLIGHT_COLOR = "#ffcc00";

/** Number badge radius: `width * NUMBER_RADIUS_SCALE` image px, never below the minimum. */
export const NUMBER_RADIUS_SCALE = 3;
export const NUMBER_RADIUS_MIN_PX = 12;

/** Mosaic pixel-block size in image pixels (derived from the stroke width, then clamped). */
export const MOSAIC_BLOCK_MIN_PX = 2;
export const MOSAIC_BLOCK_MAX_PX = 64;

/** Blur radius in image pixels: `width * BLUR_RADIUS_SCALE`, clamped to a usable range. */
export const BLUR_RADIUS_SCALE = 1.5;
export const BLUR_RADIUS_MIN_PX = 2;
export const BLUR_RADIUS_MAX_PX = 48;

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
 * The normalized rectangle spanned by a shape's first/last point (image pixels).
 *
 * Used by the rectangle-shaped tools (`highlight`, `mosaic`, `blur`); `null` when the shape has no
 * usable two-point geometry. A reverse drag (right-to-left / bottom-to-top) is normalized to a
 * positive rectangle, exactly like [`clampCrop`] does for the crop rectangle.
 */
export function shapeRect(shape: Pick<Shape, "points">): DragRect | null {
  const start = shape.points[0];
  const end = shape.points[shape.points.length - 1] ?? start;
  if (!start || !end) return null;
  if (![start[0], start[1], end[0], end[1]].every((value) => Number.isFinite(value))) return null;
  return {
    x: Math.min(start[0], end[0]),
    y: Math.min(start[1], end[1]),
    w: Math.abs(end[0] - start[0]),
    h: Math.abs(end[1] - start[1]),
  };
}

/** Mosaic pixel-block size in image pixels for a shape width (then scaled to device pixels). */
export function mosaicBlockPx(shapeWidth: number, scale = 1): number {
  const natural = Math.max(1, finite(shapeWidth, 1));
  const device = natural * Math.max(0, finite(scale, 1));
  return Math.round(clamp(device, MOSAIC_BLOCK_MIN_PX, MOSAIC_BLOCK_MAX_PX));
}

/** Blur radius in image pixels for a shape width (then scaled to device pixels). */
export function blurRadiusPx(shapeWidth: number, scale = 1): number {
  const natural = Math.max(1, finite(shapeWidth, 1)) * BLUR_RADIUS_SCALE;
  const device = natural * Math.max(0, finite(scale, 1));
  return Math.round(clamp(device, BLUR_RADIUS_MIN_PX, BLUR_RADIUS_MAX_PX));
}

/** Radius of a number badge in image pixels (derived from the stroke width). */
export function numberRadiusPx(shapeWidth: number): number {
  const scaled = Math.max(1, finite(shapeWidth, 1)) * NUMBER_RADIUS_SCALE;
  return Math.max(NUMBER_RADIUS_MIN_PX, Math.round(scaled));
}

/**
 * The next sequence value for the `number` tool: `max(existing number labels) + 1`.
 *
 * Deriving the value from the committed shapes (instead of a counter in the component) makes it
 * correct across undo: after undoing badge 3 the next badge is 3 again, not 4. Labels that are not
 * positive integers (corrupt/legacy data) are ignored.
 */
export function nextNumberValue(shapes: readonly Shape[]): number {
  let highest = 0;
  for (const shape of shapes) {
    if (shape.tool !== "number") continue;
    const value = Number.parseInt(shape.label ?? "", 10);
    if (Number.isFinite(value) && value > highest) highest = value;
  }
  return highest + 1;
}

/** Digits inside a number badge: white on dark/medium colours, near-black on pale ones. */
export function numberTextColor(background: string): string {
  try {
    return relativeLuminance(background) > 0.6 ? "#111827" : "#ffffff";
  } catch {
    // Unparseable colour (hand-edited state): white is the documented default.
    return "#ffffff";
  }
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

/** Paints a flat translucent rectangle (W1.3 `highlight`). */
function renderHighlight(ctx: CanvasRenderingContext2D, shape: Shape, scale: number): void {
  const rect = shapeRect(shape);
  if (!rect) return;
  const width = rect.w * scale;
  const height = rect.h * scale;
  if (width < 1 || height < 1) return;
  // No stroke: the highlight is a flat marker, and a stroke would double the perceived edge weight.
  // The saved PNG runs through this same function, so the opacity/colour match the preview exactly.
  ctx.globalAlpha = HIGHLIGHT_ALPHA;
  ctx.fillStyle = shape.color;
  ctx.fillRect(rect.x * scale, rect.y * scale, width, height);
}

/** Size of a `CanvasImageSource` in its natural pixels (image elements expose `naturalWidth`). */
function sourceSize(source: CanvasImageSource): { width: number; height: number } | null {
  const candidate = source as unknown as {
    naturalWidth?: number;
    naturalHeight?: number;
    videoWidth?: number;
    videoHeight?: number;
    width?: number;
    height?: number;
  };
  const width = candidate.naturalWidth ?? candidate.videoWidth ?? candidate.width ?? 0;
  const height = candidate.naturalHeight ?? candidate.videoHeight ?? candidate.height ?? 0;
  return width > 0 && height > 0 ? { width, height } : null;
}

/**
 * Reusable offscreen canvas for the mosaic downscale, created on first use.
 *
 * `DOM`-free environments (unit tests) get `null`; the editor always has a document, so this only
 * affects non-browser callers, which then simply paint nothing for the pixel tools.
 */
let scratchCanvas: HTMLCanvasElement | null = null;

function acquireScratch(width: number, height: number): CanvasRenderingContext2D | null {
  if (typeof document === "undefined") return null;
  if (!scratchCanvas) scratchCanvas = document.createElement("canvas");
  if (scratchCanvas.width !== width) scratchCanvas.width = width;
  if (scratchCanvas.height !== height) scratchCanvas.height = height;
  return scratchCanvas.getContext("2d");
}

/**
 * Paints a pixelated rectangle (W1.3 `mosaic`): downscale the base image region to one pixel per
 * block on a scratch canvas, then draw it back with smoothing disabled (nearest-neighbour), which
 * is the classic `drawImage` mosaic. Samples the **base image**, never the overlay canvas.
 */
function renderMosaic(
  ctx: CanvasRenderingContext2D,
  shape: Shape,
  scale: number,
  base: CanvasImageSource | undefined,
): void {
  if (!base) return;
  const size = sourceSize(base);
  const rect = shapeRect(shape);
  if (!size || !rect) return;
  const region = clampCrop(rect, size.width, size.height);
  if (!region) return;
  const block = mosaicBlockPx(shape.width);
  const columns = Math.max(1, Math.round(region.w / block));
  const rows = Math.max(1, Math.round(region.h / block));
  const scratch = acquireScratch(columns, rows);
  if (!scratch) return;
  scratch.imageSmoothingEnabled = false;
  scratch.clearRect(0, 0, columns, rows);
  scratch.drawImage(base, region.x, region.y, region.w, region.h, 0, 0, columns, rows);
  const previousSmoothing = ctx.imageSmoothingEnabled;
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(
    scratch.canvas,
    0,
    0,
    columns,
    rows,
    region.x * scale,
    region.y * scale,
    region.w * scale,
    region.h * scale,
  );
  ctx.imageSmoothingEnabled = previousSmoothing;
}

/**
 * Whether the canvas 2D context supports `filter` (Chromium/WebView2 do).
 *
 * Detection is a real assignment round-trip because an unsupported engine silently ignores the
 * write, which would otherwise leave the region unpainted instead of falling back to mosaic.
 */
function supportsCanvasFilter(ctx: CanvasRenderingContext2D): boolean {
  const candidate = ctx as CanvasRenderingContext2D & { filter?: unknown };
  if (typeof candidate.filter !== "string") return false;
  const previous = candidate.filter;
  try {
    candidate.filter = "blur(1px)";
    const supported = candidate.filter === "blur(1px)";
    candidate.filter = previous;
    return supported;
  } catch {
    return false;
  }
}

/**
 * Paints a blurred rectangle (W1.3 `blur`): clip to the rectangle and redraw the base image
 * through `ctx.filter = "blur(Npx)"`. The clip means the blur only replaces the selected region
 * (it samples neighbouring pixels, so edges stay soft).
 *
 * Fallback: engines without `CanvasRenderingContext2D.filter` get the mosaic implementation
 * instead — same rectangle, visibly derived from the base image, never a hole in the composition.
 */
function renderBlur(
  ctx: CanvasRenderingContext2D,
  shape: Shape,
  scale: number,
  base: CanvasImageSource | undefined,
): void {
  if (!base) return;
  const size = sourceSize(base);
  const rect = shapeRect(shape);
  if (!size || !rect) return;
  const region = clampCrop(rect, size.width, size.height);
  if (!region) return;
  if (!supportsCanvasFilter(ctx)) {
    renderMosaic(ctx, shape, scale, base);
    return;
  }
  ctx.save();
  ctx.beginPath();
  ctx.rect(region.x * scale, region.y * scale, region.w * scale, region.h * scale);
  ctx.clip();
  ctx.filter = `blur(${blurRadiusPx(shape.width, scale)}px)`;
  ctx.drawImage(base, 0, 0, size.width * scale, size.height * scale);
  ctx.restore();
}

/** Paints a numbered badge (W1.3 `number`): filled circle + centred value. */
function renderNumber(ctx: CanvasRenderingContext2D, shape: Shape, scale: number): void {
  const anchor = shape.points[0];
  const label = (shape.label ?? "").trim();
  // A number without its value is not renderable (only possible with corrupt/legacy state).
  if (!anchor || !label) return;
  const radius = numberRadiusPx(shape.width) * scale;
  if (radius < 1) return;
  ctx.beginPath();
  ctx.arc(anchor[0] * scale, anchor[1] * scale, radius, 0, Math.PI * 2);
  ctx.fillStyle = shape.color;
  ctx.fill();
  ctx.fillStyle = numberTextColor(shape.color);
  ctx.font = `600 ${Math.round(radius * 1.15)}px "Segoe UI", system-ui, sans-serif`;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(label, anchor[0] * scale, anchor[1] * scale);
}

/**
 * Draws every committed shape at `scale` (1 = natural image pixels, used for saving).
 *
 * `base` is the source image for the pixel tools (`mosaic`/`blur`). Both call sites pass it — the
 * on-screen preview canvas and the natural-resolution offscreen canvas used for the saved PNG — so
 * the two paths composite identically. Without a base those two tools paint nothing.
 *
 * Each shape is painted inside its own `save`/`restore` pair: the tools change shared context state
 * (`globalAlpha`, `filter`, `textAlign`, `textBaseline`, smoothing) and one tool must never leak
 * state into the next.
 */
export function renderShapes(
  ctx: CanvasRenderingContext2D,
  shapes: readonly Shape[],
  scale: number,
  base?: CanvasImageSource,
): void {
  if (!Number.isFinite(scale) || scale <= 0) return;
  for (const shape of shapes) {
    ctx.save();
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
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
      case "highlight":
        renderHighlight(ctx, shape, scale);
        break;
      case "mosaic":
        renderMosaic(ctx, shape, scale, base);
        break;
      case "blur":
        renderBlur(ctx, shape, scale, base);
        break;
      case "number":
        renderNumber(ctx, shape, scale);
        break;
    }
    ctx.restore();
  }
}
