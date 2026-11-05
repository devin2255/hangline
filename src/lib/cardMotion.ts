/**
 * Pure per-card motion state machine (design plan P2-2, spec §4.6 B).
 *
 * Reproduces the Tendedero `PeggedView` behaviour without React or the DOM: arrival drop/swing,
 * breeze/nudge sway, hover/press scale and a reduced-motion snap. The caller owns the clock: every
 * `stepCardMotion` integration step takes its own `dt` (clamped to [`MAX_DT`]), so the state is
 * replayable and unit-testable. `nowMs` is only recorded as an event timestamp (`lastEventAtMs`)
 * for callers/perf traces; it never feeds the integration.
 *
 * Springs:
 * - arrival offset y: [`SPRINGS.arriveY`] (`spring(response: .42, dampingFraction: .72)`, from −46 dp)
 * - arrival swing: [`SPRINGS.arriveSwing`] (`interpolatingSpring(46, 2.6)`, 16° → 0)
 * - breeze/nudge return: [`SPRINGS.breeze`] (`interpolatingSpring(38, 2.4)`)
 * - hover scale: [`SPRINGS.hover`] (`spring(response: .3, dampingFraction: .6)`)
 *
 * Scale values (1.035 hover, 0.95 pressed, 450 ms press tween) and the 300 ms ease-out nudge come
 * from `PeggedView.swift:82-85,140-155`; they are not in `motion.ts` because they are card-local.
 */

import { MAX_DT, tick } from "./spring";
import { CARD, SPRINGS, TILT, easeInOutCubic } from "./motion";

/** The phase of an active breeze/nudge sway. */
export type BreezePhase = "out" | "back";

/** One breeze/nudge sway: an ease-out from the current angle to the amplitude, then a spring back. */
export interface CardMotionBreeze {
  /** Milliseconds since the event, advanced by at most [`MAX_DT`] per step. */
  elapsedMs: number;
  /** Initial delay in milliseconds before the sway starts (0–350 ms for gusts). */
  delayMs: number;
  /** Peak rotation of the "out" phase, in degrees. */
  amplitudeDeg: number;
  /**
   * Rotation the nudge started from, in degrees. The "out" phase eases from this angle to
   * [`amplitudeDeg`] (and the delay holds it), so a nudge that interrupts an in-flight arrival
   * swing continues from the presentation value instead of popping to zero
   * (`PeggedView.swift:150-155` animates from the current swing).
   */
  fromDeg: number;
  phase: BreezePhase;
  /** Milliseconds spent in the current phase. */
  phaseElapsedMs: number;
}

/** The 450 ms ease-in-out press tween. */
export interface CardMotionPressTween {
  elapsedMs: number;
  /** Scale the tween started from, so hover → press is continuous. */
  fromScale: number;
}

/** Immutable per-card motion state; all functions return new objects. */
export interface CardMotionState {
  /** Dynamic swing angle in degrees (the per-item static tilt is applied by layout/visuals). */
  rotation: number;
  rotationVelocity: number;
  /** Arrival drop offset in dp; 0 when arrived. */
  offsetY: number;
  offsetYVelocity: number;
  scale: number;
  scaleVelocity: number;
  breeze: CardMotionBreeze | null;
  /** True while the arrival rotation spring is still settling. */
  arriveActive: boolean;
  /** Active press tween, or null while the scale is spring-driven. */
  pressTween: CardMotionPressTween | null;
  /** Last scale target, so `settledCardMotion` can be evaluated without the inputs. */
  targetScale: number;
  /** Wall-clock timestamp of the last event (observability only; motion is `dt`-driven). */
  lastEventAtMs: number;
  reducedMotion: boolean;
}

export interface CardMotionTargets {
  hovered: boolean;
  pressed: boolean;
}

export type CardMotionEvent =
  | { type: "arrive" }
  | { type: "land" }
  | { type: "copied" }
  | { type: "breeze"; amplitudeDeg: number; delayMs: number }
  | { type: "reduced-motion"; value: boolean };

/** Card-local visual scale targets from `PeggedView.swift:82-85` (design spec §3.3 P2). */
const REST_SCALE = 1;
const HOVER_SCALE = 1.035;
const PRESS_SCALE = 0.95;
/** `PeggedView.swift:85`: pressing scales over `.easeInOut(duration: 0.45)`. */
const PRESS_TWEEN_MS = 450;
/** `PeggedView.swift:151`: a nudge eases out over 0.3 s, then springs back. */
const BREEZE_OUT_MS = 300;

/**
 * Rotation snap thresholds. When the under-damped swing decays below these bounds it is visually
 * at rest (sub-pixel at card size), so the state is zeroed to keep `settledCardMotion` exact and
 * avoid waiting seconds for the velocity to cross an arbitrary epsilon.
 */
const ROTATION_SNAP_DEG = 0.1;
const ROTATION_SNAP_VELOCITY = 1.5;

/** Conservative rest bounds for [`settledCardMotion`] (design plan P2-2). */
const OFFSET_REST_DP = 0.05;
const OFFSET_REST_VELOCITY = 0.5;
const ROTATION_REST_DEG = 0.05;
const ROTATION_REST_VELOCITY = 0.5;
const SCALE_REST_EPSILON = 0.0005;

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

/** `PeggedView.swift:151` nudge curve: ease out over the 300 ms "out" phase. */
function easeOutCubic(value: number): number {
  const t = clamp01(value);
  return 1 - Math.pow(1 - t, 3);
}

function targetScaleFor(targets: CardMotionTargets): number {
  if (targets.pressed) return PRESS_SCALE;
  if (targets.hovered) return HOVER_SCALE;
  return REST_SCALE;
}

/** Creates a resting card that has not arrived yet. */
export function createCardMotion(reducedMotion: boolean): CardMotionState {
  return {
    rotation: 0,
    rotationVelocity: 0,
    offsetY: 0,
    offsetYVelocity: 0,
    scale: REST_SCALE,
    scaleVelocity: 0,
    breeze: null,
    arriveActive: false,
    pressTween: null,
    targetScale: REST_SCALE,
    lastEventAtMs: 0,
    reducedMotion,
  };
}

function startBreeze(
  state: CardMotionState,
  amplitudeDeg: number,
  delayMs: number,
): CardMotionState {
  // Reduced motion never sways: the card stays pinned at rest.
  if (state.reducedMotion) return { ...state };
  const safeDelay = Number.isFinite(delayMs) ? Math.max(0, delayMs) : 0;
  const safeAmplitude = Number.isFinite(amplitudeDeg) ? amplitudeDeg : 0;
  return {
    ...state,
    arriveActive: false,
    breeze: {
      elapsedMs: 0,
      delayMs: safeDelay,
      amplitudeDeg: safeAmplitude,
      // Capture the current angle so an in-flight swing keeps its presentation value.
      fromDeg: state.rotation,
      phase: "out",
      phaseElapsedMs: 0,
    },
  };
}

/**
 * Applies a discrete event. Pure: the input state is never mutated; `nowMs` is recorded as the
 * event timestamp and otherwise unused (integration is `dt`-driven).
 */
export function cardEvent(
  state: CardMotionState,
  event: CardMotionEvent,
  nowMs: number,
): CardMotionState {
  const stamped: CardMotionState = { ...state, lastEventAtMs: nowMs };
  switch (event.type) {
    case "arrive":
      // Reduced motion drops the card straight into place with no swing.
      if (stamped.reducedMotion) {
        return {
          ...stamped,
          rotation: 0,
          rotationVelocity: 0,
          offsetY: 0,
          offsetYVelocity: 0,
          breeze: null,
          arriveActive: false,
        };
      }
      return {
        ...stamped,
        rotation: TILT.arriveDeg,
        rotationVelocity: 0,
        offsetY: CARD.arriveOffsetDp,
        offsetYVelocity: 0,
        breeze: null,
        arriveActive: true,
      };
    case "land":
      return startBreeze(stamped, TILT.landDeg, 0);
    case "copied":
      return startBreeze(stamped, TILT.copyDeg, 0);
    case "breeze":
      return startBreeze(stamped, event.amplitudeDeg, event.delayMs);
    case "reduced-motion":
      if (!event.value) return { ...stamped, reducedMotion: false };
      return {
        ...stamped,
        reducedMotion: true,
        rotation: 0,
        rotationVelocity: 0,
        offsetY: 0,
        offsetYVelocity: 0,
        breeze: null,
        arriveActive: false,
        pressTween: null,
      };
  }
}

/**
 * Advances the card one frame. Integration uses [`tick`] (which clamps `dt` to [`MAX_DT`]); the
 * timers (breeze delay/phase and the press tween) advance by the same clamped step so a stalled
 * tab cannot skip animations.
 */
export function stepCardMotion(
  state: CardMotionState,
  targets: CardMotionTargets,
  dtSeconds: number,
): CardMotionState {
  const targetScale = targetScaleFor(targets);
  const dt =
    Number.isFinite(dtSeconds) && dtSeconds > 0 ? Math.min(dtSeconds, MAX_DT) : 0;
  const dtMs = dt * 1000;

  if (state.reducedMotion) {
    return {
      ...state,
      rotation: 0,
      rotationVelocity: 0,
      offsetY: 0,
      offsetYVelocity: 0,
      scale: targetScale,
      scaleVelocity: 0,
      breeze: null,
      arriveActive: false,
      pressTween: null,
      targetScale,
    };
  }

  // Arrival drop: the offset always relaxes to 0 with the arriveY spring.
  const offset = tick(
    { x: state.offsetY, v: state.offsetYVelocity },
    0,
    SPRINGS.arriveY.stiffness,
    SPRINGS.arriveY.damping,
    dt,
  );

  // Rotation: breeze out/back, else the arrival swing, else a residual breeze spring.
  let rotation = state.rotation;
  let rotationVelocity = state.rotationVelocity;
  let breeze = state.breeze;
  let arriveActive = state.arriveActive;

  if (breeze !== null) {
    const elapsedMs = breeze.elapsedMs + dtMs;
    if (elapsedMs <= breeze.delayMs) {
      // Still in the random gust delay: hold the captured angle (never pop to zero).
      rotation = breeze.fromDeg;
      rotationVelocity = 0;
      breeze = { ...breeze, elapsedMs };
    } else {
      const activeMs = elapsedMs - breeze.delayMs;
      if (breeze.phase === "out" && activeMs < BREEZE_OUT_MS) {
        const t = activeMs / BREEZE_OUT_MS;
        // Ease from the captured angle to the amplitude; velocity is the derivative of that curve.
        const travel = breeze.amplitudeDeg - breeze.fromDeg;
        rotation = breeze.fromDeg + easeOutCubic(t) * travel;
        rotationVelocity = (travel * 3 * (1 - t) * (1 - t) * 1000) / BREEZE_OUT_MS;
        breeze = { ...breeze, elapsedMs, phase: "out", phaseElapsedMs: activeMs };
      } else {
        if (breeze.phase === "out") {
          // Crossed 300 ms in this step: finish the out phase exactly at the amplitude.
          rotation = breeze.amplitudeDeg;
          rotationVelocity = 0;
        }
        const next = tick(
          { x: rotation, v: rotationVelocity },
          0,
          SPRINGS.breeze.stiffness,
          SPRINGS.breeze.damping,
          dt,
        );
        rotation = next.x;
        rotationVelocity = next.v;
        const phaseElapsedMs =
          breeze.phase === "out" ? activeMs - BREEZE_OUT_MS : breeze.phaseElapsedMs + dtMs;
        if (
          Math.abs(rotation) < ROTATION_SNAP_DEG &&
          Math.abs(rotationVelocity) < ROTATION_SNAP_VELOCITY
        ) {
          rotation = 0;
          rotationVelocity = 0;
          breeze = null;
        } else {
          breeze = { ...breeze, elapsedMs, phase: "back", phaseElapsedMs };
        }
      }
    }
  } else if (arriveActive) {
    const next = tick(
      { x: rotation, v: rotationVelocity },
      0,
      SPRINGS.arriveSwing.stiffness,
      SPRINGS.arriveSwing.damping,
      dt,
    );
    rotation = next.x;
    rotationVelocity = next.v;
    if (
      Math.abs(rotation) < ROTATION_SNAP_DEG &&
      Math.abs(rotationVelocity) < ROTATION_SNAP_VELOCITY
    ) {
      rotation = 0;
      rotationVelocity = 0;
      arriveActive = false;
    }
  } else {
    // Residual swing after an event already returned home (normally already zero).
    const next = tick(
      { x: rotation, v: rotationVelocity },
      0,
      SPRINGS.breeze.stiffness,
      SPRINGS.breeze.damping,
      dt,
    );
    rotation = next.x;
    rotationVelocity = next.v;
    if (
      Math.abs(rotation) < ROTATION_REST_DEG &&
      Math.abs(rotationVelocity) < ROTATION_REST_VELOCITY
    ) {
      rotation = 0;
      rotationVelocity = 0;
    }
  }

  // Scale: a 450 ms ease-in-out tween while pressed, the hover spring otherwise.
  // Sampling order: evaluate the tween at the current elapsed time first, then advance the
  // elapsed time. Advancing before sampling would make the 27th frame land exactly on 0.95 and
  // break the "> 0.95 at 450 ms" assertion by ~1e-5; it also lets the first pressed frame start
  // from the un-tweened scale.
  let scale: number;
  let scaleVelocity: number;
  let pressTween = state.pressTween;
  if (targets.pressed) {
    if (pressTween === null) {
      // Start from the current scale so hover → press stays continuous.
      pressTween = { elapsedMs: 0, fromScale: state.scale };
    }
    const progress = Math.min(1, pressTween.elapsedMs / PRESS_TWEEN_MS);
    scale = pressTween.fromScale + (PRESS_SCALE - pressTween.fromScale) * easeInOutCubic(progress);
    scaleVelocity = 0;
    pressTween = {
      ...pressTween,
      elapsedMs: Math.min(PRESS_TWEEN_MS, pressTween.elapsedMs + dtMs),
    };
  } else {
    // Releasing the press hands the scale back to the hover spring with no leftover velocity.
    const releasedPress = pressTween !== null;
    pressTween = null;
    const next = tick(
      { x: state.scale, v: releasedPress ? 0 : state.scaleVelocity },
      targetScale,
      SPRINGS.hover.stiffness,
      SPRINGS.hover.damping,
      dt,
    );
    scale = next.x;
    scaleVelocity = next.v;
  }

  return {
    ...state,
    rotation,
    rotationVelocity,
    offsetY: offset.x,
    offsetYVelocity: offset.v,
    scale,
    scaleVelocity,
    breeze,
    arriveActive,
    pressTween,
    targetScale,
  };
}

/** CSS-facing transforms: rotate the unit, translate it by the arrival offset, scale the card. */
export function cardTransforms(state: CardMotionState): {
  rotateDeg: number;
  offsetYDp: number;
  scale: number;
} {
  return {
    rotateDeg: state.rotation,
    offsetYDp: state.offsetY,
    scale: state.scale,
  };
}

/**
 * True when nothing is animating and everything sits inside the conservative rest bounds: no
 * breeze/arrival in flight, a finished (or absent) press tween, offset and rotation near zero with
 * near-zero velocities, and the scale at its last target. A press whose 450 ms tween has completed
 * counts as settled even while the pointer stays down — that is a stable plate, not an animation.
 */
export function settledCardMotion(state: CardMotionState): boolean {
  const pressSettled =
    state.pressTween === null || state.pressTween.elapsedMs >= PRESS_TWEEN_MS;
  return (
    state.breeze === null &&
    !state.arriveActive &&
    pressSettled &&
    Math.abs(state.offsetY) < OFFSET_REST_DP &&
    Math.abs(state.offsetYVelocity) < OFFSET_REST_VELOCITY &&
    Math.abs(state.rotation) < ROTATION_REST_DEG &&
    Math.abs(state.rotationVelocity) < ROTATION_REST_VELOCITY &&
    Math.abs(state.scale - state.targetScale) < SCALE_REST_EPSILON
  );
}
