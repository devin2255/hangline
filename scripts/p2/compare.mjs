import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { connectTo, evaluate, screenshotClip, sleep } from "./cdp.mjs";

const ACTION_DIR = process.env.HANG_COMPARE_CARDS ?? "";
const WATCH_DIR = process.env.HANG_WATCH_DIR ?? "";
if (!ACTION_DIR || !WATCH_DIR) {
  console.error("compare: set HANG_COMPARE_CARDS (action-*.png fixtures) and HANG_WATCH_DIR (isolated Screenshots dir); see docs/experiments/P2-visual.md runbook");
  process.exit(2);
}
const SHOTS = new URL("../../docs/experiments/p2-frames", import.meta.url).pathname
  .replace(/^\/([A-Za-z]):/, "$1:")
  .replaceAll("/", "\\");
mkdirSync(SHOTS, { recursive: true });

const panel = await connectTo("window=panel");
const flyTarget = await connectTo("window=fly");
for (let attempt = 0; attempt < 150; attempt += 1) {
  const ready = await evaluate(panel, `!!window.__HANG_FRAMES__`).catch(() => false);
  if (ready) break;
  await sleep(100);
}
await evaluate(panel, `window.__HANG_TRACE_ON__ = true`);
await evaluate(flyTarget, `window.__HANG_TRACE_ON__ = true`);

const invoke = (command, args = {}) => evaluate(panel, `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`);
const getState = async () => JSON.parse(await evaluate(panel, `(async () => { const s = await window.__TAURI_INTERNALS__.invoke("get_app_state"); return JSON.stringify(s); })()`));
const trace = async () => JSON.parse(await evaluate(panel, `JSON.stringify(window.__HANG_TRACE__ ?? [])`));
const flyTrace = async () => JSON.parse(await evaluate(flyTarget, `JSON.stringify(window.__HANG_TRACE__ ?? [])`));
const clearTrace = async () => { await evaluate(panel, `window.__HANG_CLEAR_TRACE__?.()`); await evaluate(flyTarget, `window.__HANG_CLEAR_TRACE__?.()`); };
const rectOf = async (selector) => JSON.parse(await evaluate(panel, `JSON.stringify((() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return null; const r = el.getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height }; })())`));
const shot = async (name, rect, pad = 12) => {
  if (!rect) return null;
  const path = `${SHOTS}/${name}.png`;
  await screenshotClip(panel, path, { x: Math.max(0, rect.x - pad), y: Math.max(0, rect.y - pad), width: rect.w + pad * 2, height: rect.h + pad * 2 });
  return `p2-frames/${name}.png`;
};
const pointer = (type, x, y, extra = {}) => panel.send("Input.dispatchMouseEvent", { type, x: Math.round(x), y: Math.round(y), button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0, clickCount: 1, ...extra });
const visibleHeight = await evaluate(panel, `window.innerHeight`);
const outsidePointer = async () => pointer("mouseMoved", 40, Math.round(visibleHeight - 8));
const takeDownAll = async () => {
  const state = await getState();
  // Non-destructive: the fixture files stay on disk (isolated profile).
  for (const item of state.items) { await invoke("take_down", { id: item.id, deleteFile: false }).catch(() => {}); }
  await sleep(500);
};
const addCard = async (index) => {
  const source = `${ACTION_DIR}/action-${index}.png`;
  const target = `${WATCH_DIR}/action-${index}-${Date.now()}.png`;
  copyFileSync(source, target);
};

const results = { reveal: null, arrival: null, tuck: null, hover: null, press: null, copied: null, fly: null, fall: null, screenshots: [], environment: {} };
const state0 = await getState();
results.environment = {
  items: state0.items.length,
  theme: state0.settings.theme,
  cardSize: state0.settings.cardSize,
  keep: state0.settings.keepOnLine,
  runNote:
    "same 12-item line as perf.mjs; compare takes items off the line (deleteFile:false) and adds action fixtures; screenshots are relative to docs/experiments",
};

const analyzePanelY = (samples, settleY) => {
  const ys = samples.filter((sample) => sample.kind === "panel").map((sample) => ({ t: sample.t, y: sample.data.y })).sort((a, b) => a.t - b.t);
  if (ys.length < 2) return null;
  const start = ys[0];
  const end = ys[ys.length - 1];
  const settled = ys.find((sample) => Math.abs(sample.y - settleY) <= 0.75);
  return { durationMs: settled ? Math.round(settled.t - start.t) : Math.round(end.t - start.t), startY: +start.y.toFixed(1), endY: +end.y.toFixed(1), samples: ys.length };
};

// --- Reveal: empty/tucked panel → add one card; the line springs from hidden to 0.
await takeDownAll();
await outsidePointer();
await sleep(900);
await clearTrace();
await addCard(0);
await sleep(1200);
const revealTrace = await trace();
results.reveal = analyzePanelY(revealTrace, 0);
const cardRectEarly = await rectOf(".glass-card");
results.screenshots.push(await shot("reveal", cardRectEarly, 24));

// --- Arrival: card drop-in from 16° / −46 px.
const cardSamples = revealTrace.filter((sample) => sample.kind === "card").map((sample) => ({ t: sample.t, ...sample.data })).sort((a, b) => a.t - b.t);
if (cardSamples.length > 1) {
  results.arrival = {
    swingPeakDeg: +Math.max(...cardSamples.map((sample) => Math.abs(sample.swing))).toFixed(2),
    startSwingDeg: +cardSamples[0].swing.toFixed(2),
    startArriveY: +cardSamples[0].arriveY.toFixed(1),
    minArriveY: +Math.min(...cardSamples.map((sample) => sample.arriveY)).toFixed(1),
    durationMs: Math.round(cardSamples[cardSamples.length - 1].t - cardSamples[0].t),
  };
}

// --- Hover + press + copied nudge on the newest card.
const cardRect = (await rectOf(".glass-card-slot:last-of-type .glass-card")) ?? cardRectEarly;
if (cardRect) {
  const cx = cardRect.x + cardRect.w / 2;
  const cy = cardRect.y + cardRect.h / 2;
  await clearTrace();
  await pointer("mouseMoved", cx, cy);
  await sleep(450);
  const hoverSamples = (await trace()).filter((sample) => sample.kind === "card").map((sample) => sample.data.scale);
  results.hover = hoverSamples.length ? { maxScale: +Math.max(...hoverSamples).toFixed(4) } : null;
  results.screenshots.push(await shot("hover", cardRect, 20));
  await clearTrace();
  await pointer("mousePressed", cx, cy);
  await sleep(550);
  const pressSamples = (await trace()).filter((sample) => sample.kind === "card").map((sample) => sample.data.scale);
  results.press = pressSamples.length ? { minScale: +Math.min(...pressSamples).toFixed(4) } : null;
  await clearTrace();
  await pointer("mouseReleased", cx, cy);
  await sleep(900);
  const copiedSamples = (await trace()).filter((sample) => sample.kind === "card").map((sample) => sample.data.swing);
  results.copied = copiedSamples.length ? { peakSwingDeg: +Math.max(...copiedSamples.map((value) => Math.abs(value))).toFixed(2), samples: copiedSamples.length } : null;
}

// --- Fly: add another unique card and measure the overlay animation from its trace.
await outsidePointer();
await clearTrace();
await addCard(1);
let flySamples = [];
const flyDeadline = Date.now() + 4000;
while (Date.now() < flyDeadline) {
  flySamples = (await flyTrace()).filter((sample) => sample.kind === "fly");
  const lastK = flySamples.at(-1)?.data.k ?? 0;
  if (lastK >= 0.995) break;
  await sleep(20);
}
// Let the final frame land in the trace so the measured duration is the full animation.
await sleep(150);
flySamples = (await flyTrace()).filter((sample) => sample.kind === "fly");
if (flySamples.length > 1) {
  const first = flySamples[0];
  const last = flySamples[flySamples.length - 1];
  results.fly = {
    durationMs: Math.round(last.t - first.t),
    maxK: +Math.max(...flySamples.map((sample) => sample.data.k)).toFixed(3),
    maxArcDp: +Math.max(...flySamples.map((sample) => Math.abs(sample.data.arcDp))).toFixed(1),
    maxChrome: +Math.max(...flySamples.map((sample) => sample.data.chrome)).toFixed(2),
    samples: flySamples.length,
  };
}
await sleep(700);
results.screenshots.push(await shot("fly-landed", await rectOf(".glass-card-slot:last-of-type .glass-card"), 22));

// --- Fall: take the newest card down and measure its overlay trace.
const crossRect = JSON.parse(await evaluate(panel, `JSON.stringify((() => { const buttons = [...document.querySelectorAll(".glass-card__take-down")]; const b = buttons[buttons.length - 1]; if (!b) return null; const r = b.getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height }; })())`));
await clearTrace();
if (crossRect) {
  await pointer("mouseMoved", crossRect.x + crossRect.w / 2, crossRect.y + crossRect.h / 2);
  await sleep(120);
  await pointer("mousePressed", crossRect.x + crossRect.w / 2, crossRect.y + crossRect.h / 2);
  await pointer("mouseReleased", crossRect.x + crossRect.w / 2, crossRect.y + crossRect.h / 2);
}
let fallSamples = [];
const fallDeadline = Date.now() + 4000;
while (Date.now() < fallDeadline) {
  fallSamples = (await flyTrace()).filter((sample) => sample.kind === "fall");
  const lastK = fallSamples.at(-1)?.data.k ?? 0;
  if (lastK >= 0.995) break;
  await sleep(20);
}
await sleep(150);
fallSamples = (await flyTrace()).filter((sample) => sample.kind === "fall");
if (fallSamples.length > 1) {
  const first = fallSamples[0];
  const last = fallSamples[fallSamples.length - 1];
  results.fall = {
    durationMs: Math.round(last.t - first.t),
    maxK: +Math.max(...fallSamples.map((sample) => sample.data.k)).toFixed(3),
    maxDropDp: +Math.max(...fallSamples.map((sample) => Math.abs(sample.data.dropDp))).toFixed(1),
    minOpacity: +Math.min(...fallSamples.map((sample) => sample.data.opacity)).toFixed(3),
    maxTiltDeg: +Math.max(...fallSamples.map((sample) => Math.abs(sample.data.tilt))).toFixed(2),
    samples: fallSamples.length,
  };
}
await sleep(700);
const fallRect = await rectOf(".glass-card-slot:last-of-type .glass-card");
results.screenshots.push(await shot("fall-after", fallRect, 22));

// --- Tuck: empty the line (non-destructively) and sample the CSS ease-in transition. The panel's
// own `tuck()` instrumentation pushes a `tuck` trace per rAF, so this no longer needs an OS cursor.
await clearTrace();
await takeDownAll();
let tuckSamples = [];
for (let attempt = 0; attempt < 40; attempt += 1) {
  const samples = await trace();
  tuckSamples = samples
    .filter((sample) => sample.kind === "tuck")
    .map((sample) => ({ t: sample.t, y: sample.data.y }))
    .sort((a, b) => a.t - b.t);
  if (tuckSamples.length > 2) break;
  await sleep(50);
}
await sleep(600);
if (tuckSamples.length > 1) {
  const start = tuckSamples[0];
  const end = tuckSamples[tuckSamples.length - 1];
  results.tuck = {
    durationMs: Math.round(end.t - start.t),
    startY: +start.y.toFixed(1),
    endY: +end.y.toFixed(1),
    samples: tuckSamples.length,
  };
}

// --- Breeze amplitude is covered by perf.mjs (`breeze.observed` + 60 fps sample); the comparison
// harness intentionally does not report a null amplitude metric.

writeFileSync(new URL("../../docs/experiments/P2-visual-raw.json", import.meta.url), JSON.stringify(results, null, 2) + "\n");
console.log(JSON.stringify(results, null, 2));
panel.close();
flyTarget.close();
