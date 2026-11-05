#!/usr/bin/env node
/* global Buffer */
/**
 * P4-2 runtime evidence: full-screen suppression (M4), programmatic part.
 *
 * The workstation must be **unlocked**. The script first asks the helper to probe QUNS and
 * LogonUI: on the secure desktop no user window can take the foreground and QUNS is forced to 1,
 * so it fails fast with a clear message instead of waiting for a synthetic foreground hand-off.
 *
 * Requires (dev-only, no dependencies beyond Node's built-ins):
 *   1. `pnpm dev` (Vite on 127.0.0.1:5173) running in another terminal;
 *   2. a debug build: `cargo build` in `src-tauri/` (or `pnpm tauri dev` once);
 *   3. no other Hangline instance using the same isolated profile.
 *
 *   node scripts/p4/fullscreen-e2e.mjs
 *
 * The script creates an isolated profile under `%TEMP%\hangline-p4-fullscreen`:
 * `localappdata\Hangline` holds settings/state plus neutral synthetic PNGs; `Watched` is an
 * extra watched folder used for the "new item while full screen" step. It launches the debug exe
 * with `HANG_CDP_PORT=9228`, drives the panel through CDP, and starts
 * `scripts/p4/fullscreen-window.ps1`: a borderless, topmost window exactly covering the primary
 * monitor (a synthetic stand-in for a full-screen app). The helper writes a ready file only after
 * it really is the foreground window, and a `.closed` marker when it starts closing; the script
 * refuses to measure the restore latency without that marker (it never substitutes a wall clock).
 *
 * Verified steps:
 *   1. reveal the line (second instance = the documented toggle) and assert it is visible;
 *   2. start the helper -> the line tucks and the window hides; assert the foreground branch
 *      (`foregroundCovers === true`) and, when QUNS does not suppress, that the reason is
 *      `foregroundCoversMonitor`;
 *   3. copy a new fixture image into the watched folder while full screen -> the item ingests and
 *      the webview shows the new card count, but the line must not reveal (deferred peek);
 *   4. stop the helper -> the pinned line is restored and the latency measured from the helper's
 *      close marker to panel-visible is asserted to be **< 1000 ms** (M4);
 *   5. negative cases: ordinary framed windows (plain maximized on the primary, maximized on the
 *      secondary, and manually covering the secondary monitor) and a borderless maximized window
 *      covering the secondary monitor must NOT hide the line, while the borderless synthetic
 *      full-screen helper does;
 *   6. `showOverFullScreen` false hides a pinned line even over the helper; enabling it while the
 *      line is pinned overrides the suppression (live setting read); disabling it hides again; a
 *      toggle while suppressed pins the line (idempotent) and stays hidden until the setting
 *      allows the override.
 *
 * Real D3D full-screen games, presentation mode, DRM/protected content and the QUNS D3D branch
 * remain MANUAL-PENDING; this run exercises the foreground-covers-monitor branch (the log records
 * which reason actually fired if the shell reported QUNS too).
 *
 * Evidence (JSON + panel-only screenshots) lands in `.superpowers/p4-evidence/fullscreen/`
 * (git-ignored). Only the panel page itself and synthetic fixtures are captured; the desktop,
 * the helper windows and user content are never captured.
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
import { basename, dirname, join, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const PORT = Number(process.env.HANG_CDP_PORT ?? 9228);
// `scripts/p2/cdp.mjs` reads HANG_CDP_PORT when the module is evaluated. This is a top-level
// import below so the env var is set first (the app is launched with the same port).
process.env.HANG_CDP_PORT = String(PORT);
const { connectTo, evaluate, sleep } = await import("../p2/cdp.mjs");
const EVIDENCE_DIR = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p4-evidence", "fullscreen");
const EXE = resolve(process.env.HANG_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"));
const HELPER = resolve("scripts", "p4", "fullscreen-window.ps1");
const REPO_TARGET = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "src-tauri", "target");
const POLL_MS = 50;
/** Tighter poll for latency assertions, so the measurement error stays well below the bound. */
const LATENCY_POLL_MS = 20;
const ITEM_WIDTH = 640;
const ITEM_HEIGHT = 360;
/** M4 restore bound: measured from the helper's close marker to panel-visible. */
const RESTORE_LIMIT_MS = 1000;
/** Suppression hide: 0.8 s debounce + poll + 0.3 s tuck animation, with scheduling margin. */
const HIDE_LIMIT_MS = 1500;

const base = resolve(process.env.HANG_P4_FS_BASE ?? join(tmpdir(), "hangline-p4-fullscreen"));
const localAppData = join(base, "localappdata");
const profile = join(localAppData, "Hangline");
const screenshotsDir = join(profile, "Screenshots");
const watchedDir = join(profile, "Watched");
// tauri-plugin-log's LogDir resolves the real `%LOCALAPPDATA%` known folder (SHGetKnownFolderPath),
// not the LOCALAPPDATA environment variable, so the app log is the standard one shared by the
// user's own Hangline instance. The harness starts reading at the current EOF below so stale
// lines from earlier sessions can never satisfy an assertion.
const logPath = join(
  process.env.LOCALAPPDATA ?? "",
  "app.hangline.Hangline",
  "logs",
  "Hangline.log",
);
const itemAPath = join(screenshotsDir, "p4-fullscreen-a.png");
const itemBPath = join(watchedDir, "p4-fullscreen-b.png");
const evidencePath = join(EVIDENCE_DIR, "fullscreen-e2e.json");

/**
 * The profile is disposable by construction: only `%TEMP%` or this repository's own
 * `src-tauri\target` may be cleared (the old "any path segment called target" check could clear an
 * unrelated directory).
 */
function assertDisposable(dir) {
  const resolved = resolve(dir).toLowerCase();
  const temp = resolve(tmpdir()).toLowerCase();
  const insideTemp = resolved === temp || resolved.startsWith(`${temp}${sep}`);
  const repoTarget = REPO_TARGET.toLowerCase();
  const insideTarget = resolved === repoTarget || resolved.startsWith(`${repoTarget}${sep}`);
  if (!insideTemp && !insideTarget) {
    console.error(
      `fullscreen-e2e: refusing to clear ${resolve(dir)} (must be under %TEMP% or this repo's src-tauri\\target)`,
    );
    process.exit(2);
  }
  if (basename(resolved) === "hangline") {
    console.error("fullscreen-e2e: HANG_P4_FS_BASE must be the run root, not the Hangline profile");
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
    const r = Math.round(28 + tint * 36 + 50 * t);
    const g = Math.round(64 + tint * 18 + 46 * (1 - t));
    const b = Math.round(92 + tint * 44 + 38 * (1 - t));
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

/** Writes the isolated profile: one owned item, an empty watched folder, suppression-friendly settings. */
function writeFixture() {
  rmSync(base, { recursive: true, force: true });
  mkdirSync(screenshotsDir, { recursive: true });
  mkdirSync(watchedDir, { recursive: true });
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(itemAPath, fixturePng(0));

  const now = Date.now();
  const settings = {
    captureHotkey: "Ctrl+Alt+S",
    toggleHotkey: "Ctrl+Alt+T",
    watchSystemScreenshots: false,
    watchClipboard: false,
    watchRecordings: false,
    extraFolders: [watchedDir],
    revealOnNew: true,
    revealPeekMs: 2500,
    topEdgeReveal: false,
    // Start with the spec default (off): a full-screen app must hide the line.
    showOverFullScreen: false,
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
      path: itemAPath,
      kind: "capture",
      hash: 4001,
      width: ITEM_WIDTH,
      height: ITEM_HEIGHT,
      createdAt: now - 5000,
      addedAt: now - 4000,
      updatedAt: now - 4000,
    },
  ];
  writeFileSync(join(profile, "state.json"), `${JSON.stringify(items, null, 2)}\n`);
}

/** Incremental log reader (the app writes through tauri-plugin-log; `.bak` rotation is ignored). */
class LogWatcher {
  constructor(path) {
    this.path = path;
    this.offset = 0;
    this.lines = [];
  }

  poll() {
    if (!existsSync(this.path)) return [];
    const data = readFileSync(this.path);
    if (data.length < this.offset) this.offset = 0; // rotated/truncated
    const chunk = data.subarray(this.offset).toString("utf8");
    this.offset = data.length;
    const fresh = chunk.split(/\r?\n/).filter((line) => line.length > 0);
    this.lines.push(...fresh);
    return fresh;
  }

  async wait(pattern, timeoutMs = 5000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      for (const line of this.poll()) {
        if (pattern.test(line)) return { line, at: Date.now() };
      }
      await sleep(POLL_MS);
    }
    return { line: null, at: Date.now() };
  }

  /** All lines seen so far that match `pattern` (never re-reads the file). */
  matches(pattern) {
    this.poll();
    return this.lines.filter((line) => pattern.test(line));
  }
}

const results = {
  startedAt: new Date().toISOString(),
  base,
  profile,
  cdpPort: PORT,
  exe: EXE,
  checks: {},
  timings: {},
  observations: {},
  negativeCases: {},
  steps: [],
  manualPending: [
    "Real D3D full-screen game (QUNS_RUNNING_D3D_FULL_SCREEN branch) — MANUAL-PENDING",
    "Presentation Settings / QUNS_PRESENTATION_MODE — MANUAL-PENDING",
    "DRM/protected full-screen video — MANUAL-PENDING",
    "Space/Alt+Tab foreground hand-off on a real desktop — MANUAL-PENDING",
  ],
};
let failures = 0;
let app = null;
let helper = null;
let panel = null;
const log = new LogWatcher(logPath);

function check(name, condition, detail) {
  results.checks[name] = { pass: Boolean(condition), detail };
  if (!condition) failures += 1;
  console.log(`${condition ? "PASS" : "FAIL"} ${name}${detail === undefined ? "" : `: ${JSON.stringify(detail)}`}`);
}

function step(name, detail) {
  results.steps.push({ name, at: new Date().toISOString(), detail });
  console.log(`step: ${name}`);
}

async function invoke(command, args = {}) {
  return evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

async function isRevealed() {
  return evaluate(
    panel,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
  );
}

/**
 * Installs a MutationObserver on the panel page that records every `data-revealed` transition
 * with the webview's `Date.now()`. The restore latency is measured from that DOM flip instead of a
 * 20 ms CDP poll, so the sampling error cannot eat into the 1 s M4 bound.
 */
async function installRevealObserver() {
  const installed = await evaluate(
    panel,
    `(() => {
      window.__hanglineRevealChanges = [];
      const record = (target) => {
        if (target instanceof HTMLElement && target.classList.contains("panel")) {
          window.__hanglineRevealChanges.push({
            value: target.dataset.revealed,
            at: Date.now(),
          });
        }
      };
      window.__hanglineRevealObserver = new MutationObserver((mutations) => {
        for (const mutation of mutations) record(mutation.target);
      });
      window.__hanglineRevealObserver.observe(document.body, {
        attributes: true,
        subtree: true,
        attributeFilter: ["data-revealed"],
      });
      const current = document.querySelector(".panel");
      if (current) record(current);
      return true;
    })()`,
  );
  check("panel_reveal_observer", installed === true);
}

/** The first `data-revealed === "true"` DOM flip at or after `sinceMs`, or 0. */
async function revealedAtOrAfter(sinceMs) {
  return evaluate(
    panel,
    `(() => {
      const changes = window.__hanglineRevealChanges || [];
      const hit = changes.find((change) => change.value === "true" && change.at >= ${sinceMs});
      return hit ? hit.at : 0;
    })()`,
  );
}

async function cardCount() {
  return evaluate(
    panel,
    `Number(document.querySelector(".panel")?.dataset.cardCount ?? -1)`,
  );
}

async function itemCount() {
  const state = await invoke("get_app_state");
  return Array.isArray(state?.items) ? state.items.length : -1;
}

async function setSetting(key, value) {
  await invoke("set_setting", { key, value });
}

async function waitFor(label, fn, predicate = (value) => Boolean(value), timeoutMs = 5000, pollMs = POLL_MS) {
  const started = Date.now();
  let last;
  while (Date.now() - started < timeoutMs) {
    try {
      last = await fn();
    } catch {
      last = undefined;
    }
    if (predicate(last)) {
      const at = Date.now();
      return { ok: true, value: last, elapsedMs: at - started, at };
    }
    await sleep(pollMs);
  }
  return { ok: false, value: last, elapsedMs: Date.now() - started, at: Date.now() };
}

async function waitRevealed(expected, timeoutMs = 5000, pollMs = POLL_MS) {
  return waitFor(
    `panel revealed=${expected}`,
    () => isRevealed(),
    (value) => value === expected,
    timeoutMs,
    pollMs,
  );
}

async function screenshot(name) {
  try {
    const captured = await panel.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    });
    writeFileSync(join(EVIDENCE_DIR, name), Buffer.from(captured.data, "base64"));
    return true;
  } catch (error) {
    console.warn(`fullscreen-e2e: screenshot ${name} failed: ${error}`);
    return false;
  }
}

/** Returns whether `window` covers `target` on all four sides (2 px slack, like the app). */
function coversRect(window, target, tolerance = 2) {
  if (!window || !target) return false;
  return (
    window.x <= target.x + tolerance &&
    window.y <= target.y + tolerance &&
    window.x + window.w >= target.x + target.w - tolerance &&
    window.y + window.h >= target.y + target.h - tolerance
  );
}

/**
 * Runs the helper in `probe` mode: QUNS + LogonUI + foreground class without showing a window.
 * Used to fail fast on a locked workstation (no user window can take the foreground there).
 */
function probeWorkstation() {
  const readyPath = join(base, "workstation-probe.json");
  const stopPath = join(base, "workstation-probe.stop");
  rmSync(readyPath, { force: true });
  rmSync(stopPath, { force: true });
  mkdirSync(base, { recursive: true });
  const run = spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      HELPER,
      "-Mode",
      "probe",
      "-ReadyPath",
      readyPath,
      "-StopPath",
      stopPath,
      "-Seconds",
      "5",
    ],
    { stdio: "ignore", timeout: 30000, windowsHide: true },
  );
  if (run.error) {
    throw new Error(`the workstation probe could not run: ${run.error.message}`);
  }
  if (run.status !== 0 || !existsSync(readyPath)) {
    throw new Error(
      `the workstation probe failed (exit ${run.status}${run.signal ? `, signal ${run.signal}` : ""})`,
    );
  }
  return JSON.parse(readFileSync(readyPath, "utf8"));
}

/** Launches the app with the isolated profile + CDP port; logs go to the profile's log file. */
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
  app.on("error", (error) => console.error("fullscreen-e2e: app spawn error", error));
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

/** Starts one synthetic window; resolves once it is really the foreground window. */
async function startHelper(tag, mode = "fullscreen") {
  const readyPath = join(base, `helper-${tag}-ready.json`);
  const stopPath = join(base, `helper-${tag}-stop`);
  const statusPath = join(base, `helper-${tag}-status.json`);
  rmSync(readyPath, { force: true });
  rmSync(stopPath, { force: true });
  rmSync(`${stopPath}.closed`, { force: true });
  rmSync(statusPath, { force: true });
  const child = spawn(
    "powershell.exe",
    [
      "-NoProfile",
      "-STA",
      "-WindowStyle",
      "Hidden",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      HELPER,
      "-Mode",
      mode,
      "-ReadyPath",
      readyPath,
      "-StopPath",
      stopPath,
      "-StatusPath",
      statusPath,
      "-Seconds",
      "90",
    ],
    { stdio: "ignore", windowsHide: true },
  );
  child.on("error", (error) => console.error("fullscreen-e2e: helper spawn error", error));
  const ready = await waitFor(
    "helper ready",
    () => (existsSync(readyPath) ? JSON.parse(readFileSync(readyPath, "utf8")) : null),
    (value) => value && (Number.isFinite(value.foregroundAt) || Boolean(value.error)),
    15000,
  );
  if (!ready.ok) {
    const status = existsSync(statusPath)
      ? JSON.parse(readFileSync(statusPath, "utf8"))
      : null;
    results.observations[`helper_${tag}_failed_status`] = status;
    try {
      child.kill();
    } catch {
      /* already gone */
    }
    throw new Error(
      `the synthetic ${mode} window did not become foreground within 15 s ` +
        `(child ${child.pid}, last status ${JSON.stringify(status)}). ` +
        `If foregroundClass is "Windows.UI.Core.CoreWindow"/LockApp or LogonUI.exe is running, ` +
        `the workstation is locked and no user window can take the foreground; unlock and rerun.`,
    );
  }
  if (ready.value.error) {
    throw new Error(`the synthetic ${mode} window could not be created: ${ready.value.error}`);
  }
  helper = { child, stopPath, readyPath, statusPath, info: ready.value, tag, mode };
  results.observations[`helper_${tag}`] = ready.value;
  console.log(
    `helper: ${mode}/${tag} foreground at ${ready.value.foregroundAt} (hwnd ${ready.value.hwnd}, ` +
      `style=${ready.value.style}, showCmd=${ready.value.showCmd}, borderless=${ready.value.borderless})`,
  );
  return helper;
}

/**
 * Stops the helper through its stop file and returns the epoch milliseconds at which it started
 * closing. The `.closed` marker is the only trustworthy t0 for the restore latency: if the helper
 * is killed or crashes without it, this throws instead of substituting a wall-clock time.
 */
async function stopHelper() {
  if (!helper) return null;
  const current = helper;
  helper = null;
  writeFileSync(current.stopPath, "stop");
  const exited = await Promise.race([
    new Promise((done) => current.child.once("exit", () => done(true))),
    sleep(4000).then(() => false),
  ]);
  if (!exited) {
    try {
      current.child.kill();
    } catch {
      /* already gone */
    }
    throw new Error(
      `the synthetic ${current.mode} window did not stop within 4 s; the restore latency cannot ` +
        `be measured from a trustworthy close timestamp`,
    );
  }
  const closedPath = `${current.stopPath}.closed`;
  if (!existsSync(closedPath)) {
    throw new Error(
      `the synthetic ${current.mode} window exited without writing its .closed marker; refusing ` +
        `to substitute a wall-clock time for the restore measurement`,
    );
  }
  const closedAt = Number(readFileSync(closedPath, "utf8"));
  if (!Number.isFinite(closedAt) || closedAt <= 0) {
    throw new Error(
      `the synthetic ${current.mode} window wrote an invalid .closed marker ` +
        `(${JSON.stringify(readFileSync(closedPath, "utf8"))})`,
    );
  }
  console.log(`helper: ${current.tag} stopped (closedAt=${closedAt})`);
  return closedAt;
}

/** Stops the current helper (if any), then kills the app. Never masks the main failure. */
async function cleanup() {
  try {
    await stopHelper();
  } catch (error) {
    console.error("fullscreen-e2e: helper cleanup failed:", error);
  }
  if (app && app.exitCode === null) {
    try {
      app.kill();
    } catch {
      /* already gone */
    }
  }
}

function finish(verdict) {
  results.finishedAt = new Date().toISOString();
  results.verdict = verdict;
  results.failureCount = failures;
  log.poll();
  results.logTail = log.lines.slice(-100);
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(evidencePath, `${JSON.stringify(results, null, 2)}\n`);
  console.log(`evidence: ${evidencePath}`);
  console.log(`verdict: ${verdict} (${Object.keys(results.checks).length} checks, ${failures} failed)`);
}

/**
 * One negative case: an ordinary window must not suppress the pinned line. `expectCover` asserts
 * the geometry really covers the monitor (so the old rect-only rule would have hidden the line).
 */
async function assertNoSuppression(tag, mode, label, expectCover) {
  step(`5. negative: ${label}`);
  const before = log.matches(/fullscreen: suppression on/).length;
  const launched = await startHelper(tag, mode);
  // A wrong suppress would flip after the 0.8 s debounce (+ one 100 ms poll); wait with margin.
  await sleep(1400);
  const revealed = await isRevealed();
  const after = log.matches(/fullscreen: suppression on/).length;
  const info = launched.info;
  const covered = coversRect(info.bounds, info.monitor, 2);
  results.negativeCases[tag] = {
    mode,
    label,
    borderless: info.borderless,
    maximizedPlacement: info.maximizedPlacement,
    showCmd: info.showCmd,
    bounds: info.bounds,
    monitor: info.monitor,
    coversMonitor: covered,
  };
  check(`5_${tag}_line_visible`, revealed === true, { revealed });
  check(`5_${tag}_no_suppression`, after === before, { suppressionBefore: before, suppressionAfter: after });
  if (expectCover) {
    check(`5_${tag}_covers_monitor`, covered === true, {
      bounds: info.bounds,
      monitor: info.monitor,
    });
    check(
      `5_${tag}_geometry_is_the_false_positive`,
      mode === "borderless-max"
        ? info.borderless === true && info.maximizedPlacement === true
        : info.borderless === false && info.maximizedPlacement === false,
      {
        borderless: info.borderless,
        maximizedPlacement: info.maximizedPlacement,
        showCmd: info.showCmd,
      },
    );
  }
  await stopHelper();
  await waitRevealed(true, 1500);
}

async function main() {
  assertDisposable(base);
  const workstation = probeWorkstation();
  results.workstation = workstation;
  if (workstation.logonUi || workstation.quns === 1) {
    throw new Error(
      `the workstation is locked (QUNS=${workstation.quns}, LogonUI=${workstation.logonUi}, ` +
        `foreground=${workstation.foregroundClass}). On the secure desktop no user window can take ` +
        `the foreground and QUNS is forced to 1, so full-screen suppression cannot be measured; ` +
        `unlock the session and rerun \`node scripts/p4/fullscreen-e2e.mjs\`.`,
    );
  }
  if ([2, 3, 4].includes(workstation.quns)) {
    throw new Error(
      `the shell already reports a full-screen/presentation state before the run ` +
        `(QUNS=${workstation.quns}, foreground=${workstation.foregroundClass}); close the ` +
        `full-screen app / disable Presentation Settings and rerun`,
    );
  }

  writeFixture();
  results.logPath = logPath;
  if (existsSync(logPath)) log.offset = statSync(logPath).size;
  step("profile fixture", { profile, itemA: itemAPath, watched: watchedDir });
  launchApp();

  panel = await connectTo("window=panel", 45000);
  await waitFor("panel DOM", () => evaluate(panel, `!!document.querySelector(".panel")`), Boolean, 30000);
  check("panel_started", true);
  await installRevealObserver();
  if (app.exitCode !== null) {
    throw new Error(
      "the app exited during startup; another Hangline instance may already own the single-instance lock",
    );
  }
  const pollerStarted = await log.wait(/fullscreen: poller started/, 8000);
  results.observations.pollerStarted = pollerStarted.line;
  check("poller_started", Boolean(pollerStarted.line), { line: pollerStarted.line });
  await sleep(800);

  // A deterministic hidden baseline: the startup reveal tucks on the first pointer-leave tick;
  // if the cursor happens to sit in the trigger band, toggle once explicitly.
  const startupHidden = await waitRevealed(false, 4000);
  if (!startupHidden.ok) {
    toggleLine();
    await waitRevealed(false, 4000);
  }
  log.poll();

  // 1. Reveal and assert visible.
  step("1. reveal the line with a second instance");
  toggleLine();
  const reveal = await waitRevealed(true, 5000);
  check("1_reveal_visible", reveal.ok, { elapsedMs: reveal.elapsedMs });
  results.timings.revealMs = reveal.elapsedMs;
  results.observations.firstReveal = log.matches(/reveal: shown/).slice(-1)[0] ?? null;
  const shownBeforeFullscreen = log.matches(/reveal: shown/).length;
  await screenshot("panel-01-visible.png");
  const cards = await cardCount();
  check("1_cards_present", cards === 1, { cards });

  // 2. Synthetic full screen -> the line tucks and the window hides.
  step("2. start the synthetic full-screen window");
  const firstHelper = await startHelper("suppress", "fullscreen");
  const t0 = firstHelper.info.foregroundAt;
  const tuck = await waitRevealed(false, 3000, LATENCY_POLL_MS);
  const tuckMs = tuck.at - t0;
  results.timings.suppressTuckMs = tuckMs;
  check("2_panel_tucked", tuck.ok && tuckMs <= HIDE_LIMIT_MS, { tuckMs, limitMs: HIDE_LIMIT_MS });
  const hidden = await log.wait(/reveal: hidden at .*tuck animation finished/, 4000);
  const hiddenMs = hidden.at - t0;
  results.timings.suppressWindowHiddenMs = hiddenMs;
  check("2_window_hidden", Boolean(hidden.line) && hiddenMs <= HIDE_LIMIT_MS, {
    hiddenMs,
    limitMs: HIDE_LIMIT_MS,
  });
  // The webview is the visibility source of truth: `waitRevealed` reads `data-revealed`.
  check("2_panel_webview_hidden", (await isRevealed()) === false);
  const suppressionOn = log.matches(/fullscreen: suppression on/).slice(-1)[0] ?? null;
  const tuckLine = log.matches(/reveal: tuck \(Fullscreen\)/).slice(-1)[0] ?? null;
  results.observations.suppressionOn = suppressionOn;
  results.observations.tuck = tuckLine;
  check("2_suppression_logged", Boolean(suppressionOn), { line: suppressionOn });
  check("2_tuck_reason_fullscreen", Boolean(tuckLine), { line: tuckLine });
  const reasonMatch = suppressionOn?.match(/reason (\w+), quns=(\d+), foreground_covers=(true|false)/);
  if (reasonMatch) {
    results.observations.reason = reasonMatch[1];
    results.observations.quns = Number(reasonMatch[2]);
    results.observations.foregroundCovers = reasonMatch[3] === "true";
  }
  // The foreground branch must really have fired — a suppression log line alone is not proof.
  check("2_foreground_probe_true", results.observations.foregroundCovers === true, {
    foregroundCovers: results.observations.foregroundCovers,
    reason: results.observations.reason,
  });
  const qunsSuppresses = [2, 3, 4].includes(results.observations.quns);
  check(
    "2_reason_matches_branch",
    qunsSuppresses
      ? results.observations.reason === "queryUserNotificationState"
      : results.observations.reason === "foregroundCoversMonitor",
    {
      quns: results.observations.quns,
      reason: results.observations.reason,
      expected: qunsSuppresses ? "queryUserNotificationState" : "foregroundCoversMonitor",
    },
  );
  await screenshot("panel-02-suppressed.png");

  // 3. A new item arrives while full screen: it ingests, the line must not reveal.
  step("3. new watched-folder item while full screen");
  writeFileSync(itemBPath, fixturePng(1));
  const ingested = await waitFor("item ingested", () => itemCount(), (count) => count >= 2, 15000);
  check("3_new_item_ingested", ingested.ok, { items: ingested.value, elapsedMs: ingested.elapsedMs });
  const hiddenAfterIngest = await waitRevealed(false, 1500);
  check("3_line_still_hidden", hiddenAfterIngest.ok, { elapsedMs: hiddenAfterIngest.elapsedMs });
  const cardsAfterIngest = await cardCount();
  check("3_webview_has_new_card", cardsAfterIngest === 2, { cards: cardsAfterIngest });
  const deferred = await log.wait(/new item deferred while full-screen suppression/, 3000);
  check("3_deferred_peek_logged", Boolean(deferred.line), { line: deferred.line });
  const shownWhileFull = log.matches(/reveal: shown/).length;
  results.observations.shownEventsWhileFullscreen = shownWhileFull;
  check(
    "3_no_reveal_while_fullscreen",
    shownWhileFull === shownBeforeFullscreen,
    { shownBeforeFullscreen, shownWhileFull },
  );
  await screenshot("panel-03-new-item-hidden.png");

  // 4. Stop the helper -> the pinned line comes back within the M4 1 s bound, measured from the
  //    helper's close marker (never from the request time, never from a wall-clock substitute).
  step("4. stop the synthetic full-screen window");
  const stopRequestedAt = Date.now();
  const closedAt = await stopHelper();
  const restored = await waitRevealed(true, 4000, LATENCY_POLL_MS);
  const domRevealedAt = await revealedAtOrAfter(stopRequestedAt);
  const visibleAt = domRevealedAt > 0 ? domRevealedAt : restored.at;
  const restoreMs = visibleAt - closedAt;
  const stopToVisibleMs = visibleAt - stopRequestedAt;
  results.observations.restoreTimestampSource =
    domRevealedAt > 0 ? "panel DOM MutationObserver" : "CDP poll fallback";
  results.timings.restoreAfterCloseMs = restoreMs;
  results.timings.restoreFromStopRequestMs = stopToVisibleMs;
  check("4_restored_within_m4_bound", restored.ok && restoreMs <= RESTORE_LIMIT_MS, {
    restoreMs,
    limitMs: RESTORE_LIMIT_MS,
    measuredFrom: "helper .closed marker (after the native window was destroyed)",
    timestampSource: results.observations.restoreTimestampSource,
  });
  const suppressionOff = await log.wait(/full-screen suppression off/, 3000);
  check("4_suppression_off_logged", Boolean(suppressionOff.line), { line: suppressionOff.line });
  const restoredLine = log.matches(/restored the pinned line after full-screen/).slice(-1)[0] ?? null;
  check("4_restore_logged", Boolean(restoredLine), { line: restoredLine });
  await screenshot("panel-04-restored.png");
  await sleep(1000);
  check("4_restored_stays_visible", (await isRevealed()) === true);

  // 5. Negative cases: ordinary windows must never hide the line, while the borderless full-screen
  //    helper does. Both monitors currently have a taskbar, so a plain maximize cannot cover a
  //    monitor; `covering` and `borderless-max` reproduce the taskbar-less geometry explicitly.
  await assertNoSuppression("maximized", "maximized", "plain maximized framed window on the primary", false);
  await assertNoSuppression(
    "maximized-secondary",
    "maximized-secondary",
    "maximized framed window on the secondary monitor",
    false,
  );
  await assertNoSuppression(
    "covering",
    "covering",
    "ordinary framed window covering the secondary monitor",
    true,
  );
  await assertNoSuppression(
    "borderless-max",
    "borderless-max",
    "borderless maximized window covering the secondary monitor",
    true,
  );

  // 6. showOverFullScreen=false hides a pinned line; enabling it live overrides the suppression.
  step("6a. pinned line over the helper with showOverFullScreen=false");
  const secondHelper = await startHelper("override", "fullscreen");
  const t6 = secondHelper.info.foregroundAt;
  const hidden6 = await waitRevealed(false, 3000, LATENCY_POLL_MS);
  results.timings.settingOffHideMs = hidden6.at - t6;
  check("6_setting_off_hides", hidden6.ok, { elapsedMs: hidden6.at - t6 });

  step("6b. enable showOverFullScreen while suppressed and pinned");
  const enableAt = Date.now();
  await setSetting("showOverFullScreen", true);
  const override = await waitRevealed(true, 4000, LATENCY_POLL_MS);
  results.timings.settingOnOverrideMs = override.at - enableAt;
  check("6_setting_on_overrides", override.ok, { elapsedMs: override.at - enableAt });
  await screenshot("panel-05-override.png");

  step("6c. disable showOverFullScreen again -> hidden");
  const disableAt = Date.now();
  await setSetting("showOverFullScreen", false);
  const rehidden = await waitRevealed(false, 4000, LATENCY_POLL_MS);
  results.timings.settingOffRehideMs = rehidden.at - disableAt;
  check("6_setting_off_rehides", rehidden.ok, { elapsedMs: rehidden.at - disableAt });

  step("6d. suppressed toggles pin the line (idempotent) and stay hidden with the setting off");
  const togglesBefore = log.matches(/toggle while full-screen suppressed/).length;
  toggleLine();
  await sleep(900);
  check("6_toggle_stays_hidden", (await isRevealed()) === false);
  toggleLine();
  await sleep(900);
  check("6_second_toggle_stays_hidden", (await isRevealed()) === false);
  const toggleLines = log.matches(/toggle while full-screen suppressed/).slice(-2);
  results.observations.suppressedToggles = toggleLines;
  check(
    "6_toggles_pin_the_line",
    toggleLines.length === 2 &&
      toggleLines.every((line) => line.includes("pinned=true")) &&
      log.matches(/toggle while full-screen suppressed/).length === togglesBefore + 2,
    { lines: toggleLines },
  );

  step("6e. enable showOverFullScreen -> the pinned toggle shows while still full screen");
  const enable2At = Date.now();
  await setSetting("showOverFullScreen", true);
  const override2 = await waitRevealed(true, 4000, LATENCY_POLL_MS);
  results.timings.pinnedToggleOverrideMs = override2.at - enable2At;
  check("6_pinned_toggle_overrides", override2.ok, { elapsedMs: override2.at - enable2At });

  step("6f. stop the helper; the pinned line stays visible");
  await stopHelper();
  await sleep(1200);
  check("6_stays_visible_after_helper", (await isRevealed()) === true);
  await screenshot("panel-06-final.png");

  const finalItems = await itemCount();
  check("all_items_present", finalItems === 2, { items: finalItems });
}

try {
  await main();
  finish(failures === 0 ? "PASS" : "FAIL");
} catch (error) {
  failures += 1;
  results.error = error instanceof Error ? `${error.message}\n${error.stack}` : String(error);
  console.error("fullscreen-e2e: fatal:", results.error);
  finish("FAIL");
} finally {
  try {
    await cleanup();
  } catch (error) {
    console.error("fullscreen-e2e: cleanup failed:", error);
  }
}
process.exit(failures === 0 ? 0 : 1);
