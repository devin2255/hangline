#!/usr/bin/env node
/* global Buffer, fetch, AbortSignal */
/**
 * P4-4 runtime evidence: M15 hotkey conflict, rollback and retry (programmatic part).
 *
 * Requires (dev-only, no dependencies beyond Node's built-ins):
 *   1. `pnpm dev` (Vite on http://localhost:5173) running in another terminal;
 *   2. a debug build with this task's fixes: `cargo build` in `src-tauri/`
 *      (or `pnpm tauri dev` once);
 *   3. no other Hangline instance running (single-instance would swallow the launch).
 *
 *   node scripts/p4/hotkey-conflict-e2e.mjs
 *
 * The script creates an isolated profile under `%TEMP%\hangline-p4-hotkey` (settings/state plus one
 * neutral synthetic PNG), launches the debug exe with `HANG_CDP_PORT=9230`, and drives the real IPC
 * path from the panel page through CDP (`set_hotkey` is the command the change-shortcut window
 * uses). Conflicts are produced with `scripts/p4/hotkey-holder.ps1`: a helper process registers a
 * combination with `RegisterHotKey` and holds it; a failed probe with
 * `ERROR_HOTKEY_ALREADY_REGISTERED` (1409) proves that somebody else currently owns the key.
 *
 * Verified steps (one app instance; no restart, so a killed WebView2 host can never race the next
 * start):
 *   1. a helper already owns Ctrl+Alt+T when the app starts: the toggle registration fails (logged),
 *      the persisted setting is kept, the capture shortcut still registers, and the app answers
 *      IPC;
 *   2. after the helper releases, saving the *same* value retries and registers (M15
 *      "改键窗口可重试成功" — `set_hotkey` handles an unchanged value as a registration retry);
 *   3. baseline: both accelerators are owned by Hangline (a probe cannot register either);
 *   4. helper holds Ctrl+Alt+Shift+F9 -> `set_hotkey toggle` returns a conflict error, settings and
 *      settings.json are unchanged, the old Ctrl+Alt+T is still owned by Hangline, the app answers
 *      IPC, and the log records the failure + rollback;
 *   5. after the helper releases, the same change succeeds: persisted, old key released (a probe can
 *      now register it), new key owned; a repeated save is a retry (registration attempted, no
 *      duplicate registration);
 *   6. the original accelerator is restored;
 *   7. the real shortcut page (loaded in the panel webview through `?window=shortcut`, because the
 *      locked workstation cannot open the tray menu — see below) shows the current accelerator,
 *      captures a combination, displays the conflict inline, and saves it after the helper exits.
 *
 * BLOCKED on a locked workstation (LogonUI owns the secure desktop): opening the change-shortcut
 * window through the tray menu, and pressing the real global hotkey on the keyboard. This script
 * uses the page-level substitute (navigate the panel webview to the same URL the window uses) and
 * registration-ownership probes; both are labelled in the evidence JSON. Rerun unlocked:
 *
 *   node scripts/p4/hotkey-conflict-e2e.mjs   # with the workstation unlocked
 *
 * Evidence (JSON + app-UI-only screenshots) lands in `.superpowers/p4-evidence/hotkey/`
 * (git-ignored). Only the app's own page is captured; the desktop and other windows never are.
 */

import { execFileSync, spawn } from "node:child_process";
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const PORT = Number(process.env.HANG_CDP_PORT ?? 9230);
// `scripts/p2/cdp.mjs` reads HANG_CDP_PORT when the module is evaluated: set it before the import.
process.env.HANG_CDP_PORT = String(PORT);
const { connectTo, evaluate, sleep, targets } = await import("../p2/cdp.mjs");

const EVIDENCE_DIR = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p4-evidence", "hotkey");
const EXE = resolve(process.env.HANG_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"));
const HOLDER = resolve("scripts", "p4", "hotkey-holder.ps1");
const REPO_TARGET = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "src-tauri", "target");
const POLL_MS = 50;
const ITEM_WIDTH = 640;
const ITEM_HEIGHT = 360;
/** The combination the helper holds for the rollback step (free by default). */
const BUSY_ACCELERATOR = "Ctrl+Alt+Shift+F9";
/** The combination captured in the shortcut page before the save is retried. */
const PAGE_ACCELERATOR = "Ctrl+Alt+Shift+F5";
const DEFAULT_CAPTURE = "Ctrl+Alt+S";
const DEFAULT_TOGGLE = "Ctrl+Alt+T";
const MOD_ALT = 1;
const MOD_CTRL = 2;
const MOD_SHIFT = 8;

const base = resolve(process.env.HANG_P4_HOTKEY_BASE ?? join(tmpdir(), "hangline-p4-hotkey"));
const localAppData = join(base, "localappdata");
const profile = join(localAppData, "Hangline");
const screenshotsDir = join(profile, "Screenshots");
const itemPath = join(screenshotsDir, "p4-hotkey-item.png");
const settingsPath = join(profile, "settings.json");
const statePath = join(profile, "state.json");
const evidencePath = join(EVIDENCE_DIR, "hotkey-conflict-e2e.json");
// The app logs to stdout and to a rotating file in the real `%LOCALAPPDATA%` log directory
// (tauri-plugin-log resolves that known folder, not the environment variable). The file is
// KeepOne-rotated at 40 KB — it is *deleted* when it grows past that mid-run — so the harness
// captures the app's own stdout instead and keeps it as evidence.
const appStdoutPath = join(base, "app-stdout.log");
const appStderrPath = join(base, "app-stderr.log");
const sharedLogPath = join(
  process.env.LOCALAPPDATA ?? "",
  "app.hangline.Hangline",
  "logs",
  "Hangline.log",
);

const results = {
  startedAt: new Date().toISOString(),
  base,
  profile,
  cdpPort: PORT,
  exe: EXE,
  busyAccelerator: BUSY_ACCELERATOR,
  checks: {},
  timings: {},
  observations: {},
  steps: [],
  manualPending: [
    "Press the real Ctrl+Alt+T on the keyboard after a conflict and watch the line toggle — MANUAL-PENDING (needs an interactive desktop; this run verifies registration ownership instead)",
    "Open the change-shortcut window from the tray menu — BLOCKED on a locked workstation; rerun unlocked (this run drives the same page + IPC through CDP)",
    "Start QQ/another hotkey app first, then launch Hangline — MANUAL-PENDING (helper RegisterHotKey process is the programmatic stand-in)",
  ],
};
let failures = 0;
let app = null;
let panel = null;
let panelUrl = null;
let holder = null;
let appRuns = 0;

/** Escapes a literal string (accelerators contain `+`, a regex quantifier). */
const literal = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function check(name, condition, detail) {
  results.checks[name] = { pass: Boolean(condition), detail };
  if (!condition) failures += 1;
  console.log(
    `${condition ? "PASS" : "FAIL"} ${name}${detail === undefined ? "" : `: ${JSON.stringify(detail)}`}`,
  );
}

function step(name, detail) {
  results.steps.push({ name, at: new Date().toISOString(), detail });
  console.log(`step: ${name}`);
}

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
      `hotkey-e2e: refusing to clear ${resolve(dir)} (must be under %TEMP% or this repo's src-tauri\\target)`,
    );
    process.exit(2);
  }
  if (basename(resolved) === "hangline") {
    console.error("hotkey-e2e: HANG_P4_HOTKEY_BASE must be the run root, not the Hangline profile");
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
function fixturePng() {
  const stride = ITEM_WIDTH * 3 + 1;
  const raw = Buffer.alloc(stride * ITEM_HEIGHT);
  for (let y = 0; y < ITEM_HEIGHT; y += 1) {
    const row = y * stride;
    raw[row] = 0; // filter: none
    const t = y / (ITEM_HEIGHT - 1);
    const rowBytes = raw.subarray(row + 1, row + stride);
    for (let x = 0; x < ITEM_WIDTH; x += 1) {
      rowBytes[x * 3] = Math.round(36 + 60 * t);
      rowBytes[x * 3 + 1] = Math.round(70 + 40 * (1 - t));
      rowBytes[x * 3 + 2] = Math.round(96 + 30 * (1 - t));
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

/** Writes the isolated profile: one owned item, spec-default hotkeys, every watcher off. */
function writeFixture() {
  rmSync(base, { recursive: true, force: true });
  mkdirSync(screenshotsDir, { recursive: true });
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(itemPath, fixturePng());

  const now = Date.now();
  const settings = {
    captureHotkey: DEFAULT_CAPTURE,
    toggleHotkey: DEFAULT_TOGGLE,
    watchSystemScreenshots: false,
    watchClipboard: false,
    watchRecordings: false,
    extraFolders: [],
    revealOnNew: true,
    revealPeekMs: 2500,
    topEdgeReveal: false,
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
  writeFileSync(settingsPath, `${JSON.stringify(settings, null, 2)}\n`);
  const items = [
    {
      id: "22222222222222222222222222222222",
      path: itemPath,
      kind: "capture",
      hash: 4104,
      width: ITEM_WIDTH,
      height: ITEM_HEIGHT,
      createdAt: now - 5000,
      addedAt: now - 4000,
      updatedAt: now - 4000,
    },
  ];
  writeFileSync(statePath, `${JSON.stringify(items, null, 2)}\n`);
}

/**
 * Incremental log reader (the app writes through tauri-plugin-log; `.bak` rotation is ignored).
 *
 * Only complete lines are consumed: the writer appends while this runs, so a read can end in the
 * middle of a record. The trailing partial record is kept as bytes and re-joined with the next read
 * (dropping it would lose that log line for the rest of the run).
 */
class LogWatcher {
  constructor(path) {
    this.path = path;
    this.offset = 0;
    this.partial = Buffer.alloc(0);
    this.lines = [];
  }

  poll() {
    if (!existsSync(this.path)) return [];
    const data = readFileSync(this.path);
    if (data.length < this.offset) {
      this.offset = 0; // rotated/truncated
      this.partial = Buffer.alloc(0);
    }
    const fresh = data.subarray(this.offset);
    this.offset = data.length;
    const buffer = this.partial.length > 0 ? Buffer.concat([this.partial, fresh]) : fresh;
    const lastNewline = buffer.lastIndexOf(0x0a);
    if (lastNewline < 0) {
      this.partial = Buffer.from(buffer);
      return [];
    }
    this.partial = Buffer.from(buffer.subarray(lastNewline + 1));
    const complete = buffer.subarray(0, lastNewline).toString("utf8");
    const lines = complete.split(/\r?\n/).filter((line) => line.length > 0);
    this.lines.push(...lines);
    return lines;
  }

  /**
   * Waits for a matching line, including lines already polled earlier in the run (a previous check
   * may have consumed the chunk that contained it). Use [`waitCount`] when a *new* occurrence
   * matters (e.g. a retried registration that logged the same text before).
   */
  async wait(pattern, timeoutMs = 5000) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const seen = this.matches(pattern)[0];
      if (seen) return { line: seen, at: Date.now() };
      if (Date.now() >= deadline) return { line: null, at: Date.now() };
      await sleep(POLL_MS);
    }
  }

  /** Waits until at least `minCount` accumulated lines match (for restarted app instances). */
  async waitCount(pattern, minCount, timeoutMs = 5000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      this.poll();
      const matches = this.matches(pattern);
      if (matches.length >= minCount) return { line: matches.at(-1), count: matches.length };
      await sleep(POLL_MS);
    }
    return { line: null, count: this.matches(pattern).length };
  }

  /** All lines seen so far that match `pattern` (never re-reads the file). */
  matches(pattern) {
    this.poll();
    return this.lines.filter((line) => pattern.test(line));
  }

  count(pattern) {
    return this.matches(pattern).length;
  }
}

const log = new LogWatcher(appStdoutPath);

async function invoke(command, args = {}) {
  return evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

/** Same call the change-shortcut window makes; resolves `{ok, value, error, ms}`. */
async function setHotkey(target, accelerator) {
  const started = Date.now();
  const outcome = await evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke("set_hotkey", ${JSON.stringify({ target, accelerator })})
      .then((value) => ({ ok: true, value }))
      .catch((error) => ({ ok: false, error: String(error) }))`,
  );
  return { ...outcome, ms: Date.now() - started };
}

async function settings() {
  const state = await invoke("get_app_state");
  return state?.settings ?? null;
}

function readSettingsFile() {
  return JSON.parse(readFileSync(settingsPath, "utf8"));
}

async function waitFor(label, fn, predicate = (value) => Boolean(value), timeoutMs = 5000) {
  const started = Date.now();
  let last;
  while (Date.now() - started < timeoutMs) {
    try {
      last = await fn();
    } catch {
      last = undefined;
    }
    if (predicate(last)) return { ok: true, value: last, elapsedMs: Date.now() - started };
    await sleep(POLL_MS);
  }
  return { ok: false, value: last, elapsedMs: Date.now() - started };
}

async function screenshot(name) {
  try {
    const captured = await panel.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    });
    writeFileSync(join(EVIDENCE_DIR, name), Buffer.from(captured.data, "base64"));
    results.observations[`screenshot_${name}`] = true;
    return true;
  } catch (error) {
    results.observations[`screenshot_${name}`] = false;
    results.screenshotError = String(error);
    console.warn(`hotkey-e2e: screenshot ${name} failed: ${error}`);
    return false;
  }
}

async function waitChildExit(child, timeoutMs) {
  if (child.exitCode !== null) return true;
  const exited = await Promise.race([
    new Promise((done) => child.once("exit", () => done(true))),
    sleep(timeoutMs).then(() => false),
  ]);
  if (!exited) {
    try {
      child.kill();
    } catch {
      /* already gone */
    }
  }
  return exited;
}

/** Refuses to run while a Hangline instance already owns the single-instance lock. */
function assertNoOtherHangline() {
  const count = Number(
    execFileSync("powershell.exe", [
      "-NoProfile",
      "-Command",
      "@(Get-Process hangline -ErrorAction SilentlyContinue).Count",
    ])
      .toString()
      .trim(),
  );
  if (count > 0) {
    throw new Error(
      `${count} hangline.exe process(es) already run; close the user's instance before this E2E ` +
        "(the harness never kills a process it did not start). If it is a leftover instance from an " +
        "earlier evidence run, stop it with: Get-Process hangline | Stop-Process -Force",
    );
  }
}

/**
 * PIDs of WebView2 helper processes that belong to Hangline's user-data folder.
 *
 * A force-killed app leaves them alive for a while; they keep the shared WebView2 user-data folder
 * locked and make the next start hang before `panel::create`. The harness refuses to run while a
 * real Hangline instance exists, so any match here is residue from its own (or a previous evidence)
 * run and safe to terminate.
 */
function webviewPids() {
  const raw = execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-Command",
      `@(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -like '*app.hangline.Hangline*' } | Select-Object -ExpandProperty ProcessId) -join ','`,
    ],
    { encoding: "utf8" },
  ).trim();
  return raw === "" ? [] : raw.split(",").map(Number).filter(Number.isFinite);
}

/**
 * Kills leftover WebView2 helpers and waits until none is left.
 *
 * Must only run while no Hangline process of this harness is alive.
 */
async function killStaleWebviews(reason) {
  const killed = [];
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const pids = webviewPids();
    if (pids.length === 0) {
      if (killed.length > 0) results.observations[reason] = killed;
      return { ok: true, killed, attempts: attempt };
    }
    for (const pid of pids) if (!killed.includes(pid)) killed.push(pid);
    try {
      execFileSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-Command",
          `Stop-Process -Id ${pids.join(",")} -Force -ErrorAction SilentlyContinue`,
        ],
        { stdio: "ignore" },
      );
    } catch {
      /* already gone */
    }
    await sleep(300);
  }
  results.observations[`${reason}_timeout`] = webviewPids();
  return { ok: false, killed, attempts: 40 };
}

/** Launches the app with the isolated profile + CDP port; the log goes to the shared log file. */
function launchApp() {
  if (!existsSync(EXE)) {
    throw new Error(
      `debug exe not found at ${EXE}; run \`cargo build\` in src-tauri (debug) before the E2E`,
    );
  }
  appRuns += 1;
  // Redirect the app's stdout/stderr to evidence files: tauri-plugin-log's stdout target keeps every
  // record (its file target rotates at 40 KB and would delete lines mid-run).
  const stdout = openSync(appStdoutPath, "a");
  const stderr = openSync(appStderrPath, "a");
  app = spawn(EXE, [], {
    env: { ...process.env, LOCALAPPDATA: localAppData, HANG_CDP_PORT: String(PORT) },
    stdio: ["ignore", stdout, stderr],
    windowsHide: false,
  });
  closeSync(stdout);
  closeSync(stderr);
  app.on("error", (error) => console.error("hotkey-e2e: app spawn error", error));
  console.log(`app run ${appRuns}: pid ${app.pid} with LOCALAPPDATA=${localAppData}`);
}

/** True when nothing listens on the debug port any more (a dying WebView2 may hold it briefly). */
async function cdpPortFree(port) {
  try {
    const response = await fetch(`http://127.0.0.1:${port}/json/version`, {
      signal: AbortSignal.timeout(500),
    });
    await response.arrayBuffer().catch(() => undefined);
    return false;
  } catch {
    return true;
  }
}

async function waitCdpPortFree(port, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await cdpPortFree(port)) return true;
    await sleep(250);
  }
  results.observations[`port_${port}_still_held`] = true;
  return false;
}

async function stopApp() {
  if (app && app.exitCode === null) {
    const current = app;
    app = null;
    // Kill the whole tree: the WebView2 browser process is a child of the app, and leaving it alive
    // would keep the user-data folder locked for the next start.
    try {
      execFileSync("taskkill", ["/PID", String(current.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {
      /* already gone */
    }
    await waitChildExit(current, 6000);
  }
  app = null;
  await killStaleWebviews("webviews_after_stop");
  await waitCdpPortFree(PORT, 10000);
  await sleep(400);
  return true;
}

/**
 * Launches and connects the panel, retrying a silent startup failure (a WebView2 host killed by the
 * previous run can make the next start exit before it creates a window; a fresh process succeeds).
 */
async function launchAndConnect() {
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    await killStaleWebviews("stale_webviews_before_launch");
    await waitCdpPortFree(PORT);
    launchApp();
    try {
      const visible = await waitFor(
        "panel CDP target",
        () => targets().then((list) => list.some((entry) => entry.type === "page" && entry.url.includes("window=panel"))).catch(() => false),
        Boolean,
        25000,
      );
      if (!visible.ok) throw new Error("the CDP endpoint never exposed a panel target");
      panel = await connectTo("window=panel", 20000);
      const dom = await waitFor(
        "panel DOM",
        () => evaluate(panel, `!!document.querySelector(".panel")`),
        Boolean,
        30000,
      );
      if (!dom.ok) throw new Error("the panel page never rendered");
      const panelTarget = (await targets()).find(
        (entry) => entry.type === "page" && entry.url.includes("window=panel"),
      );
      panelUrl = panelTarget?.url ?? null;
      results.observations.panelUrl = panelUrl;
      results.observations.launchAttempts = attempt;
      return attempt;
    } catch (error) {
      results.observations[`launch_attempt_${attempt}_failed`] = {
        error: String(error),
        exitCode: app?.exitCode ?? null,
        logTail: log.matches(/panicked|error while building|settings: loaded/).slice(-3),
      };
      console.error(`hotkey-e2e: launch attempt ${attempt} failed (${error}); retrying`);
      panel?.close();
      panel = null;
      await stopApp();
      await sleep(2500);
    }
  }
  throw new Error("the app never exposed the panel through CDP after 3 launch attempts");
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

/** Runs one registration probe; resolves the helper's ready payload ({ok, lastError}). */
async function probe(accelerator, tag) {
  const readyPath = join(base, `probe-${tag}-ready.json`);
  const stopPath = join(base, `probe-${tag}-stop`);
  const statusPath = join(base, `probe-${tag}-status.json`);
  for (const path of [readyPath, stopPath, statusPath]) rmSync(path, { force: true });
  const child = spawn(
    "powershell.exe",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-WindowStyle",
      "Hidden",
      "-File",
      HOLDER,
      "-Accelerator",
      accelerator,
      "-ReadyPath",
      readyPath,
      "-StopPath",
      stopPath,
      "-StatusPath",
      statusPath,
      "-Seconds",
      "20",
    ],
    { stdio: "ignore", windowsHide: true },
  );
  child.on("error", (error) => console.error("hotkey-e2e: probe spawn error", error));
  const ready = await waitFor(
    `probe ${tag}`,
    () => (existsSync(readyPath) ? JSON.parse(readFileSync(readyPath, "utf8")) : null),
    Boolean,
    15000,
  );
  if (!ready.ok) {
    try {
      child.kill();
    } catch {
      /* already gone */
    }
    throw new Error(`the registration probe for ${accelerator} did not report within 15 s`);
  }
  const info = ready.value;
  // A failed probe exits on its own; a successful one holds the key until the stop file appears.
  writeFileSync(stopPath, "stop");
  await waitChildExit(child, 4000);
  results.observations[`probe_${tag}`] = { accelerator, ...info };
  console.log(
    `probe ${tag}: ${accelerator} ok=${info.ok} lastError=${info.lastError}` +
      `${info.ok ? " (key was free)" : " (key is owned by another process)"}`,
  );
  return info;
}

async function startHolder(tag, accelerator) {
  const readyPath = join(base, `holder-${tag}-ready.json`);
  const stopPath = join(base, `holder-${tag}-stop`);
  const statusPath = join(base, `holder-${tag}-status.json`);
  for (const path of [readyPath, stopPath, statusPath]) rmSync(path, { force: true });
  const child = spawn(
    "powershell.exe",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-WindowStyle",
      "Hidden",
      "-File",
      HOLDER,
      "-Accelerator",
      accelerator,
      "-ReadyPath",
      readyPath,
      "-StopPath",
      stopPath,
      "-StatusPath",
      statusPath,
      "-Seconds",
      "180",
    ],
    { stdio: "ignore", windowsHide: true },
  );
  child.on("error", (error) => console.error("hotkey-e2e: holder spawn error", error));
  const ready = await waitFor(
    `holder ${tag}`,
    () => (existsSync(readyPath) ? JSON.parse(readFileSync(readyPath, "utf8")) : null),
    Boolean,
    15000,
  );
  if (!ready.ok) {
    try {
      child.kill();
    } catch {
      /* already gone */
    }
    throw new Error(`the hotkey holder for ${accelerator} did not report within 15 s`);
  }
  holder = { child, stopPath, statusPath, tag, info: ready.value };
  results.observations[`holder_${tag}`] = ready.value;
  if (!ready.value.ok) {
    throw new Error(
      `the helper could not register ${accelerator} (lastError ${ready.value.lastError}); ` +
        "another application owns it — pick a free combination or close that app",
    );
  }
  console.log(`holder ${tag}: ${accelerator} held (pid ${ready.value.pid})`);
  return holder;
}

/** Stops the holder through its stop file and waits until it has unregistered and exited. */
async function stopHolder() {
  if (!holder) return null;
  const current = holder;
  holder = null;
  writeFileSync(current.stopPath, "stop");
  const exited = await waitChildExit(current.child, 5000);
  if (!exited) console.warn("hotkey-e2e: holder did not stop in 5 s; terminated it");
  const status = existsSync(current.statusPath)
    ? JSON.parse(readFileSync(current.statusPath, "utf8"))
    : null;
  results.observations[`holder_${current.tag}_released`] = status;
  console.log(
    `holder ${current.tag}: released (graceful=${exited}, status=${JSON.stringify(status)})`,
  );
  return status;
}

/** Navigates the panel webview to the shortcut page (the window's own URL and frontend code). */
async function openShortcutPage(target) {
  if (!panelUrl) throw new Error("the original panel URL was never captured");
  const url = new URL(panelUrl);
  url.search = `?window=shortcut&target=${target}&theme=dark`;
  await panel.send("Page.navigate", { url: url.toString() });
  const ready = await waitFor(
    `shortcut page ${target}`,
    () => evaluate(panel, `!!document.querySelector(".shortcut")`),
    Boolean,
    20000,
  );
  if (!ready.ok) throw new Error(`the shortcut page for ${target} did not render`);
  return url.toString();
}

/** Re-opens the panel page after the shortcut-page evidence. */
async function restorePanelPage() {
  if (!panelUrl) return false;
  await panel.send("Page.navigate", { url: panelUrl });
  const ready = await waitFor(
    "panel page restored",
    () => evaluate(panel, `!!document.querySelector(".panel")`),
    Boolean,
    20000,
  );
  return ready.ok;
}

/** Dispatches one trusted key press to the shortcut page (CDP input needs no desktop focus). */
async function pressKey({ code, key, vk }, modifiers) {
  const dispatch = (type) =>
    panel.send("Input.dispatchKeyEvent", {
      type,
      modifiers,
      key,
      code,
      windowsVirtualKeyCode: vk,
      nativeVirtualKeyCode: vk,
    });
  await dispatch("keyDown");
  await dispatch("keyUp");
}

async function shortcutPageState() {
  return evaluate(
    panel,
    `(() => {
      const root = document.querySelector(".shortcut");
      const box = document.querySelector(".shortcut__capture");
      const current = document.querySelector(".shortcut__current");
      return {
        target: root?.dataset.target ?? null,
        current: current?.dataset.current ?? null,
        captured: box?.dataset.captured ?? null,
        error: box?.dataset.error ?? null,
        hint: box?.querySelector(".shortcut__hint")?.textContent ?? null,
        title: document.querySelector(".shortcut__title")?.textContent ?? null,
      };
    })()`,
  );
}

async function cleanup() {
  await stopHolder().catch(() => undefined);
  await stopApp().catch(() => undefined);
}

function finish(verdict) {
  results.finishedAt = new Date().toISOString();
  results.verdict = verdict;
  results.failureCount = failures;
  log.poll();
  results.logTail = log.lines.slice(-120);
  results.logPath = appStdoutPath;
  results.sharedLogPath = sharedLogPath;
  if (existsSync(appStderrPath)) {
    results.stderrTail = readFileSync(appStderrPath, "utf8").split(/\r?\n/).slice(-40);
  }
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(evidencePath, `${JSON.stringify(results, null, 2)}\n`);
  console.log(`evidence: ${evidencePath}`);
  console.log(
    `verdict: ${verdict} (${Object.keys(results.checks).length} checks, ${failures} failed)`,
  );
}

async function main() {
  assertDisposable(base);
  assertNoOtherHangline();
  writeFixture();
  await killStaleWebviews("stale_webviews_at_start");
  results.logPath = appStdoutPath;
  results.sharedLogPath = sharedLogPath;
  step("isolated profile", { profile, itemPath });

  // 1. The persisted toggle accelerator is already owned when the app starts.
  step("1. helper holds Ctrl+Alt+T while the app starts");
  await startHolder("startup", DEFAULT_TOGGLE);
  await launchAndConnect();
  check("app_started", true);
  const startupFailure = await log.wait(
    new RegExp(`hotkeys: failed to register toggle shortcut ${literal(DEFAULT_TOGGLE)}`),
    15000,
  );
  check("1_startup_conflict_logged", Boolean(startupFailure.line), { line: startupFailure.line });
  const startupCapture = await log.wait(
    new RegExp(`hotkeys: registered capture shortcut ${literal(DEFAULT_CAPTURE)}`),
    8000,
  );
  check("1_capture_still_registered", Boolean(startupCapture.line), { line: startupCapture.line });
  const startupSettings = await settings();
  results.observations.settingsAtStartupConflict = startupSettings;
  check("1_setting_kept", startupSettings?.toggleHotkey === DEFAULT_TOGGLE, {
    toggleHotkey: startupSettings?.toggleHotkey,
  });
  const startupResponsive = await waitFor(
    "app responsive after conflict",
    () => invoke("get_app_state"),
    (state) => Array.isArray(state?.items),
    3000,
  );
  check("1_app_responsive_after_conflict", startupResponsive.ok, {
    items: startupResponsive.value?.items?.length,
  });
  check("1_settings_json_kept", readSettingsFile().toggleHotkey === DEFAULT_TOGGLE, {
    toggleHotkey: readSettingsFile().toggleHotkey,
  });

  // 2. Release the helper; saving the same (unchanged) value must retry and register.
  step("2. release the helper and save the same accelerator again (retry)");
  const startupReleased = await stopHolder();
  check("2_helper_released", Boolean(startupReleased?.unregisterOk), startupReleased);
  const retrySameValue = await setHotkey("toggle", DEFAULT_TOGGLE);
  results.observations.sameValueRetryAfterStartupConflict = retrySameValue;
  check("2_same_value_retry_accepted", retrySameValue.ok === true, {
    error: retrySameValue.error,
    ms: retrySameValue.ms,
  });
  const retryLog = await log.wait(
    new RegExp(`set_hotkey: toggle shortcut ${literal(DEFAULT_TOGGLE)} is already persisted`),
    5000,
  );
  check("2_retry_logged", Boolean(retryLog.line), { line: retryLog.line });
  const retryRegistered = await log.wait(
    new RegExp(`hotkeys: registered toggle shortcut ${literal(DEFAULT_TOGGLE)}`),
    5000,
  );
  check("2_retry_registered", Boolean(retryRegistered.line), { line: retryRegistered.line });
  const retryOwned = await probe(DEFAULT_TOGGLE, "after-same-value-recovery");
  check(
    "2_app_now_owns_toggle",
    retryOwned.ok === false && retryOwned.lastError === 1409,
    retryOwned,
  );

  // 3. Baseline: both default accelerators are live.
  step("3. baseline registration ownership");
  const baselineToggleProbe = await probe(DEFAULT_TOGGLE, "baseline-toggle");
  check(
    "3_default_toggle_owned_by_app",
    baselineToggleProbe.ok === false && baselineToggleProbe.lastError === 1409,
    baselineToggleProbe,
  );
  const baselineCaptureProbe = await probe(DEFAULT_CAPTURE, "baseline-capture");
  check(
    "3_default_capture_owned_by_app",
    baselineCaptureProbe.ok === false && baselineCaptureProbe.lastError === 1409,
    baselineCaptureProbe,
  );

  // 4. A helper owns Ctrl+Alt+Shift+F9: the change must be rejected and rolled back.
  step("4. change toggle to the combination a helper owns");
  await startHolder("conflict", BUSY_ACCELERATOR);
  const holderProbe = await probe(BUSY_ACCELERATOR, "conflict-source");
  check(
    "4_conflict_source_really_owned",
    holderProbe.ok === false && holderProbe.lastError === 1409,
    holderProbe,
  );
  const before = await settings();
  results.observations.settingsBeforeConflict = before;
  const rejected = await setHotkey("toggle", BUSY_ACCELERATOR);
  results.observations.conflictError = rejected;
  results.timings.conflictRoundTripMs = rejected.ms;
  check(
    "4_change_rejected",
    rejected.ok === false && /hotkey registration failed/i.test(rejected.error ?? ""),
    { error: rejected.error, ms: rejected.ms },
  );
  check("4_error_names_the_accelerator", (rejected.error ?? "").includes(BUSY_ACCELERATOR), {
    error: rejected.error,
  });
  const afterRejection = await settings();
  results.observations.settingsAfterConflict = afterRejection;
  check("4_setting_unchanged_in_memory", afterRejection.toggleHotkey === DEFAULT_TOGGLE, {
    toggleHotkey: afterRejection.toggleHotkey,
  });
  check("4_capture_unchanged", afterRejection.captureHotkey === DEFAULT_CAPTURE, {
    captureHotkey: afterRejection.captureHotkey,
  });
  const settingsFileAfterConflict = readSettingsFile();
  check(
    "4_settings_json_unchanged",
    settingsFileAfterConflict.toggleHotkey === DEFAULT_TOGGLE &&
      settingsFileAfterConflict.captureHotkey === DEFAULT_CAPTURE,
    {
      toggleHotkey: settingsFileAfterConflict.toggleHotkey,
      captureHotkey: settingsFileAfterConflict.captureHotkey,
    },
  );
  const oldStillOwned = await probe(DEFAULT_TOGGLE, "after-conflict-old");
  check(
    "4_old_accelerator_still_registered",
    oldStillOwned.ok === false && oldStillOwned.lastError === 1409,
    oldStillOwned,
  );
  const responsive = await waitFor("app responsive", () => invoke("get_app_state"), (state) =>
    Array.isArray(state?.items),
  3000);
  check("4_app_responsive", responsive.ok, {
    items: responsive.value?.items?.length,
    elapsedMs: responsive.elapsedMs,
  });
  const conflictLog = await log.wait(
    new RegExp(`hotkeys: failed to register toggle shortcut ${literal(BUSY_ACCELERATOR)}`),
    4000,
  );
  check("4_conflict_logged", Boolean(conflictLog.line), { line: conflictLog.line });
  const rollbackLog = await log.wait(/settings: toggleHotkey rolled back after a failed/, 4000);
  check("4_rollback_logged", Boolean(rollbackLog.line), { line: rollbackLog.line });
  results.observations.conflictLog = conflictLog.line;
  results.observations.rollbackLog = rollbackLog.line;

  // 5. Release the helper: the same change now succeeds and moves the registration.
  step("5. release the helper and retry the same change");
  const released = await stopHolder();
  check("5_helper_released", Boolean(released?.unregisterOk), released);
  const registeredBefore = log.count(/hotkeys: registered toggle shortcut /);
  const accepted = await setHotkey("toggle", BUSY_ACCELERATOR);
  results.observations.retryResult = accepted;
  results.timings.retryMs = accepted.ms;
  check("5_change_accepted", accepted.ok === true, { error: accepted.error, ms: accepted.ms });
  const afterRetry = await settings();
  check("5_setting_persisted_in_memory", afterRetry.toggleHotkey === BUSY_ACCELERATOR, {
    toggleHotkey: afterRetry.toggleHotkey,
  });
  check("5_settings_json_persisted", readSettingsFile().toggleHotkey === BUSY_ACCELERATOR, {
    toggleHotkey: readSettingsFile().toggleHotkey,
  });
  const releasedOld = await probe(DEFAULT_TOGGLE, "after-retry-old");
  check("5_old_accelerator_released", releasedOld.ok === true, releasedOld);
  const newOwned = await probe(BUSY_ACCELERATOR, "after-retry-new");
  check(
    "5_new_accelerator_registered",
    newOwned.ok === false && newOwned.lastError === 1409,
    newOwned,
  );
  const registeredAfter = log.count(/hotkeys: registered toggle shortcut /);
  results.observations.toggleRegistrationLogs = { registeredBefore, registeredAfter };
  check("5_registration_logged", registeredAfter === registeredBefore + 1, {
    registeredBefore,
    registeredAfter,
  });

  // A repeated save of the live value is a retry: it must attempt registration, not duplicate it.
  step("5b. save the same live value again (retry, no duplicate registration)");
  const retryLive = await setHotkey("toggle", BUSY_ACCELERATOR);
  check("5b_same_value_retry_accepted", retryLive.ok === true, { error: retryLive.error });
  const registeredAfterRetry = log.count(/hotkeys: registered toggle shortcut /);
  check("5b_no_duplicate_registration", registeredAfterRetry === registeredAfter, {
    registeredAfter,
    registeredAfterRetry,
  });
  const liveRetryLog = await log.wait(
    new RegExp(`set_hotkey: toggle shortcut ${literal(BUSY_ACCELERATOR)} is already persisted`),
    3000,
  );
  check("5b_retry_logged", Boolean(liveRetryLog.line), { line: liveRetryLog.line });
  const stillOwned = await probe(BUSY_ACCELERATOR, "after-same-value-retry");
  check(
    "5b_still_owned_by_app",
    stillOwned.ok === false && stillOwned.lastError === 1409,
    stillOwned,
  );

  // 6. Restore the original accelerator.
  step("6. restore the original toggle accelerator");
  const restored = await setHotkey("toggle", DEFAULT_TOGGLE);
  check("6_restore_accepted", restored.ok === true, { error: restored.error });
  const restoredSettings = await settings();
  check("6_setting_restored", restoredSettings.toggleHotkey === DEFAULT_TOGGLE, {
    toggleHotkey: restoredSettings.toggleHotkey,
  });
  check("6_settings_json_restored", readSettingsFile().toggleHotkey === DEFAULT_TOGGLE, {
    toggleHotkey: readSettingsFile().toggleHotkey,
  });
  const restoredProbe = await probe(DEFAULT_TOGGLE, "after-restore");
  check(
    "6_restored_accelerator_registered",
    restoredProbe.ok === false && restoredProbe.lastError === 1409,
    restoredProbe,
  );
  const freedProbe = await probe(BUSY_ACCELERATOR, "after-restore-freed");
  check("6_temporary_accelerator_released", freedProbe.ok === true, freedProbe);

  // 7. Shortcut page: current accelerator, conflict text, retry success (tray window substitute).
  step("7. shortcut page: both accelerators and the conflict state");
  // The locked session reports QUNS_BUSY, so the pinned line is only allowed to show with the
  // full-screen override on; this is an isolated profile, and the reveal is only needed so the
  // panel window is really rendered for the app-UI screenshots.
  await invoke("set_setting", { key: "showOverFullScreen", value: true });
  toggleLine();
  const revealed = await waitFor(
    "panel revealed",
    () => evaluate(panel, `document.querySelector(".panel")?.dataset.revealed === "true"`),
    Boolean,
    6000,
  );
  results.observations.panelRevealedForScreenshots = revealed.ok;

  const capturePage = await openShortcutPage("capture");
  results.observations.capturePageUrl = capturePage;
  // The page fetches the full state asynchronously (`initAppState`); wait for the accelerator rather
  // than sampling the first frame after mount.
  const capturePageReady = await waitFor(
    "capture page state",
    () => shortcutPageState(),
    (state) => state?.target === "capture" && state?.current === DEFAULT_CAPTURE,
    8000,
  );
  const capturePageState = capturePageReady.value ?? (await shortcutPageState());
  results.observations.capturePageInitial = capturePageState;
  results.timings.capturePageStateMs = capturePageReady.elapsedMs;
  check("7_capture_page_target", capturePageState.target === "capture", capturePageState);
  check(
    "7_capture_page_shows_current_accelerator",
    capturePageReady.ok,
    { current: capturePageState.current },
  );
  await screenshot("shortcut-capture.png");

  await startHolder("page", PAGE_ACCELERATOR);
  await pressKey({ code: "F5", key: "F5", vk: 116 }, MOD_CTRL | MOD_ALT | MOD_SHIFT);
  const captured = await waitFor(
    "combination captured",
    () => shortcutPageState(),
    (state) => state?.captured === PAGE_ACCELERATOR,
    4000,
  );
  check("7_combination_captured", captured.ok, { captured: captured.value?.captured });
  await pressKey({ code: "Enter", key: "Enter", vk: 13 }, 0);
  const conflictUi = await waitFor(
    "conflict shown inline",
    () => shortcutPageState(),
    (state) => state?.error === "true",
    6000,
  );
  results.observations.shortcutPageConflict = conflictUi.value;
  check("7_conflict_shown_in_shortcut_page", conflictUi.ok, conflictUi.value ?? undefined);
  check(
    "7_conflict_hint_names_failure",
    /hotkey registration failed/i.test(conflictUi.value?.hint ?? ""),
    { hint: conflictUi.value?.hint },
  );
  const conflictPageUrl = await evaluate(panel, "location.search");
  check(
    "7_window_stays_open_after_conflict",
    conflictUi.value?.target === "capture" &&
      conflictUi.value?.current === DEFAULT_CAPTURE &&
      /window=shortcut/.test(conflictPageUrl) &&
      /target=capture/.test(conflictPageUrl),
    {
      target: conflictUi.value?.target,
      current: conflictUi.value?.current,
      url: conflictPageUrl,
    },
  );
  await screenshot("shortcut-conflict.png");
  check(
    "7_setting_unchanged_after_ui_conflict",
    (await settings()).captureHotkey === DEFAULT_CAPTURE,
    { captureHotkey: (await settings()).captureHotkey },
  );

  step("7b. release the helper and save the captured combination again");
  await stopHolder();
  await pressKey({ code: "Enter", key: "Enter", vk: 13 }, 0);
  const saved = await waitFor(
    "save persisted through the page",
    () => settings(),
    (state) => state?.captureHotkey === PAGE_ACCELERATOR,
    8000,
  );
  check("7b_ui_save_persisted", saved.ok, { captureHotkey: saved.value?.captureHotkey });
  const savedUi = await shortcutPageState();
  results.observations.shortcutPageSaved = savedUi;
  check("7b_ui_error_cleared", savedUi.error === "false", {
    error: savedUi.error,
    hint: savedUi.hint,
  });
  await screenshot("shortcut-saved.png");

  step("7c. the toggle page shows its own accelerator");
  const togglePage = await openShortcutPage("toggle");
  results.observations.togglePageUrl = togglePage;
  const togglePageReady = await waitFor(
    "toggle page state",
    () => shortcutPageState(),
    (state) => state?.target === "toggle" && state?.current === DEFAULT_TOGGLE,
    8000,
  );
  const togglePageState = togglePageReady.value ?? (await shortcutPageState());
  results.observations.togglePageInitial = togglePageState;
  check("7c_toggle_page_target", togglePageState.target === "toggle", togglePageState);
  check(
    "7c_toggle_page_shows_current_accelerator",
    togglePageReady.ok,
    { current: togglePageState.current },
  );
  await screenshot("shortcut-toggle.png");
  check(
    "7c_both_accelerators_shown",
    capturePageState.current === DEFAULT_CAPTURE && togglePageState.current === DEFAULT_TOGGLE,
    { capture: capturePageState.current, toggle: togglePageState.current },
  );

  // Restore the capture accelerator and put the panel page back.
  await evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke("set_hotkey", ${JSON.stringify({
      target: "capture",
      accelerator: DEFAULT_CAPTURE,
    })})`,
  );
  const captureRestored = await settings();
  check("8_capture_restored", captureRestored.captureHotkey === DEFAULT_CAPTURE, {
    captureHotkey: captureRestored.captureHotkey,
  });
  check("8_panel_page_restored", await restorePanelPage());
  const finalSettings = await settings();
  check(
    "8_final_hotkeys_are_the_defaults",
    finalSettings.captureHotkey === DEFAULT_CAPTURE && finalSettings.toggleHotkey === DEFAULT_TOGGLE,
    { captureHotkey: finalSettings.captureHotkey, toggleHotkey: finalSettings.toggleHotkey },
  );
  // Tightened final ownership: restoring the setting must also restore the live registration, so a
  // probe cannot take either default combination back (1409 = owned by another process, Hangline).
  const finalCaptureProbe = await probe(DEFAULT_CAPTURE, "final-capture");
  check(
    "8_final_capture_is_owned",
    finalCaptureProbe?.ok === false && finalCaptureProbe?.lastError === 1409,
    finalCaptureProbe,
  );
  const finalToggleProbe = await probe(DEFAULT_TOGGLE, "final-toggle");
  check(
    "8_final_toggle_is_owned",
    finalToggleProbe?.ok === false && finalToggleProbe?.lastError === 1409,
    finalToggleProbe,
  );
  check("8_no_holder_left", holder === null);
  check("8_no_panic_in_log", log.count(/panicked at/) === 0, {
    lines: log.count(/panicked at/),
  });
  const hotkeyStderr = existsSync(appStderrPath) ? readFileSync(appStderrPath, "utf8") : "";
  check("8_no_panic_in_stderr", !/panicked at/.test(hotkeyStderr), {
    stderrTail: hotkeyStderr.split(/\r?\n/).slice(-10),
  });
}

try { await main(); } catch (error) {
  failures += 1;
  results.error = error instanceof Error ? `${error.message}\n${error.stack}` : String(error);
  console.error("hotkey-e2e: fatal:", results.error);
}
// Cleanup (holders + app) runs before the evidence is written, so the JSON reflects the final state.
try {
  await cleanup();
} catch (error) {
  console.error("hotkey-e2e: cleanup failed:", error);
}
finish(failures === 0 ? "PASS" : "FAIL");
process.exit(failures === 0 ? 0 : 1);
