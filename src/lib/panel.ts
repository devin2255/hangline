/**
 * Panel-overlay geometry and timing for the Wave 2.4 hover preview.
 *
 * The preview is a **DOM overlay inside the panel window** (never a second window): it is
 * absolutely positioned in panel-client CSS pixels, `pointer-events: none`, and deliberately not
 * part of the click-through hit-rect report (the pointer stays on the hovered card while the
 * preview is visible).
 *
 * Everything here is pure: the 300 ms dwell rule, the "can a preview be shown at all" visibility
 * predicate, and the geometry that fits the original image into the fixed-height panel
 * (189/210/246 dp) with a bottom-aligned, card-centred, clamped box. The component only measures
 * the card rectangle and feeds these helpers.
 */

/** How long the pointer must rest on a card before the preview starts loading (ms). */
export const PREVIEW_HOVER_DELAY_MS = 300;

/** Maximum preview width in dp (the panel is wide, but the preview must stay a peek). */
export const PREVIEW_MAX_WIDTH = 520;

/** Gap kept between the preview and the panel edges (top/bottom/left/right), in dp. */
export const PREVIEW_EDGE_MARGIN = 8;

/**
 * Total vertical space reserved above + below the preview (`2 × PREVIEW_EDGE_MARGIN`), i.e. the
 * maximum preview height is `panelHeight − 16 dp`.
 */
export const PREVIEW_HEIGHT_INSET = PREVIEW_EDGE_MARGIN * 2;

/** A rectangle in panel-client CSS pixels. */
export interface PanelBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A size in CSS/physical pixels (only the ratio matters for the geometry). */
export interface Size {
  w: number;
  h: number;
}

/** Resolved on-screen box for the preview overlay (panel-client CSS pixels). */
export interface PreviewGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Raw `getBoundingClientRect()` shape, for the client → panel-local conversion. */
export interface ClientBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

function isPositive(value: number): boolean {
  return Number.isFinite(value) && value > 0;
}

/**
 * Converts a client rectangle into panel-client coordinates.
 *
 * The panel root is translated by the reveal/tuck spring, so `getBoundingClientRect()` includes
 * that translate; the preview is positioned in the panel's *untransformed* client space, exactly
 * like the hit rects (`collectHitRects` subtracts the same offset).
 */
export function panelLocalBox(rect: ClientBox, revealOffsetY: number): PanelBox {
  const offsetY = Number.isFinite(revealOffsetY) ? revealOffsetY : 0;
  return {
    x: Number.isFinite(rect.left) ? rect.left : 0,
    y: (Number.isFinite(rect.top) ? rect.top : 0) - offsetY,
    w: isPositive(rect.width) ? rect.width : 0,
    h: isPositive(rect.height) ? rect.height : 0,
  };
}

/** Inputs of the preview geometry rule. */
export interface PreviewGeometryInput {
  /** Hovered card box in panel-client CSS pixels (from {@link panelLocalBox}). */
  card: PanelBox;
  /** Original image size in pixels (`Item.width`/`Item.height`). */
  image: Size;
  /** Panel window size in CSS pixels (`window.innerWidth`/`innerHeight`). */
  panel: Size;
  /** Recordings never get a large preview (no still frame to show). */
  recording?: boolean;
}

/**
 * Computes the preview box:
 *
 * - scale = `min(1, maxWidth/imageWidth, maxHeight/imageHeight)` — never upscale a small original,
 *   and `maxHeight = panelHeight − 16 dp`, `maxWidth = min(520, panelWidth − 16)`;
 * - horizontally centred on the hovered card, then clamped into the panel;
 * - **bottom-aligned** with the panel's lower edge (a constant `PREVIEW_EDGE_MARGIN` above it).
 *
 * Returns `null` when the preview cannot exist (recording item, unknown/zero image size, invalid
 * panel/card numbers); the caller then keeps the panel unchanged.
 */
export function previewGeometryFor(input: PreviewGeometryInput): PreviewGeometry | null {
  const { card, image, panel, recording = false } = input;
  if (recording) return null;
  if (!isPositive(image.w) || !isPositive(image.h)) return null;
  if (!isPositive(panel.w) || !isPositive(panel.h)) return null;
  if (
    !Number.isFinite(card.x) ||
    !Number.isFinite(card.y) ||
    !Number.isFinite(card.w) ||
    !Number.isFinite(card.h)
  ) {
    return null;
  }

  const maxWidth = Math.max(1, Math.min(PREVIEW_MAX_WIDTH, panel.w - PREVIEW_HEIGHT_INSET));
  const maxHeight = Math.max(1, panel.h - PREVIEW_HEIGHT_INSET);
  const scale = Math.min(1, maxWidth / image.w, maxHeight / image.h);
  if (!Number.isFinite(scale) || scale <= 0) return null;

  const width = Math.max(1, Math.round(image.w * scale));
  const height = Math.max(1, Math.round(image.h * scale));
  const centerX = card.x + card.w / 2;
  const minX = PREVIEW_EDGE_MARGIN;
  const maxX = Math.max(minX, panel.w - width - PREVIEW_EDGE_MARGIN);
  const x = Math.round(Math.min(Math.max(centerX - width / 2, minX), maxX));
  const y = Math.round(Math.max(PREVIEW_EDGE_MARGIN, panel.h - height - PREVIEW_EDGE_MARGIN));
  return { x, y, width, height };
}

/** Inputs that decide whether the 300 ms dwell timer may start on a card. */
export interface PreviewTimerInput {
  /** The pointer is inside the card (or its hover capsule). */
  hovering: boolean;
  /** Recordings have no still frame to enlarge. */
  recording: boolean;
  /** A card menu (or its dismiss guard) is open: gestures and preview are suspended. */
  locked: boolean;
  /** The panel is tucked/hidden or frozen by an open editor. */
  paused: boolean;
  /** A native drag-out owns the pointer. */
  dragging: boolean;
}

/**
 * Whether a hover may arm the 300 ms preview timer.
 *
 * The timer is armed once per enter; mousemove inside the card never restarts it (no per-frame
 * state churn). The preview request itself re-validates the same conditions when the timer fires,
 * so a menu/drag that starts inside the 300 ms window wins.
 */
export function shouldStartPreviewTimer(input: PreviewTimerInput): boolean {
  return (
    input.hovering && !input.recording && !input.locked && !input.paused && !input.dragging
  );
}

/** Inputs that can invalidate an already-visible preview. */
export interface PreviewVisibilityInput {
  /** A card context menu is open: the menu must stay unobstructed. */
  menuOpen: boolean;
  /** A native drag-out is in flight. */
  dragging: boolean;
  /** The line is revealed (`data-revealed`); a tuck hides the whole panel. */
  panelRevealed: boolean;
  /** The window is hidden (`document.hidden`). */
  panelHidden: boolean;
  /** Recordings never preview. */
  recording: boolean;
}

/** Whether an already-visible preview may stay on screen (the panel re-checks this on changes). */
export function previewIsAllowed(input: PreviewVisibilityInput): boolean {
  return (
    !input.menuOpen &&
    !input.dragging &&
    input.panelRevealed &&
    !input.panelHidden &&
    !input.recording
  );
}
