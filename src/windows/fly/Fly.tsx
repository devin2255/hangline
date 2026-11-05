/**
 * The screen-wide fly-in / fall overlay page (`?window=fly`, design spec §4.2 / §4.6 B).
 *
 * A pre-warmed, transparent, click-through window covers the whole virtual desktop (Rust owns the
 * window; see `src-tauri/src/fly.rs`). This page only renders physical-pixel `FlyAnimation`s: it
 * converts them to CSS pixels with `cssScale = innerWidth / virtualRect.w` and drives one rAF per
 * animation. The panel keeps its own card hidden (`data-flying`) for the whole flight, so the only
 * visible card is these overlay copies.
 *
 * Card recipe (same DOM structure as the panel card):
 * - `.fly-card__glass`: full-size glass background + shadow/inset ring, opacity = chrome;
 * - `.fly-card__photo`: inset content box that **clips** the photo (`overflow: hidden`), sized by
 *   `--fly-inset`; at k = 0 it is exactly the capture rectangle, at k = 1 it is the 4 dp inset box;
 * - `.fly-card__image`: `width/height: 100%` + `object-fit: cover` — never the intrinsic pixel size;
 * - `.fly-card__pin`: 9×26 clip above the card.
 *
 * The photo is opaque from the first frame (the reference fades only glass/edge/clip in), and the
 * initial card rect is painted synchronously as inline style so no giant/unclipped photo can flash
 * before the animation effect runs. The handoff is a 160 ms cross-fade: `fly_done` is called before
 * the fade, and the fading copy survives a new `fly-animation` payload until its fade completes.
 */

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ClothesPin } from "../../components/ClothesPin";
import { assetUrl } from "../../lib/assetUrl";
import { mergeFlyAnimations, type FlyRenderEntry } from "../../lib/flyPlan";
import { flyDone, reportFlyReady } from "../../lib/ipc";
import { easeInOutCubic, FALL, FLY } from "../../lib/motion";
import { DEV_INSTRUMENTATION } from "../../lib/perf";
import { pushTrace } from "../../lib/trace";
import { installTrace } from "../../lib/trace";
import type {
  FlyAnimation,
  FlyAnimationEvent,
  FlyGeometryEvent,
  FlyThemeSetting,
  PhysicalRect,
} from "../../lib/types";
import { cssScale, fallFrame, flyFrame } from "./motion";
import "./Fly.css";

/** Where the overlay page is anchored: the virtual-screen origin and the physical→CSS scale. */
interface OverlayGeometry {
  virtualRect: PhysicalRect;
  scale: number;
}

/** Media query the panel uses for the system theme (`settings.theme === "system"`). */
const DARK_QUERY = "(prefers-color-scheme: dark)";

/** Applies `settings.theme` to the document: the CSS handles `system` via this media query. */
function applyTheme(setting: FlyThemeSetting): void {
  const resolved =
    setting === "system" ? (window.matchMedia(DARK_QUERY).matches ? "dark" : "light") : setting;
  document.documentElement.dataset.flyTheme = resolved;
  document.documentElement.dataset.flyThemeSetting = setting;
}

function readGeometry(virtualRect: PhysicalRect): OverlayGeometry {
  return {
    virtualRect,
    scale: cssScale(window.innerWidth, virtualRect.w, window.devicePixelRatio),
  };
}

/**
 * Logical-dp → CSS scale for chrome dimensions.
 *
 * The fly window spans monitors, so its own scale factor is ambiguous; the page's
 * `devicePixelRatio` is exactly the CSS↔physical ratio of the rendering surface. Rust sends the
 * target monitor's DPI, so `chromeScale = targetScale / devicePixelRatio` is 1 at 100 % and
 * 1.5/1.5 = 1 when the target is 150 % on a 150 % surface (mixed DPI: 1.5/1.0).
 */
function chromeScaleOf(animation: FlyAnimation): number {
  const target =
    Number.isFinite(animation.targetScale) && animation.targetScale > 0 ? animation.targetScale : 1;
  const dpr = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
  return target / dpr;
}

/**
 * Computes the k = 0 inline geometry so the very first paint already has the right size/position.
 *
 * Without this the absolutely positioned photo wrapper would briefly have no size (or the image
 * could paint at its intrinsic size), which is exactly the unclipped-photo flash the review found.
 */
function initialFrameStyle(
  animation: FlyAnimation,
  geometry: OverlayGeometry,
): Record<string, string> {
  const { virtualRect, scale } = geometry;
  const chrome = chromeScaleOf(animation);
  if (animation.kind === "fall") {
    // The fall starts from the final card look.
    return {
      left: `${(animation.to.x - virtualRect.x) * scale}px`,
      top: `${(animation.to.y - virtualRect.y) * scale}px`,
      width: `${animation.to.w * scale}px`,
      height: `${animation.to.h * scale}px`,
      borderRadius: `${16 * chrome}px`,
      transform: "rotate(0deg)",
      ["--fly-inset" as string]: `${4 * chrome}px`,
      ["--fly-photo-radius" as string]: `${12 * chrome}px`,
      ["--fly-chrome" as string]: "1",
      ["--fly-chrome-scale" as string]: `${chrome}`,
    };
  }
  return {
    left: `${(animation.from.x - virtualRect.x) * scale}px`,
    top: `${(animation.from.y - virtualRect.y) * scale}px`,
    width: `${animation.from.w * scale}px`,
    height: `${animation.from.h * scale}px`,
    borderRadius: "0px",
    transform: "rotate(0deg)",
    ["--fly-inset" as string]: "0px",
    ["--fly-photo-radius" as string]: "0px",
    ["--fly-chrome" as string]: "0",
    ["--fly-chrome-scale" as string]: `${chrome}`,
  };
}

/** One animation copy; owns its rAF loop and calls the lifecycle callbacks exactly once. */
function FlyCard({
  entry,
  geometry,
  onFading,
  onLanded,
  onFinished,
}: {
  entry: FlyRenderEntry;
  geometry: OverlayGeometry;
  onFading: (entry: FlyRenderEntry) => void;
  onLanded: (id: string) => void;
  onFinished: (entry: FlyRenderEntry) => void;
}) {
  const animation = entry.animation;
  const cardRef = useRef<HTMLDivElement | null>(null);
  const geometryRef = useRef(geometry);
  geometryRef.current = geometry;
  const doneRef = useRef(false);
  const [handoff, setHandoff] = useState(false);

  useLayoutEffect(() => {
    const element = cardRef.current;
    if (!element) return;
    const id = animation.id;
    const kind = animation.kind;
    const from = animation.from;
    const to = animation.to;
    const tilt = animation.tiltDeg;
    const chrome = chromeScaleOf(animation);
    const duration =
      animation.durationMs > 0
        ? animation.durationMs
        : kind === "fall"
          ? FALL.durationMs
          : FLY.durationMs;
    const started = performance.now();
    let frameHandle: number | null = null;
    let finishTimer: number | null = null;
    let disposed = false;

    const land = () => {
      if (disposed || doneRef.current) return;
      doneRef.current = true;
      // Completion first: Rust emits `fly-landed` (panel card appears) and keeps the overlay for
      // the fade. The overlay copy then cross-fades out over the newly visible panel card.
      onLanded(id);
      if (kind === "fly") {
        setHandoff(true);
        onFading(entry);
        finishTimer = window.setTimeout(() => onFinished(entry), FLY.handoffMs);
      } else {
        onFinished(entry);
      }
    };

    /** Paints one frame; returns true when the animation has reached its end. */
    const render = (now: number): boolean => {
      const raw = duration > 0 ? Math.min(1, Math.max(0, (now - started) / duration)) : 1;
      const { virtualRect, scale } = geometryRef.current;
      const fromLeft = (from.x - virtualRect.x) * scale;
      const fromTop = (from.y - virtualRect.y) * scale;
      const fromWidth = from.w * scale;
      const fromHeight = from.h * scale;
      const toLeft = (to.x - virtualRect.x) * scale;
      const toTop = (to.y - virtualRect.y) * scale;
      const toWidth = to.w * scale;
      const toHeight = to.h * scale;
      element.style.setProperty("--fly-chrome-scale", `${chrome}`);

      if (kind === "fall") {
        // The fall starts from the *final card look* (reference pre-runs `update(1)`): radius 16,
        // inset 4, photo radius 12, chrome fully opaque. Only drop/tilt/fade animate from here.
        element.style.left = `${toLeft}px`;
        element.style.top = `${toTop}px`;
        element.style.width = `${toWidth}px`;
        element.style.height = `${toHeight}px`;
        element.style.borderRadius = `${16 * chrome}px`;
        element.style.setProperty("--fly-inset", `${4 * chrome}px`);
        element.style.setProperty("--fly-photo-radius", `${12 * chrome}px`);
        element.style.setProperty("--fly-chrome", "1");
        const fall = fallFrame(raw);
        const rotation = tilt + tilt * 7 * fall.ease + fall.extraTiltDeg;
        element.style.opacity = `${fall.opacity}`;
        element.style.transform = `translate3d(0, ${fall.dropDp * chrome}px, 0) rotate(${rotation}deg)`;
        // P2-9 trace (no-op unless the comparison harness enabled it); static gate drops it in
        // release.
        if (DEV_INSTRUMENTATION) {
          pushTrace("fall", { k: raw, opacity: fall.opacity, dropDp: fall.dropDp, tilt: rotation });
        }
        return raw >= 1;
      }

      const k = easeInOutCubic(raw);
      const fly = flyFrame(k);
      const width = fromWidth + (toWidth - fromWidth) * fly.widthLerp;
      const height = fromHeight + (toHeight - fromHeight) * fly.heightLerp;
      const centerFrom = fromLeft + fromWidth / 2;
      const centerTo = toLeft + toWidth / 2;
      const left = centerFrom + (centerTo - centerFrom) * fly.widthLerp - width / 2;
      const top = fromTop + (toTop - fromTop) * fly.heightLerp - fly.arcDp * chrome;
      element.style.left = `${left}px`;
      element.style.top = `${top}px`;
      element.style.width = `${width}px`;
      element.style.height = `${height}px`;
      element.style.borderRadius = `${fly.radiusDp * chrome}px`;
      element.style.transform = `rotate(${tilt * k}deg)`;
      element.style.setProperty("--fly-inset", `${fly.insetDp * chrome}px`);
      element.style.setProperty(
        "--fly-photo-radius",
        `${Math.max(0, (fly.radiusDp - fly.insetDp) * chrome)}px`,
      );
      element.style.setProperty("--fly-chrome", `${fly.chromeOpacity}`);
      // P2-9 trace (no-op unless the comparison harness enabled it); static gate drops it in
      // release.
      if (DEV_INSTRUMENTATION) {
        pushTrace("fly", { k, chrome: fly.chromeOpacity, arcDp: fly.arcDp, left, top });
      }
      return raw >= 1;
    };

    const tick = (now: number) => {
      frameHandle = null;
      if (disposed) return;
      if (render(now)) {
        land();
        return;
      }
      frameHandle = window.requestAnimationFrame(tick);
    };

    render(performance.now());
    frameHandle = window.requestAnimationFrame(tick);
    return () => {
      disposed = true;
      if (frameHandle !== null) window.cancelAnimationFrame(frameHandle);
      if (finishTimer !== null) window.clearTimeout(finishTimer);
    };
    // The render entry is replaced (new key → remount) whenever the payload for this id changes, so
    // a re-emitted identical payload never restarts the curve and a new animation always does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entry.key, onFading, onLanded, onFinished]);

  return (
    <div
      className={handoff ? "fly-card fly-card--handoff" : "fly-card"}
      ref={cardRef}
      style={initialFrameStyle(animation, geometry)}
      data-kind={animation.kind}
      data-item-id={animation.id}
    >
      <div className="fly-card__glass" />
      <div className="fly-card__photo">
        <img
          className="fly-card__image"
          src={assetUrl(animation.imagePath, animation.imageVersion)}
          alt=""
          draggable={false}
          onError={(event) => {
            event.currentTarget.style.visibility = "hidden";
          }}
        />
      </div>
      {/* P2-8: sheen layer painted after (above) the opaque photo, like the panel's `.glass-card::before`. */}
      <div className="fly-card__sheen" aria-hidden="true" />
      <span className="fly-card__pin" aria-hidden="true">
        <ClothesPin />
      </span>
    </div>
  );
}

/** The overlay root: ready handshake, animation list and the per-animation loops. */
export function Fly() {
  const [geometry, setGeometry] = useState<OverlayGeometry | null>(null);
  /** Rendered copies: one per active animation plus the ones still cross-fading after landing. */
  const [entries, setEntries] = useState<FlyRenderEntry[]>([]);
  /** Ids whose handoff fade is in progress: a new payload must not remove their fading copies. */
  const fadingIdsRef = useRef<Set<string>>(new Set());

  // P2-9: expose the optional trace ring to the comparison harness (cleared per measurement).
  // Development-only: the release bundle drops the trace ring entirely.
  useEffect(() => {
    if (!DEV_INSTRUMENTATION) return;
    installTrace();
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlistenAnimation: (() => void) | undefined;
    let unlistenGeometry: (() => void) | undefined;
    // The listeners must exist before the ready handshake: Rust re-emits active animations on ready.
    void Promise.all([
      listen<FlyAnimationEvent>("fly-animation", ({ payload }) => {
        if (disposed) return;
        const incoming = payload?.animations ?? [];
        setEntries((previous) => mergeFlyAnimations(previous, incoming, fadingIdsRef.current));
      }),
      listen<FlyGeometryEvent>("fly-geometry", ({ payload }) => {
        if (disposed || !payload?.virtualRect) return;
        applyTheme(payload.theme ?? "system");
        setGeometry(readGeometry(payload.virtualRect));
      }),
    ])
      .then(([stopAnimation, stopGeometry]) => {
        if (disposed) {
          stopAnimation();
          stopGeometry();
          return;
        }
        unlistenAnimation = stopAnimation;
        unlistenGeometry = stopGeometry;
        return reportFlyReady().then((ready) => {
          if (disposed) return;
          applyTheme(ready.theme);
          setGeometry(readGeometry(ready.virtualRect));
        });
      })
      .catch((error: unknown) => {
        console.warn("Hangline fly: overlay initialization failed", error);
      });
    return () => {
      disposed = true;
      unlistenAnimation?.();
      unlistenGeometry?.();
    };
  }, []);

  // Follow the Windows app theme while `settings.theme === "system"`.
  useEffect(() => {
    const query = window.matchMedia(DARK_QUERY);
    const onChange = () => {
      if (document.documentElement.dataset.flyThemeSetting === "system") applyTheme("system");
    };
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);

  // A window resize (display change) only changes the physical→CSS ratio; keep it current.
  useEffect(() => {
    const onResize = () => {
      setGeometry((current) => {
        if (!current) return current;
        return {
          virtualRect: current.virtualRect,
          scale: cssScale(window.innerWidth, current.virtualRect.w, window.devicePixelRatio),
        };
      });
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const fading = useCallback((entry: FlyRenderEntry) => {
    fadingIdsRef.current.add(entry.animation.id);
  }, []);

  const landed = useCallback((id: string) => {
    // Completion before the fade: the panel card becomes visible under the fading overlay copy.
    void flyDone(id).catch((error: unknown) => {
      console.warn(`Hangline fly: fly_done for ${id} failed`, error);
    });
  }, []);

  const finished = useCallback((entry: FlyRenderEntry) => {
    fadingIdsRef.current.delete(entry.animation.id);
    // Remove exactly this copy: a newer animation for the same id must survive the stale timer.
    setEntries((current) => current.filter((candidate) => candidate.key !== entry.key));
  }, []);

  return (
    <div className="fly-layer" aria-hidden="true">
      {geometry
        ? entries.map((entry) => (
            <FlyCard
              key={entry.key}
              entry={entry}
              geometry={geometry}
              onFading={fading}
              onLanded={landed}
              onFinished={finished}
            />
          ))
        : null}
    </div>
  );
}
