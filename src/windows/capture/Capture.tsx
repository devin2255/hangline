/**
 * The freeze-frame region-selection overlay (`index.html?window=capture`, Task 16).
 *
 * Rust hides the line, grabs the virtual screen, saves the frozen frame and creates a borderless,
 * always-on-top window covering the whole virtual desktop in physical pixels. This page renders
 * that frame at `physical / devicePixelRatio` CSS pixels (the same conversion the pointer uses, so
 * mixed-DPI desktops and negative virtual origins stay exact), adds a dark mask and tracks a
 * left-button drag as the selection. Releasing the mouse confirms the selection through
 * `finish_capture(physicalRect)`; Esc / right-click cancels. A click without a real drag is
 * ignored instead of saving a 1x1 screenshot.
 *
 * The `capture-ready` payload is requested with a ready handshake (`reportCaptureReady`) after the
 * listener is registered, so a slow webview load cannot lose the frozen frame.
 */

import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import {
  cancelCapture,
  finishCapture,
  reportCaptureReady,
} from "../../lib/ipc";
import { themeSettingFromLocation, useResolvedTheme } from "../../lib/theme";
import type { CaptureReady } from "../../lib/types";
import {
  clampSelection,
  frozenFrameCssSize,
  isSelectionLargeEnough,
  normalizeSelection,
  toPhysicalRect,
  viewportScale,
  type CssRect,
  type SelectionPoint,
} from "./selection";
import "./Capture.css";

/** How often the overlay re-reads `devicePixelRatio` (a cross-DPI settle changes it once). */
const DPR_WATCH_MS = 500;

/**
 * Delays between ready-handshake attempts (ms).
 *
 * The first attempt is immediate; the page can load before Rust attached the frozen frame, and the
 * Rust command also waits for the attach, so this only covers a slow startup. If every attempt
 * fails the overlay cancels itself (a black full-screen window must never trap the user).
 */
const READY_RETRY_DELAYS_MS = [0, 100, 200, 400, 800, 1200, 1600];

/** Show the dimension label inside the selection when it does not fit above it. */
const LABEL_INSIDE_BELOW_CSS = 26;

/** One logical pointer position in CSS pixels. */
function pointFrom(event: { clientX: number; clientY: number }): SelectionPoint {
  return { x: event.clientX, y: event.clientY };
}

/** The overlay page: frozen frame + mask + drag selection. */
export function Capture() {
  /** P2-8: the raw setting arrives in the URL because this window has no app store. */
  const theme = useResolvedTheme(themeSettingFromLocation());
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    return () => {
      delete document.documentElement.dataset.theme;
    };
  }, [theme]);
  const [ready, setReady] = useState<CaptureReady | null>(null);
  const [dpr, setDpr] = useState(() => window.devicePixelRatio || 1);
  /** CSS viewport size; combined with the physical virtual rect it gives the real scale. */
  const [viewport, setViewport] = useState(() => ({
    w: Math.max(1, window.innerWidth),
    h: Math.max(1, window.innerHeight),
  }));
  const [selection, setSelection] = useState<CssRect | null>(null);

  /** Press position of the drag in flight (`null` when not dragging). */
  const startRef = useRef<SelectionPoint | null>(null);
  /** Pointer id that owns the drag (stray OS pointer events must not move the selection). */
  const pointerRef = useRef<number | null>(null);
  /** Payload at the moment of confirm (state updates are async during a pointerup). */
  const readyRef = useRef<CaptureReady | null>(null);
  /** True once finish/cancel was requested: the overlay must not send a second command. */
  const settledRef = useRef(false);
  /** Current DPR at the moment of confirm (the state value can lag one render). */
  const dprRef = useRef(dpr);

  useEffect(() => {
    readyRef.current = ready;
  }, [ready]);

  useEffect(() => {
    dprRef.current = dpr;
  }, [dpr]);

  // Ready handshake: register the listener first, then ask Rust to emit `capture-ready`.
  //
  // The page can load before the request attaches the frozen frame (the window is created hidden and
  // navigation starts early), so the handshake retries with backoff. Rust also waits for the attach
  // before emitting; if the frame still cannot be loaded the overlay cancels itself instead of
  // leaving a black full-screen window (T16 review fix).
  useEffect(() => {
    let disposed = false;
    let retryTimer: number | null = null;
    const registration = listen<CaptureReady>("capture-ready", ({ payload }) => {
      if (disposed || !payload) return;
      readyRef.current = payload;
      setReady(payload);
      setDpr(window.devicePixelRatio || payload.scale || 1);
      console.info(
        `Hangline capture: frozen frame ${payload.width}x${payload.height} px at ` +
          `(${payload.virtualRect.x}, ${payload.virtualRect.y}), scale ${payload.scale}`,
      );
      console.info(
        "Hangline capture: viewport scale " +
          viewportScale(payload.virtualRect.w, window.innerWidth, payload.scale || 1).toFixed(4) +
          ` (physical ${payload.virtualRect.w} px / CSS viewport ${window.innerWidth} px)`,
      );
    });
    const handshake = async () => {
      for (const delay of READY_RETRY_DELAYS_MS) {
        if (disposed || readyRef.current) return;
        if (delay > 0) {
          await new Promise<void>((resolve) => {
            retryTimer = window.setTimeout(resolve, delay);
          });
        }
        if (disposed || readyRef.current) return;
        try {
          await reportCaptureReady();
          return;
        } catch (error) {
          console.warn(
            "Hangline capture: ready handshake failed; retrying with backoff",
            error,
          );
        }
      }
      if (!disposed && !readyRef.current) {
        console.error(
          "Hangline capture: the frozen frame never loaded; closing the overlay to avoid a black screen",
        );
        settledRef.current = true;
        void cancelCapture().catch((error: unknown) => {
          console.warn("Hangline capture: cancel after a failed handshake failed", error);
        });
      }
    };
    void registration
      .then((unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        return handshake();
      })
      .catch((error: unknown) => {
        console.warn("Hangline capture: capture-ready listener unavailable", error);
      });
    return () => {
      disposed = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      void registration.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  // WebView2 applies one scale per window; it can settle (or change after a cross-DPI move) after
  // the first paint. Track both `devicePixelRatio` and the CSS viewport: the physical virtual rect
  // divided by the viewport is the authoritative CSS↔physical scale (WebView2 can report a wrong
  // DPR for a window spanning mixed-DPI monitors, review evidence).
  useEffect(() => {
    const update = () => {
      setDpr(window.devicePixelRatio || 1);
      setViewport({
        w: Math.max(1, window.innerWidth),
        h: Math.max(1, window.innerHeight),
      });
    };
    update();
    const timer = window.setInterval(update, DPR_WATCH_MS);
    window.addEventListener("resize", update);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("resize", update);
    };
  }, []);

  /** Escapes the overlay without creating anything (Rust destroys the window and restores the line). */
  const cancel = useCallback(() => {
    if (settledRef.current) return;
    settledRef.current = true;
    console.info("Hangline capture: cancelled (Esc/right click)");
    void cancelCapture().catch((error: unknown) => {
      console.warn("Hangline capture: cancel_capture failed", error);
    });
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      cancel();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      settledRef.current = false;
    };
  }, [cancel]);

  /** Confirms a real drag; returns false when the selection was only a click (stays open). */
  const confirm = useCallback((rect: CssRect): boolean => {
    const payload = readyRef.current;
    if (!payload || settledRef.current) return false;
    if (!isSelectionLargeEnough(rect)) {
      console.info("Hangline capture: click without a real drag ignored");
      return false;
    }
    // Authoritative viewport scale (see the DPR watch above); the reported dpr is only the fallback.
    const scale = viewportScale(
      payload.virtualRect.w,
      window.innerWidth,
      window.devicePixelRatio || dprRef.current || payload.scale || 1,
    );
    const physical = toPhysicalRect(rect, scale, {
      x: payload.virtualRect.x,
      y: payload.virtualRect.y,
    });
    settledRef.current = true;
    console.info(
      `Hangline capture: confirm CSS ${rect.x},${rect.y} ${rect.w}x${rect.h} -> ` +
        `physical ${physical.x},${physical.y} ${physical.w}x${physical.h} (scale ${scale})`,
    );
    void finishCapture(physical).catch((error: unknown) => {
      console.warn("Hangline capture: finish_capture failed", error);
      // Rust destroys the overlay on failure; if it did not (shutdown race), allow Esc to cancel.
      settledRef.current = false;
    });
    return true;
  }, []);

  const viewportSelection = useCallback(
    (from: SelectionPoint, to: SelectionPoint): CssRect =>
      clampSelection(normalizeSelection(from, to), window.innerWidth, window.innerHeight),
    [],
  );

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    // Left button only: right click cancels through the context menu below.
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const start = pointFrom(event);
    pointerRef.current = event.pointerId;
    startRef.current = start;
    setSelection(normalizeSelection(start, start));
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const start = startRef.current;
    if (!start || event.pointerId !== pointerRef.current) return;
    // Only a pressed drag may move the selection: an idle OS pointer moving over the full-screen
    // overlay (this machine has pointer drift) must never resize the user's selection.
    if ((event.buttons & 1) === 0) return;
    event.preventDefault();
    setSelection(viewportSelection(start, pointFrom(event)));
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerId !== pointerRef.current) return;
    // Read the press position before clearing it: using the cleared ref would turn every drag
    // into a zero-size "click" and silently ignore it.
    const start = startRef.current;
    if (!start) return;
    startRef.current = null;
    pointerRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    const rect = viewportSelection(start, pointFrom(event));
    setSelection(rect);
    if (!confirm(rect)) {
      // An accidental click must leave the overlay open for a real drag.
      setSelection(null);
    }
  };

  const onPointerCancel = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerId !== pointerRef.current) return;
    startRef.current = null;
    pointerRef.current = null;
    setSelection(null);
  };

  const onContextMenu = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    cancel();
  };

  // Size the frame from the frozen image's own dimensions (not the virtual-screen rect): this is
  // the exact pixel data the selection crop is taken from, and it makes a display change between the
  // grab and the window creation harmless (T16 review fix).
  const scale = ready ? viewportScale(ready.virtualRect.w, viewport.w, dpr) : dpr;
  const frame = ready ? frozenFrameCssSize({ w: ready.width, h: ready.height }, scale) : null;
  const selecting = selection !== null && selection.w > 0 && selection.h > 0;
  const origin = ready?.virtualRect ?? { x: 0, y: 0 };
  const physical = selection
    ? toPhysicalRect(selection, scale, {
        x: origin.x,
        y: origin.y,
      })
    : null;
  const labelInside = selection !== null && selection.y < LABEL_INSIDE_BELOW_CSS;

  return (
    <div
      className="capture"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onContextMenu={onContextMenu}
      data-ready={ready ? "true" : "false"}
      data-css-scale={scale.toFixed(4)}
    >
      {ready && frame ? (
        <img
          className="capture__frame"
          src={convertFileSrc(ready.tempPath)}
          style={{ width: `${frame.width}px`, height: `${frame.height}px` }}
          alt=""
          draggable={false}
        />
      ) : null}

      {selecting && selection ? (
        <div
          className="capture__selection"
          style={{
            left: `${selection.x}px`,
            top: `${selection.y}px`,
            width: `${selection.w}px`,
            height: `${selection.h}px`,
          }}
        >
          <span
            className={
              labelInside ? "capture__size capture__size--inside" : "capture__size"
            }
          >
            {physical ? `${physical.w} × ${physical.h}` : ""}
          </span>
        </div>
      ) : (
        <div className="capture__mask" />
      )}

      {!ready ? <div className="capture__loading" aria-hidden="true" /> : null}
    </div>
  );
}
