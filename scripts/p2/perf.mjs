import { writeFileSync } from "node:fs";
import { connectTo, evaluate, sleep } from "./cdp.mjs";

const WARMUP_MS = 2000;
const IDLE_MS = 10000;
const MIN_SAMPLES = 590;
const REQUIRED_ITEMS = 12;

const panel = await connectTo("window=panel");
let framesReady = false;
for (let attempt = 0; attempt < 160; attempt += 1) {
  framesReady = await evaluate(panel, "!!window.__HANG_FRAMES__").catch(() => false);
  if (framesReady) break;
  if (attempt === 25) {
    // First load can race the Vite optimizer; reload once and keep waiting.
    await evaluate(panel, "location.reload()").catch(() => {});
    await sleep(1500);
  }
  await sleep(100);
}
if (!framesReady) {
  console.error("perf: panel instrumentation did not become ready; see docs/experiments/P2-visual.md runbook");
  process.exit(1);
}

// Wait for the documented 12-item fixture instead of measuring a partial window.
let stateBefore = null;
for (let attempt = 0; attempt < 120; attempt += 1) {
  stateBefore = JSON.parse(await evaluate(panel, `(async () => {
    const s = await window.__TAURI_INTERNALS__.invoke("get_app_state");
    return JSON.stringify({ items: s.items.length, theme: s.settings.theme, cardSize: s.settings.cardSize, keep: s.settings.keepOnLine });
  })()`));
  if (stateBefore.items >= REQUIRED_ITEMS) break;
  await sleep(400);
}
const visibleCards = await evaluate(panel, "document.querySelectorAll('.glass-card').length");
if (stateBefore.items < REQUIRED_ITEMS) {
  console.error(`perf: only ${stateBefore.items} items after waiting; need ${REQUIRED_ITEMS} (see docs/experiments/P2-visual.md runbook)`);
  process.exit(1);
}

// Record whether the panel stayed revealed for the whole measurement window: a mid-run tuck would
// otherwise produce a shorter, misleadingly perfect window.
await evaluate(panel, `(() => {
  window.__HANG_PERF_MON__ = { revealed: true, samples: 0, hidden: false };
  window.__HANG_PERF_MON_ID__ = setInterval(() => {
    const panel = document.querySelector(".panel");
    window.__HANG_PERF_MON__.samples += 1;
    if (panel?.dataset.revealed !== "true" || document.hidden) {
      window.__HANG_PERF_MON__.revealed = false;
      window.__HANG_PERF_MON__.hidden = true;
    }
  }, 50);
  return true;
})()`);
await sleep(WARMUP_MS);
await evaluate(panel, "window.__HANG_FRAMES__.reset()");
await sleep(IDLE_MS);
const idle = JSON.parse(await evaluate(panel, "JSON.stringify(window.__HANG_FRAMES__.summarize())"));
const monitor = JSON.parse(await evaluate(panel, `(() => {
  clearInterval(window.__HANG_PERF_MON_ID__);
  return JSON.stringify(window.__HANG_PERF_MON__);
})()`));

// One fly: direct overlay start (same animation code) while the panel sampler runs. This measures
// the **panel** webview during the overlay work; the fly overlay's own frame rate is not
// instrumented (a separate transparent webview; CDP rAF sampling there is a P2-9 backlog item).
const newest = JSON.parse(await evaluate(panel, `(async () => {
  const s = await window.__TAURI_INTERNALS__.invoke("get_app_state");
  const item = s.items[s.items.length - 1];
  const card = [...document.querySelectorAll(".glass-card")].pop();
  const r = card.getBoundingClientRect();
  return JSON.stringify({ id: item?.id ?? null, rect: { x: Math.round(r.left * devicePixelRatio), y: Math.round(r.top * devicePixelRatio), w: Math.round(r.width * devicePixelRatio), h: Math.round(r.height * devicePixelRatio) } });
})()`));
await evaluate(panel, "window.__HANG_FRAMES__.reset()");
let flyOk = false;
if (newest.id) {
  flyOk = await evaluate(panel, `window.__TAURI_INTERNALS__.invoke("start_fly", { id: ${JSON.stringify(newest.id)}, from: null, to: ${JSON.stringify(newest.rect)}, tiltDeg: 1.2 })`).catch(() => false);
  await sleep(1100);
}
const panelDuringFly = JSON.parse(await evaluate(panel, "JSON.stringify(window.__HANG_FRAMES__.summarize())"));

// One breeze: wait for a gust tick (7–16 s) and measure the 1.5 s that follows it.
const breezeStart = Date.now();
let swung = false;
while (Date.now() - breezeStart < 17000) {
  const swinging = await evaluate(panel, `[...document.querySelectorAll(".glass-card-unit")].some((el) => Math.abs(parseFloat(getComputedStyle(el).getPropertyValue("--swing")) || 0) > 0.4)`);
  if (swinging) {
    swung = true;
    break;
  }
  await sleep(150);
}
await evaluate(panel, "window.__HANG_FRAMES__.reset()");
await sleep(1500);
const breeze = JSON.parse(await evaluate(panel, "JSON.stringify(window.__HANG_FRAMES__.summarize())"));

const summary = {
  generatedAt: new Date().toISOString(),
  method: "CDP rAF frame ring in the panel webview; debug build + vite dev; isolated LOCALAPPDATA",
  environment: {
    cards: stateBefore.items,
    visibleCards,
    theme: stateBefore.theme,
    cardSize: stateBefore.cardSize,
    keep: stateBefore.keep,
    window: "panel",
    build: "debug + vite dev",
    themeCoverage: "dark theme only (light theme not measured in this round)",
  },
  window: {
    samples: idle.samples,
    minSamples: MIN_SAMPLES,
    revealed: monitor.revealed,
    hiddenSamples: monitor.hidden,
    revealSamples: monitor.samples,
  },
  idle,
  panelDuringFly: { available: flyOk, ...panelDuringFly },
  breeze: { observed: swung, ...breeze },
  limits: { avgFps: 58, frameP95Ms: 20, worstMs: 50, minSamples: MIN_SAMPLES },
};
summary.verdict = {
  avgFps: summary.idle.fpsMean >= 58 ? "PASS" : "FAIL",
  p95Frame: summary.idle.fpsP5 >= 50 ? "PASS" : "FAIL",
  worstFrame: summary.idle.worstMs <= 50 ? "PASS" : "FAIL",
  samples: summary.idle.samples >= MIN_SAMPLES ? "PASS" : "FAIL",
  stayedRevealed: summary.window.revealed ? "PASS" : "FAIL",
};
const output = new URL("../../docs/experiments/P2-perf.json", import.meta.url);
writeFileSync(output, JSON.stringify(summary, null, 2) + "\n");
console.log(JSON.stringify(summary, null, 2));
const ok = Object.values(summary.verdict).every((value) => value === "PASS");
process.exit(ok ? 0 : 1);
