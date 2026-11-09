/**
 * In-panel card context menu (Wave 1.1).
 *
 * The menu is plain DOM inside the panel window: the panel is only 189/210/246 dp tall and any
 * extra window would break the "never a second visible surface" constraint (and the memory
 * budget). The panel reports this element's rect to Rust so the 33 ms click-through hit-test lets
 * the pointer through to it (see `interactiveRects` in `lib/cardMenu.ts`).
 *
 * The component is presentation-only: the panel owns the open state (`CardMenuState`), executes
 * the action ids, and closes the menu on outside clicks. Rows are rendered from the pure
 * `menuItemsFor`, so which rows exist for a capture/recording/watched-folder item is unit-tested
 * without a DOM.
 */

import type { RefCallback } from "react";
import {
  CARD_MENU_PADDING_Y,
  CARD_MENU_WIDTH,
  CARD_SUBMENU_WIDTH,
  menuHeightFor,
  submenuHeightFor,
  submenuLeftFor,
  submenuRowTop,
  submenuTopFor,
  type MenuActionId,
  type MenuItemNode,
  type MenuNode,
  type MenuSubmenuNode,
} from "../lib/cardMenu";
import { t } from "../lib/i18n";

export interface CardContextMenuProps {
  /** Rows from `menuItemsFor` for the card the menu belongs to. */
  items: readonly MenuNode[];
  /** Menu top-left in panel-client CSS pixels (already clamped by `openCardMenu`). */
  x: number;
  y: number;
  /** `Take down ▸` is open. */
  submenuOpen: boolean;
  /** Panel width in CSS pixels: decides whether the submenu opens right or flips left. */
  panelWidth: number;
  onSubmenuOpenChange: (open: boolean) => void;
  onAction: (action: MenuActionId) => void;
  /** Panel measurement hooks: the menu and its submenu are click islands in the hit-rect union. */
  menuRef?: RefCallback<HTMLDivElement>;
  submenuRef?: RefCallback<HTMLDivElement>;
}

function Separator() {
  return <div className="panel__card-menu-separator" role="separator" aria-hidden="true" />;
}

export function CardContextMenu({
  items,
  x,
  y,
  submenuOpen,
  panelWidth,
  onSubmenuOpenChange,
  onAction,
  menuRef,
  submenuRef,
}: CardContextMenuProps) {
  const submenu = items.find((node): node is MenuSubmenuNode => node.kind === "submenu") ?? null;
  const menuHeight = menuHeightFor(items);
  const submenuTop = submenu ? submenuTopFor(submenuRowTop(items), menuHeight, submenuHeightFor(submenu)) : 0;
  const submenuLeft = submenuLeftFor(x, panelWidth);

  const renderItem = (node: MenuItemNode, inSubmenu = false) => (
    <button
      key={`${node.action}-${node.labelKey}`}
      type="button"
      role="menuitem"
      className="panel__card-menu-row"
      aria-disabled={node.disabled === true || undefined}
      data-action={node.action}
      data-disabled={node.disabled === true ? "true" : undefined}
      title={node.disabled && node.disabledReasonKey ? t(node.disabledReasonKey) : undefined}
      // Top-level rows close an open submenu; rows *inside* the submenu must not, or moving the
      // pointer onto the submenu would immediately close it again.
      onPointerEnter={inSubmenu ? undefined : () => onSubmenuOpenChange(false)}
      onClick={(event) => {
        event.stopPropagation();
        if (node.disabled === true) return;
        onAction(node.action);
      }}
    >
      <span className="panel__card-menu-label">{t(node.labelKey)}</span>
    </button>
  );

  return (
    <div
      className="panel__card-menu"
      role="menu"
      aria-label={t("More actions")}
      data-testid="card-menu"
      ref={menuRef}
      style={{
        left: `${x}px`,
        top: `${y}px`,
        width: `${CARD_MENU_WIDTH}px`,
        paddingTop: `${CARD_MENU_PADDING_Y}px`,
        paddingBottom: `${CARD_MENU_PADDING_Y}px`,
      }}
      onContextMenu={(event) => event.preventDefault()}
      onPointerLeave={() => onSubmenuOpenChange(false)}
    >
      {items.map((node, index) => {
        if (node.kind === "separator") return <Separator key={`separator-${index}`} />;
        if (node.kind === "item") return renderItem(node);
        return (
          <button
            key={`submenu-${node.id}`}
            type="button"
            role="menuitem"
            className="panel__card-menu-row panel__card-menu-row--submenu"
            aria-haspopup="menu"
            aria-expanded={submenuOpen}
            data-action={`submenu-${node.id}`}
            onPointerEnter={() => onSubmenuOpenChange(true)}
            onClick={(event) => {
              event.stopPropagation();
              onSubmenuOpenChange(!submenuOpen);
            }}
          >
            <span className="panel__card-menu-label">{t(node.labelKey)}</span>
            <span className="panel__card-menu-chevron" aria-hidden="true">
              ▸
            </span>
          </button>
        );
      })}

      {submenu && submenuOpen ? (
        <div
          className="panel__card-menu panel__card-menu--submenu"
          role="menu"
          aria-label={t(submenu.labelKey)}
          data-testid="card-menu-submenu"
          ref={submenuRef}
          style={{
            left: `${submenuLeft}px`,
            top: `${submenuTop}px`,
            width: `${CARD_SUBMENU_WIDTH}px`,
            paddingTop: `${CARD_MENU_PADDING_Y}px`,
            paddingBottom: `${CARD_MENU_PADDING_Y}px`,
          }}
          onContextMenu={(event) => event.preventDefault()}
        >
          {submenu.items.map((node, index) =>
            node.kind === "separator" ? (
              <Separator key={`submenu-separator-${index}`} />
            ) : (
              renderItem(node, true)
            ),
          )}
        </div>
      ) : null}
    </div>
  );
}
