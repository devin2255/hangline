#!/usr/bin/env node
/* global fetch, AbortSignal */
/**
 * P4-4 supplementary autostart evidence: the **non-elevated** `HKCU\...\Run` path (spec §3.5).
 *
 * `tauri-plugin-autostart` uses `auto-launch`'s default `WindowsEnableMode::Dynamic`, which writes
 * `HKLM` when the process is elevated and falls back to `HKCU` on `ACCESS_DENIED`. The main
 * `autostart-e2e.mjs` runs under this agent session's (elevated) token, so it exercises the HKLM
 * branch. This probe starts the *same* debug exe with a normal-user token and asserts the spec's
 * HKCU location:
 *
 *   node scripts/p4/autostart-hkcu-probe.mjs            # runas /trustlevel:0x20000 when elevated
 *   node scripts/p4/autostart-hkcu-probe.mjs --direct   # always launch the exe yourself
 *
 * The non-elevated launch uses `runas /trustlevel:0x20000` (a restricted "basic user" token, no
 * credentials, no UAC prompt); when the caller is not elevated the exe is spawned directly.
 * The app log is not read here on purpose: the `runas` child does not inherit the harness's stdio,
 * and this probe only needs registry state.
 *
 * Registry safety: the probe refuses to start if a `Hangline` Run value already exists in HKCU or
 * HKLM, snapshots both hives plus the `StartupApproved\Run` markers, and restores them at the end
 * (also after a failure). Evidence lands in
 * `.superpowers/p4-evidence/autostart/hkcu-non-elevated.json` (git-ignored).
 */

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

const PORT = Number(process.env.HANG_CDP_PORT ?? 9232);
process.env.HANG_CDP_PORT = String(PORT);
const { connectTo, evaluate, sleep } = await import("../p2/cdp.mjs");

const EXE = resolve(process.env.HANG_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"));
const REGISTRY_HELPER = resolve("scripts", "p4", "registry-state.ps1");
const ARGV_HELPER = resolve("scripts", "p4", "commandline-argv.ps1");
const EVIDENCE_DIR = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p4-evidence", "autostart");
const FORCE_DIRECT = process.argv.includes("--direct");
const base = resolve(process.env.HANG_P4_HKCU_BASE ?? join(tmpdir(), "hangline-p4-hkcu"));
const localAppData = join(base, "localappdata");
const profile = join(localAppData, "Hangline");
const settingsPath = join(profile, "settings.json");
const statePath = join(profile, "state.json");
const snapshotPath = join(base, "registry-before.json");
const evidencePath = join(EVIDENCE_DIR, "hkcu-non-elevated.json");

const results = {
  startedAt: new Date().toISOString(),
  exe: EXE,
  cdpPort: PORT,
  checks: {},
  observations: {},
  steps: [],
  manualPending: [
    "Log off/on (or reboot) after enabling 开机启动 and confirm Hangline starts — MANUAL-PENDING",
  ],
};
let failures = 0;
let appPid = null;
let elevated = false;
let snapshot = null;
let registryRestored = null;
let panel = null;

/** Restores the pre-run registry state synchronously; safe from a signal/exit handler. */
function restoreRegistryNow(reason) {
  if (!snapshot || registryRestored) return false;
  try {
    registryRestored = registry("restore", { inPath: snapshotPath });
    results.observations.registryRestoredOn = reason;
    console.error(`hkcu-probe: restored the autostart registry on ${reason}`);
    return true;
  } catch (error) {
    console.error(`hkcu-probe: registry restore on ${reason} failed:`, error);
    results.observations.registryRestoreError = String(error);
    return false;
  }
}

/** A SIGINT/SIGTERM must not leave the user's `Run` value or the probe app behind. */
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.once(signal, () => {
    console.error(`hkcu-probe: ${signal}; stopping the app and restoring the registry`);
    if (appPid) {
      try {
        execFileSync("taskkill", ["/PID", String(appPid), "/T", "/F"], { stdio: "ignore" });
      } catch {
        /* already gone */
      }
      appPid = null;
    }
    restoreRegistryNow(signal);
    process.exit(130);
  });
}
process.on("exit", () => restoreRegistryNow("process exit"));

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

function assertDisposable(dir) {
  const resolved = resolve(dir);
  const temp = resolve(tmpdir());
  if (!resolved.startsWith(`${temp}\\`) && resolved !== temp) {
    console.error(`hkcu-probe: refusing to clear ${resolved} (must be under %TEMP%)`);
    process.exit(2);
  }
}

function writeFixture() {
  rmSync(base, { recursive: true, force: true });
  mkdirSync(profile, { recursive: true });
  mkdirSync(EVIDENCE_DIR, { recursive: true });
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
  writeFileSync(statePath, "[]\n");
}

async function waitFor(label, fn, predicate = (value) => Boolean(value), timeoutMs = 8000) {
  const started = Date.now();
  let last;
  while (Date.now() - started < timeoutMs) {
    try {
      last = await fn();
    } catch {
      last = undefined;
    }
    if (predicate(last)) return { ok: true, value: last, elapsedMs: Date.now() - started };
    await sleep(150);
  }
  return { ok: false, value: last, elapsedMs: Date.now() - started };
}

function powershell(command) {
  return execFileSync("powershell.exe", ["-NoProfile", "-Command", command], {
    encoding: "utf8",
  }).trim();
}

function isElevated() {
  return (
    powershell(
      "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
    ) === "True"
  );
}

function hanglinePids() {
  const raw = powershell(
    "@(Get-Process hangline -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) -join ','",
  );
  return raw === "" ? [] : raw.split(",").map(Number).filter(Number.isFinite);
}

function webviewPids() {
  const raw = powershell(
    `@(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -like '*app.hangline.Hangline*' } | Select-Object -ExpandProperty ProcessId) -join ','`,
  );
  return raw === "" ? [] : raw.split(",").map(Number).filter(Number.isFinite);
}

async function killStaleWebviews() {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const pids = webviewPids();
    if (pids.length === 0) return true;
    execFileSync(
      "powershell.exe",
      ["-NoProfile", "-Command", `Stop-Process -Id ${pids.join(",")} -Force -ErrorAction SilentlyContinue`],
      { stdio: "ignore" },
    );
    await sleep(300);
  }
  return false;
}

async function cdpPortFree() {
  try {
    const response = await fetch(`http://127.0.0.1:${PORT}/json/version`, {
      signal: AbortSignal.timeout(500),
    });
    await response.arrayBuffer().catch(() => undefined);
    return false;
  } catch {
    return true;
  }
}

async function waitCdpPortFree(timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await cdpPortFree()) return true;
    await sleep(250);
  }
  return false;
}

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
    "Hangline",
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

function describeRun(state) {
  return {
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

function sameWindowsPath(left, right) {
  const normalize = (value) =>
    String(value ?? "")
      .trim()
      .replace(/[\\/]+$/, "")
      .toLowerCase();
  return normalize(left) === normalize(right);
}

/** The exact quoted command line: one executable argument, no prefix match. */
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

function launchApp() {
  const env = { ...process.env, LOCALAPPDATA: localAppData, HANG_CDP_PORT: String(PORT) };
  if (elevated && !FORCE_DIRECT) {
    // A restricted-token child of an elevated session: the app sees a normal user token, so
    // auto-launch's Dynamic mode falls back to HKCU (the spec location).
    spawn("runas", ["/trustlevel:0x20000", EXE], {
      env,
      stdio: "ignore",
      windowsHide: true,
      detached: false,
    });
    results.observations.launchMode = "runas /trustlevel:0x20000 (non-elevated token)";
  } else {
    spawn(EXE, [], { env, stdio: "ignore", windowsHide: false });
    results.observations.launchMode = "direct";
  }
  console.log(`launch: ${results.observations.launchMode} (elevated harness=${elevated})`);
}

async function resolveAppPid(timeoutMs = 10000) {
  const found = await waitFor(
    "app pid",
    () => hanglinePids(),
    (pids) => Array.isArray(pids) && pids.length === 1,
    timeoutMs,
  );
  appPid = found.value?.[0] ?? null;
  return appPid;
}

async function stopApp() {
  if (appPid !== null) {
    try {
      execFileSync("taskkill", ["/PID", String(appPid), "/T", "/F"], { stdio: "ignore" });
    } catch {
      /* already gone */
    }
    appPid = null;
  }
  await sleep(600);
  await killStaleWebviews();
  await waitCdpPortFree(8000);
}

async function cleanup() {
  await stopApp().catch(() => undefined);
  if (snapshot && !registryRestored) {
    try {
      registryRestored = registry("restore", { inPath: snapshotPath });
      results.observations.registryRestoredInCleanup = describeRun(registryRestored);
      results.registryCleanup = results.registryCleanup ?? {
        restoredInCleanup: true,
        matchesSnapshot: JSON.stringify(registryRestored) === JSON.stringify(snapshot),
        run: registryRestored.run,
        startupApproved: registryRestored.startupApproved,
      };
    } catch (error) {
      results.registryRestoreError = String(error);
    }
  }
}

function finish(verdict) {
  results.finishedAt = new Date().toISOString();
  results.verdict = verdict;
  results.failureCount = failures;
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(evidencePath, `${JSON.stringify(results, null, 2)}\n`);
  console.log(`evidence: ${evidencePath}`);
  console.log(`verdict: ${verdict} (${Object.keys(results.checks).length} checks, ${failures} failed)`);
}

async function main() {
  assertDisposable(base);
  if (!existsSync(EXE)) {
    throw new Error(`debug exe not found at ${EXE}; run \`cargo build\` in src-tauri first`);
  }
  elevated = isElevated();
  results.observations.harnessElevated = elevated;
  if (hanglinePids().length > 0) {
    throw new Error(
      "a hangline.exe process already runs; close it before this probe " +
        "(Get-Process hangline | Stop-Process -Force stops a leftover evidence instance)",
    );
  }
  await killStaleWebviews();
  await waitCdpPortFree();
  writeFixture();
  step("snapshot registry");
  snapshot = registry("read");
  writeFileSync(snapshotPath, `${JSON.stringify(snapshot, null, 2)}\n`);
  results.observations.registryBefore = describeRun(snapshot);
  if (snapshot.run.hkcu.exists || snapshot.run.hklm.exists) {
    throw new Error(
      "a Hangline login item already exists; this probe refuses to modify the user's autostart entry",
    );
  }

  step("launch the app with a normal-user token");
  launchApp();
  const pid = await resolveAppPid(12000);
  results.observations.appPid = pid;
  check("app_started", pid !== null, { pid });
  panel = await connectTo("window=panel", 30000);
  await waitFor("panel DOM", () => evaluate(panel, `!!document.querySelector(".panel")`), Boolean, 30000);
  check("panel_connected", true);

  step("enable startAtLogin and read HKCU");
  const enableReply = await evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke("set_setting", { key: "startAtLogin", value: true })
      .then((state) => ({ ok: true, startAtLogin: state.settings.startAtLogin }))
      .catch((error) => ({ ok: false, error: String(error) }))`,
  );
  results.observations.enableReply = enableReply;
  check("enable_accepted", enableReply?.ok === true && enableReply?.startAtLogin === true, enableReply);

  const hkcuEnabled = await waitFor(
    "HKCU Run value",
    () => registry("read"),
    (state) => state?.run?.hkcu?.exists === true,
    8000,
  );
  const afterEnable = hkcuEnabled.value ?? registry("read");
  results.observations.afterEnable = describeRun(afterEnable);
  check("hkcu_value_created", afterEnable.run.hkcu.exists === true, {
    elapsedMs: hkcuEnabled.elapsedMs,
  });
  check("hklm_value_absent", afterEnable.run.hklm.exists === false, {
    hklm: afterEnable.run.hklm,
  });
  check(
    "hkcu_value_points_at_exe",
    runValueTargetsExe(afterEnable.run.hkcu.data),
    { data: afterEnable.run.hkcu.data },
  );
  check("hkcu_value_is_quoted", afterEnable.run.hkcu.data === `"${EXE}"`, {
    data: afterEnable.run.hkcu.data,
    expected: `"${EXE}"`,
  });
  const argv = commandLineArgv(afterEnable.run.hkcu.data);
  check(
    "hkcu_value_parses_to_one_exe_argument",
    argv.length === 1 && sameWindowsPath(argv[0], EXE),
    { argv, exe: EXE, parser: "CommandLineToArgvW" },
  );
  check(
    "hkcu_startup_approved_enabled",
    afterEnable.startupApproved.hkcu.exists === true &&
      afterEnable.startupApproved.hkcu.data?.[0] === 2,
    { startupApproved: afterEnable.startupApproved.hkcu },
  );

  step("disable startAtLogin and read HKCU again");
  const disableReply = await evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke("set_setting", { key: "startAtLogin", value: false })
      .then((state) => ({ ok: true, startAtLogin: state.settings.startAtLogin }))
      .catch((error) => ({ ok: false, error: String(error) }))`,
  );
  results.observations.disableReply = disableReply;
  check("disable_accepted", disableReply?.ok === true && disableReply?.startAtLogin === false, disableReply);
  const hkcuRemoved = await waitFor(
    "HKCU Run value removed",
    () => registry("read"),
    (state) => state?.run?.hkcu?.exists === false,
    8000,
  );
  const afterDisable = hkcuRemoved.value ?? registry("read");
  results.observations.afterDisable = describeRun(afterDisable);
  check("hkcu_value_removed", hkcuRemoved.ok, { elapsedMs: hkcuRemoved.elapsedMs });
  check("hklm_still_absent", afterDisable.run.hklm.exists === false);

  step("cleanup: restore the pre-run registry state");
  await stopApp();
  panel?.close();
  panel = null;
  registryRestored = registry("restore", { inPath: snapshotPath });
  results.registryCleanup = {
    matchesSnapshot: JSON.stringify(registryRestored) === JSON.stringify(snapshot),
    run: registryRestored.run,
    startupApproved: registryRestored.startupApproved,
  };
  check("registry_matches_snapshot", results.registryCleanup.matchesSnapshot, {
    before: snapshot.run,
    after: registryRestored.run,
  });
  check(
    "no_login_item_left",
    registryRestored.run.hkcu.exists === false && registryRestored.run.hklm.exists === false,
  );
  check("no_app_left", hanglinePids().length === 0, { pids: hanglinePids() });
}

try {
  await main();
} catch (error) {
  failures += 1;
  results.error = error instanceof Error ? `${error.message}\n${error.stack}` : String(error);
  console.error("hkcu-probe: fatal:", results.error);
}
try {
  await cleanup();
} catch (error) {
  console.error("hkcu-probe: cleanup failed:", error);
}
finish(failures === 0 ? "PASS" : "FAIL");
process.exit(failures === 0 ? 0 : 1);
