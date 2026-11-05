#!/usr/bin/env node
/* global Buffer, fetch, AbortSignal */
/**
 * P4-4 runtime evidence: autostart registry round-trip + `reconcile_autostart`, and the sound gate.
 *
 * Requires (dev-only, no dependencies beyond Node's built-ins):
 *   1. `pnpm dev` (Vite on http://localhost:5173) running in another terminal;
 *   2. a debug build: `cargo build` in `src-tauri/` (or `pnpm tauri dev` once);
 *   3. no other Hangline instance running (single-instance would swallow the launch).
 *
 *   node scripts/p4/autostart-e2e.mjs
 *
 * The app is launched with an isolated `%TEMP%\hangline-p4-autostart` profile and
 * `HANG_CDP_PORT=9231`; `set_setting {key:"startAtLogin", value:true|false}` goes through the real
 * IPC path (the tray switch uses the same `actions::apply`). The registry value is read back with
 * `scripts/p4/registry-state.ps1`.
 *
 * Registry safety (the registry is *not* isolated by the profile):
 *   - the harness snapshots `Run` and `Explorer\StartupApproved\Run` for the app's product name
 *     ("Hangline", from tauri.conf.json `productName` — what `tauri-plugin-autostart` uses) under
 *     both HKCU and HKLM,
 *   - it refuses to run when a Hangline login item already exists (the user's real autostart entry
 *     is never touched); `HANG_P4_AUTOSTART_ALLOW_EXISTING=1` opts into snapshot+restore mode,
 *   - at the end (also after a failure) the snapshot is restored and re-read; the comparison is
 *     written into the evidence JSON as cleanup proof.
 *
 * Verified steps:
 *   1. `startAtLogin=true` -> the Run value exists in exactly one hive, is the quoted command line
 *      `"<exe>"` (parsed back with the system `CommandLineToArgvW`), and the StartupApproved marker
 *      is enabled;
 *   2. `false` -> the value is gone again (and the log records the removal);
 *   3. the value survives quitting the app (autostart is not a per-run state);
 *   4. reconcile: setting true + value deleted outside the app -> the next start re-registers it
 *      and logs it; a second start with everything in sync re-registers nothing;
 *   4b. reconcile: a stale value (moved executable, with arguments) is repaired to the current
 *      quoted exe and the arguments are preserved;
 *   5. reconcile: a Task Manager-disabled StartupApproved marker (`0x03`) is detected and the
 *      startup path leaves it disabled instead of silently overriding the user's choice;
 *   6. reconcile: manual Run value while `startAtLogin` is off -> the next start keeps it and logs
 *      that the manual registration is left alone;
 *   6b. an explicit `startAtLogin=false` (the tray's click target when the check mark follows the
 *      real registration) removes that manual entry on the first call, even though the setting
 *      was already false;
 *   7. sound gate: `sound=false` logs the skipped cue; `sound=true` logs the played cue on the same
 *      take-down path (audible playback remains MANUAL-PENDING; there are no WAV assets — the module
 *      uses the Windows system beep, so a WAV header/bundle check does not apply).
 *
 * `HANG_P4_AUTOSTART_SPACED=1` copies the debug exe to `%TEMP%\hangline-p4-autostart\Hangline
 * Test\hangline.exe` and runs the whole flow from that spaced path, proving the login command
 * end-to-end for a Program Files/portable layout.
 *
 * Evidence (JSON) lands in `.superpowers/p4-evidence/autostart/` (git-ignored).
 */

import { execFileSync, spawn } from "node:child_process";
import {
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const PORT = Number(process.env.HANG_CDP_PORT ?? 9231);
// `scripts/p2/cdp.mjs` reads HANG_CDP_PORT when the module is evaluated: set it before the import.
process.env.HANG_CDP_PORT = String(PORT);
const { connectTo, evaluate, sleep, targets } = await import("../p2/cdp.mjs");

const EVIDENCE_DIR = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p4-evidence", "autostart");
const ORIGINAL_EXE = resolve(
  process.env.HANG_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"),
);
// Reassigned when HANG_P4_AUTOSTART_SPACED=1 copies the exe into a spaced folder.
let EXE = ORIGINAL_EXE;
const SPACED = process.env.HANG_P4_AUTOSTART_SPACED === "1";
const REGISTRY_HELPER = resolve("scripts", "p4", "registry-state.ps1");
const ARGV_HELPER = resolve("scripts", "p4", "commandline-argv.ps1");
const REPO_TARGET = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "src-tauri", "target");
const APP_NAME = "Hangline";
const POLL_MS = 100;
const ITEM_WIDTH = 640;
const ITEM_HEIGHT = 360;
const ALLOW_EXISTING = process.env.HANG_P4_AUTOSTART_ALLOW_EXISTING === "1";

const base = resolve(process.env.HANG_P4_AUTOSTART_BASE ?? join(tmpdir(), "hangline-p4-autostart"));
const localAppData = join(base, "localappdata");
const profile = join(localAppData, "Hangline");
const screenshotsDir = join(profile, "Screenshots");
const itemPath = join(screenshotsDir, "p4-autostart-item.png");
const itemPath2 = join(screenshotsDir, "p4-autostart-item-2.png");
const ITEM_ID = "33333333333333333333333333333333";
const ITEM_ID_2 = "44444444444444444444444444444444";
const settingsPath = join(profile, "settings.json");
const statePath = join(profile, "state.json");
const snapshotPath = join(base, "registry-before.json");
const afterPath = join(base, "registry-after.json");
const evidencePath = join(EVIDENCE_DIR, "autostart-e2e.json");
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
  appName: APP_NAME,
  checks: {},
  timings: {},
  observations: {},
  steps: [],
  manualPending: [
    "Audible feedback: listen to the copy/discard cue with sound on — MANUAL-PENDING (this run verifies the played/skipped log path; the user's speakers are never recorded)",
    "Enable 开机启动 in the tray, log off/on, and confirm Hangline starts — MANUAL-PENDING (registry round-trip is verified here)",
    "Task Manager > Startup shows the real state after the manual-registration case — MANUAL-PENDING (tray UI needs an interactive desktop)",
    "Tray click through the real menu (the check mark follows the registration; the IPC path it calls is verified here) — MANUAL-PENDING on the tray menu",
  ],
};
let failures = 0;
let app = null;
let appRuns = 0;
let panel = null;
let snapshot = null;
let registryRestored = null;

/** Restores the pre-run registry state synchronously; safe from a signal/exit handler. */
function restoreRegistryNow(reason) {
  if (!snapshot || registryRestored) return false;
  try {
    const after = registry("restore", { inPath: snapshotPath });
    registryRestored = after;
    results.observations.registryRestoredOn = reason;
    results.observations.registryAfterEmergencyRestore = describeRun(after);
    console.error(`autostart-e2e: restored the autostart registry on ${reason}`);
    return true;
  } catch (error) {
    console.error(`autostart-e2e: registry restore on ${reason} failed:`, error);
    results.observations.registryEmergencyRestoreError = String(error);
    return false;
  }
}

/**
 * A SIGINT/SIGTERM during a long run must not leave the user's `Run` value behind. The normal
 * cleanup path restores the snapshot; this handler also kills the app tree it launched and restores
 * synchronously before exiting (the registry is not isolated by the profile, unlike the app data).
 */
function installEmergencyCleanup() {
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    process.once(signal, () => {
      console.error(`autostart-e2e: ${signal}; stopping the app tree and restoring the registry`);
      if (app && app.pid) {
        try {
          execFileSync("taskkill", ["/PID", String(app.pid), "/T", "/F"], { stdio: "ignore" });
        } catch {
          /* already gone */
        }
        app = null;
      }
      restoreRegistryNow(signal);
      process.exit(130);
    });
  }
  // A crash/early exit still gets one synchronous restore attempt (normal runs already restored).
  process.on("exit", () => restoreRegistryNow("process exit"));
}
installEmergencyCleanup();

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
 * `src-tauri\target` may be cleared. The old "any path segment called target" check could clear an
 * unrelated `target\` directory; the guard is now anchored to the checkout.
 */
function assertDisposable(dir) {
  const resolved = resolve(dir).toLowerCase();
  const temp = resolve(tmpdir()).toLowerCase();
  const insideTemp = resolved === temp || resolved.startsWith(`${temp}${sep}`);
  const repoTarget = REPO_TARGET.toLowerCase();
  const insideTarget = resolved === repoTarget || resolved.startsWith(`${repoTarget}${sep}`);
  if (!insideTemp && !insideTarget) {
    console.error(
      `autostart-e2e: refusing to clear ${resolve(dir)} (must be under %TEMP% or this repo's src-tauri\\target)`,
    );
    process.exit(2);
  }
  if (basename(resolved) === "hangline") {
    console.error("autostart-e2e: HANG_P4_AUTOSTART_BASE must be the run root, not the Hangline profile");
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
      rowBytes[x * 3] = Math.round(52 + 40 * t);
      rowBytes[x * 3 + 1] = Math.round(74 + 26 * (1 - t));
      rowBytes[x * 3 + 2] = Math.round(108 + 20 * (1 - t));
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

/** Isolated profile: startAtLogin off, sound off, two owned items for the two sound paths. */
function writeFixture() {
  rmSync(base, { recursive: true, force: true });
  mkdirSync(screenshotsDir, { recursive: true });
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(itemPath, fixturePng());
  writeFileSync(itemPath2, fixturePng());
  const now = Date.now();
  const settings = {
    captureHotkey: "Ctrl+Alt+S",
    toggleHotkey: "Ctrl+Alt+T",
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
      id: ITEM_ID,
      path: itemPath,
      kind: "capture",
      hash: 4208,
      width: ITEM_WIDTH,
      height: ITEM_HEIGHT,
      createdAt: now - 5000,
      addedAt: now - 4000,
      updatedAt: now - 4000,
    },
    {
      id: ITEM_ID_2,
      path: itemPath2,
      kind: "capture",
      hash: 4209,
      width: ITEM_WIDTH,
      height: ITEM_HEIGHT,
      createdAt: now - 3000,
      addedAt: now - 2000,
      updatedAt: now - 2000,
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

async function settings() {
  const state = await invoke("get_app_state");
  return state?.settings ?? null;
}

async function setSetting(key, value) {
  return invoke("set_setting", { key, value });
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

/** Runs `scripts/p4/registry-state.ps1` and returns the state JSON it wrote. */
function registry(action, { inPath, data } = {}) {
  const outPath = join(base, `registry-${action}-${Date.now()}.json`);
  const args = [
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    REGISTRY_HELPER,
    "-Action",
    action,
    "-Name",
    APP_NAME,
    "-OutPath",
    outPath,
  ];
  if (inPath) args.push("-InPath", inPath);
  if (data) args.push("-Data", data);
  execFileSync("powershell.exe", args, { stdio: ["ignore", "ignore", "inherit"] });
  const raw = readFileSync(outPath, "utf8").replace(/^\uFEFF/, "");
  rmSync(outPath, { force: true });
  return JSON.parse(raw);
}

/** Which hives currently hold the Run value. */
function registeredHives(state) {
  return ["hkcu", "hklm"].filter((hive) => state?.run?.[hive]?.exists);
}

function describeRun(state) {
  return {
    hives: registeredHives(state),
    hkcu: state.run.hkcu,
    hklm: state.run.hklm,
    startupApproved: state.startupApproved,
  };
}

/** Splits a Windows command line with `CommandLineToArgvW` (the system's own rules). */
function commandLineArgv(commandLine) {
  const raw = execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      ARGV_HELPER,
      "-CommandLine",
      String(commandLine),
    ],
    { encoding: "utf8" },
  )
    .replace(/^\uFEFF/, "")
    .trim();
  return JSON.parse(raw);
}

/** Windows path equality: case-insensitive, trailing separators ignored. */
function sameWindowsPath(left, right) {
  const normalize = (value) =>
    String(value ?? "")
      .trim()
      .replace(/[\\/]+$/, "")
      .toLowerCase();
  return normalize(left) === normalize(right);
}

/**
 * The real assertion: parse the Run value with `CommandLineToArgvW` and require the executable
 * (argv[0]) to be *exactly* the current exe. A prefix check would accept `hangline.exe.bak`, and an
 * unquoted spaced path parses into several fragments instead of the executable.
 */
function runValueTargetsExe(data) {
  if (typeof data !== "string" || data.trim() === "") return false;
  try {
    const argv = commandLineArgv(data);
    return argv.length > 0 && sameWindowsPath(argv[0], EXE);
  } catch (error) {
    results.observations.argvParseError = String(error);
    return false;
  }
}

/** The exact value a correct enable writes: one quoted executable, no arguments, no trailing space. */
function expectedQuotedRunValue() {
  return `"${EXE}"`;
}

/** Whether a Run value is the canonical quoted command line for the current exe. */
function runValueIsCanonicallyQuoted(data) {
  if (typeof data !== "string") return false;
  const expected = expectedQuotedRunValue();
  return data.toLowerCase() === expected.toLowerCase();
}

async function waitRegistry(predicate, timeoutMs = 6000) {
  const started = Date.now();
  let last = null;
  while (Date.now() - started < timeoutMs) {
    last = registry("read");
    if (predicate(last)) return { ok: true, value: last, elapsedMs: Date.now() - started };
    await sleep(250);
  }
  return { ok: false, value: last, elapsedMs: Date.now() - started };
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

/** Kills leftover WebView2 helpers and waits until none is left (only while no app is running). */
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
  app.on("error", (error) => console.error("autostart-e2e: app spawn error", error));
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

/**
 * Stops the launched app. `graceful` first posts a normal close (`taskkill` without `/F`) and records
 * whether the app exited by itself before falling back to the process tree kill, so the evidence
 * distinguishes a graceful quit from the force-kill that only exists to keep WebView2 disposable.
 */
async function stopApp({ graceful = false } = {}) {
  if (app && app.exitCode === null) {
    const current = app;
    app = null;
    if (graceful) {
      let signalled = false;
      try {
        execFileSync("taskkill", ["/PID", String(current.pid)], { stdio: "ignore" });
        signalled = true;
      } catch {
        /* no console window to close; fall through to the force kill */
      }
      const exited = signalled && (await waitChildExit(current, 5000));
      results.observations.gracefulQuit = { signalled, exited, pid: current.pid };
      console.log(`app graceful quit: signalled=${signalled} exited=${exited}`);
    }
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
        () =>
          targets()
            .then((list) => list.some((entry) => entry.type === "page" && entry.url.includes("window=panel")))
            .catch(() => false),
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
      results.observations.launchAttempts = attempt;
      return attempt;
    } catch (error) {
      results.observations[`launch_attempt_${attempt}_failed`] = {
        error: String(error),
        exitCode: app?.exitCode ?? null,
      };
      console.error(`autostart-e2e: launch attempt ${attempt} failed (${error}); retrying`);
      panel?.close();
      panel = null;
      await stopApp();
      await sleep(2500);
    }
  }
  throw new Error("the app never exposed the panel through CDP after 3 launch attempts");
}

async function restartApp(label) {
  step(`${label}: restart`);
  await stopApp();
  panel?.close();
  panel = null;
  await launchAndConnect();
  await sleep(600);
}

/** Scans the source trees for audio assets (the sound module uses the system beep instead). */
function findAudioAssets() {
  const found = [];
  const skip = new Set(["node_modules", "target", "gen", "dist", ".git"]);
  const walk = (dir) => {
    if (!existsSync(dir)) return;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (!skip.has(entry.name)) walk(join(dir, entry.name));
        continue;
      }
      if (entry.isFile() && /\.(wav|mp3|ogg|flac|m4a)$/i.test(entry.name)) {
        found.push(join(dir, entry.name));
      }
    }
  };
  walk(resolve("src"));
  walk(resolve("src-tauri"));
  return found;
}

async function cleanup() {
  await stopApp().catch(() => undefined);
  if (snapshot && !registryRestored) {
    try {
      const after = registry("restore", { inPath: snapshotPath });
      registryRestored = after;
      results.observations.registryRestoredInCleanup = describeRun(after);
      results.registryCleanup = results.registryCleanup ?? {
        snapshotPath,
        afterPath: null,
        restoredInCleanup: true,
        matchesSnapshot: JSON.stringify(after) === JSON.stringify(snapshot),
        run: after.run,
        startupApproved: after.startupApproved,
      };
    } catch (error) {
      console.error("autostart-e2e: registry restore in cleanup failed:", error);
      results.registryRestoreError = String(error);
    }
  }
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
  if (SPACED) {
    if (!existsSync(ORIGINAL_EXE)) {
      throw new Error(`debug exe not found at ${ORIGINAL_EXE}; run \`cargo build\` in src-tauri first`);
    }
    const spacedDir = join(base, "Hangline Test");
    mkdirSync(spacedDir, { recursive: true });
    EXE = join(spacedDir, "hangline.exe");
    copyFileSync(ORIGINAL_EXE, EXE);
    results.exe = EXE;
    results.observations.spacedExe = {
      source: ORIGINAL_EXE,
      copy: EXE,
      containsSpace: true,
      reason: "proves the login command works from a Program Files/portable-style spaced folder",
    };
    console.log(`spaced-path mode: running ${EXE}`);
  } else {
    results.observations.spacedExe = {
      skipped: true,
      hint: "rerun with HANG_P4_AUTOSTART_SPACED=1 to exercise a spaced path end-to-end",
    };
  }
  await killStaleWebviews("stale_webviews_at_start");
  results.logPath = appStdoutPath;
  results.sharedLogPath = sharedLogPath;

  // Registry snapshot first: nothing may touch the Run key before it is captured.
  step("0. snapshot the autostart registry locations");
  snapshot = registry("read");
  writeFileSync(snapshotPath, `${JSON.stringify(snapshot, null, 2)}\n`);
  results.observations.registryBefore = describeRun(snapshot);
  const preExisting = registeredHives(snapshot);
  check(
    "0_no_preexisting_login_item",
    preExisting.length === 0 || ALLOW_EXISTING,
    { hives: preExisting, allowExisting: ALLOW_EXISTING },
  );
  if (preExisting.length > 0 && !ALLOW_EXISTING) {
    throw new Error(
      `an existing Hangline login item was found (${preExisting.join(", ")}); refusing to modify ` +
        "the user's real autostart entry. Remove it by hand or rerun with " +
        "HANG_P4_AUTOSTART_ALLOW_EXISTING=1 (snapshot+restore mode).",
    );
  }
  check("0_run_value_absent_before", preExisting.length === 0, { hives: preExisting });

  step("1. launch the app with an isolated profile");
  await launchAndConnect();
  check("app_started", true);
  const baseline = await settings();
  check("1_startAtLogin_off", baseline?.startAtLogin === false, {
    startAtLogin: baseline?.startAtLogin,
  });

  // Sound gate: sound=false must skip the cue (observable in the debug log).
  step("1b. sound gate with sound=false (take a card off the line)");
  const audioAssets = findAudioAssets();
  results.observations.audio = {
    implementation: "MessageBeep (Windows system beep) in src-tauri/src/sound.rs",
    assets: audioAssets,
  };
  check("1b_no_wav_assets_by_design", audioAssets.length === 0, { assets: audioAssets });
  const playedBefore = log.count(/sound: played the discarded beep/);
  const takeDown = await invoke("take_down", {
    id: ITEM_ID,
    deleteFile: false,
  });
  results.observations.takeDown = takeDown;
  const skipped = await log.waitCount(/sound: skipped the discarded beep \(sound is off\)/, 1, 4000);
  check("1b_skip_logged", Boolean(skipped.line), { line: skipped.line });
  const playedAfter = log.count(/sound: played the discarded beep/);
  check("1b_no_play_with_sound_off", playedAfter === playedBefore, { playedBefore, playedAfter });
  check("1b_file_not_deleted", existsSync(itemPath));
  const afterTakeDown = await invoke("get_app_state");
  check(
    "1b_card_left_the_line",
    Array.isArray(afterTakeDown?.items) && afterTakeDown.items.length === 1,
    { items: afterTakeDown?.items?.length },
  );

  // Sound play path: sound=true must reach `MessageBeep` and log the played cue (the move itself
  // never fails on the beep, so this is the observable runtime proof; audibility stays manual).
  step("1c. sound=true plays the discard cue on the same take-down path");
  const soundOnReply = await setSetting("sound", true);
  check("1c_sound_setting_on", soundOnReply?.settings?.sound === true, {
    sound: soundOnReply?.settings?.sound,
  });
  const playBeforeOn = log.count(/sound: played the discarded beep/);
  const skipBeforeOn = log.count(/sound: skipped the discarded beep/);
  const takeDownOn = await invoke("take_down", {
    id: ITEM_ID_2,
    deleteFile: false,
  });
  results.observations.takeDownWithSound = takeDownOn;
  const played = await log.waitCount(/sound: played the discarded beep/, playBeforeOn + 1, 5000);
  check("1c_play_logged", Boolean(played.line), { line: played.line });
  check(
    "1c_no_skip_with_sound_on",
    log.count(/sound: skipped the discarded beep/) === skipBeforeOn,
    { skipBeforeOn, skipAfterOn: log.count(/sound: skipped the discarded beep/) },
  );
  const afterTakeDownOn = await invoke("get_app_state");
  check(
    "1c_card_left_the_line",
    Array.isArray(afterTakeDownOn?.items) && afterTakeDownOn.items.length === 0,
    { items: afterTakeDownOn?.items?.length },
  );
  // Keep the rest of the run on the fixture's documented default (sound off).
  await setSetting("sound", false);
  results.manualPending.push(
    "Audible confirmation of the played cue (the log proves the sound=true branch ran) — MANUAL-PENDING",
  );

  step("2. startAtLogin=true");
  const enableReply = await setSetting("startAtLogin", true);
  check("2_reply_true", enableReply?.settings?.startAtLogin === true, {
    startAtLogin: enableReply?.settings?.startAtLogin,
  });
  const enabled = await waitRegistry((state) => registeredHives(state).length === 1, 8000);
  const enabledHives = registeredHives(enabled.value);
  results.observations.afterEnable = describeRun(enabled.value);
  check("2_run_value_created", enabled.ok, { elapsedMs: enabled.elapsedMs, hives: enabledHives });
  check("2_exactly_one_hive", enabledHives.length === 1, { hives: enabledHives });
  const enabledHive = enabledHives[0] ?? "hkcu";
  const enabledValue = enabled.value?.run?.[enabledHive]?.data;
  check("2_run_value_points_at_exe", runValueTargetsExe(enabledValue), {
    data: enabledValue,
    exe: EXE,
  });
  check("2_run_value_is_quoted", runValueIsCanonicallyQuoted(enabledValue), {
    data: enabledValue,
    expected: expectedQuotedRunValue(),
  });
  const enabledArgv = commandLineArgv(enabledValue);
  check(
    "2_run_value_parses_to_one_exe_argument",
    enabledArgv.length === 1 && sameWindowsPath(enabledArgv[0], EXE),
    { argv: enabledArgv, exe: EXE, parser: "CommandLineToArgvW" },
  );
  const approved = enabled.value?.startupApproved?.[enabledHive];
  check("2_startup_approved_enabled", approved?.exists === true && approved?.data?.[0] === 2, {
    startupApproved: approved,
  });
  const allowLog = await log.waitCount(/settings: registered Hangline as a login item/, 1, 4000);
  check("2_enable_logged", Boolean(allowLog.line), { line: allowLog.line });
  results.timings.enableMs = enabled.elapsedMs;

  step("3. startAtLogin=false");
  const disableReply = await setSetting("startAtLogin", false);
  check("3_reply_false", disableReply?.settings?.startAtLogin === false, {
    startAtLogin: disableReply?.settings?.startAtLogin,
  });
  const disabled = await waitRegistry((state) => registeredHives(state).length === 0, 8000);
  results.observations.afterDisable = describeRun(disabled.value);
  check("3_run_value_removed", disabled.ok, { elapsedMs: disabled.elapsedMs });
  const removeLog = await log.waitCount(/settings: removed Hangline from the login items/, 1, 4000);
  check("3_disable_logged", Boolean(removeLog.line), { line: removeLog.line });
  // The plugin leaves the Task-Manager "approved" marker behind; it is not a login item.
  results.observations.startupApprovedKeptAfterDisable =
    disabled.value?.startupApproved?.[enabledHive] ?? null;

  step("4. enable again; the login item must survive quitting the app");
  await setSetting("startAtLogin", true);
  const reEnabled = await waitRegistry((state) => registeredHives(state).length === 1, 8000);
  check("4_re_enabled", reEnabled.ok, { elapsedMs: reEnabled.elapsedMs });
  // Prefer a real close over the harness force-kill, and record which one actually ended the app.
  await stopApp({ graceful: true });
  check("4_quit_was_attempted_gracefully", results.observations.gracefulQuit?.signalled === true, {
    gracefulQuit: results.observations.gracefulQuit,
  });
  const afterQuit = registry("read");
  results.observations.afterQuit = describeRun(afterQuit);
  check("4_value_survives_quit", registeredHives(afterQuit).length === 1, {
    hives: registeredHives(afterQuit),
  });
  const quitTarget = afterQuit.run[registeredHives(afterQuit)[0]];
  check("4_quit_value_points_at_exe", runValueTargetsExe(quitTarget?.data), {
    data: quitTarget?.data,
  });

  step("4b. reconcile: a stale executable path (and unquoted value) is repaired");
  // Remove the value in both hives, then leave one stale HKCU value (moved portable build) with an
  // argument; reconcile must rewrite it to the current quoted exe and preserve the argument.
  registry("delete-run");
  const staleValue = `"C:\\Program Files\\Old Hangline\\hangline.exe" --stale-flag`;
  const staleWritten = registry("set-run", { data: staleValue });
  results.observations.staleEntry = describeRun(staleWritten);
  check("4b_stale_value_written", registeredHives(staleWritten).length === 1, {
    hives: registeredHives(staleWritten),
  });
  const staleBefore = log.count(/autostart: the login item is stale/);
  await launchAndConnect();
  const staleLog = await log.waitCount(
    /autostart: the login item is stale \(wrong executable or unquoted path\); repairing it for/,
    staleBefore + 1,
    10000,
  );
  check("4b_stale_repair_logged", Boolean(staleLog.line), { line: staleLog.line });
  const repaired = await waitRegistry((state) => {
    const hives = registeredHives(state);
    if (hives.length !== 1) return false;
    const data = state.run[hives[0]]?.data;
    return (
      typeof data === "string" &&
      data.startsWith('"') &&
      data.includes("--stale-flag") &&
      runValueTargetsExe(data)
    );
  }, 8000);
  const repairedValue = repaired.value?.run?.[registeredHives(repaired.value ?? {})[0]]?.data;
  results.observations.repairedEntry = describeRun(repaired.value ?? {});
  check("4b_stale_value_repaired", repaired.ok, { elapsedMs: repaired.elapsedMs });
  check("4b_repaired_points_at_exe", runValueTargetsExe(repairedValue), { data: repairedValue });
  check(
    "4b_repaired_arguments_preserved",
    typeof repairedValue === "string" && repairedValue.includes("--stale-flag"),
    { data: repairedValue },
  );
  await stopApp();

  step("5. reconcile: setting true but the Run value was removed outside the app");
  const deleted = registry("delete-run");
  check("5_value_removed_externally", registeredHives(deleted).length === 0);
  const reconcileBefore = log.count(/autostart: startAtLogin is on but the login item is missing/);
  await launchAndConnect();
  const reconcileA = await log.waitCount(
    /autostart: startAtLogin is on but the login item is missing; re-registering/,
    reconcileBefore + 1,
    10000,
  );
  check("5_reconcile_reregistered_logged", Boolean(reconcileA.line), { line: reconcileA.line });
  const reconciled = await waitRegistry((state) => registeredHives(state).length === 1, 8000);
  check("5_reconcile_value_restored", reconciled.ok, {
    elapsedMs: reconciled.elapsedMs,
    hives: registeredHives(reconciled.value),
  });
  const reconciledHive = registeredHives(reconciled.value)[0];
  check(
    "5_reconcile_points_at_exe",
    runValueTargetsExe(reconciled.value?.run?.[reconciledHive]?.data),
    { data: reconciled.value?.run?.[reconciledHive]?.data },
  );

  step("5b. reconcile: nothing to do when setting and registration agree");
  const inSyncBefore = log.count(
    /autostart: startAtLogin is on but the login item is missing|autostart: a login item is registered but startAtLogin is off/,
  );
  await restartApp("5b consistency restart");
  await sleep(1200);
  const inSyncAfter = log.count(
    /autostart: startAtLogin is on but the login item is missing|autostart: a login item is registered but startAtLogin is off/,
  );
  check("5b_no_reconcile_action_in_sync", inSyncAfter === inSyncBefore, {
    inSyncBefore,
    inSyncAfter,
  });

  step("5c. reconcile: a Task Manager-disabled marker is not overridden");
  await stopApp();
  registry("delete-run");
  const disabledEntry = registry("set-run", { data: `"${EXE}"` });
  check("5c_disabled_case_value_written", registeredHives(disabledEntry).length === 1, {
    hives: registeredHives(disabledEntry),
  });
  // Task Manager's disabled state byte (with the zero timestamp some entries carry).
  const disabledMarker = registry("set-approved", { data: "030000000000000000000000" });
  results.observations.taskManagerDisabled = describeRun(disabledMarker);
  check(
    "5c_disabled_marker_written",
    disabledMarker.startupApproved?.hkcu?.data?.[0] === 3,
    { marker: disabledMarker.startupApproved?.hkcu },
  );
  const disabledBefore = log.count(/Task Manager has the login item disabled/);
  await launchAndConnect();
  const disabledLog = await log.waitCount(
    /autostart: startAtLogin is on but Task Manager has the login item disabled; leaving the user's Task Manager choice alone/,
    disabledBefore + 1,
    10000,
  );
  check("5c_disabled_not_overridden_logged", Boolean(disabledLog.line), {
    line: disabledLog.line,
  });
  const afterDisabledStart = registry("read");
  results.observations.afterTaskManagerDisabledStart = describeRun(afterDisabledStart);
  check(
    "5c_run_value_kept",
    registeredHives(afterDisabledStart).length === 1 &&
      runValueTargetsExe(afterDisabledStart.run[registeredHives(afterDisabledStart)[0]]?.data),
    { hives: registeredHives(afterDisabledStart) },
  );
  check(
    "5c_disabled_marker_still_disabled",
    afterDisabledStart.startupApproved?.hkcu?.data?.[0] === 3,
    { marker: afterDisabledStart.startupApproved?.hkcu },
  );
  check(
    "5c_setting_still_on",
    (await settings())?.startAtLogin === true,
    { startAtLogin: (await settings())?.startAtLogin },
  );

  step("6. reconcile: manual Run value while startAtLogin is off");
  await setSetting("startAtLogin", false);
  const turnedOff = await waitRegistry((state) => registeredHives(state).length === 0, 8000);
  check("6_disabled_before_manual_entry", turnedOff.ok);
  await stopApp();
  const manual = registry("set-run", { data: `"${EXE}"` });
  results.observations.manualEntry = describeRun(manual);
  check("6_manual_value_registered", registeredHives(manual).length === 1, {
    hives: registeredHives(manual),
  });
  const keepBefore = log.count(/autostart: a login item is registered but startAtLogin is off/);
  await launchAndConnect();
  const keepLog = await log.waitCount(
    /autostart: a login item is registered but startAtLogin is off; keeping the manual registration/,
    keepBefore + 1,
    10000,
  );
  check("6_manual_kept_logged", Boolean(keepLog.line), { line: keepLog.line });
  const kept = registry("read");
  results.observations.afterManualStart = describeRun(kept);
  check("6_manual_value_kept", registeredHives(kept).length === 1, {
    hives: registeredHives(kept),
  });
  const keptSettings = await settings();
  check("6_setting_still_off", keptSettings?.startAtLogin === false, {
    startAtLogin: keptSettings?.startAtLogin,
  });

  step("6b. explicit disable removes the manual entry even though the setting was already off");
  // This is exactly what the tray now writes: its check mark follows the registration, so a click
  // on the manual entry computes `false` while `settings.startAtLogin` is already false. The action
  // must still run (the old `actions_for_change` guard would have skipped it and left the entry).
  const manualRemoveReply = await setSetting("startAtLogin", false);
  check("6b_reply_false", manualRemoveReply?.settings?.startAtLogin === false, {
    startAtLogin: manualRemoveReply?.settings?.startAtLogin,
  });
  const manualRemoved = await waitRegistry(
    (state) => registeredHives(state).length === 0,
    8000,
  );
  results.observations.afterManualToggleOff = describeRun(manualRemoved.value);
  check("6b_manual_entry_removed_by_first_click", manualRemoved.ok, {
    elapsedMs: manualRemoved.elapsedMs,
    hives: registeredHives(manualRemoved.value),
  });

  step("7. cleanup: restore the pre-run registry state");
  await stopApp();
  panel?.close();
  panel = null;
  const restored = registry("restore", { inPath: snapshotPath });
  registryRestored = restored;
  writeFileSync(afterPath, `${JSON.stringify(restored, null, 2)}\n`);
  results.observations.registryAfter = describeRun(restored);
  results.registryCleanup = {
    snapshotPath,
    afterPath,
    matchesSnapshot: JSON.stringify(restored) === JSON.stringify(snapshot),
    run: restored.run,
    startupApproved: restored.startupApproved,
  };
  check("7_registry_matches_snapshot", results.registryCleanup.matchesSnapshot, {
    before: snapshot.run,
    after: restored.run,
  });
  check("7_no_login_item_left", registeredHives(restored).length === 0, {
    hives: registeredHives(restored),
  });
  check("7_no_panic_in_log", log.count(/panicked at/) === 0, { lines: log.count(/panicked at/) });
  const stderrText = existsSync(appStderrPath) ? readFileSync(appStderrPath, "utf8") : "";
  check("7_no_panic_in_stderr", !/panicked at/.test(stderrText), {
    stderrTail: stderrText.split(/\r?\n/).slice(-10),
  });
}

try { await main(); } catch (error) {
  failures += 1;
  results.error = error instanceof Error ? `${error.message}\n${error.stack}` : String(error);
  console.error("autostart-e2e: fatal:", results.error);
}
// Cleanup (app kill + registry restore) runs before the evidence is written, so a failed run still
// proves that the user's registry was left untouched.
try {
  await cleanup();
} catch (error) {
  console.error("autostart-e2e: cleanup failed:", error);
}
finish(failures === 0 ? "PASS" : "FAIL");
process.exit(failures === 0 ? 0 : 1);
