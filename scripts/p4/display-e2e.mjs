#!/usr/bin/env node
/* global Buffer */
/**
 * P4-3 runtime evidence: multi-monitor, DPI and work-area placement (M2/M3 programmatic half).
 *
 * What this proves (and what it deliberately does not):
 *   1. with the line pinned, moving the pointer to monitor A (1920x1080 @100%) makes the panel
 *      follow it; the real Win32 window rectangle (GetWindowRect, from a Per-Monitor-V2 process)
 *      equals A's **work-area** top-left with width = work-area width and height = 210 dp x scale;
 *   2. the same on monitor B (1920x1080 @150%): origin = B work-area, physical size 1920x315,
 *      CSS viewport 1280x210, dpr/scale-factor 1.5, and a fixture card's DOM rect mapped to
 *      physical pixels lies inside B with no layout offset;
 *   3. both themes (dark/light) keep the same window origin and size at both monitors, and the
 *      panel height still matches the medium card size at that scale;
 *   4. the app's own `get_monitors` IPC agrees with an independent EnumDisplayMonitors probe.
 *
 * M2's "take one real screenshot on each screen" and M3's real top/left taskbar are
 * MANUAL-PENDING: this harness uses synthetic cards and the machine's real (bottom) taskbar; it
 * never captures the desktop or another app's pixels. The screenshots are panel-webview-only
 * (`Page.captureScreenshot`, no deskop pixels).
 *
 * Requirements (dev-only, no dependencies beyond Node's built-ins):
 *   1. `pnpm dev` (Vite on 127.0.0.1:5173) running;
 *   2. a debug build (`cargo build` in `src-tauri/`);
 *   3. no other Hangline instance (the single-instance plugin would absorb the launch).
 *
 *   node scripts/p4/display-e2e.mjs
 *
 * An isolated profile is created under `%TEMP%\hangline-p4-display\localappdata\Hangline`; the app
 * is launched with `HANG_CDP_PORT=9229`. `scripts/p4/display-probe.ps1` supplies the Win32 side
 * (PMv2 monitor enumeration, `GetWindowRect` by class "Tauri Window", `SetCursorPos`).
 *
 * If the workstation is locked the harness refuses to claim a clean PASS (`--allow-locked` still
 * runs it and records the lock state): window placement and the webview CDP do not need the
 * foreground, but the stated acceptance environment is an unlocked desktop.
 *
 * Evidence (JSON + panel-only PNGs) lands in `.superpowers/p4-evidence/display/` (git-ignored).
 */

import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { deflateSync } from "node:zlib";

const PORT = Number(process.env.HANG_CDP_PORT ?? 9229);
// `scripts/p2/cdp.mjs` reads HANG_CDP_PORT when the module is evaluated.
process.env.HANG_CDP_PORT = String(PORT);
const { connectTo, evaluate, sleep } = await import("../p2/cdp.mjs");

const EVIDENCE_DIR =
  process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p4-evidence", "display");
const EXE = resolve(
  process.env.HANG_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"),
);
const PROBE = resolve("scripts", "p4", "display-probe.ps1");
const ALLOW_LOCKED = process.argv.includes("--allow-locked");

/** Medium card size: panel height = photoMaxHeight(104) + chrome(106) = 210 dp (spec §3.3). */
const PANEL_HEIGHT_DP = 210;
/** Medium card width and the spec's inter-card gap, used to predict the card layout offset. */
const CARD_WIDTH_DP = 150;
const CARD_GAP_DP = 24;
const ROPE_TOP_DP = 10;
const ITEM_WIDTH = 640;
const ITEM_HEIGHT = 360;
/** Five checks per case: 2 monitors x 2 themes. */
const EXPECTED_MONITOR_COUNT = 2;

const base = resolve(process.env.HANG_P4_DISPLAY_BASE ?? join(tmpdir(), "hangline-p4-display"));
const localAppData = join(base, "localappdata");
const profile = join(localAppData, "Hangline");
const screenshotsDir = join(profile, "Screenshots");
const firstPath = join(screenshotsDir, "p4-display-a.png");
const secondPath = join(screenshotsDir, "p4-display-b.png");
// tauri-plugin-log resolves the real `%LOCALAPPDATA%` known folder, not the env override; the app
// log is therefore the standard one. Reading starts at the current EOF so stale lines cannot pass
// an assertion. The plugin rotates every 40 KB (KeepOne), so this file is evidence only, never the
// source of truth for geometry.
const logPath = join(
  process.env.LOCALAPPDATA ?? "",
  "app.hangline.Hangline",
  "logs",
  "Hangline.log",
);
const evidencePath = join(EVIDENCE_DIR, "display-e2e.json");

const results = {
  startedAt: new Date().toISOString(),
  base,
  profile,
  cdpPort: PORT,
  exe: EXE,
  checks: {},
  monitorsWin32: null,
  monitorsApp: null,
  workstation: null,
  workstationLocked: null,
  cases: {},
  screenshots: [],
  timings: {},
  observations: {},
  logLines: [],
  steps: [],
  manualPending: [
    "M2 real capture on each screen (100% + 150%) — MANUAL-PENDING (synthetic fixtures only here)",
    "M3 real top/left taskbar placement — MANUAL-PENDING (work-area math is unit-tested; the run machine has bottom taskbars)",
    "M3 placement after restart with a moved taskbar — MANUAL-PENDING",
    "Display hotplug/unplug while the panel is visible — MANUAL-PENDING (pure migration logic is unit-tested)",
    "Real 125% monitor — MANUAL-PENDING (no 125% display attached; unit tests cover 125%)",
  ],
};
let failures = 0;
let app = null;
let panel = null;

function check(name, condition, detail) {
  const pass = Boolean(condition);
  results.checks[name] = { pass, detail };
  if (!pass) failures += 1;
  console.log(
    `${pass ? "PASS" : "FAIL"} ${name}${detail === undefined ? "" : `: ${JSON.stringify(detail)}`}`,
  );
}

function step(name, detail) {
  results.steps.push({ name, at: new Date().toISOString(), detail });
  console.log(`step: ${name}`);
}

/** The profile is disposable by construction: only `%TEMP%` (or a build `target\`) is cleared. */
function assertDisposable(dir) {
  const resolved = resolve(dir).toLowerCase();
  const temp = resolve(tmpdir()).toLowerCase();
  const insideTemp = resolved === temp || resolved.startsWith(`${temp}${sep}`);
  const insideTarget = resolved.split(sep).includes("target");
  if (!insideTemp && !insideTarget) {
    console.error(
      `display-e2e: refusing to clear ${resolve(dir)} (must be under %TEMP% or target\\)`,
    );
    process.exit(2);
  }
  if (basename(resolved) === "hangline") {
    console.error("display-e2e: HANG_P4_DISPLAY_BASE must be the run root, not the Hangline profile");
    process.exit(2);
  }
}

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const typeBytes = Buffer.from(type, "ascii");
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length, 0);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBytes, data])), 0);
  return Buffer.concat([length, typeBytes, data, crc]);
}

/** Minimal RGB8 PNG: a neutral synthetic gradient; no desktop or user pixels are involved. */
function fixturePng(tint) {
  const stride = ITEM_WIDTH * 3 + 1;
  const raw = Buffer.alloc(stride * ITEM_HEIGHT);
  for (let y = 0; y < ITEM_HEIGHT; y += 1) {
    const row = y * stride;
    raw[row] = 0; // filter: none
    const t = y / (ITEM_HEIGHT - 1);
    const r = Math.round(24 + tint * 40 + 54 * t);
    const g = Math.round(70 + tint * 12 + 42 * (1 - t));
    const b = Math.round(96 + tint * 40 + 34 * (1 - t));
    const rowBytes = raw.subarray(row + 1, row + stride);
    for (let x = 0; x < ITEM_WIDTH; x += 1) {
      rowBytes[x * 3] = r;
      rowBytes[x * 3 + 1] = g;
      rowBytes[x * 3 + 2] = b;
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(ITEM_WIDTH, 0);
  header.writeUInt32BE(ITEM_HEIGHT, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 6 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Writes the isolated profile: two neutral cards, medium size, every watcher/reveal side path off. */
function writeFixture() {
  rmSync(base, { recursive: true, force: true });
  mkdirSync(screenshotsDir, { recursive: true });
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(firstPath, fixturePng(0));
  writeFileSync(secondPath, fixturePng(1));

  const now = Date.now();
  const settings = {
    captureHotkey: "Ctrl+Alt+S",
    toggleHotkey: "Ctrl+Alt+T",
    watchSystemScreenshots: false,
    watchClipboard: false,
    watchRecordings: false,
    extraFolders: [],
    revealOnNew: false,
    revealPeekMs: 2500,
    topEdgeReveal: false,
    showOverFullScreen: true,
    keepOnLine: "fit",
    cardSize: "medium",
    sound: false,
    startAtLogin: false,
    language: "en",
    theme: "dark",
    autoCheckUpdates: false,
    lastUpdateCheck: "",
  };
  writeFileSync(join(profile, "settings.json"), `${JSON.stringify(settings, null, 2)}\n`);
  const items = [
    {
      id: "11111111111111111111111111111111",
      path: firstPath,
      kind: "capture",
      hash: 7101,
      width: ITEM_WIDTH,
      height: ITEM_HEIGHT,
      createdAt: now - 5000,
      addedAt: now - 4000,
      updatedAt: now - 4000,
    },
    {
      id: "22222222222222222222222222222222",
      path: secondPath,
      kind: "capture",
      hash: 7102,
      width: ITEM_WIDTH,
      height: ITEM_HEIGHT,
      createdAt: now - 3000,
      addedAt: now - 2000,
      updatedAt: now - 2000,
    },
  ];
  writeFileSync(join(profile, "state.json"), `${JSON.stringify(items, null, 2)}\n`);
}

/** Incremental log reader; tauri-plugin-log's 40 KB KeepOne rotation resets the offset. */
class LogWatcher {
  constructor(path) {
    this.path = path;
    this.offset = 0;
    this.lines = [];
    this.rotations = 0;
  }

  poll() {
    if (!existsSync(this.path)) return [];
    const data = readFileSync(this.path);
    if (data.length < this.offset) {
      this.offset = 0;
      this.rotations += 1;
    }
    const chunk = data.subarray(this.offset).toString("utf8");
    this.offset = data.length;
    const fresh = chunk.split(/\r?\n/).filter((line) => line.length > 0);
    this.lines.push(...fresh);
    return fresh;
  }

  /** All lines seen so far matching `pattern`; also polls once so later lines are captured. */
  matches(pattern) {
    this.poll();
    return this.lines.filter((line) => pattern.test(line));
  }

  /** Keeps the evidence compact: only interesting lines, newest last. */
  curated() {
    this.poll();
    const interesting =
      /panel: moved to|pointer dwelt|display layout changed|set_setting|frontend reported ready|panel: .*px at|single-instance/;
    return this.lines.filter((line) => interesting.test(line)).slice(-80);
  }
}

const log = new LogWatcher(logPath);

/** Runs one display-probe mode (spawnSync; the helper prints one JSON document). */
function probe(args, timeoutMs = 30000) {
  const run = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      PROBE,
      ...args,
    ],
    { encoding: "utf8", timeout: timeoutMs, windowsHide: true },
  );
  if (run.error) throw new Error(`display-probe ${args.join(" ")} failed: ${run.error.message}`);
  if (run.status !== 0) {
    throw new Error(
      `display-probe ${args.join(" ")} exited ${run.status}: ` +
        `${(run.stderr || run.stdout || "").trim().slice(0, 500)}`,
    );
  }
  const text = (run.stdout ?? "").trim();
  try {
    return JSON.parse(text);
  } catch {
    throw new Error(`display-probe returned non-JSON: ${text.slice(0, 500)}`);
  }
}

const probeMonitors = () => probe(["-Mode", "monitors"]);
const probeWorkstation = () => probe(["-Mode", "workstation"]);
const probeWindows = (pid) => probe(["-Mode", "rect", "-ProcessId", String(pid)]);

/** Moves the real pointer; throws when Windows refused the move (secure desktop). */
function setCursor(x, y) {
  const moved = probe(["-Mode", "cursor", "-X", String(x), "-Y", String(y)]);
  if (!moved.ok) {
    throw new Error(
      `SetCursorPos(${x}, ${y}) failed (error ${moved.error}); the input desktop may be locked`,
    );
  }
  if (moved.cursor?.x !== x || moved.cursor?.y !== y) {
    throw new Error(
      `SetCursorPos(${x}, ${y}) reported cursor ${JSON.stringify(moved.cursor)}; ` +
        `the pointer is not observable on this desktop (locked?)`,
    );
  }
  return moved;
}

/** The panel window among the process's top-level windows: class "Tauri Window", title "Hangline". */
function panelWindow(pid) {
  const windows = probeWindows(pid);
  if (!Array.isArray(windows)) {
    throw new Error(`display-probe rect returned ${JSON.stringify(windows)}`);
  }
  const tauri = windows.filter((window) => window.className === "Tauri Window");
  const matches = tauri.filter((window) => window.title === "Hangline");
  if (matches.length !== 1) {
    results.observations.windowsAtFailure = tauri;
    throw new Error(
      `expected exactly one Tauri Window titled "Hangline", found ${tauri.length}: ` +
        JSON.stringify(tauri.map(({ title, rect, visible }) => ({ title, rect, visible }))),
    );
  }
  return matches[0];
}

/** Physical window expectations derived only from the monitor list and the spec's dp height. */
function expectedFor(monitor) {
  const scale = monitor.scaleFactor;
  return {
    x: monitor.workArea.x,
    y: monitor.workArea.y,
    w: monitor.workArea.w,
    h: Math.round(PANEL_HEIGHT_DP * scale),
    scale,
    cssWidth: monitor.workArea.w / scale,
    cssHeight: PANEL_HEIGHT_DP,
  };
}

async function invoke(command, args = {}) {
  return evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

async function isRevealed() {
  return evaluate(panel, `document.querySelector(".panel")?.dataset.revealed`);
}

async function waitFor(
  label,
  fn,
  predicate = (value) => Boolean(value),
  timeoutMs = 6000,
  pollMs = 60,
) {
  const started = Date.now();
  let last;
  while (Date.now() - started < timeoutMs) {
    try {
      last = await fn();
    } catch {
      last = undefined;
    }
    if (predicate(last)) {
      return { ok: true, value: last, elapsedMs: Date.now() - started };
    }
    await sleep(pollMs);
  }
  return { ok: false, value: last, elapsedMs: Date.now() - started };
}

async function waitRevealed(expected, timeoutMs = 5000) {
  return waitFor(
    `revealed=${expected}`,
    () => isRevealed(),
    (value) => value === String(expected),
    timeoutMs,
  );
}

/** Measures the panel webview: viewport, theme, DOM rects (CSS px) and the Rust scale factor. */
async function readPanelDom() {
  return evaluate(
    panel,
    `(async () => {
      const rect = (element) => {
        const box = element.getBoundingClientRect();
        return {
          left: box.left, top: box.top, right: box.right, bottom: box.bottom,
          width: box.width, height: box.height,
        };
      };
      const panel = document.querySelector(".panel");
      const slots = [...document.querySelectorAll("[data-item-id]")].map((element) => ({
        id: element.dataset.itemId,
        index: Number(element.dataset.index),
        ...rect(element),
      }));
      const cards = [...document.querySelectorAll(".glass-card")].map((element) => ({
        tilt: Number(element.dataset.tilt),
        ...rect(element),
      }));
      const takeDowns = [...document.querySelectorAll(".glass-card__take-down")].map(rect);
      const images = [...document.querySelectorAll(".glass-card__image")].map((element) => ({
        complete: element.complete,
        naturalWidth: element.naturalWidth,
        naturalHeight: element.naturalHeight,
        ...rect(element),
      }));
      return {
        innerWidth,
        innerHeight,
        dpr: devicePixelRatio,
        visibility: document.visibilityState,
        theme: panel?.dataset.theme,
        cardSize: panel?.dataset.cardSize,
        panelWidth: Number(panel?.dataset.panelWidth),
        cardCount: Number(panel?.dataset.cardCount),
        revealed: panel?.dataset.revealed,
        originX: Number(panel?.dataset.originX),
        originY: Number(panel?.dataset.originY),
        slots,
        cards,
        takeDowns,
        images,
        scale: await window.__TAURI_INTERNALS__.invoke("get_scale_factor"),
      };
    })()`,
  );
}

async function screenshot(name) {
  try {
    const captured = await panel.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    });
    const path = join(EVIDENCE_DIR, name);
    writeFileSync(path, Buffer.from(captured.data, "base64"));
    const bytes = statSync(path).size;
    results.screenshots.push({ name, bytes });
    if (bytes < 2000) {
      check(`screenshot_${name}`, false, { bytes, reason: "suspiciously small panel capture" });
    }
    return true;
  } catch (error) {
    console.warn(`display-e2e: screenshot ${name} failed: ${error}`);
    return false;
  }
}

/** Maps one CSS-px DOM rect to virtual-screen physical pixels through the panel window origin. */
function toPhysical(rect, origin, scale) {
  const left = Math.round(origin.x + rect.left * scale);
  const top = Math.round(origin.y + rect.top * scale);
  const right = Math.round(origin.x + rect.right * scale);
  const bottom = Math.round(origin.y + rect.bottom * scale);
  return { x: left, y: top, w: right - left, h: bottom - top, right, bottom };
}

function inside(rect, area, tolerance = 0) {
  return (
    rect.x >= area.x - tolerance &&
    rect.y >= area.y - tolerance &&
    rect.x + rect.w <= area.x + area.w + tolerance &&
    rect.y + rect.h <= area.y + area.h + tolerance
  );
}

/**
 * Predicts the slot layout from spec §3.3 (cardWidth 150, gap 24, rope sag) so "no offset" is a
 * real assertion instead of a tautology: the frontend must place the cards where the spec says.
 */
function expectedSlot(index, count, panelWidthCss) {
  const group = count * CARD_WIDTH_DP + (count - 1) * CARD_GAP_DP;
  const left = Math.max(0, (panelWidthCss - group) / 2);
  const x = left + index * (CARD_WIDTH_DP + CARD_GAP_DP);
  const sag = Math.min(30, Math.max(0, panelWidthCss * 0.018));
  const t = panelWidthCss > 0 ? Math.min(1, Math.max(0, (x + CARD_WIDTH_DP / 2) / panelWidthCss)) : 0;
  const ropeY = ROPE_TOP_DP + 4 * sag * t * (1 - t);
  return { x, top: ropeY - 9.5 };
}

function expectedWindowRect(expected) {
  return { x: expected.x, y: expected.y, w: expected.w, h: expected.h };
}

async function waitWindowRect(pid, expected, timeoutMs = 8000) {
  const wanted = expectedWindowRect(expected);
  return waitFor(
    `window rect ${JSON.stringify(wanted)}`,
    () => panelWindow(pid),
    (window) =>
      window &&
      window.rect.x === wanted.x &&
      window.rect.y === wanted.y &&
      window.rect.w === wanted.w &&
      window.rect.h === wanted.h,
    timeoutMs,
    200,
  );
}

function sameRect(a, b) {
  return a && b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
}

/** Pins the line (second instance = the documented tray toggle) if the startup reveal is showing. */
async function pinLine() {
  const hidden = await waitRevealed(false, 5000);
  if (!hidden.ok) {
    toggleLine();
    await waitRevealed(false, 5000);
  }
  toggleLine();
  return waitRevealed(true, 6000);
}

/**
 * Waits until the card slots stop moving.
 *
 * Moving the window to another DPI monitor changes the CSS viewport, so `useSlotSpring` animates
 * the slots from their old to their new layout position (an under-damped spring that can overshoot
 * by a fraction of a pixel). Measuring during that animation would mis-report a "layout offset";
 * the geometry assertions must see the settled layout, not a transient reorder frame.
 */
async function waitStableLayout(timeoutMs = 5000) {
  const started = Date.now();
  let previous = null;
  while (Date.now() - started < timeoutMs) {
    const dom = await readPanelDom();
    const layout = JSON.stringify(
      dom.slots.map((slot) => [slot.left, slot.top, slot.right, slot.bottom]),
    );
    if (previous !== null && previous === layout) {
      return { ok: true, dom, elapsedMs: Date.now() - started };
    }
    previous = layout;
    await sleep(160);
  }
  return { ok: false, dom: await readPanelDom(), elapsedMs: Date.now() - started };
}

/** One monitor/theme case: move the pointer, wait for geometry, measure and assert. */
async function measureCase({ tag, theme, monitor, previousRect = null }) {
  const expected = expectedFor(monitor);
  const center = {
    x: monitor.rect.x + Math.floor(monitor.rect.w / 2),
    y: monitor.rect.y + Math.floor(monitor.rect.h / 2),
  };
  check(`${tag}_line_visible`, (await isRevealed()) === "true", {
    revealed: await isRevealed(),
  });
  step(`${tag}: pointer to monitor ${monitor.id} at (${center.x}, ${center.y})`);
  const moved = setCursor(center.x, center.y);
  results.observations[`${tag}_cursor`] = moved;

  const settled = await waitWindowRect(app.pid, expected);
  const movedAt = settled.ok ? settled : await waitWindowRect(app.pid, expected, 3000);
  results.timings[`${tag}_geometryMs`] = movedAt.elapsedMs;
  check(`${tag}_geometry_settled`, movedAt.ok, {
    elapsedMs: movedAt.elapsedMs,
    expected: expectedWindowRect(expected),
    measured: movedAt.value?.rect ?? null,
  });

  const domWait = await waitFor(
    `${tag} webview`,
    () => readPanelDom(),
    (dom) =>
      dom &&
      Math.abs(dom.dpr - expected.scale) < 0.01 &&
      Math.abs(dom.innerWidth - expected.cssWidth) <= 1 &&
      Math.abs(dom.innerHeight - expected.cssHeight) <= 1 &&
      dom.cardCount === 2 &&
      dom.theme === theme,
    8000,
    120,
  );
  check(`${tag}_webview_settled`, domWait.ok, {
    elapsedMs: domWait.elapsedMs,
    innerWidth: domWait.value?.innerWidth ?? null,
    innerHeight: domWait.value?.innerHeight ?? null,
    dpr: domWait.value?.dpr ?? null,
    theme: domWait.value?.theme ?? null,
  });
  const settledLayout = await waitStableLayout();
  check(`${tag}_layout_settled`, settledLayout.ok, {
    elapsedMs: settledLayout.elapsedMs,
  });
  await sleep(120); // one painted frame after the resize/theme change
  const domAfter = await readPanelDom();
  const window = panelWindow(app.pid);
  const measured = {
    rect: window.rect,
    client: window.client,
    clientOrigin: window.clientOrigin,
    dpi: window.dpi,
    visible: window.visible,
  };
  await screenshot(`panel-${tag}.png`);

  // 1. Origin and physical size: the work area, never the full screen rect.
  check(`${tag}_origin_is_work_area`, window.rect.x === expected.x && window.rect.y === expected.y, {
    measured: { x: window.rect.x, y: window.rect.y },
    expected: { x: expected.x, y: expected.y },
    monitor: { rect: monitor.rect, workArea: monitor.workArea },
  });
  check(
    `${tag}_physical_size`,
    window.rect.w === expected.w && window.rect.h === expected.h,
    {
      measured: { w: window.rect.w, h: window.rect.h },
      expected: { w: expected.w, h: expected.h, dp: PANEL_HEIGHT_DP, scale: expected.scale },
    },
  );
  check(
    `${tag}_window_undecorated`,
    window.client.w === window.rect.w &&
      window.client.h === window.rect.h &&
      window.clientOrigin.x === window.rect.x &&
      window.clientOrigin.y === window.rect.y,
    { rect: window.rect, client: window.client, clientOrigin: window.clientOrigin },
  );
  check(`${tag}_window_dpi`, window.dpi === Math.round(96 * expected.scale), {
    dpi: window.dpi,
    expected: Math.round(96 * expected.scale),
  });

  // 2. The webview sees the matching CSS viewport, dpr and Rust scale factor.
  check(
    `${tag}_webview_viewport`,
    Math.abs(domAfter.innerWidth - expected.cssWidth) <= 0.5 &&
      Math.abs(domAfter.innerHeight - expected.cssHeight) <= 0.5,
    {
      innerWidth: domAfter.innerWidth,
      innerHeight: domAfter.innerHeight,
      expected: { width: expected.cssWidth, height: expected.cssHeight },
    },
  );
  check(`${tag}_dpr`, Math.abs(domAfter.dpr - expected.scale) < 0.01, {
    dpr: domAfter.dpr,
    scale: expected.scale,
  });
  check(`${tag}_scale_factor_ipc`, Math.abs(domAfter.scale - expected.scale) < 0.01, {
    scaleFactor: domAfter.scale,
    monitorScale: expected.scale,
  });
  check(`${tag}_theme_applied`, domAfter.theme === theme, {
    theme: domAfter.theme,
    expected: theme,
  });
  check(`${tag}_medium_card_size`, domAfter.cardSize === "medium", {
    cardSize: domAfter.cardSize,
  });

  // 3. Two fixture cards render, their images decode. The panel may serve the generated 432 px
  //    preview (startup thumbnail backfill) instead of the 640x360 original, so only the aspect
  //    ratio and a successful decode are fixed.
  check(
    `${tag}_two_cards_rendered`,
    domAfter.cardCount === 2 &&
      domAfter.slots.length === 2 &&
      domAfter.cards.length === 2 &&
      domAfter.images.length === 2 &&
      domAfter.images.every(
        (image) =>
          image.complete &&
          image.naturalWidth > 0 &&
          image.naturalHeight > 0 &&
          Math.abs(image.naturalWidth / image.naturalHeight - ITEM_WIDTH / ITEM_HEIGHT) < 0.01,
      ),
    {
      cardCount: domAfter.cardCount,
      slots: domAfter.slots.length,
      cards: domAfter.cards.length,
      images: domAfter.images.map((image) => ({
        complete: image.complete,
        naturalWidth: image.naturalWidth,
        naturalHeight: image.naturalHeight,
      })),
    },
  );

  // 4. Card DOM rects mapped to physical pixels lie inside the monitor (and its work area).
  const origin = { x: window.rect.x, y: window.rect.y };
  const slotPhysical = domAfter.slots.map((slot) => ({
    index: slot.index,
    ...toPhysical(slot, origin, expected.scale),
  }));
  const cardPhysical = domAfter.cards.map((card) => toPhysical(card, origin, expected.scale));
  const chromePhysical = domAfter.takeDowns.map((rect) => toPhysical(rect, origin, expected.scale));
  const slotInsideMonitor = slotPhysical.every((rect) => inside(rect, monitor.rect));
  const cardInsideWorkArea = cardPhysical.every((rect) => inside(rect, monitor.workArea));
  check(
    `${tag}_cards_inside_monitor`,
    slotInsideMonitor && cardInsideWorkArea,
    {
      monitor: monitor.rect,
      workArea: monitor.workArea,
      slots: slotPhysical,
      cards: cardPhysical,
    },
  );

  // 5. No layout offset: each slot sits exactly where spec §3.3 puts it in this CSS viewport.
  const predicted = domAfter.slots.map((slot) =>
    expectedSlot(slot.index, domAfter.slots.length, domAfter.innerWidth),
  );
  const offset = domAfter.slots.map((slot, index) => ({
    id: slot.id,
    index: slot.index,
    dx: slot.left - predicted[index].x,
    dy: slot.top - predicted[index].top,
  }));
  check(
    `${tag}_card_layout_has_no_offset`,
    offset.every((entry) => Math.abs(entry.dx) <= 0.75 && Math.abs(entry.dy) <= 0.75),
    { offset, predicted, measured: domAfter.slots },
  );

  // 6. Card chrome (take-down button) and the full card AABB are inside the panel viewport, so a
  //    150% move cannot clip the card, its chrome or the line (panel height check below).
  const viewport = { x: 0, y: 0, w: domAfter.innerWidth, h: domAfter.innerHeight };
  const slotsInViewport = domAfter.slots.every(
    (slot) => slot.left >= 0 && slot.top >= 0 && slot.right <= domAfter.innerWidth + 0.5 && slot.bottom <= domAfter.innerHeight + 0.5,
  );
  const cardsInViewport = domAfter.cards.every(
    (card) => card.left >= -0.5 && card.top >= -0.5 && card.right <= domAfter.innerWidth + 0.5 && card.bottom <= domAfter.innerHeight + 0.5,
  );
  const chromeInViewport = domAfter.takeDowns.every(
    (rect) => rect.left >= -0.5 && rect.top >= -0.5 && rect.right <= domAfter.innerWidth + 0.5 && rect.bottom <= domAfter.innerHeight + 0.5,
  );
  check(
    `${tag}_card_and_chrome_not_clipped`,
    slotsInViewport && cardsInViewport && chromeInViewport,
    {
      viewport,
      slots: domAfter.slots,
      cards: domAfter.cards,
      takeDowns: domAfter.takeDowns,
      chromePhysical,
    },
  );
  check(
    `${tag}_line_height_is_medium_at_scale`,
    domAfter.innerHeight === PANEL_HEIGHT_DP &&
      Math.round(domAfter.innerHeight * domAfter.dpr) === expected.h,
    {
      innerHeight: domAfter.innerHeight,
      dpr: domAfter.dpr,
      physical: Math.round(domAfter.innerHeight * domAfter.dpr),
      expectedPhysical: expected.h,
    },
  );

  // 7. A theme switch must not move or resize the window; compare with the same-monitor case
  //    measured under the previous theme.
  if (previousRect) {
    check(
      `${tag}_theme_keeps_geometry`,
      sameRect(window.rect, previousRect),
      { before: previousRect, after: window.rect },
    );
  }

  results.cases[tag] = {
    theme,
    monitorId: monitor.id,
    monitor,
    expected,
    measured,
    dom: {
      innerWidth: domAfter.innerWidth,
      innerHeight: domAfter.innerHeight,
      dpr: domAfter.dpr,
      scaleFactor: domAfter.scale,
      theme: domAfter.theme,
      cardSize: domAfter.cardSize,
      panelWidth: domAfter.panelWidth,
      cardCount: domAfter.cardCount,
      revealed: domAfter.revealed,
      slots: domAfter.slots,
      cards: domAfter.cards,
      takeDowns: domAfter.takeDowns,
      images: domAfter.images,
      slotPhysical,
      cardPhysical,
      predictedSlots: predicted,
      offset,
    },
    screenshot: `panel-${tag}.png`,
  };
  log.poll();
  return window.rect;
}

/** Launches the app with the isolated profile + CDP port. */
function launchApp() {
  if (!existsSync(EXE)) {
    throw new Error(
      `debug exe not found at ${EXE}; run \`cargo build\` in src-tauri (debug) before the E2E`,
    );
  }
  app = spawn(EXE, [], {
    env: { ...process.env, LOCALAPPDATA: localAppData, HANG_CDP_PORT: String(PORT) },
    stdio: "ignore",
    windowsHide: false,
  });
  app.on("error", (error) => console.error("display-e2e: app spawn error", error));
  console.log(`app: pid ${app.pid} with LOCALAPPDATA=${localAppData}`);
}

/** Second instance = the documented tray/hotkey-equivalent toggle (single-instance plugin). */
function toggleLine() {
  const child = spawn(EXE, [], {
    env: { ...process.env, LOCALAPPDATA: localAppData },
    stdio: "ignore",
    detached: true,
  });
  child.unref();
  return child.pid;
}

/** Refuses to start when a Hangline instance already owns the single-instance lock. */
function ensureNoOtherInstance() {
  const run = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      "@(Get-Process hangline -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) -join ','",
    ],
    { encoding: "utf8", timeout: 30000, windowsHide: true },
  );
  const ids = (run.stdout ?? "").trim();
  if (run.error) throw new Error(`could not check for a running Hangline: ${run.error.message}`);
  if (ids.length > 0) {
    throw new Error(
      `another Hangline instance is running (pid ${ids}); the single-instance plugin would absorb ` +
        `this launch instead of starting the isolated profile. Quit it and rerun ` +
        `\`node scripts/p4/display-e2e.mjs\`.`,
    );
  }
}

async function cleanup() {
  if (app && app.exitCode === null) {
    try {
      app.kill();
    } catch {
      /* already gone */
    }
    const exited = await Promise.race([
      new Promise((done) => app.once("exit", () => done(true))),
      sleep(4000).then(() => false),
    ]);
    if (!exited) {
      console.error("display-e2e: app did not exit within 4 s; forcing it and its webviews");
      try {
        app.kill("SIGKILL");
      } catch {
        /* already gone */
      }
      spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-NonInteractive",
          "-Command",
          "Get-Process hangline -ErrorAction SilentlyContinue | Stop-Process -Force",
        ],
        { encoding: "utf8", timeout: 15000, windowsHide: true },
      );
    }
  }
}

function finish(verdict) {
  results.finishedAt = new Date().toISOString();
  results.verdict = verdict;
  results.failureCount = failures;
  results.logLines = log.curated();
  results.logRotations = log.rotations;
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(evidencePath, `${JSON.stringify(results, null, 2)}\n`);
  console.log(`evidence: ${evidencePath}`);
  console.log(
    `verdict: ${verdict} (${Object.keys(results.checks).length} checks, ${failures} failed)`,
  );
}

async function main() {
  assertDisposable(base);
  ensureNoOtherInstance();

  const workstation = probeWorkstation();
  results.workstation = workstation;
  const locked =
    workstation.quns === 1 ||
    !workstation.inputDesktopAccessible ||
    workstation.foregroundClass === "Windows.UI.Core.CoreWindow";
  results.workstationLocked = locked;
  if (locked) {
    const message =
      `the workstation is locked (QUNS=${workstation.quns}, ` +
      `foreground=${workstation.foregroundClass}, inputDesktop=${workstation.inputDesktopAccessible}); ` +
      `window placement/CDP still work, but this is not the stated acceptance environment`;
    if (!ALLOW_LOCKED) {
      throw new Error(
        `${message}. Unlock the session and rerun \`node scripts/p4/display-e2e.mjs\`, or rerun ` +
          `with --allow-locked to record a placement-only run with the lock caveat.`,
      );
    }
    console.warn(`display-e2e: WARNING: ${message}; continuing because --allow-locked was passed`);
  }

  const win32Monitors = probeMonitors();
  results.monitorsWin32 = win32Monitors;
  if (!Array.isArray(win32Monitors)) {
    throw new Error(`display-probe monitors returned ${JSON.stringify(win32Monitors)}`);
  }
  check("two_monitors_attached", win32Monitors.length === EXPECTED_MONITOR_COUNT, {
    count: win32Monitors.length,
    monitors: win32Monitors,
  });
  const scales = win32Monitors.map((monitor) => monitor.scaleFactor).sort((a, b) => a - b);
  check(
    "monitor_scales_are_100_and_150",
    scales.length === 2 && Math.abs(scales[0] - 1) < 0.01 && Math.abs(scales[1] - 1.5) < 0.01,
    { scales },
  );
  check(
    "monitor_resolutions_are_1920x1080",
    win32Monitors.every((monitor) => monitor.rect.w === 1920 && monitor.rect.h === 1080),
    { rects: win32Monitors.map((monitor) => monitor.rect) },
  );
  if (win32Monitors.length !== EXPECTED_MONITOR_COUNT) {
    throw new Error(
      `this evidence run needs exactly ${EXPECTED_MONITOR_COUNT} monitors; ` +
        `display-probe found ${win32Monitors.length}: ${JSON.stringify(win32Monitors)}`,
    );
  }

  writeFixture();
  results.logPath = logPath;
  if (existsSync(logPath)) log.offset = statSync(logPath).size;
  step("profile fixture", { profile, items: [firstPath, secondPath], theme: "dark" });
  launchApp();

  panel = await connectTo("window=panel", 45000);
  await waitFor(
    "panel DOM",
    () => evaluate(panel, `!!document.querySelector(".panel")`),
    Boolean,
    30000,
  );
  check("panel_started", true);
  if (app.exitCode !== null) {
    throw new Error(
      "the app exited during startup; another Hangline instance may own the single-instance lock",
    );
  }

  // The app's own monitor IPC must agree with the independent Win32 enumeration (id/rect/work area/
  // scale are the wire contract the geometry math uses).
  const appMonitors = await invoke("get_monitors");
  results.monitorsApp = appMonitors;
  check(
    "app_monitors_match_win32",
    Array.isArray(appMonitors) &&
      appMonitors.length === win32Monitors.length &&
      appMonitors.every((monitor) => {
        const win32 = win32Monitors.find((candidate) => candidate.id === monitor.id);
        return (
          win32 &&
          win32.rect.x === monitor.rect.x &&
          win32.rect.y === monitor.rect.y &&
          win32.rect.w === monitor.rect.w &&
          win32.rect.h === monitor.rect.h &&
          win32.workArea.x === monitor.workArea.x &&
          win32.workArea.y === monitor.workArea.y &&
          win32.workArea.w === monitor.workArea.w &&
          win32.workArea.h === monitor.workArea.h &&
          Math.abs(win32.scaleFactor - monitor.scaleFactor) < 0.001
        );
      }),
    { app: appMonitors, win32: win32Monitors },
  );

  const normal = appMonitors.find((monitor) => Math.abs(monitor.scaleFactor - 1) < 0.01);
  const hidpi = appMonitors.find((monitor) => Math.abs(monitor.scaleFactor - 1.5) < 0.01);
  if (!normal || !hidpi) {
    throw new Error(`the expected 100% and 150% monitors are not both present: ${JSON.stringify(appMonitors)}`);
  }

  // Pin the line on the primary (100%) first: the cursor is already there, so the first visible
  // tick syncs the panel to the 100% monitor.
  step("pin the line for the placement sweep");
  setCursor(
    normal.rect.x + Math.floor(normal.rect.w / 2),
    normal.rect.y + Math.floor(normal.rect.h / 2),
  );
  const pinned = await pinLine();
  check("line_pinned", pinned.ok, { elapsedMs: pinned.elapsedMs, revealed: pinned.value });

  // --- dark theme, both monitors -------------------------------------------------------------
  step("1. dark theme on the 100% monitor");
  const darkA = await measureCase({ tag: "dark-100", theme: "dark", monitor: normal });

  step("2. dark theme on the 150% monitor");
  const darkB = await measureCase({ tag: "dark-150", theme: "dark", monitor: hidpi });
  check(
    "dark_cross_dpi_move",
    !sameRect(darkA, darkB) &&
      darkA.x === normal.workArea.x &&
      darkB.x === hidpi.workArea.x &&
      darkB.h === Math.round(PANEL_HEIGHT_DP * 1.5),
    { from: darkA, to: darkB },
  );

  // --- light theme, both monitors -------------------------------------------------------------
  step("3. switch to the light theme on the 150% monitor");
  await invoke("set_setting", { key: "theme", value: "light" });
  const lightApplied = await waitFor(
    "light theme",
    () => readPanelDom(),
    (dom) => dom && dom.theme === "light",
    6000,
    120,
  );
  check("light_theme_applied", lightApplied.ok, {
    theme: lightApplied.value?.theme ?? null,
    elapsedMs: lightApplied.elapsedMs,
  });
  await measureCase({
    tag: "light-150",
    theme: "light",
    monitor: hidpi,
    previousRect: darkB,
  });

  step("4. light theme on the 100% monitor");
  await measureCase({
    tag: "light-100",
    theme: "light",
    monitor: normal,
    previousRect: darkA,
  });

  // Theme must not have changed the per-monitor geometry at all.
  const domFinal = await readPanelDom();
  check(
    "light_theme_keeps_dpi_and_size",
    Math.abs(domFinal.dpr - normal.scaleFactor) < 0.01 &&
      domFinal.innerWidth === normal.workArea.w &&
      domFinal.innerHeight === PANEL_HEIGHT_DP,
    {
      dpr: domFinal.dpr,
      innerWidth: domFinal.innerWidth,
      innerHeight: domFinal.innerHeight,
      monitor: normal,
    },
  );

  const finalRect = panelWindow(app.pid).rect;
  check(
    "final_geometry_on_100_percent",
    finalRect.x === normal.workArea.x &&
      finalRect.y === normal.workArea.y &&
      finalRect.w === normal.workArea.w &&
      finalRect.h === Math.round(PANEL_HEIGHT_DP * normal.scaleFactor),
    { finalRect, expected: expectedWindowRect(expectedFor(normal)) },
  );

  const cards = await evaluate(
    panel,
    `document.querySelector(".panel")?.dataset.cardCount`,
  );
  check("fixture_items_present", Number(cards) === 2, { cards });
}

try {
  await main();
  finish(
    failures === 0
      ? results.workstationLocked
        ? "PASS (locked-desktop caveat: placement/CDP only)"
        : "PASS"
      : "FAIL",
  );
} catch (error) {
  failures += 1;
  results.error = error instanceof Error ? `${error.message}\n${error.stack}` : String(error);
  console.error("display-e2e: fatal:", results.error);
  finish("FAIL");
} finally {
  try {
    await cleanup();
  } catch (error) {
    console.error("display-e2e: cleanup failed:", error);
  }
}
process.exit(failures === 0 ? 0 : 1);
