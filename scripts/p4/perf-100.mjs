#!/usr/bin/env node
/* global Buffer */
/**
 * P4-1/P4-5 end-to-end evidence: retention cap + 100-card panel performance (M14).
 *
 * Prerequisites (see the P4 runbook in docs/experiments/P4-multimon-settings.md):
 *
 *   node scripts/p4/fixture-100.mjs                       # isolated %TEMP%\hangline-p4-100 profile
 *   pnpm dev                                              # Vite on 5173 (debug build loads devUrl)
 *   cargo build --manifest-path src-tauri/Cargo.toml
 *   $env:LOCALAPPDATA = "$env:TEMP\hangline-p4-100\localappdata"
 *   $env:HANG_CDP_PORT = "9227"
 *   Start-Process src-tauri\target\debug\hangline.exe
 *   node scripts/p4/perf-100.mjs
 *
 * What it measures:
 * 1. state has exactly 100 items, `keepOnLine: 100`, `cardSize: medium`;
 * 2. the panel is shown pinned (second-instance toggle = the tray/hotkey reveal path), then a 5 s
 *    trusted CDP wheel sweep (`Input.dispatchMouseEvent` type `mouseWheel`, alternating deltaY /
 *    deltaX) runs while the panel's own rAF ring (`src/lib/perf.ts`) samples frame durations and the
 *    panel's dev-only offset trace (`window.__HANG_OFFSETS__`) samples `data-offset` once per frame;
 * 3. the sweep must show **actual movement**: the mid-window offset has to be above the start
 *    offset, every quarter of the window must contain real offset changes, the trace must cover at
 *    least 4 s and hold at least 20 real moves (a static line whose wheel path silently stopped
 *    fails here; `data-max-offset` is reported as capacity only);
 * 4. the newest card is back on the right after scrolling to the newest end;
 * 5. `set_setting {key:"keepOnLine", value:25}` through the real IPC trims the state to the newest
 *    25, bumps the revision once, persists state.json/settings.json, and the 75 dropped files are
 *    still on disk (retention never deletes files);
 * 6. a short post-trim sweep still scrolls.
 *
 * No dependencies. Raw screenshots + JSON land in `.superpowers/p4-evidence/` (git-ignored); the
 * panel is transparent, so captures use a neutral solid backdrop override (app UI only, never the
 * user's desktop).
 */

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { connectTo, evaluate, sleep } from "../p2/cdp.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const base = resolve(process.env.HANG_P4_BASE ?? join(tmpdir(), "hangline-p4-100"));
const fixturePath = join(base, "fixture-100.json");
const exe = join(repoRoot, "src-tauri", "target", "debug", "hangline.exe");
const evidenceDir = resolve(
  repoRoot,
  process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p4-evidence"),
);
const SWEEP_MS = 5000;
const WARMUP_MS = 5000;
const WHEEL_INTERVAL_MS = 35;
const DIRECTION_BLOCK = 40;

const checks = {};
const steps = [];
let failures = 0;

function check(name, condition, detail) {
  checks[name] = { pass: Boolean(condition), detail };
  if (!condition) failures += 1;
  console.log(
    `${condition ? "PASS" : "FAIL"} ${name}${detail === undefined ? "" : `: ${JSON.stringify(detail)}`}`,
  );
}

function step(name, detail) {
  steps.push({ name, at: new Date().toISOString(), detail });
  console.log(`step: ${name}`);
}

function fail(message) {
  console.error(`perf-100: ${message}`);
  process.exit(2);
}

if (!existsSync(fixturePath)) {
  fail(`missing ${fixturePath}; run "node scripts/p4/fixture-100.mjs" first`);
}
if (!existsSync(exe)) {
  fail(`missing debug binary ${exe}; run "cargo build --manifest-path src-tauri/Cargo.toml"`);
}
const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));
const profile = fixture.profile;
const localAppData = fixture.localAppData;
const screenshotsDir = join(profile, "Screenshots");
mkdirSync(evidenceDir, { recursive: true });

const panel = await connectTo("window=panel");
step("panel connected", { cdpPort: process.env.HANG_CDP_PORT ?? "9222" });

const invoke = (command, args = {}) =>
  evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
const getState = async () =>
  JSON.parse(
    await evaluate(
      panel,
      `(async () => JSON.stringify(await window.__TAURI_INTERNALS__.invoke("get_app_state")))()`,
    ),
  );

// The panel installs the frame ring in a mount effect; the Vite dev page can take a moment.
let framesReady = false;
for (let attempt = 0; attempt < 200; attempt += 1) {
  framesReady = await evaluate(panel, "!!window.__HANG_FRAMES__").catch(() => false);
  if (framesReady) break;
  await sleep(100);
}
check("panel frame ring installed", framesReady);
if (!framesReady) fail("window.__HANG_FRAMES__ never appeared");

// P4-5: the movement assertion needs the panel's own offset trace. A debug binary older than this
// harness would silently fall back to the old static-capacity gate, so fail loudly instead.
let offsetsReady = false;
for (let attempt = 0; attempt < 100; attempt += 1) {
  offsetsReady = await evaluate(panel, "!!window.__HANG_OFFSETS__").catch(() => false);
  if (offsetsReady) break;
  await sleep(100);
}
check("panel offset trace installed", offsetsReady);
if (!offsetsReady) fail("window.__HANG_OFFSETS__ never appeared (rebuild the debug app / Vite dev)");

/** One DOM/metrics snapshot of the panel state that the evidence assertions use. */
async function panelMetrics() {
  return JSON.parse(
    await evaluate(
      panel,
      `(() => {
        const panel = document.querySelector(".panel");
        const slots = [...document.querySelectorAll(".glass-card-slot")];
        const last = slots[slots.length - 1];
        const rect = last?.getBoundingClientRect();
        return JSON.stringify({
          innerWidth: window.innerWidth,
          innerHeight: window.innerHeight,
          dpr: window.devicePixelRatio,
          revealed: panel?.dataset.revealed,
          visibility: document.visibilityState,
          offset: Number(panel?.dataset.offset ?? -1),
          maxOffset: Number(panel?.dataset.maxOffset ?? -1),
          rangeStart: Number(panel?.dataset.rangeStart ?? -1),
          rangeEnd: Number(panel?.dataset.rangeEnd ?? -1),
          cardCount: Number(panel?.dataset.cardCount ?? -1),
          keep: panel?.dataset.keep,
          capacity: Number(panel?.dataset.capacity ?? -1),
          lastCardId: last?.dataset.itemId ?? null,
          lastCardLeft: rect ? rect.left : null,
          lastCardRight: rect ? rect.right : null,
        });
      })()`,
    ),
  );
}

async function panelShown() {
  return evaluate(
    panel,
    `(() => {
      const panel = document.querySelector(".panel");
      return panel?.dataset.revealed === "true" && document.visibilityState === "visible";
    })()`,
  ).catch(() => false);
}

/** One second-instance launch: the same explicit toggle as the tray/hotkey (reveal or hide). */
async function toggleViaSecondInstance() {
  const before = await panelShown();
  const child = spawn(exe, [], {
    detached: true,
    stdio: "ignore",
    env: { ...process.env, LOCALAPPDATA: localAppData },
  });
  child.unref();
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    await sleep(150);
    const shown = await panelShown();
    if (shown !== before) return shown;
  }
  return panelShown();
}

/**
 * Leaves the panel in the explicit (pinned) reveal state.
 *
 * The startup reveal is a non-pinned show; a second-instance toggle on a shown line hides it and
 * the next toggle reveals it pinned, which keeps the line on screen for the whole measurement even
 * though the CDP wheel events never move the real OS pointer.
 */
let pinnedOnce = false;
async function ensurePinnedReveal() {
  for (let attempt = 0; attempt < 3 && (await panelShown()); attempt += 1) {
    await toggleViaSecondInstance();
  }
  if (!(await panelShown())) {
    await toggleViaSecondInstance();
  }
  if (!(await panelShown())) return false;
  pinnedOnce = true;
  // Let the reveal spring settle (spec: 0.42 s) before the measurement.
  await sleep(700);
  return panelShown();
}

/**
 * Re-reveals after an environmental tuck (a real desktop click outside the panel is a dismiss).
 *
 * A toggle on a hidden line is always an explicit pinned reveal, so one spawn is enough.
 */
async function ensureRevealed() {
  if (await panelShown()) return true;
  await toggleViaSecondInstance();
  const shown = await panelShown();
  if (shown) pinnedOnce = true;
  return shown;
}

// --- 1. fixture state ---------------------------------------------------------------------
let state = await getState();
for (let attempt = 0; attempt < 120 && state.items.length !== 100; attempt += 1) {
  await sleep(250);
  state = await getState();
}
check("fixture state has 100 items", state.items.length === 100, state.items.length);
check("keepOnLine is 100", state.settings.keepOnLine === 100, state.settings.keepOnLine);
check("cardSize is medium", state.settings.cardSize === "medium", state.settings.cardSize);
check(
  "all watchers are off",
  state.settings.watchSystemScreenshots === false &&
    state.settings.watchClipboard === false &&
    state.settings.watchRecordings === false,
);
check(
  "state ids match the fixture in ascending order",
  JSON.stringify(state.items.map((item) => item.id)) === JSON.stringify(fixture.ids),
);
check(
  "every item path lives in the isolated profile",
  state.items.every((item) => item.path.startsWith(`${screenshotsDir}\\`)),
);

// The startup thumbnail backfill decodes 100 sources on a background thread; measuring during it
// would blame the backfill for the scroll jank. Wait until every item has its preview and the
// revision has been quiet for a moment.
step("waiting for the startup thumbnail backfill");
const idleDeadline = Date.now() + 30000;
let idle = { thumbnails: 0, revision: state.revision, settled: false };
while (Date.now() < idleDeadline) {
  const current = await getState();
  const thumbnails = current.items.filter((item) => item.thumbPath).length;
  if (thumbnails === current.items.length) {
    await sleep(1200);
    const quiet = await getState();
    if (quiet.revision === current.revision) {
      idle = { thumbnails, revision: quiet.revision, settled: true };
      break;
    }
  }
  await sleep(200);
}
check("startup thumbnail backfill settled", idle.settled, idle);

const pinned = await ensurePinnedReveal();
check("panel is shown pinned for the measurement", pinned);
if (!pinned) fail("could not reveal the panel through the second-instance toggle");
await sleep(300);

let metrics = await panelMetrics();
check("panel reports the old 100-card window", metrics.keep === "100", metrics.keep);
check(
  "the panel starts at the newest end",
  metrics.offset === 0 && metrics.rangeEnd === 100,
  { offset: metrics.offset, rangeEnd: metrics.rangeEnd, capacity: metrics.capacity },
);
check(
  "the newest DOM card is the fixture's newest item",
  metrics.lastCardId === fixture.ids[99],
  { dom: metrics.lastCardId, newest: fixture.ids[99] },
);

// A shared dev machine (antivirus, IDE, CI) would otherwise be measured as panel jank. Wait for a
// quiet idle baseline before the gesture; the gate only waits and never re-rolls a scroll window.
step("waiting for a quiet idle baseline");
let quietBaseline = null;
const quietDeadline = Date.now() + 45000;
while (Date.now() < quietDeadline) {
  if (!(await panelShown())) await ensureRevealed();
  await evaluate(panel, "(() => { window.__HANG_FRAMES__.reset(); return true; })()");
  await sleep(2000);
  quietBaseline = JSON.parse(
    await evaluate(panel, "JSON.stringify(window.__HANG_FRAMES__.summarize())"),
  );
  if (
    quietBaseline.samples >= 100 &&
    quietBaseline.fpsMean >= 59 &&
    quietBaseline.longFrames === 0
  ) {
    break;
  }
  console.log(
    `perf-100: idle baseline still noisy (samples=${quietBaseline.samples}, ` +
      `fps=${quietBaseline.fpsMean.toFixed(2)}, longFrames=${quietBaseline.longFrames}); waiting`,
  );
  await sleep(1000);
}
check(
  "quiet idle baseline before the sweep",
  quietBaseline !== null && quietBaseline.fpsMean >= 59,
  quietBaseline,
);

// --- 2. 5 s trusted wheel sweep + frame sampling -------------------------------------------
const wheelX = Math.round(metrics.innerWidth / 2);
const wheelY = Math.round(metrics.innerHeight / 2);
const wheel = (deltaX, deltaY) =>
  panel.send("Input.dispatchMouseEvent", {
    type: "mouseWheel",
    x: wheelX,
    y: wheelY,
    deltaX,
    deltaY,
  });

/**
 * Dispatches trusted wheel events for `durationMs` without awaiting each one.
 *
 * Awaiting every dispatch makes the CDP round-trip (~50-90 ms on this machine) the sample rate
 * instead of the 35 ms gesture; queueing them keeps ~28 events/s while the panel's own rAF ring
 * measures the real frame cost. All replies are collected at the end so input errors still surface.
 */
async function sweepWheel(durationMs, { block = DIRECTION_BLOCK, intervalMs = WHEEL_INTERVAL_MS } = {}) {
  const pending = [];
  let events = 0;
  let direction = 1;
  const started = Date.now();
  while (Date.now() - started < durationMs) {
    // +deltaY walks towards older cards, -deltaX walks back towards the newest; the axis alternates
    // so both the vertical and horizontal wheel paths are exercised (spec §3.1 dominant axis).
    if (direction > 0) pending.push(wheel(0, 120));
    else pending.push(wheel(-120, 0));
    events += 1;
    if (events % block === 0) direction = -direction;
    await sleep(intervalMs);
  }
  await Promise.all(pending);
  return { events, durationMs: Date.now() - started };
}

/** Wheels back to the newest end and waits for the DOM offset to settle at 0. */
async function scrollToNewest(steps = 130) {
  const pending = [];
  for (let index = 0; index < steps; index += 1) {
    pending.push(wheel(-120, 0));
    await sleep(12);
  }
  await Promise.all(pending);
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    if ((await panelMetrics()).offset === 0) return true;
    await sleep(120);
  }
  return false;
}

// Warmup pass: walk once from the newest end to the oldest kept card (the full 91-step scroll
// window) so the first image decodes and the first new-card layout are not charged to the measured
// gesture; the 100 cards are pre-loaded when the 5 s measurement starts.
//
// The measurement is retried only when the *environment* invalidates it (a real desktop click
// outside the panel dismisses the line mid-run: this dev machine is shared with other evidence
// runs). Frames from a window in which the line stayed shown are never re-rolled.
let warmed = false;
let warmup = { events: 0, durationMs: 0 };
let warmupBack = false;
let longTaskObserver = false;
let measured = null;
let sweep = { events: 0, durationMs: 0 };
let sweepMetrics = null;
let sweepStart = null;
let movement = null;
let frames = null;
let longTasks = [];
let measurementAttempts = 0;
const MAX_MEASURE_ATTEMPTS = 3;

while (measurementAttempts < MAX_MEASURE_ATTEMPTS) {
  measurementAttempts += 1;
  const shown = pinnedOnce ? await ensureRevealed() : await ensurePinnedReveal();
  if (!shown) {
    await sleep(600);
    continue;
  }

  if (!warmed) {
    step("warmup wheel pass over the whole line");
    warmup = await sweepWheel(WARMUP_MS, { block: 1000 });
    warmupBack = await scrollToNewest();
    if (!(await panelShown())) {
      await sleep(500);
      continue;
    }
    warmed = true;
    await sleep(400);
  }

  // Long tasks (>50 ms) inside the measured window; reported, not used as a pass/fail gate alone.
  longTaskObserver = await evaluate(
    panel,
    `(() => {
      window.__HANG_LONG_TASKS__ = [];
      try {
        window.__HANG_LONG_TASK_OBSERVER__?.disconnect();
        const observer = new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) {
            window.__HANG_LONG_TASKS__.push({
              startTime: entry.startTime,
              duration: entry.duration,
              name: entry.name,
            });
          }
        });
        observer.observe({ type: "longtask", buffered: false });
        window.__HANG_LONG_TASK_OBSERVER__ = observer;
        return true;
      } catch (error) {
        return false;
      }
    })()`,
  );

  // Track that the line stayed revealed for the whole window (a mid-run tuck would shorten it).
  await evaluate(
    panel,
    `(() => {
      clearInterval(window.__HANG_PERF_MON_ID__);
      window.__HANG_PERF_MON__ = { samples: 0, hidden: false };
      window.__HANG_PERF_MON_ID__ = setInterval(() => {
        const panel = document.querySelector(".panel");
        window.__HANG_PERF_MON__.samples += 1;
        if (panel?.dataset.revealed !== "true" || document.hidden) {
          window.__HANG_PERF_MON__.hidden = true;
        }
      }, 50);
      return true;
    })()`,
  );

  step(`starting the 5 s trusted wheel sweep (attempt ${measurementAttempts})`, {
    wheelX,
    wheelY,
  });
  const sweepStartMetrics = await panelMetrics();
  sweepStart = sweepStartMetrics;
  // The frame ring and the line-offset trace start in the same page turn, so both cover exactly
  // the measured sweep. `start` records the offset the panel currently renders as sample 0.
  const t0 = await evaluate(
    panel,
    `(() => {
      window.__HANG_FRAMES__.reset();
      window.__HANG_OFFSETS__.start(Number(document.querySelector(".panel")?.dataset.offset ?? 0));
      return performance.now();
    })()`,
  );
  sweep = await sweepWheel(SWEEP_MS);
  measured = JSON.parse(
    await evaluate(
      panel,
      `(() => {
        clearInterval(window.__HANG_PERF_MON_ID__);
        return JSON.stringify({
          summary: window.__HANG_FRAMES__.summarize(),
          movement: window.__HANG_OFFSETS__.stop(),
          monitor: window.__HANG_PERF_MON__,
          t: performance.now(),
          longTasks: window.__HANG_LONG_TASKS__ ?? [],
        });
      })()`,
    ),
  );
  sweepMetrics = await panelMetrics();
  frames = measured.summary;
  movement = measured.movement;
  longTasks = measured.longTasks.filter(
    (entry) => entry.startTime >= t0 && entry.startTime <= measured.t,
  );

  if (measured.monitor.hidden) {
    console.log(
      `perf-100: attempt ${measurementAttempts} invalidated by an external tuck/dismiss; ` +
        "re-revealing and measuring again",
    );
    await sleep(700);
    continue;
  }
  break;
}

check("warmup sweep reaches the oldest end", warmup.events >= 92, warmup);
check("warmup sweep returns to the newest end", warmupBack);
check("long-task observer available", longTaskObserver);
check("measurement window stayed shown", measured !== null && measured.monitor.hidden === false, {
  attempts: measurementAttempts,
  monitor: measured?.monitor ?? null,
});

check("sweep dispatched trusted wheel events", sweep.events >= 80, {
  events: sweep.events,
  durationMs: sweep.durationMs,
});
check("frame samples >= 200", frames !== null && frames.samples >= 200, frames?.samples ?? 0);
check(
  "mean fps >= 58",
  frames !== null && frames.fpsMean >= 58,
  Math.round((frames?.fpsMean ?? 0) * 100) / 100,
);
check(
  "worst frame < 50 ms",
  frames !== null && frames.worstMs < 50,
  Math.round((frames?.worstMs ?? 0) * 100) / 100,
);
check(
  "no long-frame burst longer than 3 frames",
  frames !== null && frames.longestBurst <= 3,
  {
    longestBurst: frames?.longestBurst ?? -1,
    longFrames: frames?.longFrames ?? -1,
    longFrameMs: 20,
  },
);
// P4-5 movement assertion: the measured window has to show real offset changes, not just static
// capacity (`data-max-offset`). The review of P4-1 found the old gate `maxOffset > 0` would PASS on
// a build whose wheel path silently stopped; these three checks are the actual gate, and
// `maxOffset` is only recorded as capacity information in the evidence below.
const movementDetail = movement
  ? {
      startOffset: movement.startOffset,
      midOffset: movement.midOffset,
      endOffset: movement.endOffset,
      peakOffset: movement.peakOffset,
      moves: movement.moves,
      firstHalfMoves: movement.firstHalfMoves,
      secondHalfMoves: movement.secondHalfMoves,
      quarterMoves: movement.quarterMoves,
      samples: movement.samples,
      windowMs: Math.round(movement.windowMs),
    }
  : null;
check(
  "the sweep moved the line (mid-window offset above the start)",
  movement !== null && movement.movedMidWindow,
  movementDetail,
);
check(
  "the line kept moving through every quarter of the window",
  movement !== null && movement.movedThroughout,
  movement ? { quarterMoves: movement.quarterMoves, moves: movement.moves } : null,
);
check(
  "the offset trace recorded at least 20 real moves",
  movement !== null && movement.moves >= 20,
  movement?.moves ?? 0,
);
// The movement numbers only mean something over the measured window: a short/frozen trace must not
// pass the quarter gate on a few early samples.
check(
  "the offset trace covers at least 4 s of the measured window",
  movement !== null && movement.windowMs >= 4000,
  { windowMs: Math.round(movement?.windowMs ?? 0), minWindowMs: 4000 },
);
// Capacity only: recorded for the report, deliberately not a pass/fail gate any more.
const sweepCapacity = {
  maxOffset: sweepMetrics?.maxOffset ?? -1,
  panelCapacity: sweepMetrics?.capacity ?? -1,
  startOffset: sweepStart?.offset ?? -1,
  note: "capacity information only; movement is asserted from the panel's offset trace",
};


// --- 3. app-UI screenshot at the mid-scroll position --------------------------------------
async function capturePanel(name) {
  await panel.send("Emulation.setDefaultBackgroundColorOverride", {
    color: { r: 26, g: 30, b: 35, a: 1 },
  });
  const captured = await panel.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  writeFileSync(join(evidenceDir, name), Buffer.from(captured.data, "base64"));
  await panel.send("Emulation.setDefaultBackgroundColorOverride", {}).catch(() => undefined);
  return name;
}
const midScrollShot = await capturePanel("p4-100-cards-mid-scroll.png");

// --- 4. scroll back to the newest end and verify "newest on the right" -------------------
step("scrolling back to the newest card");
const returnedToNewest = await scrollToNewest();
check("scroll-back reaches the newest end", returnedToNewest);
await sleep(350);
const afterSweep = await getState();
metrics = await panelMetrics();
const newestMetrics = metrics;
const newest = afterSweep.items[afterSweep.items.length - 1]?.id;
check("state is still 100 after the sweep", afterSweep.items.length === 100, afterSweep.items.length);
check("scroll-offset is back at the newest end", metrics.offset === 0, metrics.offset);
check("visible window ends at the newest item", metrics.rangeEnd === 100, metrics.rangeEnd);
check("newest DOM card is on the right", metrics.lastCardId === newest, {
  dom: metrics.lastCardId,
  newest,
});
check(
  "newest card is fully inside the panel's right side",
  metrics.lastCardRight !== null &&
    metrics.lastCardRight <= metrics.innerWidth + 1 &&
    metrics.lastCardRight > metrics.innerWidth * 0.6,
  {
    lastCardLeft: Math.round(metrics.lastCardLeft ?? -1),
    lastCardRight: Math.round(metrics.lastCardRight ?? -1),
    panelWidth: metrics.innerWidth,
  },
);

// --- 5. reduce keepOnLine to 25 through the real IPC --------------------------------------
step("set_setting {key:keepOnLine, value:25}");
const beforeTrim = await getState();
const revisionBefore = beforeTrim.revision;
const settingsReply = await invoke("set_setting", { key: "keepOnLine", value: 25 });
const afterTrim = await getState();
const keptIds = afterTrim.items.map((item) => item.id);
const droppedIds = beforeTrim.items
  .slice(0, beforeTrim.items.length - afterTrim.items.length)
  .map((item) => item.id);

check("IPC reply has 25 items", settingsReply.items.length === 25, settingsReply.items.length);
check("state has exactly 25 items", afterTrim.items.length === 25, afterTrim.items.length);
check("keepOnLine is 25", afterTrim.settings.keepOnLine === 25, afterTrim.settings.keepOnLine);
check(
  "revision increased exactly once",
  afterTrim.revision === revisionBefore + 1,
  { before: revisionBefore, after: afterTrim.revision },
);
check(
  "the kept ids are exactly the newest 25 fixture items",
  JSON.stringify(keptIds) === JSON.stringify(fixture.ids.slice(75)),
);
check(
  "kept ids and paths are unchanged",
  afterTrim.items.every((item, index) => item.path === fixture.paths[75 + index]),
);
check(
  "dropped ids are exactly the oldest 75",
  JSON.stringify(droppedIds) === JSON.stringify(fixture.ids.slice(0, 75)),
);

const droppedFilesStillExist = fixture.paths
  .slice(0, 75)
  .filter((path) => existsSync(path)).length;
check(
  "retention never deletes the dropped cards' files",
  droppedFilesStillExist === 75,
  { checked: 75, existing: droppedFilesStillExist },
);
check(
  "kept cards' files still exist",
  afterTrim.items.every((item) => existsSync(item.path)),
);

const persistedItems = JSON.parse(readFileSync(join(profile, "state.json"), "utf8"));
const persistedSettings = JSON.parse(readFileSync(join(profile, "settings.json"), "utf8"));
check("state.json was persisted with 25 items", persistedItems.length === 25, persistedItems.length);
check(
  "state.json matches the live snapshot",
  JSON.stringify(persistedItems.map((item) => item.id)) === JSON.stringify(keptIds),
);
check("settings.json kept keepOnLine=25", persistedSettings.keepOnLine === 25, persistedSettings.keepOnLine);

metrics = await panelMetrics();
check("panel shows the 25-card window", metrics.keep === "25", metrics.keep);
check(
  "panel view reset to the newest end after the trim",
  metrics.offset === 0 && metrics.rangeEnd === 25,
  { offset: metrics.offset, rangeEnd: metrics.rangeEnd, capacity: metrics.capacity },
);
check("newest card is still on the right after the trim", metrics.lastCardId === newest);

// --- 6. optional post-trim scroll (the shorter line still works) -------------------------
step("post-trim 2 s wheel sweep");
await evaluate(panel, "(() => { window.__HANG_FRAMES__.reset(); return true; })()");
const postSweep = await sweepWheel(2000, { block: 20 });
const postFrames = JSON.parse(
  await evaluate(panel, "JSON.stringify(window.__HANG_FRAMES__.summarize())"),
);
const postSweepMetrics = await panelMetrics();
check("post-trim sweep moved the line", postSweepMetrics.offset > 0, {
  offset: postSweepMetrics.offset,
  maxOffset: postSweepMetrics.maxOffset,
});
const postTrimBack = await scrollToNewest();
check("post-trim scroll-back reaches the newest end", postTrimBack);
await sleep(300);
const postTrim = await panelMetrics();
check(
  "post-trim line scrolls back to the newest end",
  postTrim.offset === 0 && postTrim.rangeEnd === 25,
  { offset: postTrim.offset, rangeEnd: postTrim.rangeEnd },
);
const afterTrimShot = await capturePanel("p4-25-cards-newest-right.png");

// --- 7. evidence JSON ---------------------------------------------------------------------
const summary = {
  generatedAt: new Date().toISOString(),
  fixtureVersion: fixture.fixtureVersion,
  method:
    "debug build + Vite dev (5173) + isolated LOCALAPPDATA; CDP trusted mouseWheel sweep; " +
    "panel rAF frame ring + per-frame line-offset trace (src/lib/perf.ts, dev-only instrumentation); " +
    "screenshots are panel-only with a neutral backdrop",
  environment: {
    cdpPort: process.env.HANG_CDP_PORT ?? "9222",
    exe,
    profile,
    panel: {
      width: metrics.innerWidth,
      height: metrics.innerHeight,
      dpr: metrics.dpr,
      capacity: metrics.capacity,
      cardSize: "medium",
    },
  },
  stateBefore: {
    items: beforeTrim.items.length,
    keepOnLine: beforeTrim.settings.keepOnLine,
    revision: beforeTrim.revision,
  },
  idleBaseline: quietBaseline,
  warmup: {
    durationMs: WARMUP_MS,
    events: warmup.events,
    returnedToNewest: warmupBack,
    measurementAttempts,
  },
  sweep: {
    durationMs: sweep.durationMs,
    events: sweep.events,
    intervalMs: WHEEL_INTERVAL_MS,
    directionBlock: DIRECTION_BLOCK,
    summary: frames,
    movement,
    capacity: sweepCapacity,
    monitor: measured?.monitor ?? null,
    longTasks,
    limits: {
      minSamples: 200,
      meanFps: 58,
      worstMs: 50,
      longestBurst: 3,
      movedMidWindow: true,
      movedThroughout: true,
      minMoves: 20,
      minWindowMs: 4000,
    },
  },
  newestOnRight: {
    stateItems: afterSweep.items.length,
    offset: newestMetrics.offset,
    maxOffset: newestMetrics.maxOffset,
    rangeStart: newestMetrics.rangeStart,
    rangeEnd: newestMetrics.rangeEnd,
    domLastCardId: newestMetrics.lastCardId,
    newestId: newest,
    lastCardRight: Math.round(newestMetrics.lastCardRight ?? -1),
    panelWidth: newestMetrics.innerWidth,
  },
  trimTo25: {
    revisionBefore,
    revisionAfter: afterTrim.revision,
    replyItems: settingsReply.items.length,
    stateItems: afterTrim.items.length,
    droppedCount: droppedIds.length,
    droppedFilesStillExist,
    keptFilesExist: afterTrim.items.every((item) => existsSync(item.path)),
    persistedItems: persistedItems.length,
    persistedKeepOnLine: persistedSettings.keepOnLine,
    panelKeep: metrics.keep,
  },
  postTrimSweep: {
    durationMs: 2000,
    events: postSweep.events,
    summary: postFrames,
    finalOffset: postTrim.offset,
    finalRangeEnd: postTrim.rangeEnd,
  },
  screenshots: [midScrollShot, afterTrimShot],
  steps,
  checks,
  verdict: {
    state100: checks["fixture state has 100 items"].pass ? "PASS" : "FAIL",
    samples: checks["frame samples >= 200"].pass ? "PASS" : "FAIL",
    meanFps: checks["mean fps >= 58"].pass ? "PASS" : "FAIL",
    worstFrame: checks["worst frame < 50 ms"].pass ? "PASS" : "FAIL",
    longFrameBurst: checks["no long-frame burst longer than 3 frames"].pass ? "PASS" : "FAIL",
    sweepMoved: checks["the sweep moved the line (mid-window offset above the start)"].pass
      ? "PASS"
      : "FAIL",
    sweepMovedThroughout: checks["the line kept moving through every quarter of the window"].pass
      ? "PASS"
      : "FAIL",
    newestOnRight: checks["newest DOM card is on the right"].pass ? "PASS" : "FAIL",
    trimTo25: checks["state has exactly 25 items"].pass ? "PASS" : "FAIL",
    filesRemain: checks["retention never deletes the dropped cards' files"].pass ? "PASS" : "FAIL",
    revisionOnce: checks["revision increased exactly once"].pass ? "PASS" : "FAIL",
  },
  failures,
};
const output = join(evidenceDir, "perf-100.json");
writeFileSync(output, `${JSON.stringify(summary, null, 2)}\n`);

console.log("\n--- M14 numbers ---");
console.log(
  `sweep: samples=${frames?.samples ?? 0} meanFps=${(frames?.fpsMean ?? 0).toFixed(2)} ` +
    `p5Fps=${(frames?.fpsP5 ?? 0).toFixed(2)} worstMs=${(frames?.worstMs ?? 0).toFixed(2)} ` +
    `longFrames=${frames?.longFrames ?? 0} longestBurst=${frames?.longestBurst ?? 0} ` +
    `events=${sweep.events} attempts=${measurementAttempts}`,
);
console.log(`long tasks in window: ${longTasks.length}`);
console.log(
  `movement: start=${movement?.startOffset ?? -1} mid=${movement?.midOffset ?? -1} ` +
    `end=${movement?.endOffset ?? -1} peak=${movement?.peakOffset ?? -1} ` +
    `moves=${movement?.moves ?? -1} quarters=[${(movement?.quarterMoves ?? []).join(",")}] ` +
    `movedMidWindow=${movement?.movedMidWindow ?? false} ` +
    `movedThroughout=${movement?.movedThroughout ?? false} ` +
    `(capacity maxOffset=${sweepCapacity.maxOffset}, reported only)`,
);
console.log(
  `state: ${beforeTrim.items.length} -> ${afterTrim.items.length} (keepOnLine 25), ` +
    `revision ${revisionBefore} -> ${afterTrim.revision}, dropped files exist ` +
    `${droppedFilesStillExist}/75`,
);
console.log(`evidence: ${output}`);
console.log(`verdict: ${failures === 0 ? "PASS" : `FAIL (${failures})`}`);
process.exit(failures === 0 ? 0 : 1);
