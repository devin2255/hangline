/**
 * One rAF loop per card that drives the P2-2 motion state machine and publishes its result as CSS
 * custom properties (design plan P2-4, spec §4.6 B).
 *
 * The hook is framework glue only: it owns the loop, the event queue and the CSS variable writes,
 * while all motion decisions live in the pure `cardMotion` module. It writes on the unit element:
 *
 * - `--swing`: dynamic swing/tilt contribution (deg) — the static `--tilt` stays in React style;
 * - `--arrive-y`: arrival drop offset (px, dp);
 * - `--card-scale`: hover/press scale.
 *
 * `--card-scale` is written on the **unit** and read by `.glass-card` through CSS custom-property
 * inheritance (documented deviation from "write it on the card element" in the P2-4 brief): the
 * public interface only receives `unitRef`, and inheritance gives the exact same value without a
 * second ref or DOM lookup.
 *
 * The loop stops as soon as the state is settled, and every event/target change restarts it. While
 * `paused` (panel hidden) the loop is stopped without advancing; resuming continues from the last
 * state with a normal first frame (`dt = 1/60`) so a long pause cannot become one giant step. A
 * pause clears only the hover/press pose (scale back to 1): arrival/breeze state stays intact, so a
 * card inserted while the line is tucked still plays its full 16°/−46 px drop-in on the next
 * reveal (the normal new-item path emits `state` before `reveal`, so cards mount paused).
 */

import { useCallback, useEffect, useRef } from "react";
import type { RefObject } from "react";
import {
  cardEvent,
  createCardMotion,
  settledCardMotion,
  stepCardMotion,
  type CardMotionEvent,
  type CardMotionState,
  type CardMotionTargets,
} from "./cardMotion";
import { DEV_INSTRUMENTATION } from "./perf";
import { MAX_DT } from "./spring";
import { pushTrace } from "./trace";

export interface CardMotionOptions {
  /** `prefers-reduced-motion`: snap to the end state instead of animating. */
  reducedMotion: boolean;
  /** Panel hidden/tucked: stop the loop without advancing the state. */
  paused: boolean;
}

export interface CardMotionControls {
  /** Card mounted on the line: drop it in from −46 dp / 16°. */
  arrive(): void;
  /** Fly-in handoff completed: land with a small 2.2° sway. */
  land(): void;
  /** Copy succeeded: nudge with a 3° sway. */
  copied(): void;
  /** Gust wind: sway by `amplitudeDeg` after `delayMs`. */
  breeze(amplitudeDeg: number, delayMs: number): void;
  /** Hover/press inputs. Starts the loop whenever they change. */
  setTargets(targets: CardMotionTargets): void;
}

/** Stable no-op target set used until the first pointer event. */
const REST_TARGETS: CardMotionTargets = { hovered: false, pressed: false };

function sameTargets(a: CardMotionTargets, b: CardMotionTargets): boolean {
  return a.hovered === b.hovered && a.pressed === b.pressed;
}

export function useCardMotion(
  unitRef: RefObject<HTMLElement | null>,
  options: CardMotionOptions,
): CardMotionControls {
  const stateRef = useRef<CardMotionState>(createCardMotion(options.reducedMotion));
  const targetsRef = useRef<CardMotionTargets>(REST_TARGETS);
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const frameRef = useRef<number | null>(null);
  /** Timestamp of the previous animated frame; null means the next frame starts a fresh 1/60 s. */
  const lastTsRef = useRef<number | null>(null);

  /** Publishes the state as the CSS variables the unit/card styles read. */
  const publish = useCallback(() => {
    const unit = unitRef.current;
    if (!unit) return;
    const state = stateRef.current;
    unit.style.setProperty("--swing", `${state.rotation}deg`);
    unit.style.setProperty("--arrive-y", `${state.offsetY}px`);
    unit.style.setProperty("--card-scale", `${state.scale}`);
    // P2-9 trace (no-op unless the comparison harness enabled it). The static gate drops the call
    // and its object literal from release.
    if (DEV_INSTRUMENTATION) {
      pushTrace("card", {
        swing: state.rotation,
        arriveY: state.offsetY,
        scale: state.scale,
      });
    }
  }, [unitRef]);

  const startLoop = useCallback(() => {
    if (optionsRef.current.paused) return;
    if (frameRef.current !== null) return;
    frameRef.current = window.requestAnimationFrame(function frame(timestamp) {
      frameRef.current = null;
      if (optionsRef.current.paused) {
        lastTsRef.current = null;
        return;
      }
      const last = lastTsRef.current;
      const dt = last === null ? 1 / 60 : Math.min(Math.max((timestamp - last) / 1000, 0), MAX_DT);
      lastTsRef.current = timestamp;
      stateRef.current = stepCardMotion(stateRef.current, targetsRef.current, dt);
      publish();
      if (settledCardMotion(stateRef.current)) {
        lastTsRef.current = null;
        return;
      }
      startLoop();
    });
  }, [publish]);

  const stopLoop = useCallback(() => {
    if (frameRef.current !== null) {
      window.cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
    lastTsRef.current = null;
  }, []);

  const dispatch = useCallback(
    (event: CardMotionEvent) => {
      stateRef.current = cardEvent(stateRef.current, event, performance.now());
      publish();
      startLoop();
    },
    [publish, startLoop],
  );

  const arrive = useCallback(() => dispatch({ type: "arrive" }), [dispatch]);
  const land = useCallback(() => dispatch({ type: "land" }), [dispatch]);
  const copied = useCallback(() => dispatch({ type: "copied" }), [dispatch]);
  const breeze = useCallback(
    (amplitudeDeg: number, delayMs: number) => dispatch({ type: "breeze", amplitudeDeg, delayMs }),
    [dispatch],
  );
  const setTargets = useCallback(
    (targets: CardMotionTargets) => {
      if (sameTargets(targetsRef.current, targets)) return;
      targetsRef.current = targets;
      startLoop();
    },
    [startLoop],
  );

  // Reduced-motion toggles are events too: they snap the state and publish the end pose.
  useEffect(() => {
    dispatch({ type: "reduced-motion", value: options.reducedMotion });
  }, [dispatch, options.reducedMotion]);

  // Pausing stops the loop without advancing. Only the hover/press pose is cleared (the panel is
  // hidden, so a stale scale would otherwise show up on reveal); rotation, arrival offset, breeze
  // and their velocities are preserved so a pending arrival continues on unpause. Resuming starts
  // from the stored state with a fresh 1/60 s first frame.
  useEffect(() => {
    if (!options.paused) {
      startLoop();
      return;
    }
    stopLoop();
    targetsRef.current = REST_TARGETS;
    stateRef.current = {
      ...stateRef.current,
      scale: 1,
      scaleVelocity: 0,
      pressTween: null,
      targetScale: 1,
    };
    publish();
  }, [options.paused, publish, startLoop, stopLoop]);

  // Cancel the frame on unmount: a stale rAF must never touch a detached element.
  useEffect(() => stopLoop, [stopLoop]);

  return { arrive, land, copied, breeze, setTargets };
}
