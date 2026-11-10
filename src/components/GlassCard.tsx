/**
 * One glass card hanging from the rope (design spec §3.3 / §3.1, Tendedero `PeggedView.swift`).
 *
 * Geometry: the slot is the fixed `cardWidth(size)` cell used by the line layout; the pin sits at
 * the slot's top centre, its top `PIN_ABOVE` above the rope, and the visual card (sized to the
 * photo + a 4 dp glass frame) is centred below it, overlapping the pin by `CARD_PIN_OVERLAP`.
 * `.glass-card-unit` carries the P2-4 motion: a static `--tilt` plus the hook's `--swing` /
 * `--arrive-y`; the card element carries the inherited `--card-scale`.
 *
 * Motion (P2-4): one rAF per card via `useCardMotion`; hover/press targets come from pointer events
 * on the article, gusts from the panel's timer (`gust` prop), fly-in landing from `flyState`.
 * Reorders spring the slot through `--slot-dx/--slot-dy`; the slot also fades near the line edges.
 *
 * The glass look is simulated with layered gradients, a 0.75 dp inner top highlight, a 0.5 dp
 * inset ring and a drop shadow. `backdrop-filter` cannot blur the desktop through a transparent
 * WebView2 window, so it is only a progressive enhancement and the gradients carry the material.
 *
 * Task 12 gestures:
 * - single click: copy after the system double-click interval, then show the `Copied` pill 1.2 s;
 * - double click: cancel the pending copy and open with the default app (no clipboard change);
 * - top-left cross (26 dp hit area): take the card down; Rust decides whether the file is recycled
 *   (only Hangline-owned screenshot files; watched-folder items just leave the line).
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, RefObject } from "react";
import { assetUrl } from "../lib/assetUrl";
import { HOLD_CLICK_GUARD_MS, createCardGesture, shouldRunCardClick, shouldStartCardPress, shouldSuppressCardClick } from "../lib/cardGesture";
import type { CardGesture } from "../lib/cardGesture";
import { gustAmplitude, gustDelay } from "../lib/gust";
import { t } from "../lib/i18n";
import { copyItem, openItem, takeDown } from "../lib/ipc";
import { createLongPress, LONG_PRESS_MOVE_THRESHOLD_PX } from "../lib/longPress";
import type { LongPress } from "../lib/longPress";
import { PREVIEW_HOVER_DELAY_MS, shouldStartPreviewTimer } from "../lib/panel";
import {
  CARD_PIN_OVERLAP,
  CARD_RADIUS,
  PIN_HEIGHT,
  cardVisualSize,
  cardWidth,
  edgeFadeOpacity,
  fileNameOf,
  photoSize,
  showCardImage,
  tiltFor,
} from "../lib/layout";
import type { CardImageSource } from "../lib/layout";
import { SPRINGS } from "../lib/motion";
import { tick as springTick } from "../lib/spring";
import { useCardMotion } from "../lib/useCardMotion";
import { createCopiedPillTimer } from "../lib/pillTimer";
import type { CopiedPillTimer } from "../lib/pillTimer";
import type { CardSize, Item } from "../lib/types";
import { ClothesPin } from "./ClothesPin";
import { CopiedPill } from "./CopiedPill";

/** Windows' default double-click interval, used until `GetDoubleClickTime` answers. */
const DEFAULT_DOUBLE_CLICK_MS = 500;

/** Vertical offset of the card top from the pin top: pin height minus the 12 dp overlap (14 dp). */
const CARD_TOP_FROM_PIN = PIN_HEIGHT - CARD_PIN_OVERLAP;

/** Slot spring thresholds: a fraction of a CSS pixel / px per second counts as at rest. */
const SLOT_REST_PX = 0.05;
const SLOT_REST_VELOCITY = 0.5;

interface SlotSpringOffset {
  x: number;
  vx: number;
  y: number;
  vy: number;
}

function writeSlotOffset(element: HTMLElement | null, offset: SlotSpringOffset): void {
  if (!element) return;
  element.style.setProperty("--slot-dx", `${offset.x}px`);
  element.style.setProperty("--slot-dy", `${offset.y}px`);
}

/**
 * Springs the slot from its previous position to the new one when a reorder moves the card.
 *
 * The slot's `left/top` are already the new layout position; the visual offset starts at
 * `previous − next` and relaxes to 0 with `SPRINGS.item`, so reorders glide instead of jumping.
 * The first render starts exactly at the target (no animation), reduced motion snaps, and a paused
 * panel snaps too (it is hidden, so an unseen animation would be wasted).
 */
function useSlotSpring(
  slotRef: RefObject<HTMLElement | null>,
  x: number,
  y: number,
  paused: boolean,
  reducedMotion: boolean,
): void {
  const targetRef = useRef<{ x: number; y: number } | null>(null);
  const offsetRef = useRef<SlotSpringOffset>({ x: 0, vx: 0, y: 0, vy: 0 });
  const frameRef = useRef<number | null>(null);
  const lastTsRef = useRef<number | null>(null);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  /** Latest `start`, so the frame callback can schedule the next frame without a deps cycle. */
  const startRef = useRef<() => void>(() => {});

  const stop = useCallback(() => {
    if (frameRef.current !== null) {
      window.cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
    lastTsRef.current = null;
  }, []);

  const start = useCallback(() => {
    if (pausedRef.current || frameRef.current !== null) return;
    frameRef.current = window.requestAnimationFrame(function frame(timestamp) {
      frameRef.current = null;
      if (pausedRef.current) {
        lastTsRef.current = null;
        return;
      }
      const last = lastTsRef.current;
      const dt = last === null ? 1 / 60 : Math.min(Math.max((timestamp - last) / 1000, 0), 1 / 30);
      lastTsRef.current = timestamp;
      const offset = offsetRef.current;
      const nextX = springTick(
        { x: offset.x, v: offset.vx },
        0,
        SPRINGS.item.stiffness,
        SPRINGS.item.damping,
        dt,
      );
      const nextY = springTick(
        { x: offset.y, v: offset.vy },
        0,
        SPRINGS.item.stiffness,
        SPRINGS.item.damping,
        dt,
      );
      offsetRef.current = { x: nextX.x, vx: nextX.v, y: nextY.x, vy: nextY.v };
      writeSlotOffset(slotRef.current, offsetRef.current);
      const settledX =
        Math.abs(nextX.x) < SLOT_REST_PX && Math.abs(nextX.v) < SLOT_REST_VELOCITY;
      const settledY =
        Math.abs(nextY.x) < SLOT_REST_PX && Math.abs(nextY.v) < SLOT_REST_VELOCITY;
      if (settledX && settledY) {
        offsetRef.current = { x: 0, vx: 0, y: 0, vy: 0 };
        writeSlotOffset(slotRef.current, offsetRef.current);
        lastTsRef.current = null;
        return;
      }
      startRef.current();
    });
  }, [slotRef]);
  startRef.current = start;

  // Snap the visual offset when the panel is hidden or reduced motion is active.
  useEffect(() => {
    if (!paused && !reducedMotion) return;
    stop();
    offsetRef.current = { x: 0, vx: 0, y: 0, vy: 0 };
    writeSlotOffset(slotRef.current, offsetRef.current);
  }, [paused, reducedMotion, slotRef, stop]);

  // A reorder (x/top change) starts the spring from the delta between the old and new layout.
  // Layout effect: `--slot-dx/dy` must be committed before the browser paints the new position,
  // otherwise a reordered card flashes at its destination for one frame.
  useLayoutEffect(() => {
    const previous = targetRef.current;
    targetRef.current = { x, y };
    if (previous === null) return;
    const dx = previous.x - x;
    const dy = previous.y - y;
    if (dx === 0 && dy === 0) return;
    if (pausedRef.current || reducedMotion) {
      offsetRef.current = { x: 0, vx: 0, y: 0, vy: 0 };
      writeSlotOffset(slotRef.current, offsetRef.current);
      return;
    }
    offsetRef.current = {
      x: offsetRef.current.x + dx,
      vx: 0,
      y: offsetRef.current.y + dy,
      vy: 0,
    };
    writeSlotOffset(slotRef.current, offsetRef.current);
    start();
  }, [x, y, reducedMotion, slotRef, start]);

  useEffect(() => stop, [stop]);
}

export interface GlassCardProps {
  item: Item;
  /** Position in the visible slice; used for data attributes/runtime evidence. */
  index: number;
  size: CardSize;
  /** Left edge of the fixed-width slot inside the line, in dp. */
  x: number;
  /** Slot top = pin top = `pinTop(ropeY)` at the card's centre, in dp. */
  top: number;
  /** Panel width in dp, used for the edge fade (`LineView.swift:78`). */
  panelWidth: number;
  /** Right-alignment shift of an overflowing line, so the fade uses panel coordinates. */
  lineShiftPx?: number;
  /** Panel hidden/tucked: card motion stops without advancing (P2-4). */
  paused?: boolean;
  /** Gust counter from the panel timer; a change triggers one breeze per card (P2-4). */
  gust?: number;
  /** Fly-in overlay state (P2-5): hide the card while flying, land with a sway afterwards. */
  flyState?: "none" | "flying";
  /** P2-6: the fall overlay is drawing this card; hide the panel copy immediately. */
  falling?: boolean;
  /**
   * P2-6: called synchronously right before a take-down so the panel can start the fall overlay
   * animation for this card (the card hides immediately; Rust `take_down` still removes the item).
   */
  onFallStart?: (item: Item, element: HTMLElement | null) => void;
  /** `prefers-reduced-motion` from the panel (P2-4); snaps the motion state. */
  reducedMotion?: boolean;
  /** Ref to the rotated glass surface — the source of the reported physical hit rect. */
  cardRef?: (element: HTMLElement | null) => void;
  /** True while this card's native drag-out is in flight (the OS drag loop owns the pointer). */
  dragging?: boolean;
  /** System double-click interval in milliseconds. */
  doubleClickMs?: number;
  /**
   * Optional gesture overrides (Task 13/Task 15 reuse the card surface).
   *
   * When omitted the card runs the built-in `copy_item` / `open_item` / `take_down` commands.
   * Supplying one replaces only that action; the gesture timing is unchanged.
   */
  onCopy?: (item: Item) => void;
  onOpen?: (item: Item) => void;
  onRemove?: (item: Item) => void;
  /**
   * Called when a 450 ms hold completes (design spec §3.1/§3.7). The panel opens the markup editor
   * for images (P3-4) and the trim window for recordings (P3-6).
   */
  onLongPress?: (item: Item) => void;
  /**
   * P2-6: the built-in `take_down` failed and Rust kept the item; the panel rolls back the hidden
   * falling state. The error is only for logging (the panel's recovery is unconditional).
   */
  onTakeDownFailed?: (id: string, error: unknown) => void;
  /**
   * A card menu (or its just-closed dismiss guard) is active: click/double-click/long-press/drag
   * are suspended until it closes. Decided by the panel, which owns the menu state.
   */
  gesturesLocked?: boolean;
  /** Right-click on the card: opens the panel's in-window context menu at the pointer. */
  onContextMenu?: (item: Item, clientX: number, clientY: number) => void;
  /** Hover capsule's `More actions`: opens the same menu anchored to the capsule button. */
  onMoreActions?: (item: Item, anchor: HTMLElement) => void;
  /** Hover capsule's middle action (markup for images, trim for recordings). */
  onEdit?: (item: Item) => void;
  /** Reports the mounted hover capsule so the panel adds its rect to the click-through hit rects. */
  onActionsElement?: (element: HTMLElement | null) => void;
  /**
   * Wave 2.4 hover preview: after the 300 ms dwell the card asks the panel to show the enlarged
   * original (`item` + the card element used for geometry); `null` hides it immediately.
   *
   * The panel owns the single rendered preview — cards never render large images themselves.
   */
  onPreviewRequest?: (item: Item | null, card: HTMLElement | null) => void;
}

export function GlassCard({
  item,
  index,
  size,
  x,
  top,
  panelWidth,
  lineShiftPx = 0,
  paused = false,
  gust = 0,
  flyState = "none",
  falling = false,
  onFallStart,
  reducedMotion = false,
  cardRef,
  dragging = false,
  doubleClickMs = DEFAULT_DOUBLE_CLICK_MS,
  onCopy,
  onOpen,
  onRemove,
  onLongPress,
  onTakeDownFailed,
  gesturesLocked = false,
  onContextMenu,
  onMoreActions,
  onEdit,
  onActionsElement,
  onPreviewRequest,
}: GlassCardProps) {
  const slotWidth = cardWidth(size);
  const visual = cardVisualSize(item, size);
  const photo = photoSize(item.width, item.height, size);
  const tilt = tiltFor(item.id);
  const name = fileNameOf(item.path);
  const isRecording = item.kind === "recording";
  const slotRef = useRef<HTMLDivElement | null>(null);
  const unitRef = useRef<HTMLDivElement | null>(null);
  const motion = useCardMotion(unitRef, { reducedMotion, paused });
  const { arrive, land, copied, breeze, setTargets } = motion;
  const [hovered, setHovered] = useState(false);
  /** Latest pointer state, mirrored in refs so pointer handlers build the target in one place. */
  const hoveredRef = useRef(false);
  const pressedRef = useRef(false);
  /**
   * P2-7 image fallback chain: cached thumb → original file → tile placeholder.
   *
   * A card whose thumb generation failed (HEIC, corrupt file) has no `thumbPath` and starts
   * directly at the original path; an unreadable thumb falls back to the original once. P3-5
   * recordings only render an image when their first-frame preview exists.
   */
  const [imageSource, setImageSource] = useState<CardImageSource>(() =>
    item.thumbPath ? "thumb" : item.kind === "recording" ? "failed" : "original",
  );
  /** Whether the 1.2 s `Copied` pill is currently in the card's status area. */
  const [pillVisible, setPillVisible] = useState(false);
  const pillTimer = useRef<CopiedPillTimer | null>(null);
  const gesture = useRef<CardGesture | null>(null);
  /** The 450 ms hold controller (created once; its callback reads the latest props through refs). */
  const longPress = useRef<LongPress | null>(null);
  /** Latest hold handler, so recreating the controller is never necessary on a prop change. */
  const longPressAction = useRef<() => void>(() => {});
  /** `performance.now()` of the last fired hold; the post-fire click guard is derived from it. */
  const holdFiredAt = useRef<number | null>(null);
  /** Press origin for the 4 dp movement check that separates a click from a drag. */
  const pressOrigin = useRef<{ x: number; y: number } | null>(null);
  /**
   * The current press travelled past the 4 dp drag threshold.
   *
   * The OLE drag loop normally swallows the release, but when the browser still emits a `click`
   * after a moved press that click must not copy (and a `dblclick` must not open).
   */
  const movedPress = useRef(false);
  const flyStateRef = useRef(flyState);
  const gustRef = useRef(gust);
  /** Latest item snapshot for the stable gesture callbacks.
   *
   * A `state` broadcast replaces every item object even when the item is unchanged. If the copy
   * callback closed over `item` directly, its identity would change on every broadcast and the
   * gesture effect would recreate the controller, silently cancelling a pending single-click copy.
   */
  const itemRef = useRef(item);
  /** False after unmount: a delayed copy promise must not set state or nudge a detached card. */
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  /** The article element, needed to re-sync hover from the real pointer state after a drag. */
  const articleRef = useRef<HTMLElement | null>(null);
  /** Combined article ref: keeps the local hover probe and the panel's hit-rect reporter in sync. */
  const attachCardRef = useCallback(
    (element: HTMLElement | null) => {
      articleRef.current = element;
      cardRef?.(element);
    },
    [cardRef],
  );
  /** Combined hover-capsule ref: the panel measures it so the capsule is a click island. */
  const attachActionsRef = useCallback(
    (element: HTMLDivElement | null) => {
      onActionsElement?.(element);
    },
    [onActionsElement],
  );

  // --- Wave 2.4 hover preview: 300 ms dwell, one request per pointer enter ----------------------
  /** Pending dwell timer (armed on enter / capsule leave, cleared on leave / interaction). */
  const previewTimerRef = useRef<number | null>(null);
  /** A preview request is currently shown by the panel (so a hide request is needed). */
  const previewShownRef = useRef(false);
  const clearPreviewTimer = useCallback(() => {
    if (previewTimerRef.current !== null) {
      window.clearTimeout(previewTimerRef.current);
      previewTimerRef.current = null;
    }
  }, []);
  /** Hides an already-visible preview (and drops a pending one); safe to call repeatedly. */
  const cancelPreview = useCallback(() => {
    clearPreviewTimer();
    if (!previewShownRef.current) return;
    previewShownRef.current = false;
    onPreviewRequest?.(null, null);
  }, [clearPreviewTimer, onPreviewRequest]);
  /**
   * Arms the dwell timer once per pointer enter.
   *
   * Mousemove inside the card never restarts it (no per-frame state churn); the callback
   * re-validates the gate so a menu open / drag / pause that starts inside the 300 ms window wins.
   */
  const armPreviewTimer = useCallback(() => {
    if (!onPreviewRequest) return;
    if (
      !shouldStartPreviewTimer({
        hovering: hoveredRef.current,
        recording: isRecording,
        locked: gesturesLocked,
        paused,
        dragging,
      })
    ) {
      return;
    }
    clearPreviewTimer();
    previewTimerRef.current = window.setTimeout(() => {
      previewTimerRef.current = null;
      if (previewShownRef.current || !hoveredRef.current) return;
      if (gesturesLocked || paused || dragging) return;
      const card = articleRef.current;
      if (!card) return;
      previewShownRef.current = true;
      onPreviewRequest(itemRef.current, card);
    }, PREVIEW_HOVER_DELAY_MS);
  }, [clearPreviewTimer, dragging, gesturesLocked, isRecording, onPreviewRequest, paused]);

  // Anything that suspends card interaction also hides an already-visible preview immediately.
  useEffect(() => {
    if (paused || dragging || gesturesLocked) cancelPreview();
  }, [cancelPreview, dragging, gesturesLocked, paused]);

  // A detached card must not fire the delayed request or leave a timer behind.
  useEffect(() => {
    return () => clearPreviewTimer();
  }, [clearPreviewTimer]);

  useSlotSpring(slotRef, x, top, paused, reducedMotion);

  const syncTargets = useCallback(() => {
    setTargets({ hovered: hoveredRef.current, pressed: pressedRef.current });
  }, [setTargets]);

  /** Drops every hover/press target: used when the pointer is gone or the panel is not live. */
  const clearTargets = useCallback(() => {
    hoveredRef.current = false;
    pressedRef.current = false;
    setHovered(false);
    setTargets({ hovered: false, pressed: false });
  }, [setTargets]);

  /** Latest `dragging` value, so the effect below can tell drag-start from drag-end. */
  const draggingRef = useRef(dragging);

  // A native drag owns the pointer (the OS loop swallows enter/leave/up until it ends) and a tuck
  // pauses the panel: both would otherwise keep a stale hover shadow / 0.95 press pose around.
  useEffect(() => {
    const wasDragging = draggingRef.current;
    draggingRef.current = dragging;
    if (dragging) {
      longPress.current?.cancel();
      clearTargets();
      return;
    }
    if (!wasDragging) return;
    // Drag just ended. The OS loop swallowed pointer events and no `pointerenter` fires while the
    // pointer never left the card, so re-sync from the real hover state instead of leaving the
    // card un-hovered until the next mouse move.
    const hovering = articleRef.current?.matches(":hover") ?? false;
    hoveredRef.current = hovering;
    pressedRef.current = false;
    setHovered(hovering);
    setTargets({ hovered: hovering, pressed: false });
  }, [clearTargets, dragging, setTargets]);

  // A tuck hides the panel: clear the pose so it cannot reappear on reveal, and drop a pending hold.
  useEffect(() => {
    if (!paused) return;
    longPress.current?.cancel();
    clearTargets();
  }, [clearTargets, paused]);

  // Arrival: drop in once, unless the fly overlay is bringing the card in (P2-5 calls `land`).
  // Layout effect: `arrive()` publishes `--swing: 16deg` / `--arrive-y: -46px` synchronously, so
  // the very first painted frame of a newly inserted card is already the animated start pose.
  useLayoutEffect(() => {
    if (flyStateRef.current !== "flying") arrive();
  }, [arrive]);

  // Fly-in handoff: the overlay has landed the card on the line, so sway from rest.
  useEffect(() => {
    const previous = flyStateRef.current;
    flyStateRef.current = flyState;
    if (previous === "flying" && flyState === "none") land();
  }, [flyState, land]);

  // One breeze per gust counter increment; the panel already gates hidden/reduced-motion timers,
  // but re-check here so a stale tick can never animate a paused card.
  useEffect(() => {
    if (gust === gustRef.current) return;
    gustRef.current = gust;
    if (paused || reducedMotion) return;
    breeze(gustAmplitude(Math.random()), gustDelay(Math.random()));
  }, [gust, paused, reducedMotion, breeze]);

  useEffect(() => {
    itemRef.current = item;
  }, [item]);

  // Annotating a card rewrites its file in place, and the startup backfill can add a thumb later:
  // give the new picture (and the cache-first path) a fresh chance to load. `updatedAt` changes on
  // every markup save/trim, which also recovers a card whose image had failed to decode before the
  // edit. A recording without a preview can never fall back to the mp4, so it starts at `failed`
  // (filename tile + play badge) until the backfill/refresh attaches one.
  useEffect(() => {
    setImageSource(
      item.thumbPath ? "thumb" : item.kind === "recording" ? "failed" : "original",
    );
  }, [item.kind, item.path, item.thumbPath, item.updatedAt]);

  // One restartable 1.2 s pill timer per mounted card; cancelled on unmount.
  useEffect(() => {
    const timer = createCopiedPillTimer(() => setPillVisible(false));
    pillTimer.current = timer;
    return () => {
      timer.cancel();
      if (pillTimer.current === timer) pillTimer.current = null;
    };
  }, []);

  const showCopied = useCallback(() => {
    if (!mountedRef.current) return;
    setPillVisible(true);
    pillTimer.current?.restart();
    copied();
  }, [copied]);

  const handleCopy = useCallback(() => {
    const current = itemRef.current;
    if (onCopy) {
      onCopy(current);
      copied();
      return;
    }
    void copyItem(current.id).then(showCopied, (error: unknown) => {
      console.warn(`Hangline card: copy of ${current.path} failed`, error);
    });
  }, [copied, onCopy, showCopied]);

  const handleOpen = useCallback(() => {
    const current = itemRef.current;
    if (onOpen) {
      onOpen(current);
      return;
    }
    void openItem(current.id).catch((error: unknown) => {
      console.warn(`Hangline card: open of ${current.path} failed`, error);
    });
  }, [onOpen]);

  const handleTakeDown = useCallback(() => {
    gesture.current?.cancel();
    longPress.current?.cancel();
    setPillVisible(false);
    pillTimer.current?.cancel();
    const current = itemRef.current;
    // P2-6: start the fall overlay before Rust removes the item; the panel hides this copy
    // immediately so only the overlay card is visible while falling.
    onFallStart?.(current, articleRef.current);
    if (onRemove) {
      onRemove(current);
      return;
    }
    void takeDown(current.id, true).catch((error: unknown) => {
      // Rust keeps the item when the recycle/removal fails; the panel must roll the hidden card
      // back (fallingIds/fallStarted + hit rects) instead of leaving an invisible click-eater.
      onTakeDownFailed?.(current.id, error);
      console.warn(`Hangline card: take-down of ${current.path} failed`, error);
    });
  }, [onFallStart, onRemove, onTakeDownFailed]);

  // The gesture controller owns the pending single-click timer; recreate it when the system
  // interval or the callbacks change, cancelling any click that belonged to the old one.
  useEffect(() => {
    const controller = createCardGesture({
      doubleClickMs,
      onCopy: handleCopy,
      onOpen: handleOpen,
    });
    gesture.current = controller;
    return () => {
      controller.cancel();
      if (gesture.current === controller) gesture.current = null;
    };
  }, [doubleClickMs, handleCopy, handleOpen]);

  /**
   * The hold completed without cancellation (design spec §3.1/§3.7).
   *
   * A press that becomes a hold is no longer a click: drop the pressed pose and any pending
   * single-click copy before opening the editor. `longPress.held` then swallows the click the
   * browser delivers on release, and the short post-fire guard covers a quick follow-up double
   * click (see `shouldSuppressCardClick`).
   */
  const handleLongPress = useCallback(() => {
    holdFiredAt.current = performance.now();
    gesture.current?.cancel();
    pressedRef.current = false;
    setPillVisible(false);
    pillTimer.current?.cancel();
    syncTargets();
    onLongPress?.(itemRef.current);
  }, [onLongPress, syncTargets]);
  longPressAction.current = handleLongPress;

  // One controller per card; it is cancelled on unmount and never recreated for a prop change.
  useEffect(() => {
    const controller = createLongPress({ onLongPress: () => longPressAction.current() });
    longPress.current = controller;
    return () => {
      controller.cancel();
      if (longPress.current === controller) longPress.current = null;
    };
  }, []);

  // A hold must not survive the panel window losing focus (Alt+Tab, the editor opening, the line
  // tucking). Pointer up/leave/cancel and the effects below cover the other endings.
  useEffect(() => {
    const onBlur = () => longPress.current?.cancel();
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  /** Whether a click/dblclick belongs to the hold that just fired and must not copy or open. */
  const suppressCardClick = useCallback((): boolean => {
    return shouldSuppressCardClick(
      longPress.current?.held ?? false,
      performance.now(),
      holdFiredAt.current,
      Math.max(HOLD_CLICK_GUARD_MS, doubleClickMs),
    );
  }, [doubleClickMs]);

  const version = item.updatedAt ?? 0;
  const thumbSrc = item.thumbPath ? assetUrl(item.thumbPath, version) : null;
  // P3-5 review: a recording's only renderable source is its first-frame preview. Deriving the URL
  // from `thumbPath` (instead of the image-only `thumb → original` state machine) guarantees the
  // `<img>` is never transiently pointed at the mp4/mov while a preview is being attached.
  const imageSrc =
    item.kind === "recording"
      ? (thumbSrc ?? "")
      : imageSource === "thumb" && thumbSrc
        ? thumbSrc
        : assetUrl(item.path, version);
  const showImage =
    item.kind === "recording"
      ? Boolean(thumbSrc) && imageSource !== "failed"
      : showCardImage(item.kind, Boolean(item.thumbPath), imageSource);
  // The unit is the motion wrapper: the static tilt + hook-driven swing/arrival live on it, the
  // pin sits at its top and the card starts `CARD_TOP_FROM_PIN` lower through its top padding.
  const unitStyle = {
    "--tilt": `${tilt}deg`,
    paddingTop: `${CARD_TOP_FROM_PIN}px`,
  } as CSSProperties;
  const cardStyle = {
    width: `${visual.width}px`,
    height: `${visual.height}px`,
    "--card-radius": `${CARD_RADIUS}px`,
  } as CSSProperties;
  const gestureHint = t("Click to copy, double-click to open");
  const takeDownLabel = t("Take down");
  const editLabel = isRecording ? t("Trim recording") : t("Markup");
  const actionsVisible = hovered && !paused && !dragging && !gesturesLocked && !pillVisible;

  return (
    <div
      className="glass-card-slot"
      ref={slotRef}
      style={{
        left: `${x}px`,
        top: `${top}px`,
        width: `${slotWidth}px`,
        height: `${CARD_TOP_FROM_PIN + visual.height}px`,
        opacity: edgeFadeOpacity(x + lineShiftPx + slotWidth / 2, panelWidth),
      }}
      data-item-id={item.id}
      data-index={index}
      data-kind={item.kind}
      data-dragging={dragging ? "true" : undefined}
      data-hovered={hovered ? "true" : undefined}
      data-flying={flyState === "flying" ? "true" : undefined}
      data-falling={falling ? "true" : undefined}
    >
      <div className="glass-card-unit" style={unitStyle} ref={unitRef}>
        <ClothesPin className="glass-card-slot__pin" />
        <article
          className="glass-card"
          style={cardStyle}
          ref={attachCardRef}
          aria-label={`${name}. ${gestureHint}`}
          title={gestureHint}
          data-tilt={tilt}
          data-hovered={hovered ? "true" : undefined}
          data-copied={pillVisible ? "true" : "false"}
          onPointerEnter={() => {
            hoveredRef.current = true;
            setHovered(true);
            syncTargets();
            armPreviewTimer();
          }}
          onPointerLeave={() => {
            longPress.current?.cancel();
            pressOrigin.current = null;
            hoveredRef.current = false;
            pressedRef.current = false;
            setHovered(false);
            syncTargets();
            cancelPreview();
          }}
          onPointerMove={(event) => {
            // The controller compares the radial travel with the same 4 dp drag threshold: moving
            // further turns the press into a drag and cancels the hold. The same threshold tells
            // the click handler that this press was a drag, so a release click cannot copy.
            const origin = pressOrigin.current;
            if (origin && !movedPress.current) {
              const dx = event.clientX - origin.x;
              const dy = event.clientY - origin.y;
              if (dx * dx + dy * dy > LONG_PRESS_MOVE_THRESHOLD_PX ** 2) {
                movedPress.current = true;
                gesture.current?.cancel();
              }
            }
            longPress.current?.pointerMove(event.clientX, event.clientY);
          }}
          onPointerDown={(event) => {
            // Only the primary button on an unlocked card body starts the press pose: a right press
            // belongs to the context menu, a middle press to nothing, and an open menu suspends
            // every card gesture (Wave 1.1 arbitration).
            if (
              !shouldStartCardPress({ button: event.button, locked: gesturesLocked, paused, dragging })
            ) {
              return;
            }
            // The take-down cross and the hover capsule keep their own click targets.
            if ((event.target as HTMLElement).closest("[data-action]")) return;
            // A press is the start of a gesture (click / hold / drag): a large preview would only
            // obstruct it.
            cancelPreview();
            pressedRef.current = true;
            pressOrigin.current = { x: event.clientX, y: event.clientY };
            movedPress.current = false;
            syncTargets();
            longPress.current?.pointerDown(event.clientX, event.clientY);
          }}
          onPointerUp={() => {
            longPress.current?.pointerUp();
            pressOrigin.current = null;
            pressedRef.current = false;
            syncTargets();
          }}
          onPointerCancel={() => {
            // The browser cancelled the gesture (OS drag handoff, touch cancel): both hover and
            // press are stale, so drop all targets instead of only releasing the press.
            longPress.current?.cancel();
            pressOrigin.current = null;
            clearTargets();
          }}
          onClick={(event) => {
            event.stopPropagation();
            // An open menu (or the click that dismissed it) must never also copy the card.
            if (!shouldRunCardClick(gesturesLocked)) {
              event.preventDefault();
              return;
            }
            // A moved press was a drag, not a click: never copy on its release click.
            if (movedPress.current) {
              movedPress.current = false;
              event.preventDefault();
              return;
            }
            // A completed hold must not copy: swallow the click the browser delivers on release.
            if (suppressCardClick()) {
              event.preventDefault();
              return;
            }
            gesture.current?.click(event.detail);
          }}
          onDoubleClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            if (!shouldRunCardClick(gesturesLocked)) return;
            // A moved press and the first click of a quick double-click after a hold must not open.
            if (movedPress.current) {
              movedPress.current = false;
              return;
            }
            if (suppressCardClick()) return;
            gesture.current?.doubleClick();
          }}
          onContextMenu={(event) => {
            // The panel renders its own menu; the WebView2 default menu must never appear.
            event.preventDefault();
            event.stopPropagation();
            if (gesturesLocked) return;
            gesture.current?.cancel();
            longPress.current?.cancel();
            pressedRef.current = false;
            syncTargets();
            onContextMenu?.(itemRef.current, event.clientX, event.clientY);
          }}
        >
          <div
            className="glass-card__photo"
            style={{ width: `${photo.width}px`, height: `${photo.height}px` }}
          >
            {showImage ? (
              <img
                className="glass-card__image"
                src={imageSrc}
                alt=""
                loading="lazy"
                decoding="async"
                draggable={false}
                onError={() =>
                  setImageSource((current) =>
                    // A recording has no image fallback below its preview: never point the <img> at
                    // the mp4, go straight to the filename tile + play badge.
                    current === "thumb" && thumbSrc && item.kind !== "recording"
                      ? "original"
                      : "failed",
                  )
                }
              />
            ) : (
              <div
                className={
                  isRecording
                    ? "glass-card__fallback glass-card__fallback--recording"
                    : "glass-card__fallback"
                }
              >
                {isRecording ? null : (
                  <svg
                    className="glass-card__fallback-icon"
                    viewBox="0 0 24 18"
                    aria-hidden="true"
                    focusable="false"
                  >
                    <rect x="1" y="1" width="22" height="16" rx="2.5" />
                    <circle cx="7" cy="6" r="1.8" />
                    <path d="M3 15l5-6 4 4 3-2 5 4" />
                  </svg>
                )}
                <span className="glass-card__fallback-name" title={item.path}>
                  {name}
                </span>
              </div>
            )}

            {isRecording ? <span className="glass-card__play" aria-hidden="true" /> : null}
          </div>

          {/* Bottom-centre status area: the Copied pill (1.2 s) hangs just under the card. */}
          <div className="glass-card__status">
            {pillVisible ? <CopiedPill label={t("Copied")} /> : null}
          </div>

          {/* 26 dp hit area containing the 20 dp glass circle; the glyph shows on hover. */}
          <button
            type="button"
            className="glass-card__take-down"
            aria-label={takeDownLabel}
            title={takeDownLabel}
            data-action="take-down"
            onClick={(event) => {
              event.stopPropagation();
              handleTakeDown();
            }}
            onDoubleClick={(event) => {
              event.stopPropagation();
            }}
          >
            <span className="glass-card__take-down-glyph" aria-hidden="true">
              <svg
                className="glass-card__take-down-icon"
                viewBox="0 0 12 12"
                aria-hidden="true"
                focusable="false"
              >
                <path d="M2.6 2.6 9.4 9.4M9.4 2.6 2.6 9.4" />
              </svg>
            </span>
          </button>

          {/*
            Hover action capsule (Wave 1.2): three actions that used to require a right-click.
            It is a child of the article on purpose: moving the pointer from the card into the
            capsule never fires a pointerleave, so the capsule cannot disappear before it is
            reached. The transparent top padding is a hover/click bridge, not dead space.
          */}
          {actionsVisible ? (
            <div
              className="glass-card__actions"
              data-action="card-actions"
              role="group"
              aria-label={t("More actions")}
              ref={attachActionsRef}
              // The capsule is the foreground interaction surface: it always wins over the peek.
              onPointerEnter={() => cancelPreview()}
              onPointerLeave={() => armPreviewTimer()}
              onPointerDown={(event) => event.stopPropagation()}
              onClick={(event) => event.stopPropagation()}
              onDoubleClick={(event) => event.stopPropagation()}
              onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                onContextMenu?.(itemRef.current, event.clientX, event.clientY);
              }}
            >
              <button
                type="button"
                className="glass-card__action"
                data-action="card-copy"
                aria-label={t("Copy")}
                title={t("Copy")}
                onClick={(event) => {
                  event.stopPropagation();
                  gesture.current?.cancel();
                  pressedRef.current = false;
                  syncTargets();
                  handleCopy();
                }}
              >
                <svg viewBox="0 0 12 12" aria-hidden="true" focusable="false">
                  <rect x="4" y="1.4" width="6.2" height="7.2" rx="1.2" />
                  <path d="M8 10.4H2.6a1.2 1.2 0 0 1-1.2-1.2V4" />
                </svg>
              </button>
              <button
                type="button"
                className="glass-card__action"
                data-action="card-edit"
                aria-label={editLabel}
                title={editLabel}
                onClick={(event) => {
                  event.stopPropagation();
                  gesture.current?.cancel();
                  pressedRef.current = false;
                  syncTargets();
                  (onEdit ?? onLongPress)?.(itemRef.current);
                }}
              >
                {isRecording ? (
                  <svg viewBox="0 0 12 12" aria-hidden="true" focusable="false">
                    <rect x="1.2" y="2.4" width="9.6" height="7.2" rx="1.4" />
                    <path d="M4.8 4.8v2.4l2-1.2z" className="glass-card__action-fill" />
                  </svg>
                ) : (
                  <svg viewBox="0 0 12 12" aria-hidden="true" focusable="false">
                    <path d="M2.2 9.8l.5-2.3 5.4-5.4 1.8 1.8-5.4 5.4z" />
                    <path d="M6.9 3.4l1.8 1.8" />
                  </svg>
                )}
              </button>
              <button
                type="button"
                className="glass-card__action"
                data-action="card-more"
                aria-label={t("More actions")}
                title={t("More actions")}
                onClick={(event) => {
                  event.stopPropagation();
                  onMoreActions?.(itemRef.current, event.currentTarget);
                }}
              >
                <svg viewBox="0 0 12 12" aria-hidden="true" focusable="false">
                  <circle cx="2.6" cy="6" r="0.9" className="glass-card__action-fill" />
                  <circle cx="6" cy="6" r="0.9" className="glass-card__action-fill" />
                  <circle cx="9.4" cy="6" r="0.9" className="glass-card__action-fill" />
                </svg>
              </button>
            </div>
          ) : null}
        </article>
      </div>
    </div>
  );
}
