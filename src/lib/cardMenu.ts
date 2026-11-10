/**
 * Card context menu and hover action capsule model (Wave 1.1 / W1.2).
 *
 * The panel window is click-through except for rectangles reported with `set_hit_rects`, and it is
 * only 189/210/246 dp tall (small/medium/large), so the menu cannot be a native/separate window:
 * it is plain DOM rendered inside the panel. This module is the pure half — menu structure per
 * item kind, open/close state, panel clamping, and the union of interactive rectangles the panel
 * reports to Rust. Keeping it DOM-free makes the rules (which rows a recording gets, when the
 * side take-downs are disabled, how the anchor is clamped) unit-testable.
 *
 * Height budget: the Wave 1 four-group menu was 6 rows + 3 separators ≈ 178 dp (fits the 189 dp
 * small panel). Wave 2 appends a fifth group (`Search`), which pushes the full menu to 210 dp — taller
 * than the medium (210 dp) and small (189 dp) panels. [`menuItemsFor`] therefore compacts itself via
 * [`fitMenuToPanel`]: it drops top-level separators from the bottom up until the menu fits the panel
 * passed in `MenuItemsOptions.panelHeight`. All rows are always kept; only decorative group breaks go.
 */

import type { Item, PhysicalRect } from "./types";

/** Stable ids for every executable menu row; the panel maps them to IPC calls. */
export type MenuActionId =
  | "copyImage"
  | "copyAsFile"
  | "open"
  | "showInExplorer"
  | "markup"
  | "trim"
  | "takeDownRecycle"
  | "takeDownRemove"
  | "takeDownLeft"
  | "takeDownRight"
  | "search";

/** One executable row. `disabled` rows stay visible (they explain the state) but do nothing. */
export interface MenuItemNode {
  kind: "item";
  action: MenuActionId;
  /** i18n key (`Copy image`, `Markup`, …); resolved by the component, not by this module. */
  labelKey: string;
  disabled?: boolean;
  /** i18n key shown as the disabled row's tooltip (`Nothing to take down`). */
  disabledReasonKey?: string;
}

export interface MenuSeparatorNode {
  kind: "separator";
}

/** A row inside a submenu: an action or a separator (submenus do not nest). */
export type MenuRowNode = MenuItemNode | MenuSeparatorNode;

/** The only submenu in the card menu: `Take down ▸` (recycle/remove/side take-downs). */
export interface MenuSubmenuNode {
  kind: "submenu";
  id: "takeDown";
  labelKey: string;
  /** Child rows; separators are allowed, nested submenus are not. */
  items: MenuRowNode[];
}

export type MenuNode = MenuItemNode | MenuSeparatorNode | MenuSubmenuNode;

/** An item's position on the whole line; drives "nothing on that side" disabled states. */
export interface MenuItemsOptions {
  /** Index in `AppState.items` (oldest = 0, newest = last). */
  index: number;
  /** Total number of items on the line. */
  total: number;
  /**
   * Panel window height in CSS pixels (`window.innerHeight`), used to compact the menu so it always
   * fits the 189/210/246 dp panel. Omitted → the full menu (tests / callers that clamp themselves).
   */
  panelHeight?: number;
}

/** Compact menu metrics (dp); see the height-budget note in the file comment. */
export const CARD_MENU_WIDTH = 176;
export const CARD_SUBMENU_WIDTH = 156;
/**
 * Left offset of the submenu inside the menu: overlapping the menu edge by 2 dp keeps the pointer
 * path from the `Take down ▸` row into the submenu contiguous (a gap would fire a pointerleave and
 * close the submenu before it can be reached).
 */
export const CARD_SUBMENU_LEFT = CARD_MENU_WIDTH - 2;
export const CARD_MENU_ROW_HEIGHT = 26;
export const CARD_MENU_SEPARATOR_HEIGHT = 6;
export const CARD_MENU_PADDING_Y = 2;
/** CSS border of the menu surface (0.5 dp top + 0.5 dp bottom), part of its outer height. */
export const CARD_MENU_BORDER_HEIGHT = 1;
/** Space kept between the menu and the panel edges when clamping. */
export const CARD_MENU_EDGE_MARGIN = 4;

/** A card the recycle-bin take-down may delete: only Hangline's own screenshots. */
export function canRecycleOnTakeDown(item: Item): boolean {
  return item.kind === "capture";
}

/** Recordings have no still image: their copy row is `Copy as file` and they trim instead of mark up. */
export function isRecordingItem(item: Item): boolean {
  return item.kind === "recording";
}

function row(labelKey: string, action: MenuActionId): MenuItemNode {
  return { kind: "item", action, labelKey };
}

function separator(): MenuSeparatorNode {
  return { kind: "separator" };
}

/** Keeps malformed positions from producing NaN disabled states. */
function normalizePosition(index: number, total: number): { index: number; total: number } {
  const safeIndex = Number.isFinite(index) ? Math.max(0, Math.trunc(index)) : 0;
  const safeTotal = Number.isFinite(total) ? Math.max(0, Math.trunc(total)) : 0;
  return { index: safeIndex, total: safeTotal };
}

/**
 * Builds the five-group card menu for `item` (Wave 1.1 + Wave 2.3):
 *
 * 1. copy — `Copy image` (images only) / `Copy as file`;
 * 2. open — `Open` / `Show in Explorer`;
 * 3. edit — `Markup` for images, `Trim recording` for recordings;
 * 4. take down — submenu with recycle/remove plus side take-downs;
 * 5. search — the global `Search` window (last row, present for every item kind).
 *
 * `Take down to the left/right` removes every card on that side and never deletes files, so it is
 * disabled (not hidden) when the item has no neighbour on that side: the row stays visible so the
 * reason is readable instead of the menu silently missing an entry.
 *
 * When `options.panelHeight` is given the result is compacted by [`fitMenuToPanel`] (separators
 * dropped from the bottom up) so the menu is always fully visible inside the short panel window.
 */
export function menuItemsFor(item: Item, options: MenuItemsOptions): MenuNode[] {
  const recording = isRecordingItem(item);
  const { index, total } = normalizePosition(options.index, options.total);
  const leftDisabled = index <= 0;
  const rightDisabled = index >= total - 1;

  const copyGroup: MenuNode[] = recording
    ? [row("Copy as file", "copyAsFile")]
    : [row("Copy image", "copyImage"), row("Copy as file", "copyAsFile")];
  const openGroup: MenuNode[] = [row("Open", "open"), row("Show in Explorer", "showInExplorer")];
  const editGroup: MenuNode[] = recording
    ? [row("Trim recording", "trim")]
    : [row("Markup", "markup")];

  const takeDownItems: MenuRowNode[] = [];
  if (canRecycleOnTakeDown(item)) {
    takeDownItems.push(row("Move to Recycle Bin", "takeDownRecycle"));
  }
  takeDownItems.push(row("Remove from line", "takeDownRemove"));
  takeDownItems.push(separator());
  takeDownItems.push({
    kind: "item",
    action: "takeDownLeft",
    labelKey: "Take down to the left",
    disabled: leftDisabled,
    disabledReasonKey: leftDisabled ? "Nothing to take down" : undefined,
  });
  takeDownItems.push({
    kind: "item",
    action: "takeDownRight",
    labelKey: "Take down to the right",
    disabled: rightDisabled,
    disabledReasonKey: rightDisabled ? "Nothing to take down" : undefined,
  });

  const nodes: MenuNode[] = [
    ...copyGroup,
    separator(),
    ...openGroup,
    separator(),
    ...editGroup,
    separator(),
    { kind: "submenu", id: "takeDown", labelKey: "Take down", items: takeDownItems },
    separator(),
    row("Search", "search"),
  ];
  return fitMenuToPanel(nodes, options.panelHeight ?? 0);
}

/** Flattens a menu tree (top level + the take-down submenu) into its executable rows. */
export function flattenMenuItems(nodes: readonly MenuNode[]): MenuItemNode[] {
  const result: MenuItemNode[] = [];
  for (const node of nodes) {
    if (node.kind === "item") {
      result.push(node);
    } else if (node.kind === "submenu") {
      for (const child of node.items) {
        if (child.kind === "item") result.push(child);
      }
    }
  }
  return result;
}

/** Height of one node in dp. */
export function menuNodeHeight(node: MenuNode): number {
  return node.kind === "separator" ? CARD_MENU_SEPARATOR_HEIGHT : CARD_MENU_ROW_HEIGHT;
}

/** Total dp height of a menu (rows + separators + vertical padding). */
export function menuHeightFor(nodes: readonly MenuNode[]): number {
  let height = CARD_MENU_PADDING_Y * 2;
  for (const node of nodes) height += menuNodeHeight(node);
  return height;
}

/** Outer dp height of the rendered menu surface (content + the 0.5 dp top/bottom border). */
export function menuOuterHeightFor(nodes: readonly MenuNode[]): number {
  return menuHeightFor(nodes) + CARD_MENU_BORDER_HEIGHT;
}

/** Number of top-level group separators in a menu (submenu separators are not counted). */
function topLevelSeparatorCount(nodes: readonly MenuNode[]): number {
  return nodes.filter((node) => node.kind === "separator").length;
}

/** Returns `nodes` without the last `count` top-level separators (all rows are preserved). */
function withoutTrailingSeparators(nodes: readonly MenuNode[], count: number): MenuNode[] {
  if (count <= 0) return [...nodes];
  const separatorIndexes: number[] = [];
  nodes.forEach((node, index) => {
    if (node.kind === "separator") separatorIndexes.push(index);
  });
  const drop = new Set(separatorIndexes.slice(Math.max(0, separatorIndexes.length - count)));
  return nodes.filter((node, index) => node.kind !== "separator" || !drop.has(index));
}

/**
 * Compacts a menu until its outer height fits the panel window.
 *
 * Wave 2 appends the `Search` group, making the full menu taller than the 210 dp medium and
 * 189 dp small panels. Rather than hiding actions (they are all relevant) the decorative
 * separators are dropped from the bottom up until the menu fits: on medium the `Search` separator
 * goes, on small every group break goes and the menu becomes a single column of rows. The panel's
 * clamp ([`openCardMenu`]) also shrinks its edge margin for an exact fit.
 */
export function fitMenuToPanel(nodes: readonly MenuNode[], panelHeight: number): MenuNode[] {
  if (!Number.isFinite(panelHeight) || panelHeight <= 0) return [...nodes];
  const separatorCount = topLevelSeparatorCount(nodes);
  for (let dropped = 0; dropped <= separatorCount; dropped += 1) {
    const candidate = withoutTrailingSeparators(nodes, dropped);
    if (menuOuterHeightFor(candidate) <= panelHeight) return candidate;
  }
  return withoutTrailingSeparators(nodes, separatorCount);
}

/** Total dp height of the take-down submenu. */
export function submenuHeightFor(submenu: MenuSubmenuNode): number {
  return menuHeightFor(submenu.items);
}

/**
 * Top of the submenu inside the menu box.
 *
 * The menu is already clamped inside the panel, and the take-down row sits at the bottom (row top
 * ≈ 150 in a 178 dp menu), so a submenu anchored at the row top would fall out of the panel. It is
 * therefore pulled up until its bottom aligns with the menu's bottom, which keeps every submenu row
 * reachable without another window or a scroll model.
 */
export function submenuTopFor(rowTop: number, menuHeight: number, submenuHeight: number): number {
  const maxTop = menuHeight - submenuHeight;
  if (!Number.isFinite(rowTop) || !Number.isFinite(maxTop)) return 0;
  return Math.max(0, Math.min(Math.trunc(rowTop), Math.trunc(maxTop)));
}

/** dp offset of the submenu trigger row from the menu's top edge. */
export function submenuRowTop(nodes: readonly MenuNode[]): number {
  let top = CARD_MENU_PADDING_Y;
  for (const node of nodes) {
    if (node.kind === "submenu") return top;
    top += menuNodeHeight(node);
  }
  return top;
}

/**
 * Clamps the menu's top-left corner to the panel so it is fully visible.
 *
 * `anchorX/anchorY` are panel-client CSS pixels (the pointer position or the capsule button). The
 * menu opens to the right/below the anchor and is pulled back inside the panel near its right or
 * bottom edge.
 */
export function clampMenuPosition(
  anchorX: number,
  anchorY: number,
  menuWidth: number,
  menuHeight: number,
  panelWidth: number,
  panelHeight: number,
  margin: number = CARD_MENU_EDGE_MARGIN,
): { x: number; y: number } {
  const safeMargin = Number.isFinite(margin) ? Math.max(0, margin) : CARD_MENU_EDGE_MARGIN;
  const safeWidth = Number.isFinite(menuWidth) ? Math.max(1, menuWidth) : CARD_MENU_WIDTH;
  const safeHeight = Number.isFinite(menuHeight) ? Math.max(1, menuHeight) : CARD_MENU_ROW_HEIGHT;
  const safePanelWidth = Number.isFinite(panelWidth) ? Math.max(1, panelWidth) : safeWidth;
  const safePanelHeight = Number.isFinite(panelHeight) ? Math.max(1, panelHeight) : safeHeight;
  const maxX = Math.max(safeMargin, safePanelWidth - safeWidth - safeMargin);
  const maxY = Math.max(safeMargin, safePanelHeight - safeHeight - safeMargin);
  const x = Number.isFinite(anchorX) ? anchorX : safeMargin;
  const y = Number.isFinite(anchorY) ? anchorY : safeMargin;
  return {
    x: Math.round(Math.min(Math.max(x, safeMargin), maxX)),
    y: Math.round(Math.min(Math.max(y, safeMargin), maxY)),
  };
}

/**
 * Horizontal offset of the submenu inside the menu (dp).
 *
 * Normally the submenu opens to the right (`CARD_SUBMENU_LEFT`). The menu itself is clamped to the
 * panel's right edge, so a right-click on a card near that edge would push the submenu off-screen;
 * in that case it flips to the left. On a panel too narrow for either side the right expansion is
 * kept (the submenu is clamped by the panel's overflow, and the panel is at least 3 cards wide, so
 * this only happens in a degenerate viewport).
 */
export function submenuLeftFor(
  menuX: number,
  panelWidth: number,
  margin: number = CARD_MENU_EDGE_MARGIN,
): number {
  const rightEdge = menuX + CARD_SUBMENU_LEFT + CARD_SUBMENU_WIDTH + margin;
  if (rightEdge <= panelWidth) return CARD_SUBMENU_LEFT;
  const leftOffset = -(CARD_SUBMENU_WIDTH - 2);
  if (menuX + leftOffset >= margin) return leftOffset;
  return CARD_SUBMENU_LEFT;
}

/** Open card menu: which card it belongs to, where it renders, and whether Take down ▸ is open. */
export interface CardMenuState {
  itemId: string;
  /** Menu top-left in panel-client CSS pixels. */
  x: number;
  y: number;
  submenuOpen: boolean;
}

export interface OpenCardMenuInput {
  itemId: string;
  /** Anchor in panel-client CSS pixels (pointer position, or the capsule button's corner). */
  anchorX: number;
  anchorY: number;
  /** Menu height in dp, from {@link menuHeightFor}. */
  menuHeight: number;
  /** Panel size in CSS pixels (`window.innerWidth`/`innerHeight`). */
  panelWidth: number;
  panelHeight: number;
}

/**
 * Creates the open state for `itemId`, clamped inside the panel with the submenu closed.
 *
 * The edge margin shrinks when the menu nearly fills the panel (the Wave 2 compact menu can be an
 * exact fit on the smallest panel): the largest symmetric margin that still keeps the menu fully
 * inside is used, down to 0. The caller must pass the menu's **outer** height
 * ([`menuOuterHeightFor`]) for this to be exact.
 */
export function openCardMenu(input: OpenCardMenuInput): CardMenuState {
  const spare = Number.isFinite(input.panelHeight)
    ? input.panelHeight - input.menuHeight
    : Number.POSITIVE_INFINITY;
  const maxMargin = Number.isFinite(spare)
    ? Math.floor(spare / 2)
    : CARD_MENU_EDGE_MARGIN;
  const margin = Math.max(0, Math.min(CARD_MENU_EDGE_MARGIN, maxMargin));
  const position = clampMenuPosition(
    input.anchorX,
    input.anchorY,
    CARD_MENU_WIDTH,
    input.menuHeight,
    input.panelWidth,
    input.panelHeight,
    margin,
  );
  return {
    itemId: input.itemId,
    x: position.x,
    y: position.y,
    submenuOpen: false,
  };
}

/** Opens/closes the `Take down ▸` submenu; keeps the same state object when nothing changes. */
export function setSubmenuOpen(
  state: CardMenuState | null,
  open: boolean,
): CardMenuState | null {
  if (!state) return state;
  if (state.submenuOpen === open) return state;
  return { ...state, submenuOpen: open };
}

/** True while a menu is open: card click/double-click/long-press/drag must be suppressed. */
export function shouldSuppressCardGesture(state: CardMenuState | null): boolean {
  return state !== null;
}

/** A rectangle in panel-client CSS pixels (raw `getBoundingClientRect()` numbers). */
export interface CssBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Converts one CSS box to the physical panel-client rectangle Rust's hit-test thread uses. */
export function toPhysicalRect(box: CssBox, scale: number, offsetY: number): PhysicalRect {
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
  const safeOffset = Number.isFinite(offsetY) ? offsetY : 0;
  return {
    x: Math.round(box.x * safeScale),
    y: Math.round((box.y - safeOffset) * safeScale),
    w: Math.round(box.w * safeScale),
    h: Math.round(box.h * safeScale),
  };
}

export interface InteractiveRectsInput {
  /** Card boxes in panel-client CSS pixels; clipped to `viewport` when provided. */
  cards: readonly CssBox[];
  /** The card track viewport (scrolled/overhanging cards must not block clicks). */
  viewport: CssBox | null;
  /** Overlays measured from the DOM: hover capsule, menu, submenu, update notice, … */
  overlays: ReadonlyArray<CssBox | null | undefined>;
  scale: number;
  /** Current reveal/tuck translateY (the reported rects are in untransformed client pixels). */
  offsetY: number;
}

/**
 * Builds the physical hit-rect list the panel reports to Rust.
 *
 * Cards are clipped to the track viewport (a card outside it must not catch clicks); overlays are
 * not clipped because they are already clamped/positioned inside the panel and must stay clickable
 * islands (the hover capsule hangs below a card, and the menu floats over cards). Empty/zero-size
 * boxes are dropped: `set_hit_rects` with a stale empty rect would swallow the whole panel.
 */
export function interactiveRects(input: InteractiveRectsInput): PhysicalRect[] {
  const { cards, viewport, overlays, scale, offsetY } = input;
  const rects: PhysicalRect[] = [];
  const push = (box: CssBox) => {
    if (!Number.isFinite(box.w) || !Number.isFinite(box.h)) return;
    if (box.w < 1 || box.h < 1) return;
    rects.push(toPhysicalRect(box, scale, offsetY));
  };

  for (const card of cards) {
    let left = card.x;
    let top = card.y;
    let right = card.x + card.w;
    let bottom = card.y + card.h;
    if (viewport) {
      left = Math.max(left, viewport.x);
      top = Math.max(top, viewport.y);
      right = Math.min(right, viewport.x + viewport.w);
      bottom = Math.min(bottom, viewport.y + viewport.h);
    }
    if (right - left < 1 || bottom - top < 1) continue;
    push({ x: left, y: top, w: right - left, h: bottom - top });
  }

  for (const overlay of overlays) {
    if (overlay) push(overlay);
  }
  return rects;
}
