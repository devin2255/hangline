/**
 * Tests for the pure card-menu model (Wave 1.1 / W1.2).
 *
 * These lock the rules that the panel and the Rust hit-test rely on: which rows each item kind
 * gets, when the side take-downs are disabled, the compact height budget, the panel clamping, and
 * the interactive-rectangle union (cards clipped to the viewport; capsule/menu added as clickable
 * islands). The menu itself is rendered by `CardContextMenu`, but every decision that can be wrong
 * in a hard-to-see way lives here.
 */

import { describe, expect, it } from "vitest";
import type { Item, ItemKind } from "./types";
import {
  CARD_MENU_EDGE_MARGIN,
  CARD_MENU_PADDING_Y,
  CARD_MENU_ROW_HEIGHT,
  CARD_MENU_SEPARATOR_HEIGHT,
  CARD_MENU_WIDTH,
  canRecycleOnTakeDown,
  clampMenuPosition,
  flattenMenuItems,
  interactiveRects,
  isRecordingItem,
  menuHeightFor,
  menuItemsFor,
  menuNodeHeight,
  menuOuterHeightFor,
  openCardMenu,
  setSubmenuOpen,
  shouldSuppressCardGesture,
  submenuHeightFor,
  submenuLeftFor,
  submenuRowTop,
  submenuTopFor,
  toPhysicalRect,
  type MenuNode,
  type MenuSubmenuNode,
} from "./cardMenu";

function makeItem(kind: ItemKind, id = "item-1"): Item {
  return {
    id,
    path: `C:\\shots\\${id}.png`,
    kind,
    hash: 1,
    width: 800,
    height: 600,
    createdAt: 1,
    addedAt: 1,
    note: "",
    tags: [],
  };
}

function itemNodes(nodes: MenuNode[]) {
  return flattenMenuItems(nodes);
}

function submenuOf(nodes: MenuNode[]): MenuSubmenuNode {
  const found = nodes.find((node): node is MenuSubmenuNode => node.kind === "submenu");
  if (!found) throw new Error("expected a take-down submenu");
  return found;
}

describe("menuItemsFor", () => {
  it("gives an owned screenshot the full five-group menu with recycle/remove/side/search rows", () => {
    const nodes = menuItemsFor(makeItem("capture"), { index: 1, total: 3 });
    const actions = itemNodes(nodes).map((node) => node.action);

    expect(actions).toEqual([
      "copyImage",
      "copyAsFile",
      "open",
      "showInExplorer",
      "markup",
      "takeDownRecycle",
      "takeDownRemove",
      "takeDownLeft",
      "takeDownRight",
      "search",
    ]);
    // Four group separators on the top level; the submenu adds its own before the side rows.
    expect(nodes.filter((node) => node.kind === "separator")).toHaveLength(4);
    const submenu = submenuOf(nodes);
    expect(submenu.labelKey).toBe("Take down");
    expect(submenu.items.filter((node) => node.kind === "item").map((n) => n.action)).toEqual([
      "takeDownRecycle",
      "takeDownRemove",
      "takeDownLeft",
      "takeDownRight",
    ]);
  });

  it("gives a recording copy-as-file, no markup, trim, the non-deleting take-downs and search", () => {
    const nodes = menuItemsFor(makeItem("recording"), { index: 1, total: 3 });
    const actions = itemNodes(nodes).map((node) => node.action);

    expect(actions).toEqual([
      "copyAsFile",
      "open",
      "showInExplorer",
      "trim",
      "takeDownRemove",
      "takeDownLeft",
      "takeDownRight",
      "search",
    ]);
    expect(actions).not.toContain("copyImage");
    expect(actions).not.toContain("markup");
    expect(actions).not.toContain("takeDownRecycle");
  });

  it("keeps the global Search row for every item kind", () => {
    for (const kind of ["capture", "recording", "watchedFolder", "clipboard", "systemScreenshot"] as const) {
      const actions = itemNodes(menuItemsFor(makeItem(kind), { index: 0, total: 1 })).map(
        (node) => node.action,
      );
      expect(actions).toContain("search");
    }
  });

  it("never offers the recycle bin for watched folders, clipboard items or system screenshots", () => {
    for (const kind of ["watchedFolder", "clipboard", "systemScreenshot"] as const) {
      const item = makeItem(kind);
      expect(canRecycleOnTakeDown(item)).toBe(false);
      const actions = itemNodes(menuItemsFor(item, { index: 0, total: 1 })).map((n) => n.action);
      expect(actions).toContain("takeDownRemove");
      expect(actions).not.toContain("takeDownRecycle");
      // Non-recording kinds keep the still-image rows.
      expect(actions).toContain("copyImage");
      expect(actions).toContain("markup");
    }
    expect(canRecycleOnTakeDown(makeItem("capture"))).toBe(true);
    expect(isRecordingItem(makeItem("recording"))).toBe(true);
    expect(isRecordingItem(makeItem("capture"))).toBe(false);
  });

  it("disables both side take-downs on a single-card line and explains why", () => {
    const nodes = menuItemsFor(makeItem("capture"), { index: 0, total: 1 });
    const sideRows = itemNodes(nodes).filter((node) => node.action.startsWith("takeDown"));
    expect(sideRows.map((node) => node.disabled === true)).toEqual([false, false, true, true]);
    expect(sideRows[2].disabledReasonKey).toBe("Nothing to take down");
    expect(sideRows[3].disabledReasonKey).toBe("Nothing to take down");
  });

  it("disables only the side without a neighbour (first/last card)", () => {
    const first = itemNodes(menuItemsFor(makeItem("capture"), { index: 0, total: 5 }));
    const last = itemNodes(menuItemsFor(makeItem("capture"), { index: 4, total: 5 }));
    const middle = itemNodes(menuItemsFor(makeItem("capture"), { index: 2, total: 5 }));
    const sideState = (rows: typeof first) =>
      rows
        .filter((node) => node.action === "takeDownLeft" || node.action === "takeDownRight")
        .map((node) => ({ disabled: node.disabled, reason: node.disabledReasonKey }));

    expect(sideState(first)).toEqual([
      { disabled: true, reason: "Nothing to take down" },
      { disabled: false, reason: undefined },
    ]);
    expect(sideState(last)).toEqual([
      { disabled: false, reason: undefined },
      { disabled: true, reason: "Nothing to take down" },
    ]);
    expect(sideState(middle)).toEqual([
      { disabled: false, reason: undefined },
      { disabled: false, reason: undefined },
    ]);
  });

  it("survives malformed positions from a desynced snapshot", () => {
    const nodes = menuItemsFor(makeItem("capture"), {
      index: Number.NaN,
      total: Number.POSITIVE_INFINITY,
    });
    const sideRows = itemNodes(nodes).filter(
      (node) => node.action === "takeDownLeft" || node.action === "takeDownRight",
    );
    expect(sideRows[0].disabled).toBe(true);
    expect(sideRows[1].disabled).toBe(true);
  });
});

describe("menu geometry", () => {
  it("keeps every row and compacts separators so the menu fits the 189/210/246 dp panels", () => {
    const full = menuItemsFor(makeItem("capture"), { index: 0, total: 1 });
    const fullRows = full.filter((node) => node.kind !== "separator");
    expect(fullRows).toHaveLength(7); // Wave 2 added the Search row to the Wave 1 six
    expect(full.filter((node) => node.kind === "separator")).toHaveLength(4);
    expect(menuHeightFor(full)).toBe(
      CARD_MENU_PADDING_Y * 2 +
        fullRows.length * CARD_MENU_ROW_HEIGHT +
        4 * CARD_MENU_SEPARATOR_HEIGHT,
    );
    expect(menuHeightFor(full)).toBe(210);
    expect(menuOuterHeightFor(full)).toBe(211);

    // Large panel: the full grouped menu fits, no compaction.
    const large = menuItemsFor(makeItem("capture"), { index: 0, total: 1, panelHeight: 246 });
    expect(large.filter((node) => node.kind === "separator")).toHaveLength(4);
    expect(menuOuterHeightFor(large)).toBeLessThanOrEqual(246);

    // Medium panel: only the last separator (before Search) is dropped.
    const medium = menuItemsFor(makeItem("capture"), { index: 0, total: 1, panelHeight: 210 });
    expect(medium.filter((node) => node.kind === "separator")).toHaveLength(3);
    expect(menuOuterHeightFor(medium)).toBeLessThanOrEqual(210);
    expect(itemNodes(medium)).toHaveLength(itemNodes(full).length);
    expect(itemNodes(medium).map((node) => node.action)).toContain("search");

    // Small panel: every group separator is dropped, but no action is.
    const small = menuItemsFor(makeItem("capture"), { index: 0, total: 1, panelHeight: 189 });
    expect(small.filter((node) => node.kind === "separator")).toHaveLength(0);
    expect(menuOuterHeightFor(small)).toBeLessThanOrEqual(189);
    expect(itemNodes(small).map((node) => node.action)).toEqual(
      itemNodes(full).map((node) => node.action),
    );
  });

  it("keeps the recording menu inside the 189 dp panel without compaction", () => {
    const recording = menuItemsFor(makeItem("recording"), {
      index: 1,
      total: 2,
      panelHeight: 189,
    });
    // Six rows + the four separators are 185 dp outer — the small panel fits them untouched.
    expect(recording.filter((node) => node.kind === "separator")).toHaveLength(4);
    expect(menuOuterHeightFor(recording)).toBeLessThanOrEqual(189);
  });

  it("makes the recording menu shorter (one copy row, no recycle row)", () => {
    const recordingHeight = menuHeightFor(
      menuItemsFor(makeItem("recording"), { index: 1, total: 2 }),
    );
    const captureHeight = menuHeightFor(menuItemsFor(makeItem("capture"), { index: 1, total: 2 }));
    expect(recordingHeight).toBeLessThan(captureHeight);
    expect(recordingHeight).toBe(184);
    expect(captureHeight).toBe(210);
  });

  it("measures separators and submenu rows for the submenu height", () => {
    const submenu = submenuOf(menuItemsFor(makeItem("capture"), { index: 1, total: 2 }));
    expect(submenuHeightFor(submenu)).toBe(
      CARD_MENU_PADDING_Y * 2 + 4 * CARD_MENU_ROW_HEIGHT + CARD_MENU_SEPARATOR_HEIGHT,
    );
    const invalid = itemNodes(menuItemsFor(makeItem("capture"), { index: 0, total: 1 }))[0];
    expect(menuNodeHeight(invalid)).toBe(CARD_MENU_ROW_HEIGHT);
  });

  it("pulls the submenu up so it cannot fall out of the menu's bottom edge", () => {
    const submenu = submenuOf(menuItemsFor(makeItem("capture"), { index: 1, total: 2 }));
    const rowTop = submenuRowTop(menuItemsFor(makeItem("capture"), { index: 1, total: 2 }));
    const menuHeight = menuHeightFor(menuItemsFor(makeItem("capture"), { index: 1, total: 2 }));
    const submenuHeight = submenuHeightFor(submenu);

    expect(rowTop).toBeGreaterThan(0);
    const top = submenuTopFor(rowTop, menuHeight, submenuHeight);
    expect(top).toBeGreaterThanOrEqual(0);
    expect(top + submenuHeight).toBeLessThanOrEqual(menuHeight);
    // A row that already fits keeps its own top.
    expect(submenuTopFor(6, 200, 100)).toBe(6);
  });

  it("clamps the menu fully inside the panel and keeps the 4 dp margin", () => {
    expect(clampMenuPosition(1900, 185, CARD_MENU_WIDTH, 178, 1920, 189)).toEqual({
      x: 1920 - CARD_MENU_WIDTH - CARD_MENU_EDGE_MARGIN,
      y: 189 - 178 - CARD_MENU_EDGE_MARGIN,
    });
    expect(clampMenuPosition(-10, -10, CARD_MENU_WIDTH, 178, 1920, 210)).toEqual({
      x: CARD_MENU_EDGE_MARGIN,
      y: CARD_MENU_EDGE_MARGIN,
    });
    // A panel narrower than the menu pins the menu at the margin instead of going negative.
    expect(clampMenuPosition(50, 50, CARD_MENU_WIDTH, 178, 100, 100).x).toBe(
      CARD_MENU_EDGE_MARGIN,
    );
    // Malformed anchors fall back to the margin instead of NaN.
    const fallback = clampMenuPosition(Number.NaN, Number.NaN, CARD_MENU_WIDTH, 178, 1920, 210);
    expect(Number.isFinite(fallback.x)).toBe(true);
    expect(Number.isFinite(fallback.y)).toBe(true);
  });

  it("opens with the submenu closed and clamps the capsule anchor", () => {
    const state = openCardMenu({
      itemId: "item-1",
      anchorX: 1880,
      anchorY: 170,
      menuHeight: 178,
      panelWidth: 1920,
      panelHeight: 189,
    });
    expect(state).toEqual({
      itemId: "item-1",
      x: 1920 - CARD_MENU_WIDTH - CARD_MENU_EDGE_MARGIN,
      y: 189 - 178 - CARD_MENU_EDGE_MARGIN,
      submenuOpen: false,
    });
  });

  it("keeps the menu's own right-edge clamp from pushing the submenu off-screen", () => {
    // Mid-panel: the submenu expands to the right of the menu.
    expect(submenuLeftFor(100, 1920)).toBeGreaterThan(0);
    // The menu is clamped near the right edge (x = 1920 - 176 - 4) → the submenu flips left and
    // stays inside the panel.
    const clampedX = 1920 - CARD_MENU_WIDTH - CARD_MENU_EDGE_MARGIN;
    const flipped = submenuLeftFor(clampedX, 1920);
    expect(flipped).toBeLessThan(0);
    expect(clampedX + flipped).toBeGreaterThanOrEqual(CARD_MENU_EDGE_MARGIN);
    // A degenerate panel too narrow for either side keeps the design's right expansion.
    expect(submenuLeftFor(4, 100)).toBeGreaterThan(0);
  });

  it("shrinks the edge margin when the compact menu nearly fills the panel", () => {
    // 187 dp menu in a 189 dp panel: only 2 dp spare, so the symmetric margin is 1 (not 4).
    const tight = openCardMenu({
      itemId: "item-1",
      anchorX: 10,
      anchorY: 10,
      menuHeight: 187,
      panelWidth: 1920,
      panelHeight: 189,
    });
    expect(tight.y).toBe(1);
    // An exact fit uses margin 0 instead of pushing the menu off the panel edge.
    const exact = openCardMenu({
      itemId: "item-1",
      anchorX: 10,
      anchorY: 10,
      menuHeight: 189,
      panelWidth: 1920,
      panelHeight: 189,
    });
    expect(exact.y).toBe(0);
    expect(exact.y + 189).toBeLessThanOrEqual(189);
  });

  it("toggles the submenu without mutating the closed state object", () => {
    const state = openCardMenu({
      itemId: "item-1",
      anchorX: 10,
      anchorY: 10,
      menuHeight: 178,
      panelWidth: 1920,
      panelHeight: 210,
    });
    const opened = setSubmenuOpen(state, true);
    expect(opened?.submenuOpen).toBe(true);
    expect(state.submenuOpen).toBe(false);
    expect(setSubmenuOpen(opened, true)).toBe(opened);
    expect(setSubmenuOpen(opened, false)?.submenuOpen).toBe(false);
    expect(setSubmenuOpen(null, true)).toBeNull();
  });

  it("reports that card gestures must be suppressed exactly while a menu is open", () => {
    const closed = openCardMenu({
      itemId: "item-1",
      anchorX: 10,
      anchorY: 10,
      menuHeight: 178,
      panelWidth: 1920,
      panelHeight: 210,
    });
    expect(shouldSuppressCardGesture(null)).toBe(false);
    expect(shouldSuppressCardGesture(closed)).toBe(true);
  });
});

describe("interactiveRects", () => {
  it("converts boxes to physical pixels with the DPR scale and the reveal offset", () => {
    const rects = interactiveRects({
      cards: [{ x: 100, y: 20, w: 150, h: 100 }],
      viewport: { x: 0, y: 0, w: 1920, h: 210 },
      overlays: [],
      scale: 1.5,
      offsetY: -10,
    });
    expect(rects).toEqual([{ x: 150, y: 45, w: 225, h: 150 }]);
    expect(toPhysicalRect({ x: 10.4, y: 10.6, w: 20, h: 20 }, 1, 0)).toEqual({
      x: 10,
      y: 11,
      w: 20,
      h: 20,
    });
  });

  it("clips cards to the track viewport and drops cards fully outside it", () => {
    const rects = interactiveRects({
      cards: [
        { x: -40, y: 0, w: 100, h: 50 }, // clipped at the left edge
        { x: 1900, y: 0, w: 100, h: 50 }, // clipped at the right edge
        { x: 2500, y: 0, w: 100, h: 50 }, // fully outside → dropped
      ],
      viewport: { x: 0, y: 0, w: 1920, h: 210 },
      overlays: [],
      scale: 1,
      offsetY: 0,
    });
    expect(rects).toEqual([
      { x: 0, y: 0, w: 60, h: 50 },
      { x: 1900, y: 0, w: 20, h: 50 },
    ]);
  });

  it("adds the hover capsule and menu as unclipped clickable islands", () => {
    const rects = interactiveRects({
      cards: [{ x: 10, y: 10, w: 20, h: 20 }],
      viewport: { x: 0, y: 0, w: 1920, h: 210 },
      // The capsule hangs below the card (outside the card rect); the menu floats over it.
      overlays: [
        { x: 5, y: 40, w: 40, h: 30 },
        { x: 100, y: 8, w: CARD_MENU_WIDTH, h: 178 },
        null,
        undefined,
      ],
      scale: 1,
      offsetY: 0,
    });
    expect(rects).toHaveLength(3);
    expect(rects[1]).toEqual({ x: 5, y: 40, w: 40, h: 30 });
    expect(rects[2]).toEqual({ x: 100, y: 8, w: CARD_MENU_WIDTH, h: 178 });
  });

  it("drops zero-size boxes so a closed/collapsed overlay cannot swallow the panel", () => {
    const rects = interactiveRects({
      cards: [{ x: 10, y: 10, w: 0, h: 20 }],
      viewport: null,
      overlays: [{ x: 0, y: 0, w: 0, h: 0 }],
      scale: 1,
      offsetY: 0,
    });
    expect(rects).toEqual([]);
  });

  it("falls back to scale 1 for a malformed DPR instead of reporting NaN rects", () => {
    const rects = interactiveRects({
      cards: [{ x: 10, y: 10, w: 20, h: 20 }],
      viewport: null,
      overlays: [],
      scale: Number.NaN,
      offsetY: Number.NaN,
    });
    expect(rects).toEqual([{ x: 10, y: 10, w: 20, h: 20 }]);
    expect(rects.every((rect) => Object.values(rect).every(Number.isFinite))).toBe(true);
  });

  it("flattens only executable rows (separators and the submenu container are not actions)", () => {
    const captureRows = flattenMenuItems(menuItemsFor(makeItem("capture"), { index: 1, total: 3 }));
    const recordingRows = flattenMenuItems(
      menuItemsFor(makeItem("recording"), { index: 1, total: 3 }),
    );
    expect(captureRows.every((row) => row.kind === "item")).toBe(true);
    expect(recordingRows).toHaveLength(8);
    expect(new Set(captureRows.map((row) => row.action)).size).toBe(captureRows.length);
  });
});
