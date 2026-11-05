# Hangline P2 Visual Soul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Hangline look and move like Tendedero: final glass cards / aluminium pins / rope, real spring physics (arrival swing, breeze/gust, hover/press), screen-wide fly-in from the capture area, screen-wide fall on take-down, light/dark polish, 60 fps with ≤12 cards.

**Architecture:** A pure TS motion layer (`src/lib/motion.ts`, `src/lib/cardMotion.ts`) reproduces the Tendedero spring/curve parameters; the panel integrates it per card via one rAF hook and CSS variables. Screen-wide animations use one pre-warmed, transparent, click-through `fly` WebviewWindow driven by a tiny React route; Rust owns its lifecycle (≤2 concurrent animations, hidden when idle) and the panel asks for animations over IPC. Thumbnails are generated in Rust at ingest so the panel never decodes 4K originals.

**Tech Stack:** Tauri 2.12 + Rust 1.90 + React 19/TS 5/Vite 7 + Zustand; no new runtime dependencies; `image` crate already present for thumbnails; CDP for runtime evidence.

**Spec:** `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` (P2 amendments of 2026-11-02: fly window warm pool, `start_fly`/`start_fall`/`fly_done`, `fly-animation`/`fly-landed`, `Item.thumb_path`, §3.3 P2 geometry, §4.6 P2 motion).

## Global Constraints

- Windows 10 22H2 / 11 x64 only; no new runtime dependencies; no app-initiated network (the reference-download dev script is optional and never shipped).
- All source code/comments in English; docs in Chinese. Brand Hangline / 晾图绳; do not copy Tendedero code or assets (reference measurements only; the official GIF/PNGs stay in git-ignored `.superpowers/p2-reference/`).
- Motion must respect `prefers-reduced-motion` (snap to final state, no breeze/gust/flight — drop-in instead) and stop rAF when the panel is hidden.
- Performance target (P2 DoD): 60 fps with 12 cards; average ≥ 58 fps, p95 frame ≤ 20 ms, no frame > 50 ms while the line is visible and idle.
- Evidence privacy: committed screenshots may contain only Hangline UI or neutral test content; no private desktop, no Tendedero promotional assets.
- Never edit `docs/superpowers/specs|plans` from a task implementer; report deviations to the controller.
- Quality gates per task: `cargo test`, `cargo check --all-targets` 0 warnings, `pnpm check`; commit on `feat/core`; clean tree.

---

## Reference data (measured 2026-11-02)

Source of truth: `research/ui/src/{Line.swift,LineView.swift,PeggedView.swift,CaptureFlight.swift}` (copied from Tendedero main; MIT + name/icon restriction — used as reference only) and the official demo GIF (130 frames × 40 ms = 5.2 s, 960×340, local `.superpowers/p2-reference/demo-light.gif`).

| Item | Value | Source |
|---|---|---|
| Card tilt | random ±2.5°, anchored to the item, whole pin+card rotates | Line.swift:14, PeggedView.swift:23 |
| Photo box | max width `cardWidth-14`, max height `photoMaxHeight`, aspect preserved; card = photo + 8 dp | PeggedView.swift:38-49 |
| Card radius / inset | 16 dp / 4 dp; photo radius 12 | PeggedView.swift:158-160 |
| Pin | 9×26 dp, radius 3.5, horizontal 4-stop metal, slot 5×1.4 at top 8.5, shadow black .30/r2/y1.5 | PeggedView.swift:184-213 |
| Pin/rope/card | pin top = ropeY − 9.5 dp; card top = pin top + 14 dp (12 dp overlap); card visual width = photo width + 8 | LineView.swift:28-29, PeggedView.swift:18-26 |
| Hover / press | scale 1.035 shadow `.26/r14/y8`; press 0.95 over 450 ms ease-in-out; normal shadow `.18/r10/y5` | PeggedView.swift:82-85 |
| Arrival | card from −46 dp, swing 16° → 0, y spring response .42/ζ .72, rotation interpolatingSpring(46, 2.6) | PeggedView.swift:127-136 |
| Breeze / gust | nudge 3° (copy), 2.2° (landing), amplitude 1.6–3.4° with 0–350 ms delay, return spring(38, 2.4); gust every 7–16 s | PeggedView.swift:140-155, Line.swift:304-310 |
| Reveal / tuck | reveal spring(response .42, ζ .82) = k≈224/c≈25; tuck `easeIn 220 ms`; hidden offset `-(panelHeight+12)` | LineView.swift:86-88 |
| Item reorder | spring(response .55, ζ .78) ≈ k≈130.5/c≈17.8 | LineView.swift:82 |
| Fly-in | 0.65 s `easeInOutCubic`, arc `sin(πk)*30`, size/radius/inset/clip opacity interpolated (glass/chrome fades in from k=0.35), tilt interpolated, 0.16 s cross-fade handoff, 120 Hz sampling | CaptureFlight.swift:31-33,190-223 |
| Fall | 0.55 s, 520 dp down, `k³`, extra tilt `tilt*7+20`, opacity `1-e` | CaptureFlight.swift:169-179 |
| Edge fade | card opacity `min(1, max(0, min(x, width−x)/90))` | LineView.swift:78 |
| Empty hint | capsule, 12 pt rounded medium text, rope middle +34 | LineView.swift:65-69,96-105 |
| Demo reference (measured by P2-1) | reveal appears 1.32 s, reaches rest 1.44 s, overshoot +29.3 px at 1.52 s, settled 2.00 s; tuck starts 4.80 s, last pixels 4.92 s, clean 4.96 s; 3 cards; rest tilts −1.10°/+1.39°/−3.49°; Copied pill 3.20–3.84 s. The GIF is a promo animation: use it for timing/geometry cross-checks, not for spring calibration | `.superpowers/p2-reference/demo-light.gif` + `docs/experiments/P2-reference.md` §4 |

Local reference tooling (never committed, git-ignored under `.superpowers/`):

- `scripts/p2-reference/fetch.mjs` — downloads Tendedero `docs/images/{hero-light,hero-dark,demo-light,demo-dark,bento-light,bento-dark,icon}.{png,gif}` into `.superpowers/p2-reference/`; prints sizes. Exits 0 on success; skips existing files.
- `scripts/p2-reference/analyze_demo.py` — Pillow script: frame count/durations, detects the reveal start (first frame whose top strip is non-background), tuck end, card bounding boxes and tilt per frame; writes `.superpowers/p2-reference/demo-measurements.json`.

---

## Evidence tooling and known-drawable notes (controller, 2026-11-02)

- **CDP is no longer reachable via `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`**: wry 0.57.1 always passes its own explicit default browser args, so that env var is ignored. P2-4 must add a **debug-only** hook: when `HANG_CDP_PORT` is set and `cfg(debug_assertions)` is true, create the panel/capture/fly/shortcut webviews with `.additional_browser_args("--remote-debugging-port=<port>")` (one shared helper, e.g. `src-tauri/src/devtools.rs`); release builds must compile the hook out. Evidence tasks then attach with CDP without temporary source patches. Verify: debug run exposes port 9222; `cargo build --release` succeeds with the env var ignored; grep the release binary for `remote-debugging-port` is absent (or the helper is `#[cfg(debug_assertions)]`-gated so it cannot exist).
- **Unknown image dimensions**: P2-3 changed 0×0 items (recordings without a first frame, HEIC files `image` cannot decode) to fill the photo budget (medium 136×104 → card 144×112) instead of the old 16:9 aspect estimate. This is intentional until P3 supplies recording first frames / HEIC decode; P2-9 must record it under known limitations and the P2-6/P2-7 work must not assume dims exist.
- **Copied-pill evidence**: the P2-3 evidence session could not copy (clipboard write refused while the desktop was locked), so the pill screenshot is deferred to P2-8/P2-9; no fabricated shot. Do not treat the missing screenshot as a visual failure.

## File structure

```
src/lib/motion.ts              # P2 tokens: springs, fly/fall curves, card/rope numbers (new)
src/lib/motion.test.ts         # token/curve unit tests (new)
src/lib/cardMotion.ts          # pure per-card motion state machine (new)
src/lib/cardMotion.test.ts     # convergence/overshoot/reduced-motion tests (new)
src/lib/useCardMotion.ts       # rAF hook writing CSS variables (new)
src/lib/layout.ts              # photo/card/pin geometry from the reference (modified)
src/lib/layout.test.ts         # geometry tests updated (modified)
src/components/GlassCard.tsx   # final card + motion integration (modified)
src/components/ClothesPin.tsx  # exact 9×26 aluminium pin (rewritten)
src/components/CopiedPill.tsx  # glass capsule (modified)
src/windows/panel/Panel.tsx    # empty hint, gust, fly/fall orchestration, edge fade (modified)
src/windows/panel/Panel.css    # final materials + motion variables (modified)
src/windows/fly/Fly.tsx        # fly/fall overlay route (new)
src/windows/fly/Fly.css        # overlay materials (new)
src/windows/fly/motion.ts      # pure fly/fall progress math (new)
src/windows/fly/motion.test.ts # pure tests (new)
src/lib/types.ts               # FlyAnimation/FlyMetrics/Item.thumbPath (modified)
src/lib/ipc.ts                 # startFly/startFall/flyDone/takeCaptureOrigin (modified)
src/main.tsx                   # `?window=fly` route (modified)
src-tauri/src/fly.rs           # warm overlay window + lifecycle + concurrency (new)
src-tauri/src/app_state.rs     # transient capture origins (modified)
src-tauri/src/capture.rs       # record capture origin for flight (modified)
src-tauri/src/lib.rs           # commands/events/window + thumb backfill hookup (modified)
src-tauri/src/item.rs          # thumb_path field (modified)
src-tauri/src/thumbnails.rs    # 432 px thumbnail cache (new)
src-tauri/src/watchers/*.rs    # generate thumb after ingest (modified)
src/i18n/{en,zh-Hans}.json     # "Take a screenshot and it will hang here" (modified)
docs/experiments/P2-reference.md  # reference table + method/limitations (new)
docs/experiments/P2-visual.md     # runtime visual/perf/frame-comparison evidence (new)
docs/experiments/P2-perf.json     # numbers only (new)
docs/acceptance/P2-core.md        # P2 acceptance record + manual checklist (new)
scripts/p2-reference/fetch.mjs    # local reference downloader (new, dev-only)
scripts/p2-reference/analyze_demo.py # local GIF analysis (new, dev-only)
scripts/p2/perf.mjs            # CDP fps/motion-trace harness (new, dev-only)
```

---

### Task P2-1: Motion tokens and reference measurements

**Files:**
- Create: `src/lib/motion.ts`, `src/lib/motion.test.ts`
- Create: `scripts/p2-reference/fetch.mjs`, `scripts/p2-reference/analyze_demo.py`
- Create: `docs/experiments/P2-reference.md`

**Interfaces:**
- Produces: `interface SpringPreset { stiffness: number; damping: number }`; `springFromResponse(response: number, dampingFraction: number): SpringPreset`; `SPRINGS: { reveal, item, arriveY, hover, arriveSwing, breeze }`; `FLY` / `FALL` / `TILT` / `GUST` / `EDGE_FADE` / `CARD` constants; `easeInOutCubic`, `smoothstep`, `fallEase` helpers. Card/rope geometry stays in `layout.ts` (Task P2-3).

- [ ] **Step 1: Write the failing test**

```ts
// src/lib/motion.test.ts
import { describe, expect, it } from "vitest";
import {
  FLY, FALL, SPRINGS, TILT, GUST, EDGE_FADE,
  springFromResponse, easeInOutCubic, smoothstep, fallEase,
} from "./motion";

describe("springFromResponse", () => {
  it("reproduces the Tendedero reveal spring (response .42, damping .82)", () => {
    const s = springFromResponse(0.42, 0.82);
    expect(s.stiffness).toBeCloseTo(223.8, 0);
    expect(s.damping).toBeCloseTo(24.5, 0);
  });
  it("reproduces the item reorder spring (response .55, damping .78)", () => {
    const s = springFromResponse(0.55, 0.78);
    expect(s.stiffness).toBeCloseTo(130.5, 0);
    expect(s.damping).toBeCloseTo(17.8, 0);
  });
});

describe("P2 tokens", () => {
  it("uses the Tendedero fly-in and fall curves", () => {
    expect(FLY.durationMs).toBe(650);
    expect(FLY.arcDp).toBe(30);
    expect(FLY.chromeStart).toBe(0.35);
    expect(FLY.handoffMs).toBe(160);
    expect(FALL.durationMs).toBe(550);
    expect(FALL.dropDp).toBe(520);
  });
  it("keeps the reference tilt/gust/edge-fade numbers", () => {
    expect(TILT.maxDeg).toBe(2.5);
    expect(GUST.minIntervalMs).toBe(7000);
    expect(GUST.maxDelayMs).toBe(350);
    expect(GUST.minAmplitudeDeg).toBe(1.6);
    expect(EDGE_FADE.widthDp).toBe(90);
    expect(SPRINGS.reveal.stiffness).toBeCloseTo(224, 0);
    expect(SPRINGS.item.damping).toBeCloseTo(17.8, 0);
  });
});

describe("curves", () => {
  it("easeInOutCubic is symmetric and monotonic", () => {
    expect(easeInOutCubic(0)).toBe(0);
    expect(easeInOutCubic(1)).toBe(1);
    expect(easeInOutCubic(0.5)).toBeCloseTo(0.5);
    expect(easeInOutCubic(0.3) + easeInOutCubic(0.7)).toBeCloseTo(1);
  });
  it("smoothstep clamps outside its window", () => {
    expect(smoothstep(0.2, 0.35, 1)).toBe(0);
    expect(smoothstep(0.7, 0.35, 1)).toBeCloseTo(0.5576, 4); // standard smoothstep, not 0.5
    expect(smoothstep(1.4, 0.35, 1)).toBe(1);
  });
  it("fallEase is cubic", () => {
    expect(fallEase(0.5)).toBeCloseTo(0.125);
    expect(fallEase(1)).toBe(1);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails**

`pnpm exec vitest run src/lib/motion.test.ts` → FAIL (module missing).

- [ ] **Step 3: Implement `src/lib/motion.ts`**

```ts
/** Tendedero-derived motion/visual tokens (design spec §3.3/§4.6 P2 amendments). */
export interface SpringPreset { stiffness: number; damping: number }

/** Converts SwiftUI `spring(response:dampingFraction:)` into k/c for a unit mass. */
export function springFromResponse(response: number, dampingFraction: number): SpringPreset {
  const omega = (2 * Math.PI) / response;
  return { stiffness: Math.round(omega * omega * 10) / 10, damping: Math.round(2 * dampingFraction * omega * 10) / 10 };
}

export const SPRINGS = {
  reveal: { stiffness: 224, damping: 25 },        // response .42 ζ .82
  item: springFromResponse(0.55, 0.78),           // layout reorder
  arriveY: springFromResponse(0.42, 0.72),        // card drop-in from -46 dp
  hover: springFromResponse(0.3, 0.6),            // hover/press scale
  arriveSwing: { stiffness: 46, damping: 2.6 },   // 16° → 0
  breeze: { stiffness: 38, damping: 2.4 },        // 1.6–3.4° → 0
} as const satisfies Record<string, SpringPreset>;

export const FLY = { durationMs: 650, arcDp: 30, chromeStart: 0.35, handoffMs: 160, fadeMs: 160 } as const;
export const FALL = { durationMs: 550, dropDp: 520 } as const;
export const TILT = { maxDeg: 2.5, arriveDeg: 16, landDeg: 2.2, copyDeg: 3 } as const;
export const GUST = { minIntervalMs: 7000, maxIntervalMs: 16000, maxDelayMs: 350, minAmplitudeDeg: 1.6, maxAmplitudeDeg: 3.4 } as const;
export const EDGE_FADE = { widthDp: 90 } as const;
export const CARD = { arriveOffsetDp: -46 } as const;

export function easeInOutCubic(x: number): number {
  const t = Math.min(1, Math.max(0, x));
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}
export function smoothstep(x: number, a: number, b: number): number {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
}
export function fallEase(x: number): number {
  const t = Math.min(1, Math.max(0, x));
  return t * t * t;
}
```

- [ ] **Step 4: Write the local reference scripts**

`scripts/p2-reference/fetch.mjs` (Node 22 `fetch`, no deps):

```js
#!/usr/bin/env node
// Dev-only: downloads Tendedero promotional reference assets for local frame comparison.
// The assets are Tendedero's (MIT + name/icon restriction) and are NEVER committed.
import { mkdir, writeFile, access } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../.superpowers/p2-reference");
const base = "https://raw.githubusercontent.com/alejandrobujan/tendedero/main/docs/images/";
const files = ["hero-light.png", "hero-dark.png", "demo-light.gif", "demo-dark.gif", "bento-light.png", "bento-dark.png", "icon.png"];

await mkdir(root, { recursive: true });
for (const name of files) {
  const dest = join(root, name);
  try { await access(dest); console.log(`skip ${name}`); continue; } catch {}
  const response = await fetch(base + name);
  if (!response.ok) { console.error(`FAIL ${name}: HTTP ${response.status}`); process.exitCode = 1; continue; }
  const bytes = Buffer.from(await response.arrayBuffer());
  await writeFile(dest, bytes);
  console.log(`ok ${name} ${bytes.length} bytes`);
}
console.log(`reference dir: ${root} (git-ignored)`);
```

`scripts/p2-reference/analyze_demo.py` (dev-only, Pillow):

```python
"""Dev-only: measure the Tendedero demo GIF (frames, reveal/tuck timing, card boxes).
Writes .superpowers/p2-reference/demo-measurements.json. Not shipped; never commit the GIF."""
import json, os
from PIL import Image, ImageSequence

REF = os.path.join(os.path.dirname(__file__), "../../.superpowers/p2-reference")
gif = Image.open(os.path.join(REF, "demo-light.gif"))
frames = [f.convert("RGB") for f in ImageSequence.Iterator(gif)]
durations = [f.info.get("duration", 40) for f in ImageSequence.Iterator(Image.open(os.path.join(REF, "demo-light.gif")))]

def busy_ratio(img):
    # Demo background is a smooth pastel gradient; cards/pins add saturation and dark pixels.
    px = img.resize((240, 85)).load()
    busy = 0
    for y in range(85):
        for x in range(240):
            r, g, b = px[x, y]
            if max(r, g, b) - min(r, g, b) > 40 or (r + g + b) / 3 < 90:
                busy += 1
    return busy / (240 * 85)

ratios = [busy_ratio(f) for f in frames]
threshold = 0.02
revealed = next((i for i, r in enumerate(ratios) if r > threshold), None)
tucked = next((i for i in range(len(ratios) - 1, -1, -1) if ratios[i] > threshold), None)
result = {
    "frames": len(frames),
    "size": list(gif.size),
    "durations": durations,
    "totalMs": sum(durations),
    "busyRatios": [round(r, 4) for r in ratios],
    "revealFrame": revealed,
    "tuckEndFrame": tucked,
}
json.dump(result, open(os.path.join(REF, "demo-measurements.json"), "w"), indent=2)
print(json.dumps({k: result[k] for k in ("frames", "size", "revealFrame", "tuckEndFrame", "totalMs")}, indent=2))
```

Run both scripts once; confirm `demo-measurements.json` exists and `revealFrame`/`tuckEndFrame` are plausible (reveal between 1.2–1.6 s, tuck after 3.4 s). Record the numbers in `docs/experiments/P2-reference.md` together with the table from the "Reference data" section above, the method, the copyright note (assets local-only, not committed) and the limitation that Tendedero's macOS recording cannot be produced on this machine, so comparison is frame-timing + geometry + motion-trace based.

- [ ] **Step 5: Run tests and commit**

Run: `pnpm exec vitest run src/lib/motion.test.ts` → PASS; `pnpm check` green.
Commit: `feat: P2 motion tokens and reference measurements`.

---

### Task P2-2: Card motion state machine

**Files:**
- Create: `src/lib/cardMotion.ts`, `src/lib/cardMotion.test.ts`
- Modify: `src/lib/spring.ts` only if a shared helper is needed (keep `tick` backward compatible)

**Interfaces:**
- Produces: `CardMotionState`, `CardMotionTargets { hovered: boolean; pressed: boolean }`, `createCardMotion(reducedMotion: boolean): CardMotionState`, `cardEvent(state, event, nowMs)`, `stepCardMotion(state, targets, dtSeconds): CardMotionState`, `cardTransforms(state): { rotateDeg: number; offsetYDp: number; scale: number }`, `settledCardMotion(state): boolean`. Events: `{ type: "arrive" } | { type: "land" } | { type: "copied" } | { type: "breeze"; amplitudeDeg: number; delayMs: number } | { type: "reduced-motion"; value: boolean }`.

- [ ] **Step 1: Write the failing test**

```ts
// src/lib/cardMotion.test.ts
import { describe, expect, it } from "vitest";
import { cardEvent, createCardMotion, cardTransforms, stepCardMotion, settledCardMotion } from "./cardMotion";

function run(state, targets, frames, dt = 1 / 60) {
  let s = state;
  for (let i = 0; i < frames; i += 1) s = stepCardMotion(s, targets, dt);
  return s;
}

describe("card arrival", () => {
  it("starts 46 dp above, 16° tilted, and settles", () => {
    let s = createCardMotion(false);
    s = cardEvent(s, { type: "arrive" }, 0);
    expect(cardTransforms(s).rotateDeg).toBe(16);
    expect(cardTransforms(s).offsetYDp).toBeCloseTo(-46, 0);
    s = run(s, { hovered: false, pressed: false }, 240);
    expect(cardTransforms(s).offsetYDp).toBeGreaterThan(-0.6);
    expect(cardTransforms(s).rotateDeg).toBeLessThan(0.6);
    expect(settledCardMotion(s)).toBe(true);
  });
});

describe("breeze and nudges", () => {
  it("peaks near the amplitude then returns to rest", () => {
    let s = createCardMotion(false);
    s = cardEvent(s, { type: "arrive" }, 0);
    s = run(s, { hovered: false, pressed: false }, 120);
    s = cardEvent(s, { type: "breeze", amplitudeDeg: 3, delayMs: 0 }, 2000);
    let peak = 0;
    for (let i = 0; i < 120; i += 1) { s = stepCardMotion(s, { hovered: false, pressed: false }, 1 / 60); peak = Math.max(peak, Math.abs(cardTransforms(s).rotateDeg)); }
    expect(peak).toBeGreaterThan(2.2);
    expect(peak).toBeLessThan(3.6);
  });
  it("copied/land nudges are small and settle", () => {
    let s = cardEvent(createCardMotion(false), { type: "copied" }, 0);
    s = run(s, { hovered: false, pressed: false }, 180);
    expect(Math.abs(cardTransforms(s).rotateDeg)).toBeLessThan(0.5);
  });
});

describe("hover/press scale", () => {
  it("overshoots above 1.035 (under-damped) then settles", () => {
    let s = createCardMotion(false);
    let max = 1;
    for (let i = 0; i < 60; i += 1) { s = stepCardMotion(s, { hovered: true, pressed: false }, 1 / 60); max = Math.max(max, cardTransforms(s).scale); }
    expect(max).toBeGreaterThan(1.035);
    expect(cardTransforms(s).scale).toBeCloseTo(1.035, 2);
  });
  it("press reaches 0.95 over 450 ms", () => {
    let s = createCardMotion(false);
    for (let i = 0; i < 27; i += 1) s = stepCardMotion(s, { hovered: false, pressed: true }, 1 / 60);
    expect(cardTransforms(s).scale).toBeLessThan(0.98);
    expect(cardTransforms(s).scale).toBeGreaterThan(0.95);
  });
});

describe("reduced motion", () => {
  it("snaps to rest instantly", () => {
    const s = cardEvent(createCardMotion(true), { type: "arrive" }, 0);
    expect(cardTransforms(s)).toEqual({ rotateDeg: 0, offsetYDp: 0, scale: 1 });
    const next = stepCardMotion(s, { hovered: true, pressed: false }, 1 / 60);
    expect(cardTransforms(next).scale).toBe(1.035);
  });
});
```

- [ ] **Step 2: Run and confirm failure**

`pnpm exec vitest run src/lib/cardMotion.test.ts` → FAIL (module missing).

- [ ] **Step 3: Implement `src/lib/cardMotion.ts`**

Implementation requirements (use `tick` from `./spring` with `MAX_DT` clamping):
- State: `rotation`, `rotationVelocity`, `offsetY`, `offsetYVelocity`, `scale`, `scaleVelocity`, `breeze: { elapsedMs, delayMs, amplitudeDeg, phase: "out" | "back", phaseElapsedMs } | null`, `arriveActive`, `reducedMotion`.
- `createCardMotion` → zeroed state, scale 1, no breeze.
- `cardEvent`:
  - `arrive`: offsetY = −46, offsetYVelocity = 0, rotation = 16, rotationVelocity = 0, breeze = null (reduced motion → all zero).
  - `land`: start breeze phase at amplitude 2.2 with 0 delay; `copied`: amplitude 3 with 0 delay.
  - `breeze`: set breeze `{ elapse 0, delay, amplitude, phase: "out" }`.
  - `reduced-motion`: if true, zero rotation/offset/breeze and scale target handled by step.
- `stepCardMotion`:
  - reduced motion: rotation = 0, offsetY = 0, scale = target scale (pressed 0.95 else hovered 1.035 else 1); return.
  - offsetY: `tick({x: offsetY, v: offsetYVelocity}, 0, SPRINGS.arriveY.stiffness, SPRINGS.arriveY.damping, dt)`.
  - breeze: if `elapsed < delay` no rotation change; then `phase "out"`: `rotation = easeOut(t/0.3) * amplitude` for 300 ms; then `phase "back"`: integrate rotation spring to 0 with `SPRINGS.breeze`; when |rot|/|v| small → breeze = null.
  - else: integrate rotation spring to 0 with `SPRINGS.arriveSwing` (arrival) or `SPRINGS.breeze` (after nudge) — keep a `settleSpring` flag set by the event.
  - scale target = 0.95 pressed, else hovered 1.035, else 1; integrate with `SPRINGS.hover`.
  - CSS: return rotation (state.rotation), offsetY, scale; `settledCardMotion` true when breeze null and |offsetY|<0.05, |offsetYVelocity|<0.5, |rotation|<0.05, |rotationVelocity|<0.5, |scale−target|<0.0005.
- `cardTransforms` returns `{ rotateDeg: rotation, offsetYDp: offsetY, scale }`.

- [ ] **Step 4: Run tests and commit**

`pnpm exec vitest run src/lib/cardMotion.test.ts src/lib/spring.test.ts` → PASS; `pnpm check`.
Commit: `feat: P2 card motion state machine`.

---

### Task P2-3: Final card, pin and rope visuals

**Files:**
- Modify: `src/lib/layout.ts`, `src/lib/layout.test.ts`
- Modify: `src/components/GlassCard.tsx`, `src/components/ClothesPin.tsx`, `src/components/CopiedPill.tsx`
- Modify: `src/windows/panel/Panel.css`, `src/windows/panel/Panel.tsx` (empty hint only in this task)
- Modify: `src/i18n/en.json`, `src/i18n/zh-Hans.json`

**Interfaces:**
- Produces in `layout.ts`: `PIN_ABOVE = 9.5`, `CARD_PIN_OVERLAP = 12`, `CARD_VISUAL_INSET = 4`, `photoSize(width, height, size): { width: number; height: number }`, `cardVisualSize(item, size): { width: number; height: number }`, `pinTop(ropeY): number`, `cardTopFromRope(ropeY): number`, `edgeFadeOpacity(x, panelWidth): number`. Existing `cardX/capacity/visibleSlice/wheelStep/tiltFor/ropeY/cardTop` stay; `cardPhotoHeight/cardHeight/cardAspect` are replaced by the new functions (update callers).

- [ ] **Step 1: Write failing geometry tests** (append to `layout.test.ts`)

```ts
import { photoSize, cardVisualSize, pinTop, cardTopFromRope, edgeFadeOpacity, PIN_ABOVE, CARD_PIN_OVERLAP } from "./layout";

it("photo fits maxWidth cardWidth-14 and maxHeight photoMaxHeight, aspect preserved", () => {
  const wide = photoSize(1920, 1080, "medium");    // 16:9
  expect(wide.width).toBeCloseTo(136, 0);
  expect(wide.height).toBeCloseTo(76.5, 0);
  const tall = photoSize(800, 1600, "medium");     // 1:2
  expect(tall.height).toBeCloseTo(104, 0);
  expect(tall.width).toBeCloseTo(52, 0);
  const card = cardVisualSize({ width: 800, height: 1600 }, "medium");
  expect(card.width).toBeCloseTo(60, 0);           // photo + 2*4 inset
  expect(card.height).toBeCloseTo(112, 0);
});

it("pin and card hang exactly like Tendedero", () => {
  const rope = ropeY(1920, 960);
  expect(pinTop(rope)).toBeCloseTo(rope - PIN_ABOVE, 2);
  expect(cardTopFromRope(rope)).toBeCloseTo(rope - PIN_ABOVE + (26 - CARD_PIN_OVERLAP), 2);
});

it("cards fade near the line edges", () => {
  expect(edgeFadeOpacity(0, 1920)).toBe(0);
  expect(edgeFadeOpacity(45, 1920)).toBeCloseTo(0.5, 1);
  expect(edgeFadeOpacity(96, 1920)).toBe(1);
  expect(edgeFadeOpacity(1824, 1920)).toBe(1);
});
```

- [ ] **Step 2: Run and confirm failure**, then implement:

```ts
export const PIN_ABOVE = 9.5;
export const CARD_PIN_OVERLAP = 12;
const PHOTO_SIDE_PX = 7; // (cardWidth - 14) / 2

export function photoSize(width: number, height: number, size: CardSize): { width: number; height: number } {
  const maxW = cardWidth(size) - 14;
  const maxH = photoMaxHeight(size);
  if (!(width > 0) || !(height > 0)) return { width: maxW, height: maxH };
  const scale = Math.min(maxW / width, maxH / height);
  return { width: width * scale, height: height * scale };
}
export function cardVisualSize(item: Pick<Item, "width" | "height">, size: CardSize) {
  const photo = photoSize(item.width, item.height, size);
  return { width: photo.width + 2 * CARD_VISUAL_INSET, height: photo.height + 2 * CARD_VISUAL_INSET };
}
export function pinTop(ropeTopY: number): number { return ropeTopY - PIN_ABOVE; }
export function cardTopFromRope(ropeTopY: number): number { return ropeTopY - PIN_ABOVE + (PIN_HEIGHT - CARD_PIN_OVERLAP); }
export function edgeFadeOpacity(x: number, panelWidth: number): number {
  return Math.min(1, Math.max(0, Math.min(x, panelWidth - x) / EDGE_FADE.widthDp));
}
```

Remove `cardPhotoHeight/cardHeight/cardAspect` and fix `Panel.tsx`/`GlassCard.tsx` callers (card slot keeps `cardWidth(size)` for layout spacing; the visual card inside is centred and variable width).

- [ ] **Step 3: Rewrite `ClothesPin.tsx` to the exact source pin**

SVG 9×26: outer rounded rect (radius 3.5) filled with a horizontal 4-stop gradient `#b3b3b3 → #ededed → #d1d1d1 → #9e9e9e` (white-space gradient), stroke `linear-gradient(#e6e6e6 → #2e2e2e)` 0.6; slot `<rect x=2 y=8.5 width=5 height=1.4 rx=.7 fill="rgba(0,0,0,.32)">`; CSS `.clothes-pin { filter: drop-shadow(0 1.5px 2px rgba(0,0,0,.30)); }` on the SVG (matches source shadow). Keep the SVG purely decorative.

- [ ] **Step 4: Rewrite the card DOM/CSS in `GlassCard.tsx`**

Structure:

```tsx
<div className="glass-card-slot" style={{ left: `${x}px`, top: `${pinTopY}px`, width: `${slotWidth}px`, height: `${pinVisibleHeight}px` }}>
  <div className="glass-card-unit" ref={unitRef}>
    <ClothesPin className="glass-card-slot__pin" />
    <article className="glass-card" style={{ width: cardW, height: cardH, marginLeft: (slotWidth - cardW) / 2, marginTop: PIN_HEIGHT - CARD_PIN_OVERLAP }}>
      <div className="glass-card__photo" style={{ borderRadius: 12 }}>{image}</div>
      ...
    </article>
  </div>
</div>
```

- `slotWidth = cardWidth(size)`; `pinTopY = pinTop(ropeYAtCardCenter)`; unit rotates around top centre (`transform-origin: 50% 0`), pin at top centre, card offset down so the pin overlaps 12 dp; photo keeps aspect (`object-fit: contain` with exact `photoSize` box, background transparent/dominant colour fill).
- Remove the slot-hover CSS transition for scale (motion hook owns scale/rotation in P2-4); this task keeps the DOM/CSS correct with static transforms and hover shadow classes.
- Card CSS: radius 16, inset 4 padding; the 0.5 dp edge is an **inset ring inside the box-shadow** (border adds layout and breaks the 4/4 inset); shadow normal `.18/r10/y5` and hover `.26/r14/y8` (light/dark from variables), photo inner stroke `rgba(255,255,255,.18) 0.5`, cross = 20×20 glass circle at top-left (3 dp padding) visible on hover (`opacity 0→1, scale .6→1`), hit area still 26 dp via an outer button, recording play = 28×28 glass circle with 11 pt triangle (`currentColor`, theme-aware), copied pill = glass capsule bottom offset 16 dp.
- Add `.panel__hint` capsule in `Panel.tsx` when `state.items.length === 0`: text `t("Take a screenshot and it will hang here")`, centred at the Tendedero position — `top = ropeY(panelWidth, panelWidth/2) + 34`, `left: 50%`, `transform: translate(-50%, -50%)` (SwiftUI `.position` semantics); CSS: 12 px rounded-medium, secondary text, `padding: 6px 12px`, glass capsule with `backdrop-filter` fallback gradients.
- i18n add key `"Take a screenshot and it will hang here"` → zh `"截一张图，它会挂在这里"`.
- Update `Panel.css` selectors for the new DOM (`.glass-card-unit`, `.glass-card-slot__pin` absolute at top centre, remove old top offsets) while keeping hit-rect reporting (`cardRef` on `<article>`; the reported rect is the rotated AABB, unchanged).

- [ ] **Step 5: Run tests, check the rendered panel with CDP screenshots (light/dark), commit**

`pnpm check` green; `cargo test` untouched. Runtime: start dev app, screenshot empty-line hint, one/two cards at light+dark; attach to evidence later task.
Commit: `feat: P2 final card, pin and rope visuals`.

---

### Task P2-4: Panel choreography (sway, gust, layout springs, hint)

**Files:**
- Create: `src/lib/useCardMotion.ts`
- Modify: `src/components/GlassCard.tsx`, `src/windows/panel/Panel.tsx`, `src/windows/panel/Panel.css`
- Test: pure decisions live in `cardMotion.test.ts` (P2-2) and `layout.test.ts`; hook itself is exercised in runtime evidence.

**Interfaces:**
- `useCardMotion(unitRef: RefObject<HTMLElement>, options: { reducedMotion: boolean; paused: boolean })` → `{ arrive(): void; land(): void; copied(): void; breeze(amplitudeDeg: number, delayMs: number): void; setTargets(targets: CardMotionTargets): void }`; writes CSS variables `--swing`, `--arrive-y`, `--card-scale` through `element.style.setProperty`.
- `GlassCard` props add `paused?: boolean`, `gust?: number`, `flyState?: "none" | "flying"`, and reports hover/press into the hook internally.

- [ ] **Step 1: Implement `useCardMotion.ts`**

One rAF loop per card, started on mount and on every event; each frame: `stepCardMotion`, write vars, stop when settled and no new event. On unmount cancel. If `paused` (panel hidden) stop the loop without advancing; on unpause continue from last state (no giant dt: pass `Math.min(dt, 1/30)`).
`GlassCard` uses it:
- on mount: `arrive()` unless the item is flying in (then wait for `flyState → none` and call `land()`);
- `onPointerEnter/Leave`: `setTargets({hovered, pressed})`; `onPointerDown`: pressed true; `onPointerUp/Leave`: pressed false;
- `onChange(gust)`: `breeze(randomAmplitude(), randomDelay())` (only if !paused, !reducedMotion);
- copy success: `copied()`; 
- CSS uses `transform: translate3d(0, var(--arrive-y, 0), 0) rotate(var(--swing, 0deg)) scale(var(--card-scale, 1))` on `.glass-card-unit` (rotation first then scale on the card element per source; split: unit gets `translateY + rotate`, card gets `scale` via `--card-scale`).
- Source-like anchor: unit `transform-origin: 50% 0`; card `transform-origin: 50% 0`.

- [ ] **Step 2: Panel wiggle/position integration**

- `Panel.tsx`: add `gust` counter with a `setTimeout` chain: when `revealed && items.length > 0 && !document.hidden`, schedule next at `GUST.min + random*(max-min)` (7–16 s), increment; clear on tuck/unmount.
- Pass `paused={!revealed}` and `gust` to each `GlassCard`.
- Position: keep `left/top` from layout but animate x/y changes using CSS custom props + a per-card slot spring? For P2, reorder animation uses the card unit's `--slot-x/--slot-y`? The slot is absolutely positioned; simplest faithful approach: the slot itself gets `transform: translate3d(var(--slot-dx), var(--slot-dy), 0)` where targets are set on prop changes through a small spring in the component (reuse `springTick` with `SPRINGS.item`). Implement a `useSlotSpring` inside `GlassCard` that on x/top prop change springs offset from previous to new target (initial mount at target, no animation). Write `--slot-dx/--slot-dy` on slot.
- Edge fade: style `opacity: edgeFadeOpacity(x + cardW/2, panelWidth)` on the slot (updated every render).
- Empty hint already rendered in P2-3; hide when any card exists.

- [ ] **Step 3: CSS**

- Remove `.glass-card-slot:hover .glass-card` scale/transition; hover shadow still driven by `:hover` (fast 160 ms cross-fade of shadow only); hover scale/rotation now JS.
- `.glass-card-unit { transform: translate3d(0, var(--arrive-y,0px),0) rotate(var(--swing,0deg)); transform-origin: 50% 0; will-change: transform; }`
- `.glass-card { transform: scale(var(--card-scale,1)); transform-origin: 50% 0; }`
- Reduced motion: no CSS transitions; hook snaps.
- `data-flying="true"` hides the card (`visibility: hidden`) while the overlay draws it; `data-falling` hides immediately.

- [ ] **Step 4: Verify runtime, commit**

Runtime evidence: screenshots of an arrival frame, breeze frame (trace log), hover/press frames; `pnpm check` green.
Commit: `feat: P2 card sway, breeze and layout springs`.

---

### Task P2-5: Fly-in overlay (Rust + frontend)

> Note (controller, after P2-3): `layout.ts` no longer exports `cardTop`; use `cardTopFromRope` for the fly target (the dead wrapper was removed in 9b427ea).

**Files:**
- Create: `src-tauri/src/fly.rs`, `src/windows/fly/Fly.tsx`, `src/windows/fly/Fly.css`, `src/windows/fly/motion.ts`, `src/windows/fly/motion.test.ts`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/app_state.rs`, `src-tauri/src/capture.rs`, `src-tauri/src/item.rs` (transient origin helpers only)
- Modify: `src/main.tsx`, `src/lib/types.ts`, `src/lib/ipc.ts`, `src/windows/panel/Panel.tsx`

**Interfaces:**
- Rust commands: `start_fly(app, id: String, from: Option<PhysicalRect>, to: PhysicalRect, tilt_deg: f64) -> Result<bool, String>`; `start_fall(app, id: String, image_path: String, rect: PhysicalRect, tilt_deg: f64) -> Result<bool, String>`; `fly_done(app, id: String)`; `fly_window_ready(app) -> Result<PhysicalRect, String>` (returns virtual rect + marks ready); `take_capture_origin(app, id: String) -> Option<PhysicalRect>`.
- Rust events: `fly-animation` to `fly` window: `{ animations: FlyAnimation[] }`; `fly-landed` to `panel`: `{ id }`.
- TS: `interface FlyAnimation { id: string; kind: "fly" | "fall"; imagePath: string; from: PhysicalRect; to: PhysicalRect; tiltDeg: number; durationMs: number }`; `ipc.startFly(...)`, `ipc.startFall(...)`, `ipc.flyDone(...)`, `ipc.takeCaptureOrigin(...)`.
- `capture::finish_capture` records the crop origin (physical rect on the virtual screen) keyed by the new item id in `AppState` (bounded map, e.g. 8 entries, cleared when taken or after 30 s).

**Rust `fly.rs` design:**
- Consts: `WINDOW_LABEL = "fly"`, `MAX_ANIMATIONS = 2`, `WARMUP_TIMEOUT = Duration::from_secs(5)`.
- State (managed): `Mutex<FlyState { window: Option<WebviewWindow>, ready: bool, active: BTreeMap<String, Kind> }>`.
- `ensure_window(app)`: create once — transparent, decorations false, always on top, skip taskbar, focusable false, shadow false, visible **false**, resizable false; set physical position/size to `geometry::virtual_screen_rect()`; URL `index.html?window=fly`; `set_ignore_cursor_events(true)`.
- `start_fly`/`start_fall`: if `!ready` or active.len() >= 2 → `Ok(false)`; insert active with kind; ensure window; `show()` + `set_ignore_cursor_events(true)`; emit `fly-animation` with the full active list (serialize from stored payloads, not just ids — store `FlyAnimation` structs in the state).
- `fly_done`: remove id; if kind fly also emit `fly-landed` to panel; if active empty hide window.
- `report_fly_ready`: sets ready, returns virtual rect (the page uses it to compute `cssScale = innerWidth / rect.w`).
- On app exit: hide/destroy the window (no special handling needed; process exit).
- Tests (pure, no AppHandle): `fallback_from_rect(to, monitors)` returns a 120×80 dp rect centred on the monitor containing `to`; `can_start(active_len, ready)`; `active_list_json` ordering; `tilt` passthrough. Any AppHandle-dependent code stays thin.

**Frontend overlay `Fly.tsx`:**
- On mount: `const { virtualRect } = await ipc.flyWindowReady()`; store `cssScale = innerWidth / virtualRect.w`.
- Listen `fly-animation`: maintain a map id→animation; start rAF loops for new ids; compute CSS coords: `left = (from.x - virtualRect.x) * cssScale` etc.
- Each animation: for `kind === "fly"`, at each frame `k = easeInOutCubic(elapsed/durationMs)`: width/height lerp from `from` to `to`; top-centre position lerp + `sin(πk)*30*cssScale` upward arc; radius 0→16; inset 0→4; glass/chrome opacity `smoothstep(k, 0.35, 1)`; rotation `tilt*k`; shadow; the photo layer uses `object-fit: cover` and the clip drawn at the top edge (`width 9`, `height 26`, top `h-12`); when `k>=1`: add `.handoff` (fade 160 ms) then `ipc.flyDone(id)`; remove.
- `kind === "fall"`: `e = fallEase(t)`; translateY +520dp*e; rotation `tilt + (tilt*7+20)*e`; opacity `1-e`; at end `ipc.flyDone(id)` (no landed event).
- Window is click-through: CSS `pointer-events: none` everywhere; the whole page has `background: transparent`.
- `motion.ts` pure math: `flyFrame(k) -> { widthLerp, arcDp, insetDp, radiusDp, chromeOpacity, rotationDeg }`, `fallFrame(k) -> { dropDp, extraTiltDeg, opacity }`; `cssScale(innerWidth, virtualWidth)`; tests for endpoints/symmetry/monotonicity.

**Panel integration:**
- **Hit rects during flight**: exclude a card with `flyState="flying"` from `collectHitRects` (no rest AABB for an invisible card), so the panel stays click-through over the frozen screen during the flight.
- Track `prevItemIds` in a ref; on every `state` snapshot compute added ids. For the newest added item (only when exactly one is new and none of ours is flying): `const from = item.kind === "capture" ? await ipc.takeCaptureOrigin(item.id) : null;` `const to = physicalCardRect(item)` computed from layout: `left = cardX(index,capacity,size,panelWidth) + (slotWidth - cardVisual.width)/2`, `top = cardTopFromRope(ropeY(panelWidth, centerX))`; convert to physical with `window.devicePixelRatio`; `tilt = tiltFor(item.id)`; `const accepted = await ipc.startFly(item.id, from, to, tilt)`; if accepted set `flyingId` state (card renders with `data-flying`), else render normally; if rejected because overlay busy, show the card with the normal arrive animation.
- On `fly-landed`: clear `flyingId`, card renders and calls `land()` via the `flyState` prop transition.
- If more than one item is added at once, no flight (all drop in) — document.
- `Panel.tsx` must not animate while hidden: gate gust/arrive on `revealed`.

**Runtime evidence:** invoke `request_capture` + `finish_capture` via CDP (as Task 16 did) with a known rect, record the fly window screenshots at k≈0.15/0.5/0.9 and the landed card, plus a clipboard item (fallback-from-centre) flight; log lines `fly: started id=... kind=fly`, `fly: landed`; assert overlay window is `visible` during and hidden after; verify `data-flying` hides the card.
Manual pending: real-capture feel/arc.

- [ ] Steps: write pure tests → fail → implement Rust + frontend → wire → runtime evidence → commit: `feat: P2 screen-wide fly-in overlay`.

---

### Task P2-6: Fall animation and take-down choreography

**Files:**
- Modify: `src/windows/panel/Panel.tsx`, `src/components/GlassCard.tsx`, `src/lib/types.ts` (if fall event types needed)
- Modify: `src-tauri/src/fly.rs` only if fall payload fields need extension (imagePath already present)

**Interfaces:**
- Panel helper (pure, testable): `planFallSequence(removedIds: string[], rects: Map<string, PhysicalRect>, tiltFor: (id: string) => number, now: number): Array<{ id: string; delayMs: number; rect: PhysicalRect; tiltDeg: number }>` — stagger 60 ms, max 2 accepted by the overlay (the caller stops sending once `startFall` returns false > 2 consecutive times).

- [ ] **Step 1: Pure test**

```ts
it("staggers falls 60 ms and keeps the input order", () => {
  const plan = planFallSequence(["a", "b", "c"], new Map([["a", rect], ["b", rect], ["c", rect]]), () => 1.5, 0);
  expect(plan.map((p) => p.delayMs)).toEqual([0, 60, 120]);
  expect(plan.map((p) => p.id)).toEqual(["a", "b", "c"]);
});
it("skips ids without a known rect", () => {
  expect(planFallSequence(["a", "b"], new Map([["b", rect]]), () => 0, 0)).toHaveLength(1);
});
```

- [ ] **Step 2: Integrate**
- Take-down click (`GlassCard` cross): freeze `rect = lastReportedRect(id)` (physical) and `tilt`; call `ipc.startFall(id, item.path, rect, tilt)` (await, ignore false); mark id in `fallStartedRef`; call `takeDown` as today. Card hides immediately (`data-falling`).
- `Panel.tsx`: keep a `Map<string, {rect, tilt, path}>` of last rendered cards (updated from hit-rect reporting / layout). On state snapshot diff, for every id removed: if not already `fallStarted`, enqueue fall. Run `planFallSequence` and execute with `setTimeout` delays: call `startFall`; drop entries when overlay rejects (>2 active) and stop scheduling further (documented: at most 2 falls on screen).
- No fall for ids removed while the panel was hidden (avoid stale rects; skip).
- Tray "take everything down" removes all items in one snapshot → the same diff path plays up to 2 falls with 60 ms stagger; sound is already handled by Rust (first only).
- Prune (file gone) uses the same path (known rect) with the item path accessible from the previous snapshot.
- Reduced motion: no falls; cards disappear immediately.

- [ ] **Step 3: Runtime evidence**
Trigger a cross click via CDP on a card; record overlay frames (start/mid/end) and the removed state; tray all-down via CDP-invoked `take_down_all` (or the tray command path) showing 2 falls stagger; verify overlay hidden afterwards and no orphan items.
- [ ] **Step 4: Commit** `feat: P2 take-down fall animation`.

---

### Task P2-7: Thumbnail cache

**Files:**
- Create: `src-tauri/src/thumbnails.rs`
- Modify: `src-tauri/src/item.rs`, `src-tauri/src/app_state.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/watchers/{dirs,clipboard}.rs`, `src-tauri/src/capture.rs`, `src/lib/types.ts`, `src/components/GlassCard.tsx`, `src/windows/fly/Fly.tsx` (keep original for flight)
- Test: `src-tauri/src/thumbnails.rs` unit tests; TS type only.

**Interfaces:**
- `Item.thumb_path: Option<PathBuf>` serialized as `thumbPath`; TS `Item.thumbPath?: string | null`.
- `thumbnails::cache_dir() -> Result<PathBuf>` = `%LOCALAPPDATA%\Hangline\Thumbnails`; `THUMB_MAX_EDGE: u32 = 432`.
- `thumbnails::ensure_thumb(app: &AppHandle, item: &mut Item) -> Option<PathBuf>`: if source is a recording (mp4/mov) → None (first frame is P3); if cache file exists and is newer than source → Some; else decode source with `image::open`, `thumbnail()` to fit 432, write `<id>.png` atomically (`.tmp` + rename), return path. On any error log + None.
- `thumbnails::backfill(app)`: spawn a named thread; for each item missing `thumb_path` or whose cache is stale → ensure; update AppState item and emit state once (revision bump) when any changed.

- [ ] **Step 1: Tests**

```rust
#[test]
fn thumbnails_fit_the_long_edge_and_are_idempotent() {
    let dir = test_dir("thumbs");
    let src = dir.join("source.png");
    image::RgbaImage::from_pixel(4000, 3000, image::Rgba([10, 20, 30, 255])).save(&src).unwrap();
    let p1 = thumbnails::thumb_for(&src, &dir, "item1").unwrap();
    let img = image::open(&p1).unwrap();
    assert_eq!(img.width().max(img.height()), 432);
    let first = std::fs::metadata(&p1).unwrap().modified().unwrap();
    let p2 = thumbnails::thumb_for(&src, &dir, "item1").unwrap();
    assert_eq!(p1, p2);
    assert_eq!(first, std::fs::metadata(&p2).unwrap().modified().unwrap());
}
#[test]
fn corrupt_sources_return_none() {
    let dir = test_dir("thumbs-bad");
    let src = dir.join("bad.png");
    std::fs::write(&src, b"not a png").unwrap();
    assert!(thumbnails::thumb_for(&src, &dir, "bad").is_none());
}
```

- [ ] **Step 2: Wire**
- `Item::new` keeps `thumb_path: None`; deserialization default `None`.
- After every ingest add (`watchers` dir/clipboard save, `capture::finish_capture`): mutate the new item's `thumb_path = ensure_thumb(...)` **before** persisting/emitting, so the first state broadcast already has it; if generation fails leave None.
- Startup: after `AppState::load`, call `thumbnails::backfill(&app)`; the thread updates items and emits state.
- Frontend card image `src = convertFileSrc(item.thumbPath ?? item.path)`; keep `onError` fallback to original then tile; fly overlay always uses `item.path`.
- Thumbnail dir is under `$LOCALDATA/Hangline/**` (already in asset scope).
- `cargo test` includes the new tests; keep 0 warnings.

- [ ] **Step 3: Runtime evidence**
Ingest a 4K screenshot; verify `<id>.png` 432 px exists, state JSON shows `thumbPath`, panel loads thumb (CDP network request to asset URL), restart backfill works; commit `feat: P2 thumbnail cache for 4K cards`.

---

### Task P2-8: Light/dark material polish

**Files:**
- Modify: `src/windows/panel/Panel.css`, `src/styles/base.css`, `src/windows/capture/Capture.css`, `src/windows/shortcut/Shortcut.css`, `src/windows/fly/Fly.css`, `src/windows/panel/Panel.tsx` (theme attribute already exists), `src/windows/capture/Capture.tsx`, `src/windows/shortcut/Shortcut.tsx`
- Test: visual evidence + optional contrast unit test (pure luminance helper in `src/lib/color.ts` + test)

**Interfaces:**
- `src/lib/color.ts`: `relativeLuminance(hex)`, `contrastRatio(a,b)` (WCAG) used for the hint/pill text colours; test `#f3f7fa` on `rgba(14,19,25,.86)` composited over white/black gives ≥4.5.

- [ ] **Step 1:** Snapshot the reference (local, git-ignored) hero/demo light+dark; extract palette values with Python (dominant colours of the card frame, pill, hint, rope, background) and record them in `docs/experiments/P2-reference.md`.
- [ ] **Step 2:** Align CSS tokens: light theme glass fill/borders/shadows and dark theme values sampled from the reference; ensure the card border, photo inner stroke, pin gradient and pill remain legible on both backgrounds; `color-scheme` per theme; the empty hint/cross/play use the same glass material.
- [ ] **Step 3:** Make capture/shortcut/fly windows follow the theme: read `settings.theme` via `get_app_state`/`state` events (capture window can call `get_app_state`) and set `data-theme` from the same `system/light/dark` resolution (shared helper `resolveTheme(theme, prefersDark)` in `src/lib/theme.ts` + unit test).
- [ ] **Step 4:** Screenshots light+dark of panel (empty/cards/hint/pill), capture overlay, shortcut window; contrast check; commit `feat: P2 light/dark material polish`.

---

### Task P2-9: Performance, frame comparison and acceptance

**Files:**
- Create: `src/lib/perf.ts` (+ test), modify `src/windows/panel/Panel.tsx` to record rAF samples; `scripts/p2/perf.mjs`; `scripts/p2/compare.mjs`; `docs/experiments/P2-visual.md`; `docs/experiments/P2-perf.json`; `docs/acceptance/P2-core.md`; modify `README.md` / `README.zh-Hans.md` screenshots section; `.github/workflows/ci.yml` unchanged.
- Test: `perf.test.ts` for percentile/long-frame math.

**Interfaces:**
- `src/lib/perf.ts`: `createFrameRing(capacity=600)`, `pushFrame(ring, dtMs)`, `summarize(ring): { fpsMean, fpsP5, worstMs, longFrames }`, `LONG_FRAME_MS = 20`, exposed as `window.__HANG_FRAMES__` ring in the panel (small, always on).
- `scripts/p2/perf.mjs`: connects to CDP (port 9222), evaluates `Runtime.evaluate` on the panel target: `window.__HANG_FRAMES__.summarize()` after a warm-up and a 10 s idle window with 12 cards; also samples during a fly and a breeze; writes `docs/experiments/P2-perf.json` + prints PASS/FAIL against the DoD.
- `scripts/p2/compare.mjs`: triggers known animations through CDP (`reveal`, `copy`, `take-down`, `capture`), records `window.__HANG_TRACE__` (per-frame reveal offset, card rotation/scale, fly progress) plus screenshots at fixed timestamps; compares the samples to the reference expectations in `P2-reference.md` (reveal 420±80 ms, tuck 220±60 ms, fly 650±60 ms with arc 30 dp, fall 550±60 ms/520 dp, swing peak 2.2–3.6°) and writes `docs/experiments/P2-visual.md` tables; produces a committed contact sheet of **our own** frames (no Tendedero frames committed).

- [ ] **Steps:** implement perf/trace instrumentation + tests; run the harness on the dev app with 12 cards (4K originals) and record fps; if below target, optimize (thumbnail usage, remove backdrop-filter when necessary, transform-only) and re-measure; run frame comparison and record tables/limitations (Tendedero assets local-only, fair-use, not committed); write acceptance doc with P2 DoD statuses (`60 fps` PASS/FAIL, frame comparison PASS with method, manual feel checklist: arrival swing, breeze, hover/press, fly from capture, fall, light/dark); update README screenshots (light/dark, app-UI-only); gates green; commit `docs: P2 visual acceptance and performance evidence`. Do **not** create the tag — the Lead tags `v0.2.0-visual` after the final whole-branch review.

---

## Manual acceptance checklist (for `docs/acceptance/P2-core.md` §manual, user-run)

1. Cold start with 3+ screenshots: cards drop in and swing once (`16° → 0`), line reveal ~0.42 s.
2. Wait 7–16 s: the line gets a small breeze (1.6–3.4°), cards settle.
3. **Real mouse (not CDP)** hover a card: hover shadow + ~3.5 % growth with soft overshoot; click-copy works (Rust hittest/click-through gate); press and hold shrinks to 0.95 over ~0.45 s, release springs back. Repeat while the card is swaying (hit target must not flicker).
4. Copy a card: small 3° nudge + glass `Copied` pill.
5. Capture a region with `Ctrl+Alt+S`: the shot flies from the selected area to its card (0.65 s, gentle arc), lands with a small sway.
6. Clipboard/system screenshot (no capture rect): flies in from the screen centre.
7. Take a card down (×): it falls 520 dp over 0.55 s, tilting and fading; file goes to the Recycle Bin only for own-dir files.
8. Tray → Take everything down: up to two cards fall with a 60 ms stagger, the rest disappear.
9. Light/dark: switch Windows theme or tray Language/theme setting; glass, rope, pin, pill and hint stay legible.
10. Reduced motion (Windows Settings → Accessibility → Visual effects → Animation effects off): no swap/fall/breeze; states snap.

## Self-review checklist (controller)

- **Spec coverage:** P2 DoD = glass cards/pins/rope, sway, fly-in, fall, light/dark, frame comparison, 60 fps; tasks P2-1..P2-9 cover every item (tokens, physics, visuals, choreography, fly, fall, thumbnails, theme polish, perf/frame acceptance).
- **No placeholders:** every task names files, exact constants, commands, tests and commits; no TBD/TODO; no "similar to Task N".
- **Type consistency:** `PhysicalRect`, `FlyAnimation`, `CardMotion*`, `photoSize/cardVisualSize/pinTop/cardTopFromRope/edgeFadeOpacity`, `Item.thumbPath`, `start_fly/start_fall/fly_done/fly_window_ready/take_capture_origin` are used consistently across tasks.
- **Risk:** fly overlay window WebView2 cold start is avoided by pre-warming; if the overlay is unavailable, the panel falls back to drop-in (never blocks ingest). Reduced motion/time budgets are explicit.
