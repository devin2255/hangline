/**
 * The panel window: rope, clothes pins, glass cards, thumbnails, the reveal/tuck animation and
 * the physical hit-rect reporter.
 *
 * Task 11 adds the motion half: Rust emits `reveal` when the line slides in (new item peek,
 * hotkey/tray toggle, optional top-edge dwell) and `tuck` when it slides out. The panel animates
 * `translateY` from `-(panelHeight + 12)` to 0 with a semi-implicit Euler spring (see
 * `lib/spring.ts`); the tuck itself is a 220 ms CSS `ease-in`, after which Rust hides the window
 * (no invisible window stays behind). `prefers-reduced-motion` switches to immediate placement.
 *
 * Busy gestures (pointer pressed, menu open) are reported to Rust with `set_panel_busy`, so the
 * controller never tucks the line out from under a click/drag.
 *
 * Geometry: Rust sizes the window to the target monitor's work area (spec §3.3). The webview
 * lays the line out in CSS pixels (dp) and reports every visible card's rect back in *physical*
 * panel-client pixels, because the 30 Hz hit-test thread works in physical coordinates.
 */

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react";
import { CardContextMenu } from "../../components/CardContextMenu";
import { GlassCard } from "../../components/GlassCard";
import { Rope } from "../../components/Rope";
import {
  afterDrag,
  copyItem,
  copyItemAsFile,
  getDoubleClickIntervalMs,
  getEditorState,
  getPendingUpdate,
  installUpdate,
  openAnnotate,
  openItem,
  openSearch,
  openTrim,
  reportPanelReady,
  setHitRects,
  setPanelBusy,
  showInExplorer,
  startDrag,
  startFall,
  startFly,
  syncPanelGeometry,
  takeCaptureOrigin,
  takeDown,
  takeDownSide,
} from "../../lib/ipc";
import {
  interactiveRects,
  menuItemsFor,
  menuOuterHeightFor,
  openCardMenu,
  setSubmenuOpen,
  shouldSuppressCardGesture,
  type CardMenuState,
  type CssBox,
  type MenuActionId,
} from "../../lib/cardMenu";
import { nextGustDelay } from "../../lib/gust";
import { FALL, FLY } from "../../lib/motion";
import {
  createFrameRing,
  DEV_INSTRUMENTATION,
  pushFrame,
  summarize,
  summarizeOffsets,
  type FrameRing,
  type OffsetSample,
} from "../../lib/perf";
import { installTrace, pushTrace, traceEnabled } from "../../lib/trace";
import { planFallSequence } from "../../lib/fallPlan";
import type { FallFrameInfo } from "../../lib/fallPlan";
import {
  capacity,
  cardTopFromRope,
  cardVisualSize,
  cardWidth,
  cardX,
  lineShift,
  lineWidth,
  maxItems,
  maxScrollOffset,
  nextScrollOffset,
  pinTop,
  ropeY,
  tiltFor,
  visibleSlice,
} from "../../lib/layout";
import { tick as springTick } from "../../lib/spring";
import { t } from "../../lib/i18n";
import { initAppState, useAppStore } from "../../lib/state";
import { useResolvedTheme } from "../../lib/theme";
import type {
  DragEndedEvent,
  EditorStateEvent,
  FlyLandedEvent,
  Item,
  ItemKind,
  PanelGeometryEvent,
  PhysicalRect,
  RevealEvent,
  TrimStateEvent,
  TuckEvent,
  UpdateCheckFailedEvent,
  UpdateCheckFinishedEvent,
  UpdateCheckStartedEvent,
  UpdateInfo,
  UpdateProgressEvent,
} from "../../lib/types";
import { DRAG_ENDED_TIMEOUT_MS, isStaleDragEnded } from "./dragSession";
import { exceedsDragThreshold } from "./dragThreshold";
import { resolveHitRectScale } from "./hitRectScale";
import "./Panel.css";

/** Stable empty list: a missing state must not invalidate layout effects on every render. */
const NO_ITEMS: Item[] = [];

/** A press on a card that may turn into a native drag after travelling the 4 dp threshold. */
interface DragOrigin {
  pointerId: number;
  /** Press position in CSS pixels (dp). */
  x: number;
  y: number;
  /** Item id captured at press time; a `state` broadcast mid-drag must not lose the target. */
  itemId: string;
  /** File path captured at press time; this is what `start_drag` receives. */
  path: string;
  /** Element that owns pointer capture for the press (the card surface). */
  capture: HTMLElement | null;
  /** True once the threshold started the native drag (exactly once per press). */
  started: boolean;
}

/**
 * A drag whose `drag-ended` has not been handled yet.
 *
 * The native drag ends outside this webview, so a delayed `drag-ended` is the only prune signal.
 * The id/effect therefore live in their own ref: a fresh pointerdown must not drop them (it only
 * clears the pressed visual), and the 30 s watchdog settles them before resetting.
 */
interface PendingDrag {
  /** Item id captured when the native drag started. */
  id: string;
  /** Effect of the last `drag-ended` for this drag; `"none"` until the event arrives. */
  effect: DragEndedEvent["effect"];
}

/** Reveal spring parameters from design spec §3.3 (`response 0.42 s`, ζ ≈ 0.84). */
const SPRING_STIFFNESS = 224;
const SPRING_DAMPING = 25;

/**
 * The panel's P5-2 update notice (the app's "panel pill" notification pattern).
 *
 * `checking`/`check-failed` are manual-check feedback from the tray action; `available` carries the
 * announcement and renders the `Install update` button; `installing` displays download progress;
 * `install-failed` keeps the actionable error visible for a few seconds.
 */
type UpdateNotice =
  | { kind: "checking" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "installing"; info: UpdateInfo }
  | { kind: "check-failed"; detail?: string }
  | { kind: "install-failed"; detail?: string };

/** How long a failed update notice stays visible (ms). */
const UPDATE_FAILURE_NOTICE_MS = 8000;

/** Extra dp hidden above the screen edge when tucked (`-(panelHeight + 12)`). */
const TUCK_OVERSHOOT_DP = 12;

/** Reduced-motion override: place the panel immediately instead of spring-animating. */
const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

/** How often the visible panel compares `devicePixelRatio` with the cached hit-rect scale. */
const DPI_WATCH_MS = 500;

/**
 * P2-6 fall watchdog delay: the overlay fall lasts 550 ms; if the item is still on the line after
 * this budget, `take_down` failed (or its state was lost) and the hidden card must be restored.
 */
const FALL_WATCHDOG_MS = FALL.durationMs + 950;

/** CSS class that disables the tuck transition while the spring drives the transform. */
const SPRINGING_CLASS = "panel--springing";

/**
 * How long a click that dismissed the card menu stays swallowed (ms).
 *
 * The card menu closes on `pointerdown` outside it, and the `click` that follows the same press
 * would otherwise reach the card underneath and schedule its delayed copy. The guard is shorter
 * than the system double-click interval, so it only ever covers the dismissing press.
 */
const MENU_DISMISS_GUARD_MS = 400;

/**
 * P4-5 dev-only offset trace capacity: one sample per frame is ~300 samples for a 5 s sweep, so
 * 2000 samples cover a much longer gesture without unbounded memory.
 */
const OFFSET_TRACE_CAPACITY = 2000;

/** Current CSS viewport width in dp (= the work-area width Rust sized the window to). */
function usePanelWidth(): number {
  const [width, setWidth] = useState(() => Math.max(1, window.innerWidth));
  useEffect(() => {
    const onResize = () => setWidth(Math.max(1, window.innerWidth));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return width;
}

/**
 * Reads the element's current translateY in CSS pixels.
 *
 * During the 220 ms CSS tuck transition the computed transform is the painted position, which is
 * exactly where a spring started by an early re-reveal must begin.
 */
function currentTranslateY(element: HTMLElement): number {
  const transform = window.getComputedStyle(element).transform;
  if (!transform || transform === "none") return 0;
  try {
    return new DOMMatrixReadOnly(transform).m42;
  } catch {
    return 0;
  }
}

export function Panel() {
  const panelRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  /** Mounted glass surfaces keyed by item id; only these are reported as hit rects. */
  const cardRefs = useRef(new Map<string, HTMLElement>());
  /**
   * Cached DPI scale (physical pixels per CSS pixel) for hit-rect reporting.
   *
   * Refreshed from `window.devicePixelRatio` on every collection, on resize and by a 500 ms watch,
   * because a cross-DPI move can keep the CSS viewport unchanged and fire no resize at all.
   */
  const scaleRef = useRef<number | null>(null);
  /** Serializes hit-rect reports so an older layout can never overwrite a newer one. */
  const reportChain = useRef<Promise<void>>(Promise.resolve());
  const reportFrame = useRef<number | null>(null);
  /** Latest `scheduleReport`; lets a collection request a follow-up report without a hook cycle. */
  const scheduleReportRef = useRef<() => void>(() => {});

  // --- native drag-out bookkeeping (Task 13) ------------------------------------------------
  /** Press that may become a native drag; cleared on pointerup/cancel. */
  const dragOriginRef = useRef<DragOrigin | null>(null);
  /**
   * Drag whose `drag-ended` has not been handled yet, keyed by the id captured at drag start.
   *
   * A `state` broadcast (a watcher add, a prune) replaces every item object mid-drag, so the id
   * must not be looked up in the render that handles `drag-ended`. Keeping it apart from the
   * pressed visual means a fresh pointerdown cannot discard a delayed event's prune target.
   */
  const pendingDragRef = useRef<PendingDrag | null>(null);
  /** Mirrors the pressed state synchronously so two pointermoves cannot start two drags. */
  const draggingRef = useRef(false);
  /** Card whose native drag-out is in flight; drives the `data-dragging` visual. */
  const [draggingId, setDraggingId] = useState<string | null>(null);
  /** Highest Rust drag-session token whose `drag-ended` event was handled. */
  const lastDragSessionRef = useRef(0);
  /** Fires if `drag-ended` never arrives and clears the pressed state. */
  const dragWatchdogRef = useRef<number | null>(null);
  /**
   * A hold completed on the current press: the same press must not start a native drag after the
   * editor opened (the browser may still deliver pointermove to this window while it is held).
   */
  const suppressDragRef = useRef(false);
  /**
   * Latest busy flag reported to Rust.
   *
   * A native drag ends outside this webview (the OS drag loop owns the mouse), so the pointerup
   * that normally clears the flag may never arrive. `drag-ended` must be able to clear the flag
   * that pointerdown set, so the mirror lives at component scope instead of inside the listener
   * effect. The flag is the OR of the pointer gesture and the open modal editors: while the markup
   * editor or the trim window is open the line must not tuck either, even though the panel already
   * lost focus.
   */
  const busyRef = useRef(false);
  /** A pointer press/drag/menu gesture is in flight in this panel window. */
  const pointerBusyRef = useRef(false);
  /** A card context menu is open in this panel window (Wave 1.1): the line must not tuck. */
  const menuOpenRef = useRef(false);
  /** The Rust-tracked markup editor window is open (P3-4). */
  const editorOpenRef = useRef(false);
  /** The Rust-tracked trim window is open (P3-6; same busy contract as the markup editor). */
  const trimOpenRef = useRef(false);
  /** Mirror for the `data-busy` evidence attribute. */
  const [busyReported, setBusyReported] = useState(false);
  /** The markup editor is open: freeze card motion and keep the line from tucking. */
  const [editorOpen, setEditorOpen] = useState(false);
  /** The trim window is open: same busy/motion contract as the markup editor. */
  const [trimOpen, setTrimOpen] = useState(false);
  const syncBusy = useCallback(() => {
    const next =
      pointerBusyRef.current ||
      editorOpenRef.current ||
      trimOpenRef.current ||
      menuOpenRef.current;
    setBusyReported(next);
    if (next === busyRef.current) return;
    busyRef.current = next;
    void setPanelBusy(next).catch((error: unknown) => {
      console.warn("Hangline panel: set_panel_busy unavailable", error);
    });
  }, []);
  const reportBusy = useCallback(
    (next: boolean) => {
      pointerBusyRef.current = next;
      syncBusy();
    },
    [syncBusy],
  );

  /**
   * Opens the markup editor (images) or the trim window (recordings).
   *
   * Shared by the long-press gesture, the hover capsule's middle action and the card menu's
   * Markup/Trim row, so the optimistic busy flag (the line must not tuck in the gap before Rust's
   * `editor-state`/`trim-state` event) is written exactly once.
   */
  const openEditorFor = useCallback(
    (item: Item) => {
      if (item.kind === "recording") {
        trimOpenRef.current = true;
        setTrimOpen(true);
        syncBusy();
        void openTrim(item.id).catch((error: unknown) => {
          console.warn(`Hangline panel: cannot open the trim window for ${item.id}`, error);
          trimOpenRef.current = false;
          setTrimOpen(false);
          syncBusy();
        });
        return;
      }
      editorOpenRef.current = true;
      setEditorOpen(true);
      syncBusy();
      void openAnnotate(item.id).catch((error: unknown) => {
        console.warn(`Hangline panel: cannot open the markup editor for ${item.id}`, error);
        editorOpenRef.current = false;
        setEditorOpen(false);
        syncBusy();
      });
    },
    [syncBusy],
  );

  const state = useAppStore((store) => store.state);
  const items = state?.items ?? NO_ITEMS;
  const size = state?.settings.cardSize ?? "medium";
  const keepSetting = state?.settings.keepOnLine ?? "fit";
  const themeSetting = state?.settings.theme ?? "system";
  const theme = useResolvedTheme(themeSetting);
  const panelWidth = usePanelWidth();
  /** System single-click delay so a double-click's first click never copies (spec §3.1). */
  const [doubleClickMs, setDoubleClickMs] = useState(500);
  /**
   * Horizontal scroll position, in cards, from the newest end (`0` = newest on the right).
   *
   * The panel owns this offset exclusively: the viewport never scrolls natively (the wheel
   * handler calls `preventDefault`), so `scrollLeft` and the offset can never both move the line.
   */
  const [scrollOffset, setScrollOffset] = useState(0);
  /**
   * Id of the card currently drawn by the fly overlay (P2-5).
   *
   * While set, the panel renders that card with `flyState="flying"` (hidden via `data-flying`) and
   * excludes it from hit-rect reporting, so the panel stays click-through over the frozen desktop
   * while the overlay animates; `fly-landed` clears it and the card plays its landing sway.
   */
  const [flyingId, setFlyingId] = useState<string | null>(null);

  // The line keeps `max(capacity, keepOnLine)` cards (Task 14); "fit" keeps exactly one screen.
  const fits = capacity(panelWidth, size);
  const lineLimit = maxItems(keepSetting, fits);
  const maxOffset = maxScrollOffset(items.length, lineLimit, fits);
  const offset = Math.min(scrollOffset, maxOffset);
  const range = visibleSlice(items.length, lineLimit, fits, offset);
  // Oldest → newest, newest on the right (Rust keeps `items` sorted ascending).
  const visible = items.slice(range.start, range.end);
  const newestId = items.length > 0 ? items[items.length - 1].id : "";

  /** Latest layout inputs for the async fly-start path (it runs after a reveal/await). */
  const layoutRef = useRef({ items, size, panelWidth, fits, lineLimit, offset });
  layoutRef.current = { items, size, panelWidth, fits, lineLimit, offset };

  // --- card context menu + hover action capsule (Wave 1.1 / 1.2) ------------------------------
  const [menu, setMenu] = useState<CardMenuState | null>(null);
  /**
   * A click that dismissed the menu is swallowed for {@link MENU_DISMISS_GUARD_MS} so it cannot
   * also schedule the card's delayed copy/open underneath.
   */
  const [menuDismissGuard, setMenuDismissGuard] = useState(false);
  const menuDismissGuardTimer = useRef<number | null>(null);
  /** DOM boxes of the menu, its submenu and the hover capsule: click islands for the hit tester. */
  const menuElRef = useRef<HTMLDivElement | null>(null);
  const submenuElRef = useRef<HTMLDivElement | null>(null);
  const actionsElRef = useRef<HTMLElement | null>(null);
  /**
   * Immediate hit-rect report for overlays that appear/disappear between layout signals.
   *
   * The capsule mounts on hover (a child-local state change: the panel does not re-render), so a
   * frame-coalesced report could be ~1 frame late; the capsule would then briefly be visible but
   * still inside the click-through area. Routed through a ref because `reportNow` is declared later
   * in the component (the ref is assigned on every render once it exists).
   */
  const reportNowRef = useRef<() => void>(() => {});
  /** True while card gestures must stay suspended (menu open or its dismiss guard). */
  const menuSuppressed = shouldSuppressCardGesture(menu) || menuDismissGuard;

  const setMenuBusy = useCallback(
    (open: boolean) => {
      menuOpenRef.current = open;
      syncBusy();
    },
    [syncBusy],
  );

  const setMenuElement = useCallback((element: HTMLDivElement | null) => {
    menuElRef.current = element;
    scheduleReportRef.current();
    reportNowRef.current();
  }, []);
  const setSubmenuElement = useCallback((element: HTMLDivElement | null) => {
    submenuElRef.current = element;
    scheduleReportRef.current();
    reportNowRef.current();
  }, []);
  /** Keeps the hover capsule's rect in the click-through hit-rect union while it is mounted. */
  const setActionsElement = useCallback((element: HTMLElement | null) => {
    actionsElRef.current = element;
    scheduleReportRef.current();
    reportNowRef.current();
  }, []);

  const clearMenuDismissGuard = useCallback(() => {
    if (menuDismissGuardTimer.current !== null) {
      window.clearTimeout(menuDismissGuardTimer.current);
      menuDismissGuardTimer.current = null;
    }
    setMenuDismissGuard(false);
  }, []);

  /**
   * Closes the menu. `guard` is used when the close was caused by a pointerdown outside the menu:
   * the click that follows the same press must not reach the card underneath.
   */
  const closeCardMenu = useCallback(
    (guard = false) => {
      const wasOpen = menuOpenRef.current;
      setMenuBusy(false);
      setMenu(null);
      if (guard && wasOpen) {
        setMenuDismissGuard(true);
        if (menuDismissGuardTimer.current !== null) {
          window.clearTimeout(menuDismissGuardTimer.current);
        }
        menuDismissGuardTimer.current = window.setTimeout(() => {
          menuDismissGuardTimer.current = null;
          setMenuDismissGuard(false);
        }, MENU_DISMISS_GUARD_MS);
      }
      scheduleReportRef.current();
    },
    [setMenuBusy],
  );

  /** Opens the menu for `item` at a panel-client CSS anchor (pointer or capsule corner). */
  const openCardMenuAt = useCallback(
    (item: Item, anchorClientX: number, anchorClientY: number) => {
      const list = layoutRef.current.items;
      const index = list.findIndex((candidate) => candidate.id === item.id);
      if (index < 0) return;
      // `panelHeight` lets the pure model compact itself (Wave 2: the Search group makes the full
      // menu taller than the medium/small panel); the outer height includes the menu border.
      const nodes = menuItemsFor(item, {
        index,
        total: list.length,
        panelHeight: window.innerHeight,
      });
      // The reveal/tuck transform moves every client rect; absolute children are positioned in the
      // panel's untransformed client space, so the anchor must be un-translated first.
      const offsetY = panelRef.current ? currentTranslateY(panelRef.current) : 0;
      clearMenuDismissGuard();
      setMenuBusy(true);
      setMenu(
        openCardMenu({
          itemId: item.id,
          anchorX: anchorClientX,
          anchorY: anchorClientY - offsetY,
          menuHeight: menuOuterHeightFor(nodes),
          panelWidth: window.innerWidth,
          panelHeight: window.innerHeight,
        }),
      );
      scheduleReportRef.current();
    },
    [clearMenuDismissGuard, setMenuBusy],
  );

  /** Opens the same menu anchored to the hover capsule's `More actions` button. */
  const openCardMenuAtElement = useCallback(
    (item: Item, anchor: HTMLElement) => {
      const rect = anchor.getBoundingClientRect();
      openCardMenuAt(item, rect.left, rect.bottom + 2);
    },
    [openCardMenuAt],
  );

  const setMenuSubmenu = useCallback((open: boolean) => {
    setMenu((current) => setSubmenuOpen(current, open));
  }, []);
  /** Id of the overlay card (mirrored in a ref so async paths never read a stale state). */
  const flyingIdRef = useRef<string | null>(null);
  /** A flight waiting for the line to be revealed (new-item `state` arrives before `reveal`). */
  const pendingFlightRef = useRef<{ id: string } | null>(null);
  /** 500 ms fallback when the reveal event never arrives (brief: start anyway after 500 ms). */
  const flightTimerRef = useRef<number | null>(null);
  /**
   * Panel-side watchdog: if `fly-landed` never arrives (overlay crash), the card must not stay
   * hidden forever. Cleared by `fly-landed`, item removal and unmount.
   */
  const flightWatchdogRef = useRef<number | null>(null);
  /** Item ids from the previous snapshot; `null` means the first snapshot (no flight). */
  const prevItemIdsRef = useRef<string[] | null>(null);
  /** Last measured rest frames of rendered cards, used as the fall overlay start geometry. */
  const lastCardFramesRef = useRef<Map<string, FallFrameInfo>>(new Map());
  /** Item kind per rendered card (recordings skip falls; pruned with the frames). */
  const lastCardKindsRef = useRef<Map<string, ItemKind>>(new Map());
  /** Ids whose fall overlay animation already started (take-down + state-diff dedupe). */
  const fallStartedRef = useRef<Set<string>>(new Set());
  /** Ids the overlay is currently drawing: their panel copies hide via `data-falling`. */
  const [fallingIds, setFallingIds] = useState<Set<string>>(() => new Set());
  /** Panel window physical origin on the virtual screen (from `sync_panel_geometry`/event). */
  const panelOriginRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });
  /** Pending 60 ms-stagger timers for batched falls; cleared on tuck/unmount. */
  const fallTimersRef = useRef<Set<number>>(new Set());
  /** Per-id fall watchdogs: restore a card if `take_down` failed or never removed it. */
  const fallWatchdogsRef = useRef<Map<string, number>>(new Map());

  // --- update notice (P5-2) ------------------------------------------------------------------
  /** Current update notice; `null` renders nothing. */
  const [updateNotice, setUpdateNotice] = useState<UpdateNotice | null>(null);
  /** Latest download progress (only meaningful while the notice is `installing`). */
  const [updateProgress, setUpdateProgress] = useState<UpdateProgressEvent | null>(null);
  /** The notice pill; its rect is reported so the `Install update` button is clickable. */
  const updateNoticeRef = useRef<HTMLDivElement>(null);
  /** Auto-hide timer for failed notices. */
  const updateNoticeTimerRef = useRef<number | null>(null);
  const clearUpdateNoticeTimer = useCallback(() => {
    if (updateNoticeTimerRef.current !== null) {
      window.clearTimeout(updateNoticeTimerRef.current);
      updateNoticeTimerRef.current = null;
    }
  }, []);
  /** Shows a failed check/install notice and auto-hides it after a few seconds. */
  const showUpdateFailure = useCallback(
    (kind: "check-failed" | "install-failed", detail?: string) => {
      clearUpdateNoticeTimer();
      setUpdateNotice({ kind, detail });
      updateNoticeTimerRef.current = window.setTimeout(() => {
        updateNoticeTimerRef.current = null;
        setUpdateNotice((current) => (current?.kind === kind ? null : current));
      }, UPDATE_FAILURE_NOTICE_MS);
    },
    [clearUpdateNoticeTimer],
  );
  /** Starts the explicit install after the user clicks the notice action. */
  const startUpdateInstall = useCallback(() => {
    clearUpdateNoticeTimer();
    setUpdateProgress(null);
    setUpdateNotice((current) =>
      current?.kind === "available" ? { kind: "installing", info: current.info } : current,
    );
    void installUpdate().catch((error: unknown) => {
      console.warn("Hangline panel: install_update failed", error);
      const detail = error instanceof Error ? error.message : String(error);
      showUpdateFailure("install-failed", detail);
    });
  }, [clearUpdateNoticeTimer, showUpdateFailure]);

  // A new newest card always comes down showing the newest end (spec: "always shows the newest"),
  // and a keepOnLine change restarts the view at the newest card instead of a stale offset. Other
  // item changes (a prune, a take-down) only clamp the offset, keeping the user's place.
  useEffect(() => {
    setScrollOffset(0);
  }, [newestId, keepSetting]);

  // cardSize / window width changes recompute the capacity; a shorter line clamps the offset.
  useEffect(() => {
    setScrollOffset((current) => (current > maxOffset ? maxOffset : current));
  }, [maxOffset]);

  // Single wheel model: one non-passive listener (React's root `wheel` handler is passive and could
  // not `preventDefault`), one card per event on the dominant axis, clamped to the kept window.
  useEffect(() => {
    const track = trackRef.current;
    if (!track) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      // Scrolling moves the line under the menu; a menu anchored to the old layout must close.
      closeCardMenu(false);
      setScrollOffset((current) =>
        nextScrollOffset(current, event.deltaX, event.deltaY, maxOffset),
      );
    };
    track.addEventListener("wheel", onWheel, { passive: false });
    return () => track.removeEventListener("wheel", onWheel);
  }, [closeCardMenu, maxOffset]);

  // Every offset change is logged with its visible range; the runtime evidence reads these lines
  // and `data-offset`/`data-range-*` from the panel DOM.
  useEffect(() => {
    console.info(
      `Hangline panel: offset ${offset}/${maxOffset} -> items[${range.start}..${range.end}) of ` +
        `${items.length} (keep ${lineLimit}, capacity ${fits}, cardSize ${size})`,
    );
  }, [offset, maxOffset, range.start, range.end, items.length, lineLimit, fits, size]);

  // --- reveal / tuck motion (Task 11) ------------------------------------------------------
  /** Whether the line is meant to be on screen; drives the `data-revealed` debug attribute. */
  const [revealed, setRevealed] = useState(true);
  /** `prefers-reduced-motion`: place the panel immediately instead of animating. */
  const [reducedMotion, setReducedMotion] = useState(() =>
    window.matchMedia(REDUCED_MOTION_QUERY).matches,
  );
  /** Breeze counter (P2-4): increments on the 7–16 s gust timer; cards sway from it. */
  const [gust, setGust] = useState(0);
  /** `document.hidden` mirrored as state so background windows stop the gust chain. */
  const [documentHidden, setDocumentHidden] = useState(() => document.hidden);
  /** Current spring sample in CSS pixels (position relative to the revealed 0 offset). */
  const motion = useRef({ x: 0, v: 0 });
  /** The one running rAF handle, if any. */
  const springFrame = useRef<number | null>(null);
  /**
   * P2-9 frame ring (600 samples ≈ 10 s at 60 fps); filled only while the panel is visible.
   *
   * Deliberately `null` until the dev-only install effect creates it: a `useRef(createFrameRing(600))`
   * argument is evaluated on every render even in a release build, so the ring constructor would
   * survive tree-shaking (P4-6 review). Release never allocates a ring.
   */
  const frameRingRef = useRef<FrameRing | null>(null);
  /**
   * P4-5 dev-only line-offset trace: one sample per frame, so the perf harness can prove the
   * measured wheel sweep actually moved the line instead of asserting the static `maxOffset`.
   */
  const offsetTraceRef = useRef<OffsetSample[] | null>(null);
  /** Latest reveal/reduced-motion values for the async fly-start path (effects read refs). */
  const revealedRef = useRef(revealed);
  revealedRef.current = revealed;
  const reducedMotionRef = useRef(reducedMotion);
  reducedMotionRef.current = reducedMotion;
  const documentHiddenRef = useRef(documentHidden);
  documentHiddenRef.current = documentHidden;
  /** Fresh panel origin mirrored as state for the `data-origin-*` evidence attributes. */
  const [panelOrigin, setPanelOrigin] = useState<{ x: number; y: number }>({ x: 0, y: 0 });

  useEffect(() => {
    const onVisibility = () => setDocumentHidden(document.hidden);
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, []);

  // P2-9/P4-5: expose the frame ring, the motion trace and the line-offset trace to the CDP
  // harnesses. Development only: `DEV_INSTRUMENTATION` is statically false in a release build, so
  // these globals (and the sampling loop below) are dropped from the release bundle.
  useEffect(() => {
    if (!DEV_INSTRUMENTATION) return;
    // Allocate the ring here, behind the static gate, so release has no constructor call at all.
    frameRingRef.current = createFrameRing(600);
    installTrace();
    window.__HANG_FRAMES__ = {
      summarize: () => summarize(frameRingRef.current),
      reset: () => {
        frameRingRef.current = createFrameRing(600);
      },
    };
    window.__HANG_OFFSETS__ = {
      start: (startOffset) => {
        offsetTraceRef.current = [{ t: performance.now(), offset: startOffset }];
      },
      sample: (t, offset) => {
        const trace = offsetTraceRef.current;
        if (trace && trace.length < OFFSET_TRACE_CAPACITY) trace.push({ t, offset });
      },
      stop: () => {
        const trace = offsetTraceRef.current ?? [];
        offsetTraceRef.current = null;
        return summarizeOffsets(trace);
      },
    };
    return () => {
      delete window.__HANG_FRAMES__;
      delete window.__HANG_OFFSETS__;
      offsetTraceRef.current = null;
      frameRingRef.current = null;
    };
  }, []);

  // The sampler only runs while the panel is visible and drops everything when hidden. The offset
  // sample is appended from the same rAF tick so the trace shares the frame clock the harness reads.
  useEffect(() => {
    if (!DEV_INSTRUMENTATION || !revealed || documentHidden) return;
    let last: number | null = null;
    let handle = 0;
    const tick = (timestamp: number) => {
      // Read the ref every tick: `__HANG_FRAMES__.reset()` swaps in a fresh ring mid-trace.
      const ring = frameRingRef.current;
      if (ring && last !== null) pushFrame(ring, timestamp - last);
      last = timestamp;
      window.__HANG_OFFSETS__?.sample(timestamp, layoutRef.current.offset);
      handle = window.requestAnimationFrame(tick);
    };
    handle = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(handle);
  }, [revealed, documentHidden]);

  // Gust chain (P2-4): one timer while the line is on screen, populated, motion is allowed and
  // the window is visible. Tucking, emptying the line, reduced motion or a hidden document stops
  // the chain; revealing again starts a fresh 7–16 s delay. Each tick bumps `gust`, and every card
  // derives its own amplitude/delay from the change. The dependency is "has items", not the count:
  // adding/removing cards while the line stays populated must not postpone the next gust forever.
  const hasItems = items.length > 0;
  useEffect(() => {
    if (!revealed || !hasItems || reducedMotion || documentHidden) return;
    let disposed = false;
    let timer: number | null = null;
    const schedule = () => {
      timer = window.setTimeout(() => {
        timer = null;
        if (disposed) return;
        setGust((current) => current + 1);
        schedule();
      }, nextGustDelay(Math.random()));
    };
    schedule();
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [documentHidden, hasItems, reducedMotion, revealed]);

  // --- P2-5 fly-in orchestration ------------------------------------------------------------
  /**
   * Starts the queued flight for the one newly added item.
   *
   * The layout is read from `layoutRef` here (not in the diff effect) because this path is async:
   * it may run after the reveal spring started, and the target must be the card's final rest
   * position. Capture items pass their real selection rect; other kinds pass `null` so Rust uses
   * the spec §4.6 C monitor-centre fallback.
   */
  const startPendingFlight = useCallback(async () => {
    const pending = pendingFlightRef.current;
    pendingFlightRef.current = null;
    if (flightTimerRef.current !== null) {
      window.clearTimeout(flightTimerRef.current);
      flightTimerRef.current = null;
    }
    if (!pending || flyingIdRef.current || reducedMotionRef.current) return;
    const id = pending.id;
    const {
      items: currentItems,
      size: cardSize,
      panelWidth: width,
      fits: capacityCount,
      lineLimit: keepCount,
      offset: scroll,
    } = layoutRef.current;
    const item = currentItems.find((candidate) => candidate.id === id);
    if (!item) return;
    // Recordings never use the fly overlay: an undecodable video would fly as a blank card, so
    // they use the normal arrival drop-in instead.
    if (item.kind === "recording") return;
    try {
      // The panel window position is the physical origin of every panel-relative rect; the command
      // also re-fits the window, so the target card rect below is exact.
      const geometry = await syncPanelGeometry();
      if (flyingIdRef.current) return;
      const originX = geometry.position?.x ?? 0;
      const originY = geometry.position?.y ?? 0;
      panelOriginRef.current = { x: originX, y: originY };
      setPanelOrigin({ x: originX, y: originY });
      const scale = window.devicePixelRatio || 1;
      const visibleRange = visibleSlice(currentItems.length, keepCount, capacityCount, scroll);
      const visibleItems = currentItems.slice(visibleRange.start, visibleRange.end);
      const index = visibleItems.findIndex((candidate) => candidate.id === id);
      if (index < 0) return;
      const count = visibleItems.length;
      const x = cardX(index, count, cardSize, width);
      const line = lineWidth(width, count, cardSize);
      // A panel narrower than the 3-card minimum right-aligns the overflowing line (`lineShift`).
      // `left` is viewport-space and therefore includes the shift; the rope/top stay in **line**
      // coordinates because the whole `.panel__line` is translated by the same shift via CSS, and
      // the rendered card's top uses `ropeY(line, x + cardWidth/2)` (unshifted) too.
      const shift = lineShift(width, line);
      const ropeX = x + cardWidth(cardSize) / 2;
      const visual = cardVisualSize(item, cardSize);
      const left = x + shift + (cardWidth(cardSize) - visual.width) / 2;
      const top = cardTopFromRope(ropeY(line, ropeX));
      const to: PhysicalRect = {
        x: Math.round(originX + left * scale),
        y: Math.round(originY + top * scale),
        w: Math.max(1, Math.round(visual.width * scale)),
        h: Math.max(1, Math.round(visual.height * scale)),
      };
      const from =
        item.kind === "capture" ? await takeCaptureOrigin(id).catch(() => null) : null;
      const accepted = await startFly(id, from, to, tiltFor(id), item.updatedAt ?? 0);
      if (!accepted) {
        console.info(`Hangline panel: fly overlay busy; ${id} uses the arrival drop-in`);
        return;
      }
      flyingIdRef.current = id;
      setFlyingId(id);
      if (flightWatchdogRef.current !== null) window.clearTimeout(flightWatchdogRef.current);
      flightWatchdogRef.current = window.setTimeout(() => {
        flightWatchdogRef.current = null;
        if (flyingIdRef.current !== id) return;
        console.warn(`Hangline panel: no fly-landed for ${id}; restoring the card`);
        flyingIdRef.current = null;
        setFlyingId(null);
      }, FLY.durationMs + 1500);
      console.info(
        `Hangline panel: flying ${id} from ` +
          (from ? `capture origin (${from.x}, ${from.y})` : "the monitor-centre fallback"),
      );
    } catch (error) {
      console.warn(`Hangline panel: fly-in of ${id} failed`, error);
    }
  }, []);

  // --- P2-6 take-down falls -----------------------------------------------------------------
  /**
   * Measures a card's unrotated rest AABB in physical panel-client pixels.
   *
   * Same transform-suppression recipe as `collectHitRects`: the fall overlay starts from the card's
   * rest pose, so swing/arrival/reorder/scale transforms must not leak into the frozen rect.
   */
  const measureCardRestRect = useCallback((element: HTMLElement): PhysicalRect => {
    const slot = element.closest<HTMLElement>(".glass-card-slot");
    const unit = element.parentElement;
    const targets = [slot, unit, element].filter((target): target is HTMLElement => !!target);
    const saved = targets.map((target) => ({
      target,
      value: target.style.getPropertyValue("transform"),
      priority: target.style.getPropertyPriority("transform"),
    }));
    for (const entry of saved) entry.target.style.setProperty("transform", "none", "important");
    const rect = element.getBoundingClientRect();
    for (const entry of saved) {
      if (entry.value === "") entry.target.style.removeProperty("transform");
      else entry.target.style.setProperty("transform", entry.value, entry.priority);
    }
    const scale = window.devicePixelRatio || 1;
    const offsetY = panelRef.current ? currentTranslateY(panelRef.current) : 0;
    return {
      x: Math.round(rect.left * scale),
      y: Math.round((rect.top - offsetY) * scale),
      w: Math.max(1, Math.round(rect.width * scale)),
      h: Math.max(1, Math.round(rect.height * scale)),
    };
  }, []);

  /** Converts a panel-client physical rect to virtual-screen physical coordinates. */
  const toVirtualRect = useCallback((rect: PhysicalRect): PhysicalRect => {
    const origin = panelOriginRef.current;
    return { x: origin.x + rect.x, y: origin.y + rect.y, w: rect.w, h: rect.h };
  }, []);

  /** Cancels every pending 60 ms-stagger fall timer. */
  const cancelFallTimers = useCallback(() => {
    for (const timer of fallTimersRef.current) window.clearTimeout(timer);
    fallTimersRef.current.clear();
  }, []);

  /**
   * Restores a card whose fall never removed it.
   *
   * Used for a failed `take_down` (Rust keeps the item when the recycle fails) and by the per-id
   * watchdog. The measured frame is kept: the card is back at the same rest position.
   *
   * If the overlay already accepted the fall (the common failure case: `take_down` failed after
   * `start_fall` succeeded), that accepted animation keeps playing for at most its 550 ms and then
   * finishes on its own; the card visibly pops back now and no clean cancel command exists, so the
   * overlay copy can briefly overlap the restored panel card. This is accepted rather than adding
   * a new IPC command just for the failure path.
   */
  const rollbackFall = useCallback((id: string, reason: string) => {
    const watchdog = fallWatchdogsRef.current.get(id);
    if (watchdog !== undefined) {
      window.clearTimeout(watchdog);
      fallWatchdogsRef.current.delete(id);
    }
    const wasFalling = fallingIds.has(id) || fallStartedRef.current.has(id);
    fallStartedRef.current.delete(id);
    setFallingIds((current) => {
      if (!current.has(id)) return current;
      const next = new Set(current);
      next.delete(id);
      return next;
    });
    if (wasFalling) console.warn(`Hangline panel: restoring ${id} after a failed take-down (${reason})`);
    scheduleReportRef.current();
  }, [fallingIds]);

  /**
   * P2-6 watchdog per fall id: if the item is still on the line after the fall budget, the overlay
   * (or `take_down`) failed and the hidden card is restored. A removed item clears the timer.
   */
  const armFallWatchdog = useCallback(
    (id: string) => {
      const existing = fallWatchdogsRef.current.get(id);
      if (existing !== undefined) window.clearTimeout(existing);
      const timer = window.setTimeout(() => {
        fallWatchdogsRef.current.delete(id);
        const stillOnLine = layoutRef.current.items.some((item) => item.id === id);
        if (!stillOnLine) {
          fallStartedRef.current.delete(id);
          return;
        }
        rollbackFall(id, "the card was not removed");
      }, FALL_WATCHDOG_MS);
      fallWatchdogsRef.current.set(id, timer);
    },
    [rollbackFall],
  );

  /**
   * Take-down click path (P2-6): starts the fall overlay for one card.
   *
   * Runs synchronously from `GlassCard` before Rust `take_down`; the panel copy hides immediately
   * via `fallingIds` (the overlay draws the card while it falls), and the item removal still comes
   * from the normal `state` broadcast. A rejected overlay or a failed `take_down` restores the card
   * (the per-id watchdog is the safety net).
   */
  const handleFallStart = useCallback(
    (item: Item, element: HTMLElement | null) => {
      if (reducedMotionRef.current || !revealedRef.current || documentHiddenRef.current) return;
      if (item.kind === "recording") return;
      const id = item.id;
      if (fallStartedRef.current.has(id)) return;
      let frame = lastCardFramesRef.current.get(id);
      if (!frame && element) {
        frame = {
          rect: measureCardRestRect(element),
          tiltDeg: tiltFor(id),
          path: item.path,
          imageVersion: item.updatedAt ?? 0,
        };
        lastCardKindsRef.current.set(id, item.kind);
      }
      if (!frame) return;
      fallStartedRef.current.add(id);
      setFallingIds((current) => {
        if (current.has(id)) return current;
        const next = new Set(current);
        next.add(id);
        return next;
      });
      armFallWatchdog(id);
      void startFall(
        id,
        item.path,
        toVirtualRect(frame.rect),
        frame.tiltDeg,
        item.updatedAt ?? frame.imageVersion,
      )
        .then((accepted) => {
          if (!accepted) {
            console.info(`Hangline panel: fall overlay rejected ${id}; the card just disappears`);
          }
        })
        .catch((error: unknown) => {
          console.warn(`Hangline panel: fall of ${id} failed`, error);
        });
    },
    [armFallWatchdog, measureCardRestRect, toVirtualRect],
  );

  /** Rollback hook for `GlassCard` when the built-in `take_down` rejects. */
  const handleTakeDownFailed = useCallback(
    (id: string, error: unknown) => {
      console.warn(`Hangline panel: take-down of ${id} failed`, error);
      rollbackFall(id, "take_down rejected");
    },
    [rollbackFall],
  );

  /**
   * Menu-driven single take-down: start the fall overlay first (same as the card's own cross),
   * then ask Rust; a rejection restores the hidden card through the same rollback path.
   */
  const takeDownItem = useCallback(
    (item: Item, deleteFile: boolean) => {
      handleFallStart(item, cardRefs.current.get(item.id) ?? null);
      void takeDown(item.id, deleteFile).catch((error: unknown) => {
        handleTakeDownFailed(item.id, error);
      });
    },
    [handleFallStart, handleTakeDownFailed],
  );

  /**
   * Executes one card-menu row. The menu closes first so a slow/open IPC never leaves it on
   * screen; every failure is logged (the menu has no error surface, matching the rest of the
   * card actions, which log and keep the card).
   */
  const runMenuAction = useCallback(
    (action: MenuActionId) => {
      const itemId = menu?.itemId ?? "";
      const item = layoutRef.current.items.find((candidate) => candidate.id === itemId) ?? null;
      if (item && (action === "markup" || action === "trim")) {
        // Set the editor busy flag *before* clearing the menu's: the line must stay expanded
        // across the handoff, not flap for one IPC round-trip.
        openEditorFor(item);
        closeCardMenu(false);
        return;
      }
      closeCardMenu(false);
      // `Search` is a global action: it does not need the card, only the window.
      if (action === "search") {
        void openSearch().catch((error: unknown) => {
          console.warn("Hangline panel: open_search failed", error);
        });
        return;
      }
      if (!item) return;
      switch (action) {
        case "copyImage":
          void copyItem(item.id).catch((error: unknown) => {
            console.warn(`Hangline panel: copy image of ${item.path} failed`, error);
          });
          return;
        case "copyAsFile":
          void copyItemAsFile(item.id).catch((error: unknown) => {
            console.warn(`Hangline panel: copy as file of ${item.path} failed`, error);
          });
          return;
        case "open":
          void openItem(item.id).catch((error: unknown) => {
            console.warn(`Hangline panel: open of ${item.path} failed`, error);
          });
          return;
        case "showInExplorer":
          void showInExplorer(item.id).catch((error: unknown) => {
            console.warn(`Hangline panel: show in Explorer of ${item.path} failed`, error);
          });
          return;
        case "takeDownRecycle":
          takeDownItem(item, true);
          return;
        case "takeDownRemove":
          takeDownItem(item, false);
          return;
        case "takeDownLeft":
        case "takeDownRight": {
          const side = action === "takeDownLeft" ? "before" : "after";
          void takeDownSide(item.id, side)
            .then((count) => {
              console.info(
                `Hangline panel: take_down_side ${side} of ${item.id} took down ${count} card(s)`,
              );
            })
            .catch((error: unknown) => {
              console.warn(`Hangline panel: take_down_side ${side} of ${item.id} failed`, error);
            });
          return;
        }
      }
    },
    [closeCardMenu, menu, openEditorFor, takeDownItem],
  );

  // The menu is closed by an outside press, Escape (best effort: the panel is focusable:false),
  // a wheel scroll or the line tucking. Listeners exist only while the menu is open.
  useEffect(() => {
    if (!menu) return;
    const isInsideMenu = (target: EventTarget | null): boolean => {
      if (!(target instanceof Node)) return false;
      return Boolean(menuElRef.current?.contains(target) || submenuElRef.current?.contains(target));
    };
    const onPointerDown = (event: PointerEvent) => {
      if (isInsideMenu(event.target)) return;
      // The dismissing press must not also copy/open the card under the pointer.
      closeCardMenu(true);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeCardMenu(false);
    };
    const onBlur = () => closeCardMenu(false);
    const onVisibility = () => {
      if (document.visibilityState === "hidden") closeCardMenu(false);
    };
    // The panel is click-through around the menu: a pointer that leaves the window cannot deliver
    // the outside pointerdown, so leaving the window closes the menu too.
    const onPointerLeave = () => closeCardMenu(false);
    document.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("blur", onBlur);
    document.addEventListener("visibilitychange", onVisibility);
    document.documentElement.addEventListener("pointerleave", onPointerLeave);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("visibilitychange", onVisibility);
      document.documentElement.removeEventListener("pointerleave", onPointerLeave);
    };
  }, [closeCardMenu, menu]);

  // The card the menu belongs to can be removed by a state broadcast (tray prune, side take-down,
  // editor save): a menu pointing at a missing item must close instead of acting on stale data.
  useEffect(() => {
    if (!menu) return;
    if (!items.some((item) => item.id === menu.itemId)) closeCardMenu(false);
  }, [closeCardMenu, items, menu]);

  /**
   * State-diff path (P2-6): plans falls for ids removed in one snapshot.
   *
   * Tray "take everything down", a drag prune and file-gone deletions can remove several cards at
   * once. The overlay accepts at most two concurrent falls, so entries are staggered 60 ms and the
   * remaining timers are cancelled once `start_fall` rejects twice in a row (the rest just disappear).
   */
  const scheduleFalls = useCallback(
    (removedIds: string[]) => {
      if (reducedMotionRef.current || !revealedRef.current || documentHiddenRef.current) return;
      const candidates = removedIds.filter((id) => {
        // A card the fly overlay still owns is skipped deliberately: its panel copy is hidden
        // (`data-flying`), it has no fresh rest frame, and a second animation would compete for the
        // overlay's two-animation budget. C1 removed the old "payload is reused and would collide"
        // reason (a same-id fall now gets its own render key), but the take-down simply disappearing
        // during a flight is still the intended UX; this is a policy filter, not a payload identity
        // requirement.
        if (id === flyingIdRef.current) return false;
        if (lastCardKindsRef.current.get(id) === "recording") return false;
        return lastCardFramesRef.current.has(id);
      });
      const frames = new Map<string, FallFrameInfo>();
      for (const id of candidates) {
        const frame = lastCardFramesRef.current.get(id);
        if (frame) frames.set(id, frame);
      }
      const now = performance.now();
      const plan = planFallSequence(candidates, frames, now);
      if (plan.length === 0) return;
      const timers: number[] = [];
      let stopped = false;
      let rejectStreak = 0;
      const cancelRemaining = () => {
        stopped = true;
        for (const timer of timers) {
          window.clearTimeout(timer);
          fallTimersRef.current.delete(timer);
        }
      };
      for (const entry of plan) {
        // Anchor each timer on the planner's absolute `startAtMs`: the 60 ms stagger lives in the
        // pure function, and an async gap between planning and scheduling cannot drift it.
        const delayMs = Math.max(0, Math.round(entry.startAtMs - performance.now()));
        const timer = window.setTimeout(() => {
          fallTimersRef.current.delete(timer);
          if (stopped || fallStartedRef.current.has(entry.id)) return;
          fallStartedRef.current.add(entry.id);
          armFallWatchdog(entry.id);
          void startFall(
            entry.id,
            entry.path,
            toVirtualRect(entry.rect),
            entry.tiltDeg,
            entry.imageVersion,
          )
            .then((accepted) => {
              if (accepted) {
                rejectStreak = 0;
                return;
              }
              rejectStreak += 1;
              if (rejectStreak >= 2) {
                console.info(
                  "Hangline panel: fall overlay is busy; the remaining cards disappear",
                );
                cancelRemaining();
              }
            })
            .catch((error: unknown) => {
              console.warn(`Hangline panel: fall of ${entry.id} failed`, error);
            });
        }, delayMs);
        timers.push(timer);
        fallTimersRef.current.add(timer);
      }
    },
    [armFallWatchdog, toVirtualRect],
  );

  // Diff the item list: only a single newly added item flies; a batch drops in normally (a flight
  // per card would exceed the overlay's two-animation budget and make the line look chaotic).
  useEffect(() => {
    const ids = items.map((item) => item.id);
    const previous = prevItemIdsRef.current;
    prevItemIdsRef.current = ids;
    // A card removed while the overlay still plays it must not leave a stuck invisible card.
    if (flyingIdRef.current && !ids.includes(flyingIdRef.current)) {
      flyingIdRef.current = null;
      setFlyingId(null);
      if (flightWatchdogRef.current !== null) {
        window.clearTimeout(flightWatchdogRef.current);
        flightWatchdogRef.current = null;
      }
    }
    if (previous === null) return;
    // Removals: one snapshot can drop several cards (tray all-down, prune, batch actions).
    const removed = previous.filter((id) => !ids.includes(id));
    if (removed.length > 0) {
      const unplanned = removed.filter((id) => !fallStartedRef.current.has(id));
      if (unplanned.length > 0) scheduleFalls(unplanned);
      const removedSet = new Set(removed);
      setFallingIds((current) => {
        let changed = false;
        const next = new Set(current);
        for (const id of removedSet) {
          if (next.delete(id)) changed = true;
        }
        return changed ? next : current;
      });
      for (const id of removedSet) {
        const watchdog = fallWatchdogsRef.current.get(id);
        if (watchdog !== undefined) {
          window.clearTimeout(watchdog);
          fallWatchdogsRef.current.delete(id);
        }
        lastCardFramesRef.current.delete(id);
        lastCardKindsRef.current.delete(id);
        fallStartedRef.current.delete(id);
      }
    }
    // Prune stale fall frames/watchdogs for ids that are no longer on the line at all (e.g. cards
    // beyond the retained window after a tray-all-down): a stale off-screen rect must not be used.
    const idSet = new Set(ids);
    for (const id of [...lastCardFramesRef.current.keys()]) {
      if (idSet.has(id)) continue;
      lastCardFramesRef.current.delete(id);
      lastCardKindsRef.current.delete(id);
    }
    for (const id of [...fallWatchdogsRef.current.keys()]) {
      if (idSet.has(id)) continue;
      const watchdog = fallWatchdogsRef.current.get(id);
      if (watchdog !== undefined) window.clearTimeout(watchdog);
      fallWatchdogsRef.current.delete(id);
      fallStartedRef.current.delete(id);
    }
    const added = ids.filter((id) => !previous.includes(id));
    if (added.length !== 1) return;
    if (flyingIdRef.current || pendingFlightRef.current) return;
    pendingFlightRef.current = { id: added[0] };
    // New-item `state` arrives before `reveal` when the line is tucked; wait for the reveal so the
    // drop-in plays once the line is visible, with the brief's 500 ms fallback if it never arrives.
    if (revealedRef.current) {
      void startPendingFlight();
    } else if (flightTimerRef.current === null) {
      flightTimerRef.current = window.setTimeout(() => {
        flightTimerRef.current = null;
        // The reveal never came (revealOnNew off / dismissed): animating to an invisible panel
        // would play the flight for nothing, so cancel it and render the card normally.
        if (!revealedRef.current) {
          pendingFlightRef.current = null;
          return;
        }
        void startPendingFlight();
      }, 500);
    }
  }, [items, scheduleFalls, startPendingFlight]);

  // Paused conditions: a fall must never play from an invisible panel, and pending stagger timers
  // and fall flags are cleared so a removal right after reveal cannot reuse stale ids. Frames are
  // additionally cleared on tuck (the reveal handler repopulates them, see the report trigger).
  useEffect(() => {
    if (revealed && !reducedMotion && !documentHidden) return;
    cancelFallTimers();
    fallStartedRef.current.clear();
    if (!revealed) {
      lastCardFramesRef.current.clear();
      lastCardKindsRef.current.clear();
    }
  }, [cancelFallTimers, documentHidden, reducedMotion, revealed]);

  // A reveal starts a fresh layout: repopulate the fall frames (cleared on tuck) before a batch
  // removal can plan falls. The report is scheduled through the ref because `scheduleReport` is
  // created later in the component.
  useEffect(() => {
    if (!revealed) return;
    const frame = window.requestAnimationFrame(() => scheduleReportRef.current());
    return () => window.cancelAnimationFrame(frame);
  }, [revealed]);

  // A reveal that arrives after the new-item snapshot releases the queued flight.
  useEffect(() => {
    if (revealed && pendingFlightRef.current) void startPendingFlight();
  }, [revealed, startPendingFlight]);

  // Panel-side end of a flight: the overlay is gone, so the card becomes visible and lands.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<FlyLandedEvent>("fly-landed", ({ payload }) => {
      if (disposed) return;
      if (!payload?.id || payload.id !== flyingIdRef.current) return;
      flyingIdRef.current = null;
      setFlyingId(null);
      if (flightWatchdogRef.current !== null) {
        window.clearTimeout(flightWatchdogRef.current);
        flightWatchdogRef.current = null;
      }
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: fly-landed listener unavailable", error);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Fresh panel geometry from Rust: the reveal controller moves the window to follow the pointer
  // across monitors, so the cached virtual-screen origin must not go stale (falls would land at the
  // old monitor's coordinates).
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<PanelGeometryEvent>("panel-geometry", ({ payload }) => {
      if (disposed || typeof payload?.x !== "number" || typeof payload?.y !== "number") return;
      panelOriginRef.current = { x: payload.x, y: payload.y };
      setPanelOrigin({ x: payload.x, y: payload.y });
      console.info(
        `Hangline panel: origin updated to (${payload.x}, ${payload.y}) ` +
          `(scale ${payload.scale ?? "?"})`,
      );
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: panel-geometry listener unavailable", error);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Drop a queued flight and pending fall timers/watchdogs on unmount.
  useEffect(
    () => () => {
      if (flightTimerRef.current !== null) window.clearTimeout(flightTimerRef.current);
      if (flightWatchdogRef.current !== null) window.clearTimeout(flightWatchdogRef.current);
      for (const timer of fallTimersRef.current) window.clearTimeout(timer);
      fallTimersRef.current.clear();
      for (const timer of fallWatchdogsRef.current.values()) window.clearTimeout(timer);
      fallWatchdogsRef.current.clear();
      pendingFlightRef.current = null;
    },
    [],
  );

  /** Applies a translateY offset imperatively: per-frame spring updates must not re-render. */
  const applyOffset = useCallback((offset: number) => {
    motion.current.x = offset;
    const panel = panelRef.current;
    if (panel) panel.style.transform = `translate3d(0, ${offset.toFixed(2)}px, 0)`;
    // P2-9 reveal/tuck trace (no-op unless the comparison harness enabled it); static gate drops
    // the call in release.
    if (DEV_INSTRUMENTATION) pushTrace("panel", { y: offset });
  }, []);

  const stopSpring = useCallback(() => {
    if (springFrame.current !== null) {
      window.cancelAnimationFrame(springFrame.current);
      springFrame.current = null;
    }
  }, []);

  /** Ends a spring and re-enables the CSS tuck transition. */
  const settleSpring = useCallback(() => {
    stopSpring();
    motion.current.v = 0;
    panelRef.current?.classList.remove(SPRINGING_CLASS);
  }, [stopSpring]);

  /** Runs the reveal spring from the currently painted offset to `target` on rAF. */
  const startSpring = useCallback(
    (target: number) => {
      stopSpring();
      const panel = panelRef.current;
      if (!panel) return;
      // An interrupted tuck may still be mid-transition: start from what is actually painted.
      motion.current = { x: currentTranslateY(panel), v: 0 };
      panel.classList.add(SPRINGING_CLASS);

      const started = performance.now();
      let last = started;
      const step = (timestamp: number) => {
        const next = springTick(
          motion.current,
          target,
          SPRING_STIFFNESS,
          SPRING_DAMPING,
          (timestamp - last) / 1000,
        );
        last = timestamp;
        motion.current = next;
        applyOffset(next.x);
        if (Math.abs(next.x - target) <= 0.1 && Math.abs(next.v) <= 1) {
          applyOffset(target);
          settleSpring();
          console.info(
            `Hangline panel: reveal spring settled in ${(timestamp - started).toFixed(0)} ms ` +
              `(translateY ${target}px)`,
          );
          return;
        }
        springFrame.current = window.requestAnimationFrame(step);
      };
      springFrame.current = window.requestAnimationFrame(step);
    },
    [applyOffset, settleSpring, stopSpring],
  );

  /** The fully hidden offset: the whole panel plus 12 dp above the screen's top edge. */
  const tuckedOffset = useCallback((): number => {
    const height = panelRef.current?.offsetHeight ?? window.innerHeight;
    return -(height + TUCK_OVERSHOOT_DP);
  }, []);

  /** Rust asked to hide the line: CSS ease-in 220 ms; Rust hides the window ~300 ms later. */
  const tuck = useCallback(() => {
    // A menu belongs to the revealed line: tucking hides its anchor and any click islands.
    closeCardMenu(false);
    stopSpring();
    const panel = panelRef.current;
    panel?.classList.remove(SPRINGING_CLASS);
    const offset = tuckedOffset();
    motion.current = { x: offset, v: 0 };
    applyOffset(offset);
    setRevealed(false);
    console.info(
      `Hangline panel: tuck at ${performance.now().toFixed(0)} ms -> translateY ${offset}px`,
    );
    // P2-9 tuck measurement: the 220 ms motion is a CSS transition, so `applyOffset` only records
    // the start. Only the comparison harness enables this sampling (max 400 ms after a tuck);
    // normal/release runs skip it entirely. The `DEV_INSTRUMENTATION` gate is static, so release
    // drops the `traceEnabled()` call and the whole module behind it (P4-6 review).
    if (DEV_INSTRUMENTATION && traceEnabled()) {
      const started = performance.now();
      const sample = (timestamp: number) => {
        const element = panelRef.current;
        if (!element) return;
        const y = currentTranslateY(element);
        pushTrace("tuck", { y });
        if (timestamp - started > 400 || Math.abs(y - offset) <= 0.5) return;
        window.requestAnimationFrame(sample);
      };
      window.requestAnimationFrame(sample);
    }
  }, [applyOffset, closeCardMenu, stopSpring, tuckedOffset]);

  /** Rust asked to show the line: spring from tucked to 0 (immediate under reduced motion). */
  const reveal = useCallback(
    (peek: boolean, reason: string) => {
      setRevealed(true);
      console.info(
        `Hangline panel: reveal (${reason}, peek=${peek}) at ${performance.now().toFixed(0)} ms`,
      );
      if (reducedMotion) {
        stopSpring();
        panelRef.current?.classList.remove(SPRINGING_CLASS);
        motion.current = { x: 0, v: 0 };
        applyOffset(0);
        return;
      }
      startSpring(0);
    },
    [applyOffset, reducedMotion, startSpring, stopSpring],
  );

  useEffect(() => {
    const query = window.matchMedia(REDUCED_MOTION_QUERY);
    const onChange = (event: MediaQueryListEvent) => setReducedMotion(event.matches);
    setReducedMotion(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  // Rust is the only producer of reveal/tuck; the panel just animates the requested direction.
  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    void Promise.all([
      listen<RevealEvent>("reveal", ({ payload }) => {
        if (!disposed) reveal(payload?.peek ?? false, payload?.reason ?? "unknown");
      }),
      listen<TuckEvent>("tuck", () => {
        if (!disposed) tuck();
      }),
    ])
      .then((registered) => {
        if (disposed) registered.forEach((unlisten) => unlisten());
        else unlisteners.push(...registered);
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: reveal/tuck listener unavailable", error);
      });
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
      stopSpring();
    };
  }, [reveal, stopSpring, tuck]);

  // --- native drag-out lifecycle ------------------------------------------------------------
  const clearDragWatchdog = useCallback(() => {
    if (dragWatchdogRef.current !== null) {
      window.clearTimeout(dragWatchdogRef.current);
      dragWatchdogRef.current = null;
    }
  }, []);

  /** Clears the pressed/drag visual state; a pending `drag-ended` target is left untouched. */
  const resetDragState = useCallback(() => {
    clearDragWatchdog();
    draggingRef.current = false;
    dragOriginRef.current = null;
    setDraggingId(null);
  }, [clearDragWatchdog]);

  /**
   * Hands the pending drag id/effect to Rust exactly once; safe when no drag is pending.
   *
   * Cancels are included: Rust's effect-driven policy only prunes a card whose file really
   * disappeared. The `drag-ended` path records the effect first; the watchdog path uses the
   * conservative `"none"` (lost event) so a vanished file is still pruned.
   */
  const settleDragSession = useCallback(() => {
    const pending = pendingDragRef.current;
    pendingDragRef.current = null;
    if (!pending) return;
    void afterDrag(pending.id, pending.effect).catch((error: unknown) => {
      console.warn(`Hangline panel: after_drag for ${pending.id} failed`, error);
    });
  }, []);

  /**
   * Starts the frontend watchdog as soon as a drag is requested.
   *
   * Rust normally reports the end through `drag-ended`, but that event can be lost (emit failure,
   * a destroyed window) or never arrive (a hung OLE loop). Without this guard the card would stay
   * pressed and the panel busy forever.
   */
  const armDragWatchdog = useCallback(() => {
    clearDragWatchdog();
    dragWatchdogRef.current = window.setTimeout(() => {
      dragWatchdogRef.current = null;
      if (!draggingRef.current) return;
      console.warn(
        `Hangline panel: no drag-ended event within ${DRAG_ENDED_TIMEOUT_MS} ms; ` +
          "clearing the pressed state (the Rust drag session may still be running)",
      );
      // The event was lost: settle the id anyway (idempotent), so a file that moved away is pruned.
      settleDragSession();
      resetDragState();
      reportBusy(false);
    }, DRAG_ENDED_TIMEOUT_MS);
  }, [clearDragWatchdog, reportBusy, resetDragState, settleDragSession]);

  /** Starts the native drag once for the press; the id and path stay captured until it ends. */
  const startNativeDrag = useCallback(
    (itemId: string, path: string) => {
      if (draggingRef.current) return;
      // A new drag proves the previous gesture is over: settle a `drag-ended` that never arrived
      // before its pending target is replaced.
      settleDragSession();
      draggingRef.current = true;
      pendingDragRef.current = { id: itemId, effect: "none" };
      setDraggingId(itemId);
      armDragWatchdog();
      // Resolves as soon as Rust accepted the session; the OLE loop itself runs on an STA thread
      // and the final DROPEFFECT arrives through `drag-ended`.
      void startDrag(path).catch((error: unknown) => {
        console.warn(`Hangline panel: native drag-out of ${path} unavailable`, error);
        resetDragState();
        reportBusy(false);
      });
    },
    [armDragWatchdog, reportBusy, resetDragState, settleDragSession],
  );

  // Rust owns the OLE session and reports every ending (drop, cancel or failure) here. The panic
  // path also emits, and stale/duplicate events are ignored by their session token.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<DragEndedEvent>("drag-ended", ({ payload }) => {
      if (disposed) return;
      // A late event for an older session must not clear the state of a newer drag.
      if (isStaleDragEnded(payload.sessionId, lastDragSessionRef.current)) {
        console.debug(
          `Hangline panel: ignoring stale drag-ended (session=${payload.sessionId}, ` +
            `last=${lastDragSessionRef.current})`,
        );
        return;
      }
      if (payload.sessionId !== undefined) lastDragSessionRef.current = payload.sessionId;
      // Record the reported effect on the pending drag, then settle it: the id was captured when
      // the drag started, so a state broadcast that removed the card cannot lose the target.
      const pending = pendingDragRef.current;
      if (pending) pending.effect = payload.effect ?? "none";
      if (payload.ok === false || payload.error) {
        console.warn("Hangline panel: native drag-out failed", payload.error ?? payload.effect);
      } else {
        console.info(
          `Hangline panel: drag ended (effect=${payload.effect}, dropped=${payload.dropped})`,
        );
      }
      resetDragState();
      reportBusy(false);
      // Cancels included: Rust decides from the effect and the file's existence, and only prunes
      // a card whose file really disappeared.
      settleDragSession();
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: drag-ended listener unavailable", error);
      });
    return () => {
      disposed = true;
      clearDragWatchdog();
      unlisten?.();
    };
  }, [clearDragWatchdog, reportBusy, resetDragState, settleDragSession]);

  /**
   * Rust reports both modal editor windows opening/closing (P3-4 markup, P3-6 trim).
   *
   * The panel mirrors each into the shared busy flag (so the reveal controller never tucks the line
   * under an editor) and into `paused` (so card motion freezes while the user edits). A lost event
   * cannot wedge the panel: `open_*` re-emits the state whenever it reuses a window, a failed
   * optimistic open rolls itself back, and `get_editor_state` reconciles a panel that reloaded
   * while an editor was already open.
   *
   * The reconciliation fetch is guarded by the number of events observed since it started: an
   * event that arrives while the request is in flight is newer than the snapshot and wins.
   */
  useEffect(() => {
    let disposed = false;
    let unlistenAnnotate: (() => void) | undefined;
    let unlistenTrim: (() => void) | undefined;
    /** Events applied since this effect mounted; used to drop a stale reconciliation snapshot. */
    let observed = 0;
    const applyAnnotationState = (open: boolean, source: string) => {
      observed += 1;
      editorOpenRef.current = open;
      setEditorOpen(open);
      syncBusy();
      console.info(
        `Hangline panel: markup editor ${open ? "open" : "closed"} (${source}, busy=${open ? "true" : "false"})`,
      );
    };
    const applyTrimState = (open: boolean, source: string) => {
      observed += 1;
      trimOpenRef.current = open;
      setTrimOpen(open);
      syncBusy();
      console.info(
        `Hangline panel: trim window ${open ? "open" : "closed"} (${source}, busy=${open ? "true" : "false"})`,
      );
    };
    void (async () => {
      try {
        const [stopAnnotate, stopTrim] = await Promise.all([
          listen<EditorStateEvent>("editor-state", ({ payload }) => {
            if (disposed) return;
            applyAnnotationState(Boolean(payload?.open), "event");
          }),
          listen<TrimStateEvent>("trim-state", ({ payload }) => {
            if (disposed) return;
            applyTrimState(Boolean(payload?.open), "event");
          }),
        ]);
        if (disposed) {
          stopAnnotate();
          stopTrim();
          return;
        }
        unlistenAnnotate = stopAnnotate;
        unlistenTrim = stopTrim;
      } catch (error) {
        console.warn("Hangline panel: editor-state listeners unavailable", error);
        return;
      }
      const snapshotAt = observed;
      try {
        const snapshot = await getEditorState();
        if (disposed || snapshotAt !== observed) return;
        applyAnnotationState(snapshot.annotateOpen, "reconciled");
        applyTrimState(snapshot.trimOpen, "reconciled");
      } catch (error) {
        console.warn("Hangline panel: get_editor_state unavailable", error);
      }
    })();
    return () => {
      disposed = true;
      unlistenAnnotate?.();
      unlistenTrim?.();
    };
  }, [syncBusy]);

  // Unmount (panel window destroyed): release the busy flag so a re-created panel starts free.
  useEffect(() => {
    return () => {
      pointerBusyRef.current = false;
      editorOpenRef.current = false;
      trimOpenRef.current = false;
      menuOpenRef.current = false;
      busyRef.current = false;
      if (menuDismissGuardTimer.current !== null) {
        window.clearTimeout(menuDismissGuardTimer.current);
        menuDismissGuardTimer.current = null;
      }
      void setPanelBusy(false).catch(() => undefined);
    };
  }, []);

  /**
   * Remembers a left-button press on a card. The take-down cross keeps its own click; a native
   * drag can start only from the card surface.
   */
  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>): void {
    if (event.button !== 0) return;
    // An open menu (or the press that just dismissed it) must not start a drag on the card
    // underneath; the same press only closes the menu.
    if (menuSuppressed) return;
    const target = event.target instanceof Element ? event.target : null;
    // The take-down cross and the hover capsule own their presses; they are not drag origins.
    if (!target || target.closest("[data-action]")) return;
    const slot = target.closest<HTMLElement>("[data-item-id]");
    const itemId = slot?.dataset.itemId;
    if (!slot || !itemId) return;
    const item = items.find((candidate) => candidate.id === itemId);
    if (!item) return;

    // A fresh press proves the mouse is back: a lost `drag-ended` must never wedge this gesture.
    // Rust still rejects a second concurrent drag with "already in progress". A new press is also
    // a new gesture, so a long-press from the previous one cannot block this drag.
    suppressDragRef.current = false;
    resetDragState();
    const capture = slot.querySelector<HTMLElement>("article");
    // Capture on the card surface, not the viewport: a plain click must still target the card.
    capture?.setPointerCapture(event.pointerId);
    dragOriginRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      itemId,
      path: item.path,
      capture,
      started: false,
    };
  }

  /** Starts the native drag exactly once, as soon as the press travels more than 4 dp. */
  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>): void {
    if (menuSuppressed) {
      // A menu was open when this press started (or it just dismissed it): never hand the press
      // to the OLE drag loop.
      dragOriginRef.current = null;
      return;
    }
    if (suppressDragRef.current) {
      // The press became a hold and opened the editor: never hand it to the OLE drag loop too.
      dragOriginRef.current = null;
      return;
    }
    const origin = dragOriginRef.current;
    if (!origin || origin.started || origin.pointerId !== event.pointerId) return;
    if (!exceedsDragThreshold(event.clientX - origin.x, event.clientY - origin.y)) return;
    origin.started = true;
    // WebView2 must not begin its own HTML5 drag while the OS drag takes over the same press.
    event.preventDefault();
    startNativeDrag(origin.itemId, origin.path);
  }

  /** Ends the press tracking; the native drag (if any) still ends through `drag-ended`. */
  function handlePointerEnd(event: ReactPointerEvent<HTMLDivElement>): void {
    suppressDragRef.current = false;
    const origin = dragOriginRef.current;
    if (!origin || origin.pointerId !== event.pointerId) return;
    dragOriginRef.current = null;
    if (origin.capture?.hasPointerCapture(event.pointerId)) {
      origin.capture.releasePointerCapture(event.pointerId);
    }
  }

  /**
   * Long-press callback from a card (P3-4 / P3-6).
   *
   * Images open the markup editor, recordings open the trim window. Whatever the kind, a completed
   * hold must not turn into a native drag on the same press.
   */
  const handleLongPress = useCallback(
    (item: Item) => {
      // Whatever the kind, a completed hold must not turn into a native drag on the same press.
      suppressDragRef.current = true;
      openEditorFor(item);
    },
    [openEditorFor],
  );

  // Report a pressed/menu gesture so Rust never tucks the line out from under it.
  useEffect(() => {
    const onPointerDown = () => reportBusy(true);
    const onPointerUp = () => reportBusy(false);
    const onBlur = () => reportBusy(false);
    const onVisibility = () => {
      if (document.visibilityState === "hidden") reportBusy(false);
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("pointerup", onPointerUp, true);
    window.addEventListener("pointercancel", onPointerUp, true);
    window.addEventListener("blur", onBlur);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("pointerup", onPointerUp, true);
      window.removeEventListener("pointercancel", onPointerUp, true);
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("visibilitychange", onVisibility);
      reportBusy(false);
    };
  }, [reportBusy]);

  // Pause rAF while the window is hidden; snap to the logical position so a later reveal is clean.
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState !== "hidden") return;
      stopSpring();
      panelRef.current?.classList.remove(SPRINGING_CLASS);
      motion.current = { x: revealed ? 0 : tuckedOffset(), v: 0 };
      applyOffset(motion.current.x);
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [applyOffset, revealed, stopSpring, tuckedOffset]);

  const collectHitRects = useCallback(async (): Promise<void> => {
    // `devicePixelRatio` is authoritative: a cross-DPI move can keep the same CSS viewport (and
    // fire no resize), but the physical rects Rust tests against scale with the window's DPR.
    const resolved = resolveHitRectScale(scaleRef.current, window.devicePixelRatio);
    const scale = resolved.scale;
    scaleRef.current = scale;
    if (resolved.refreshed) {
      // Re-report after the current collection: the browser may still be re-rasterizing at the new
      // scale, and one extra report is cheaper than a stale 152 dp-wide click target on a 200% screen.
      scheduleReportRef.current();
    }
    const track = trackRef.current;
    if (!track) return;

    // The whole panel is translated during reveal/tuck. `getBoundingClientRect()` includes that
    // transform, but the hit-test thread works in the window's *untransformed* client pixels:
    // subtract the current translateY so a rect reported while tucked is still correct after the
    // spring settles (a transform change does not fire ResizeObserver, so there is no report
    // between frames anyway).
    const offsetY = panelRef.current ? currentTranslateY(panelRef.current) : 0;

    // The viewport rect clips scrolled/overhanging cards: a card outside it must not block clicks.
    const viewport = track.getBoundingClientRect();
    // P2-5: a card that the fly overlay currently draws is invisibly hidden in the panel; it must
    // not report a rest AABB either, otherwise the panel would catch clicks over the frozen screen
    // during the flight (the plan's "hit rects during flight" rule). P2-6: a falling card is hidden
    // for the same reason (the overlay owns it until the item is removed).
    const cards = [...cardRefs.current.values()].filter((card) => {
      const slot = card.closest<HTMLElement>(".glass-card-slot");
      return slot?.dataset.flying !== "true" && slot?.dataset.falling !== "true";
    });

    // Measure the *rest* pose, not a mid-swing snapshot: P2-4 writes the arrival offset, swing,
    // reorder and press-scale through transforms, and a swinging card must never make its click
    // target flicker. Temporarily suppressing the transforms yields the unrotated layout box for
    // every card; the values are restored before this function returns.
    const saved: Array<{
      element: HTMLElement;
      property: string;
      value: string;
      priority: string;
    }> = [];
    const suppressTransform = (element: HTMLElement | null) => {
      if (!element) return;
      saved.push({
        element,
        property: "transform",
        value: element.style.getPropertyValue("transform"),
        priority: element.style.getPropertyPriority("transform"),
      });
      element.style.setProperty("transform", "none", "important");
    };
    for (const card of cards) {
      suppressTransform(card.closest<HTMLElement>(".glass-card-slot"));
      suppressTransform(card.parentElement);
      suppressTransform(card);
    }

    const cardBoxes: CssBox[] = [];
    try {
      for (const card of cards) {
        const rect = card.getBoundingClientRect();
        // P2-6: remember the full rest frame (unclipped, unrotated) for a later fall; the overlay
        // needs the card's start rectangle in physical panel-client pixels. The kind is recorded
        // here (not only on the click path) so a batch removal can skip recordings reliably.
        // Not addressed in P2-7; P2-9 follow-up: the line's slot edge-fade opacity is not frozen
        // into the frame, so a fall starting from the clipped oldest card begins at full opacity.
        const itemId = card.closest<HTMLElement>("[data-item-id]")?.dataset.itemId;
        if (itemId) {
          const item = layoutRef.current.items.find((candidate) => candidate.id === itemId);
          if (item) {
            lastCardKindsRef.current.set(itemId, item.kind);
            lastCardFramesRef.current.set(itemId, {
              rect: {
                x: Math.round(rect.left * scale),
                y: Math.round((rect.top - offsetY) * scale),
                w: Math.max(1, Math.round(rect.width * scale)),
                h: Math.max(1, Math.round(rect.height * scale)),
              },
              tiltDeg: tiltFor(itemId),
              path: item.path,
              imageVersion: item.updatedAt ?? 0,
            });
          }
        }
        cardBoxes.push({ x: rect.left, y: rect.top, w: rect.width, h: rect.height });
      }
    } finally {
      for (const entry of saved) {
        if (entry.value === "") entry.element.style.removeProperty(entry.property);
        else entry.element.style.setProperty(entry.property, entry.value, entry.priority);
      }
    }

    // Overlays are clickable islands outside the card track. The hover capsule hangs below its
    // card, the context menu floats over the cards (its submenu is measured separately), and the
    // P5-2 update notice's `Install update` button sits at the panel bottom. They are already
    // clamped/positioned inside the panel, so they are added without viewport clipping; a closed
    // overlay is `null` and contributes nothing (an empty array is still reported on close).
    const measure = (element: HTMLElement | null): CssBox | null => {
      if (!element || !element.isConnected) return null;
      const rect = element.getBoundingClientRect();
      if (rect.width < 1 || rect.height < 1) return null;
      return { x: rect.left, y: rect.top, w: rect.width, h: rect.height };
    };
    const rects = interactiveRects({
      cards: cardBoxes,
      viewport: {
        x: viewport.left,
        y: viewport.top,
        w: viewport.width,
        h: viewport.height,
      },
      overlays: [
        measure(actionsElRef.current),
        measure(menuElRef.current),
        measure(submenuElRef.current),
        measure(updateNoticeRef.current),
      ],
      scale,
      offsetY,
    });
    await setHitRects(rects);
  }, []);

  const reportNow = useCallback((): Promise<void> => {
    const run = reportChain.current
      .then(() => collectHitRects())
      .catch((error: unknown) => {
        // Plain `vite` (outside Tauri) has no IPC; the panel still renders for styling work.
        console.warn("Hangline panel: hit rect reporting unavailable", error);
      });
    reportChain.current = run;
    return run;
  }, [collectHitRects]);
  // Overlay ref callbacks (capsule/menu/submenu) fire during commit, before layout effects; they
  // route through this ref so a newly mounted capsule/menu is reported without waiting for a frame.
  reportNowRef.current = () => {
    void reportNow();
  };

  /** Coalesces layout/scroll signals into one report per animation frame. */
  const scheduleReport = useCallback(() => {
    if (reportFrame.current !== null) return;
    reportFrame.current = window.requestAnimationFrame(() => {
      reportFrame.current = null;
      void reportNow();
    });
  }, [reportNow]);

  // Keep the ref current for a collection that detects a DPR change and asks for a follow-up
  // report: routing that through a ref avoids a dependency cycle between collect and schedule.
  // Effects run in declaration order, so this assignment precedes the ready-handshake report.
  useEffect(() => {
    scheduleReportRef.current = scheduleReport;
  }, [scheduleReport]);

  // The menu is a click island in the click-through panel. Report it immediately when it opens and
  // when it closes: the close report replaces the old menu rect (possibly with an empty array) so a
  // dismissed menu can never keep swallowing clicks over the desktop. The hover capsule reports
  // itself through its ref callback (`setActionsElement`).
  useEffect(() => {
    void reportNow();
  }, [menu, reportNow]);

  // P2-5/P2-6: re-report the hit rects when the fly/fall hidden sets change so the cards enter and
  // leave the click-through area immediately (a failed take-down rollback must not leave a stale
  // hidden card). Falls also rely on the fresh rest frames written by each report.
  useEffect(() => {
    scheduleReport();
  }, [fallingIds, flyingId, scheduleReport]);

  // P5-2: update notices. Listeners are registered before `get_pending_update`, so a check that
  // finished while the webview was loading is caught by the initial query instead of being lost.
  // Automatic checks only ever produce `update-available`; the manual-check events carry the
  // "checking"/"couldn't check" feedback for the tray action.
  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const announce = (info: UpdateInfo) => {
      if (disposed) return;
      clearUpdateNoticeTimer();
      setUpdateNotice((current) =>
        current?.kind === "installing" ? current : { kind: "available", info },
      );
      scheduleReportRef.current();
    };
    void Promise.all([
      listen<UpdateInfo>("update-available", ({ payload }) => {
        if (payload) announce(payload);
      }),
      listen<UpdateCheckStartedEvent>("update-check-started", () => {
        if (disposed) return;
        setUpdateNotice((current) =>
          current?.kind === "available" || current?.kind === "installing"
            ? current
            : { kind: "checking" },
        );
        scheduleReportRef.current();
      }),
      listen<UpdateCheckFinishedEvent>("update-check-finished", () => {
        if (disposed) return;
        setUpdateNotice((current) => (current?.kind === "checking" ? null : current));
        scheduleReportRef.current();
      }),
      listen<UpdateCheckFailedEvent>("update-check-failed", ({ payload }) => {
        if (disposed) return;
        showUpdateFailure("check-failed", payload?.message);
        scheduleReportRef.current();
      }),
      listen<UpdateProgressEvent>("update-progress", ({ payload }) => {
        if (!disposed && payload) setUpdateProgress(payload);
      }),
    ])
      .then(async (registered) => {
        if (disposed) {
          registered.forEach((unlisten) => unlisten());
          return;
        }
        unlisteners.push(...registered);
        try {
          const pending = await getPendingUpdate();
          if (!disposed && pending) announce(pending);
        } catch (error) {
          console.warn("Hangline panel: get_pending_update unavailable", error);
        }
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: update listeners unavailable", error);
      });
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
      clearUpdateNoticeTimer();
    };
  }, [clearUpdateNoticeTimer, showUpdateFailure]);

  // Start the IPC state handshake once for this window.
  useEffect(() => {
    initAppState();
  }, []);

  // The click/double-click arbitration uses the user's real Windows double-click speed. If IPC is
  // unavailable (plain `vite`), the Windows default of 500 ms keeps the panel usable.
  useEffect(() => {
    let disposed = false;
    void getDoubleClickIntervalMs()
      .then((milliseconds) => {
        if (disposed || !Number.isFinite(milliseconds)) return;
        setDoubleClickMs(Math.max(0, Math.round(milliseconds)));
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: get_double_click_interval_ms unavailable", error);
      });
    return () => {
      disposed = true;
    };
  }, []);

  // Ready handshake order matters: Rust shows the hidden panel only after the first hit rects
  // are in place, so the first visible frame already ignores clicks in transparent areas.
  useEffect(() => {
    let disposed = false;
    void (async () => {
      scaleRef.current = null;
      await reportNow();
      if (disposed) return;
      try {
        await reportPanelReady("panel");
      } catch (error) {
        console.warn("Hangline panel: ready handshake unavailable", error);
      }
    })();
    return () => {
      disposed = true;
    };
  }, [reportNow]);

  // Re-report whenever the geometry the rects depend on changes. `draggingId` matters because the
  // drag state changes the card's painted material (and P2-4's JS press scale will change its box):
  // without a re-report the hit rects would stay stale after the visual resets, leaving clickable
  // areas that no longer match the cards. `offset`/`lineLimit` cover the Task 14 scroll and
  // keepOnLine changes: a wheel step re-reports the physical rects of the newly visible cards.
  useLayoutEffect(() => {
    scheduleReport();
  }, [
    items,
    size,
    panelWidth,
    draggingId,
    offset,
    lineLimit,
    scheduleReport,
    updateNotice,
    updateProgress,
  ]);

  // Fonts and lazy thumbnails can change a card's painted box after the first layout.
  useEffect(() => {
    const track = trackRef.current;
    const panel = panelRef.current;
    let disposed = false;

    const onScroll = () => scheduleReport();
    const onLoad = () => scheduleReport();
    const onResize = () => {
      // A resize may also mean a DPI change (new monitor): re-seed the cache from the browser so the
      // very next collection uses the new scale. Equal-logical-size moves never resize; the DPR
      // watch below covers those.
      scaleRef.current = window.devicePixelRatio;
      scheduleReport();
    };

    track?.addEventListener("scroll", onScroll, { passive: true });
    // `load` does not bubble, but capture catches every <img> inside the track.
    track?.addEventListener("load", onLoad, true);
    window.addEventListener("resize", onResize);
    const observer = new ResizeObserver(() => scheduleReport());
    if (panel) observer.observe(panel);
    if (document.fonts) {
      void document.fonts.ready
        .then(() => {
          if (!disposed) scheduleReport();
        })
        .catch(() => {
          /* font loading is best effort; layout is already reported */
        });
    }

    return () => {
      disposed = true;
      track?.removeEventListener("scroll", onScroll);
      track?.removeEventListener("load", onLoad, true);
      window.removeEventListener("resize", onResize);
      observer.disconnect();
      if (reportFrame.current !== null) {
        window.cancelAnimationFrame(reportFrame.current);
        reportFrame.current = null;
      }
    };
  }, [scheduleReport]);

  // A cross-DPI move with an unchanged CSS viewport (e.g. 1920 dp @100% -> 1920 dp @200%) fires
  // neither a resize nor a ResizeObserver callback, so the hit-rect scale needs its own watch.
  // 500 ms is invisible to the user and costs one property read; a hidden panel (document hidden)
  // is skipped because Rust already stops its hit-test loop while hidden.
  useEffect(() => {
    let disposed = false;
    let last = window.devicePixelRatio;
    const check = () => {
      if (disposed || document.visibilityState === "hidden") return;
      const dpr = window.devicePixelRatio;
      if (!Number.isFinite(dpr) || dpr <= 0 || dpr === last) return;
      last = dpr;
      scaleRef.current = dpr;
      console.info(`Hangline panel: devicePixelRatio changed to ${dpr}; re-reporting hit rects`);
      scheduleReport();
    };
    const timer = window.setInterval(check, DPI_WATCH_MS);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [scheduleReport]);

  // Rust owns the panel window geometry (Task 14): `set_setting` re-applies it when `cardSize`/
  // `keepOnLine` change, and the 30 Hz reveal controller follows the pointer across monitors. This
  // call is the startup resync plus the "all monitors + Rust-chosen target" log; the frontend
  // deliberately does not call it again after a cardSize change so there is one geometry owner.
  useEffect(() => {
    let disposed = false;
    void syncPanelGeometry()
      .then((geometry) => {
        if (disposed) return;
        const origin = {
          x: geometry.position?.x ?? 0,
          y: geometry.position?.y ?? 0,
        };
        panelOriginRef.current = origin;
        setPanelOrigin(origin);
        for (const monitor of geometry.monitors) {
          console.info(
            `Hangline panel: monitor ${monitor.id} ${monitor.rect.w}x${monitor.rect.h} at ` +
              `(${monitor.rect.x}, ${monitor.rect.y}), work area ${monitor.workArea.w}x` +
              `${monitor.workArea.h} at (${monitor.workArea.x}, ${monitor.workArea.y}), ` +
              `scale ${monitor.scaleFactor}${monitor.isPrimary ? ", primary" : ""}`,
          );
        }
        const target = geometry.target;
        if (!target) {
          console.warn("Hangline panel: Rust did not choose a target monitor");
        } else {
          const pixels = geometry.size;
          const position = geometry.position;
          console.info(
            `Hangline panel: Rust target ${target.id} (work area ${target.workArea.w}x` +
              `${target.workArea.h} px, scale ${target.scaleFactor}); window ` +
              `${pixels ? `${pixels.width}x${pixels.height} px` : "unknown size"} at ` +
              `${position ? `(${position.x}, ${position.y})` : "unknown position"}; ` +
              `moved=${geometry.moved}`,
          );
        }
        // Rust has finished any window move/resize by the time the command resolves: re-seed the
        // hit-rect scale from the browser's DPR and re-report at the new geometry (the move may
        // have landed on a different-DPI monitor without a frontend-initiated resize).
        scaleRef.current = window.devicePixelRatio;
        scheduleReport();
      })
      .catch((error: unknown) => {
        console.warn("Hangline panel: sync_panel_geometry unavailable", error);
      });
    return () => {
      disposed = true;
    };
  }, [scheduleReport]);

  const setCardRef = useCallback((id: string, element: HTMLElement | null) => {
    if (element) cardRefs.current.set(id, element);
    else cardRefs.current.delete(id);
  }, []);

  const count = visible.length;
  // The visible slice normally fits the panel (the capacity formula leaves 200 dp of margin).
  // Only a panel narrower than the minimum 3 card group overflows; it is right-aligned so the
  // newest card stays fully visible (the oldest edge is clipped, with no native scrollbar).
  const line = lineWidth(panelWidth, count, size);
  const shift = lineShift(panelWidth, line);
  const lineStyle = {
    width: `${line}px`,
    transform: `translateX(${shift}px)`,
  } as CSSProperties;

  // Rows for the open menu (Wave 1.1 + Wave 2.3). Built from the *live* item list so the side
  // take-downs are disabled correctly when the line changes under an open menu; `panelHeight`
  // applies the same compaction the open path used, so the rendered height matches the clamped
  // position. A menu whose item disappeared is closed by the effect above and renders nothing.
  const menuItem = menu ? (items.find((item) => item.id === menu.itemId) ?? null) : null;
  const menuNodes =
    menu && menuItem
      ? menuItemsFor(menuItem, {
          index: items.findIndex((item) => item.id === menuItem.id),
          total: items.length,
          panelHeight: window.innerHeight,
        })
      : null;

  // P5-2 notice text. The pill is deliberately small: availability + one action, manual-check
  // feedback, or the coarse download percentage while installing.
  const updatePercent =
    updateProgress?.total && updateProgress.total > 0
      ? Math.min(100, Math.round((updateProgress.chunk / updateProgress.total) * 100))
      : null;
  let updateNoticeLabel: string | null = null;
  let updateNoticeDetail: string | undefined;
  if (updateNotice?.kind === "checking") {
    updateNoticeLabel = t("Checking for updates…");
  } else if (updateNotice?.kind === "available") {
    updateNoticeLabel = `${t("Update available")} — v${updateNotice.info.version}`;
    updateNoticeDetail = updateNotice.info.notes ?? undefined;
  } else if (updateNotice?.kind === "installing") {
    updateNoticeLabel =
      updatePercent === null
        ? t("Installing update…")
        : `${t("Installing update…")} ${updatePercent}%`;
    updateNoticeDetail = updateNotice.info.notes ?? undefined;
  } else if (updateNotice?.kind === "check-failed") {
    updateNoticeLabel = t("Couldn't check for updates");
    updateNoticeDetail = updateNotice.detail;
  } else if (updateNotice?.kind === "install-failed") {
    updateNoticeLabel = t("Update failed");
    updateNoticeDetail = updateNotice.detail;
  }

  return (
    <div
      className="panel"
      ref={panelRef}
      data-theme={theme}
      data-card-size={size}
      data-panel-width={Math.round(panelWidth)}
      data-capacity={fits}
      data-card-count={count}
      data-keep={keepSetting === "fit" ? "fit" : lineLimit}
      data-offset={offset}
      data-max-offset={maxOffset}
      data-range-start={range.start}
      data-range-end={range.end}
      data-revealed={revealed}
      data-editor-open={editorOpen}
      data-trim-open={trimOpen}
      data-busy={busyReported}
      data-gust={gust}
      data-origin-x={panelOrigin.x}
      data-origin-y={panelOrigin.y}
      data-falling-count={fallingIds.size}
      data-card-menu-open={menu !== null}
      // The panel is a custom chrome: the WebView2 default context menu must never appear, not
      // even on the transparent studio area around the line (Wave 1.1).
      onContextMenu={(event) => event.preventDefault()}
    >
      <div
        className="panel__viewport"
        ref={trackRef}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerEnd}
        onPointerCancel={handlePointerEnd}
        onLostPointerCapture={handlePointerEnd}
        onDragStart={(event) => event.preventDefault()}
      >
        <div className="panel__line" style={lineStyle}>
          <Rope width={line} />
          {count === 0 ? (
            <p
              className="panel__hint"
              style={{ top: `${ropeY(panelWidth, panelWidth / 2) + 34}px` }}
            >
              {t("Take a screenshot and it will hang here")}
            </p>
          ) : null}
          {visible.map((item, index) => {
            const x = cardX(index, count, size, panelWidth);
            const top = pinTop(ropeY(line, x + cardWidth(size) / 2));
            return (
              <GlassCard
                key={item.id}
                item={item}
                index={index}
                size={size}
                x={x}
                top={top}
                panelWidth={panelWidth}
                lineShiftPx={shift}
                paused={!revealed || documentHidden || editorOpen || trimOpen}
                gust={gust}
                flyState={flyingId === item.id ? "flying" : "none"}
                falling={fallingIds.has(item.id)}
                onFallStart={handleFallStart}
                onTakeDownFailed={handleTakeDownFailed}
                onLongPress={handleLongPress}
                onEdit={openEditorFor}
                onContextMenu={openCardMenuAt}
                onMoreActions={openCardMenuAtElement}
                onActionsElement={setActionsElement}
                gesturesLocked={menuSuppressed}
                reducedMotion={reducedMotion}
                doubleClickMs={doubleClickMs}
                dragging={draggingId === item.id}
                cardRef={(element) => setCardRef(item.id, element)}
              />
            );
          })}
        </div>
      </div>
      {updateNoticeLabel ? (
        <div
          className="panel__update"
          data-testid="update-notice"
          data-kind={updateNotice?.kind}
          data-version={
            updateNotice?.kind === "available" || updateNotice?.kind === "installing"
              ? updateNotice.info.version
              : undefined
          }
          data-progress={updatePercent ?? undefined}
          ref={updateNoticeRef}
          role="status"
          title={updateNoticeDetail}
        >
          <span className="panel__update-label">{updateNoticeLabel}</span>
          {updateNotice?.kind === "available" ? (
            <button
              type="button"
              className="panel__update-action"
              data-action="install-update"
              onClick={startUpdateInstall}
            >
              {t("Install update")}
            </button>
          ) : null}
        </div>
      ) : null}
      {/*
        Card context menu (Wave 1.1): rendered inside the panel window (no second window, no window
        resize). `CardContextMenu` measures itself through `setMenuElement`/`setSubmenuElement`; the
        panel folds those DOM boxes into the click-through hit rects so the menu is clickable.
      */}
      {menu && menuNodes ? (
        <CardContextMenu
          items={menuNodes}
          x={menu.x}
          y={menu.y}
          submenuOpen={menu.submenuOpen}
          panelWidth={window.innerWidth}
          onSubmenuOpenChange={setMenuSubmenu}
          onAction={runMenuAction}
          menuRef={setMenuElement}
          submenuRef={setSubmenuElement}
        />
      ) : null}
    </div>
  );
}
