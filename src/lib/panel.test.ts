/**
 * Pure hover-preview rules for Wave 2.4.
 *
 * The preview overlay itself is DOM-only, but the rules that can be wrong invisibly are pure: the
 * 300 ms dwell gate, the "is the preview still allowed" predicate (menu/drag/tuck/hidden), and the
 * geometry that fits the original image into the fixed-height panel bottom-aligned and clamped.
 */

import { describe, expect, it } from "vitest";
import {
  PREVIEW_EDGE_MARGIN,
  PREVIEW_HEIGHT_INSET,
  PREVIEW_HOVER_DELAY_MS,
  PREVIEW_MAX_WIDTH,
  panelLocalBox,
  previewGeometryFor,
  previewIsAllowed,
  shouldStartPreviewTimer,
} from "./panel";

const PANEL = { w: 1920, h: 210 };
/** A medium card's visual box, centred on a 1920 dp panel. */
const CENTERED_CARD = { x: 885, y: 14, w: 150, h: 112 };

describe("preview constants", () => {
  it("uses the Wave 2.4 dwell, width cap and 16 dp height inset", () => {
    expect(PREVIEW_HOVER_DELAY_MS).toBe(300);
    expect(PREVIEW_MAX_WIDTH).toBe(520);
    expect(PREVIEW_EDGE_MARGIN).toBe(8);
    expect(PREVIEW_HEIGHT_INSET).toBe(16);
  });
});

describe("previewGeometryFor", () => {
  it("fits a landscape image by the panel height, bottom-aligned and card-centred", () => {
    const geometry = previewGeometryFor({
      card: CENTERED_CARD,
      image: { w: 1920, h: 1080 },
      panel: PANEL,
    });
    expect(geometry).toEqual({ x: 788, y: 8, width: 345, height: 194 });
    // Bottom aligned: 8 dp above the panel's lower edge.
    expect((geometry?.y ?? 0) + (geometry?.height ?? 0)).toBe(PANEL.h - PREVIEW_EDGE_MARGIN);
  });

  it("fits a portrait image by the panel height too and centres it on the card", () => {
    const geometry = previewGeometryFor({
      card: { x: 0, y: 14, w: 150, h: 112 },
      image: { w: 600, h: 1200 },
      panel: PANEL,
    });
    expect(geometry).toEqual({ x: 27, y: 8, width: 97, height: 194 });
  });

  it("caps the width at 520 dp on a wide, short image", () => {
    const geometry = previewGeometryFor({
      card: CENTERED_CARD,
      image: { w: 4000, h: 1000 },
      panel: { w: 1920, h: 246 },
    });
    expect(geometry).toEqual({ x: 700, y: 108, width: 520, height: 130 });
  });

  it("never upscales an image that is smaller than the preview budget", () => {
    const geometry = previewGeometryFor({
      card: CENTERED_CARD,
      image: { w: 200, h: 100 },
      panel: PANEL,
    });
    expect(geometry).toEqual({ x: 860, y: 102, width: 200, height: 100 });
  });

  it("clamps the box inside the panel near both edges", () => {
    const left = previewGeometryFor({
      card: { x: 0, y: 14, w: 150, h: 112 },
      image: { w: 1000, h: 500 },
      panel: PANEL,
    });
    expect(left?.x).toBe(PREVIEW_EDGE_MARGIN);
    const right = previewGeometryFor({
      card: { x: 1770, y: 14, w: 150, h: 112 },
      image: { w: 1000, h: 500 },
      panel: PANEL,
    });
    expect(right?.x).toBe(PANEL.w - (right?.width ?? 0) - PREVIEW_EDGE_MARGIN);
    // Both stay fully inside the panel.
    for (const geometry of [left, right]) {
      expect(geometry).not.toBeNull();
      expect((geometry?.x ?? 0) + (geometry?.width ?? 0)).toBeLessThanOrEqual(
        PANEL.w - PREVIEW_EDGE_MARGIN,
      );
    }
  });

  it("keeps the height within the panel height minus 16 dp for every aspect ratio", () => {
    for (const image of [
      { w: 100_000, h: 1 },
      { w: 1, h: 100_000 },
      { w: 12000, h: 12000 },
      { w: 4, h: 3 },
    ]) {
      const geometry = previewGeometryFor({ card: CENTERED_CARD, image, panel: PANEL });
      expect(geometry).not.toBeNull();
      expect(geometry?.height ?? 0).toBeLessThanOrEqual(PANEL.h - PREVIEW_HEIGHT_INSET);
      expect(geometry?.width ?? 0).toBeLessThanOrEqual(PREVIEW_MAX_WIDTH);
      expect(geometry?.width ?? 0).toBeGreaterThanOrEqual(1);
      expect(geometry?.height ?? 0).toBeGreaterThanOrEqual(1);
    }
  });

  it("returns null for recordings, unknown image sizes and malformed panel/card numbers", () => {
    expect(
      previewGeometryFor({
        card: CENTERED_CARD,
        image: { w: 800, h: 600 },
        panel: PANEL,
        recording: true,
      }),
    ).toBeNull();
    expect(
      previewGeometryFor({ card: CENTERED_CARD, image: { w: 0, h: 600 }, panel: PANEL }),
    ).toBeNull();
    expect(
      previewGeometryFor({ card: CENTERED_CARD, image: { w: 800, h: 600 }, panel: { w: 0, h: 0 } }),
    ).toBeNull();
    expect(
      previewGeometryFor({
        card: { x: Number.NaN, y: 0, w: 150, h: 100 },
        image: { w: 800, h: 600 },
        panel: PANEL,
      }),
    ).toBeNull();
  });
});

describe("panelLocalBox", () => {
  it("subtracts the reveal/tuck translate from the client rect", () => {
    expect(panelLocalBox({ left: 10, top: 40, width: 150, height: 100 }, -25)).toEqual({
      x: 10,
      y: 65,
      w: 150,
      h: 100,
    });
    expect(panelLocalBox({ left: 10, top: 40, width: 150, height: 100 }, 0)).toEqual({
      x: 10,
      y: 40,
      w: 150,
      h: 100,
    });
  });

  it("survives malformed numbers without emitting NaN", () => {
    const box = panelLocalBox(
      { left: Number.NaN, top: 40, width: -1, height: 0 },
      Number.NaN,
    );
    expect(box).toEqual({ x: 0, y: 40, w: 0, h: 0 });
  });
});

describe("shouldStartPreviewTimer", () => {
  it("arms on an idle hover of an image", () => {
    expect(
      shouldStartPreviewTimer({
        hovering: true,
        recording: false,
        locked: false,
        paused: false,
        dragging: false,
      }),
    ).toBe(true);
  });

  it("never arms for recordings, an open menu, a paused panel or a drag", () => {
    const base = {
      hovering: true,
      recording: false,
      locked: false,
      paused: false,
      dragging: false,
    };
    expect(shouldStartPreviewTimer({ ...base, hovering: false })).toBe(false);
    expect(shouldStartPreviewTimer({ ...base, recording: true })).toBe(false);
    expect(shouldStartPreviewTimer({ ...base, locked: true })).toBe(false);
    expect(shouldStartPreviewTimer({ ...base, paused: true })).toBe(false);
    expect(shouldStartPreviewTimer({ ...base, dragging: true })).toBe(false);
  });
});

describe("previewIsAllowed", () => {
  it("keeps a revealed image preview while nothing else is going on", () => {
    expect(
      previewIsAllowed({
        menuOpen: false,
        dragging: false,
        panelRevealed: true,
        panelHidden: false,
        recording: false,
      }),
    ).toBe(true);
  });

  it("hides the preview for a menu, a drag, a tuck, a hidden window or a recording", () => {
    const base = {
      menuOpen: false,
      dragging: false,
      panelRevealed: true,
      panelHidden: false,
      recording: false,
    };
    expect(previewIsAllowed({ ...base, menuOpen: true })).toBe(false);
    expect(previewIsAllowed({ ...base, dragging: true })).toBe(false);
    expect(previewIsAllowed({ ...base, panelRevealed: false })).toBe(false);
    expect(previewIsAllowed({ ...base, panelHidden: true })).toBe(false);
    expect(previewIsAllowed({ ...base, recording: true })).toBe(false);
  });
});
