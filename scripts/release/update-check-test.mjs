#!/usr/bin/env node
/* global Buffer, setTimeout */
/**
 * P5-2 local updater evidence: no public release repository is required.
 *
 * The harness serves a minimal Tauri `latest.json` from localhost, starts the **debug** app with an
 * isolated profile and `HANG_UPDATE_ENDPOINT` (the debug-only endpoint override, see
 * `src-tauri/src/updater.rs`), and drives the real IPC/events through WebView2's DevTools protocol.
 * It proves:
 *
 * 1. an automatic startup check announces `0.4.1` (fixture version, derived as the next patch of the
 *    app version), emits `update-available`, shows the panel notice with the install button, and
 *    persists `lastUpdateCheck`; nothing is downloaded;
 * 2. `autoCheckUpdates: false` performs **zero** requests at startup while a manual check still runs
 *    and persists the timestamp;
 * 3. a manual check against a closed endpoint returns a clean error and shows "Couldn't check for
 *    updates"; an automatic check against the same endpoint stays quiet and does not crash;
 * 4. a recent `lastUpdateCheck` (24 h gate) suppresses the startup request, and a manual check
 *    bypasses the gate;
 * 5. the install button is wired and the fake **unsigned** artifact is rejected by signature
 *    verification ("Update failed"), leaving the app running on the old version — no partial
 *    install is claimed;
 * 6. a dead first endpoint falls through to the second endpoint (the same order the app relies on
 *    for the mainland mirror fallback), and the check still announces the fixture version.
 *
 * A successfully **signed** update install/restart is deliberately **not** attempted here: that
 * needs a real signed release and is CI/manual-pending (see the P5-2 report).
 *
 * Requirements:
 *   - `src-tauri/target/debug/hangline.exe` and `dist/index.html` exist (build with
 *     `pnpm tauri build --debug --no-bundle`, or pass `--build`);
 *   - no other Hangline instance is running (the app is single-instance).
 *
 * Usage:
 *   node scripts/release/update-check-test.mjs [--build]
 *
 * Evidence (JSON + captured app logs) lands in `.superpowers/p5-evidence/update-check/`
 * (git-ignored). Exit code 0 only when every check passes.
 */

import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const EVIDENCE_DIR = join(REPO_ROOT, ".superpowers", "p5-evidence", "update-check");
const RUN_DIR = join(EVIDENCE_DIR, "run");
// Some hardened machines refuse writes from executables launched from the dev workspace. Set
// HANG_UPDATE_EXE to a copy of the debug build outside the workspace to run the harness there.
const EXE = process.env.HANG_UPDATE_EXE
  ? resolve(process.env.HANG_UPDATE_EXE)
  : join(REPO_ROOT, "src-tauri", "target", "debug", "hangline.exe");
const DIST_INDEX = join(REPO_ROOT, "dist", "index.html");
const CDP_PORT = Number(process.env.HANG_UPDATE_CDP_PORT ?? 9339);
const BUILD = process.argv.includes("--build");

const results = { checks: {}, steps: [], scenarios: {}, environment: {} };
let failures = 0;

// Filled by `main` after `HANG_CDP_PORT` is set (the helper reads it at module import time).
let connectTo = null;
let evaluate = null;

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

const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));

function bumpPatch(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`unsupported app version ${version}`);
  return `${match[1]}.${match[2]}.${Number(match[3]) + 1}`;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeSettings(profile, settings) {
  const dir = join(profile, "localappdata", "Hangline");
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "settings.json"), `${JSON.stringify(settings, null, 2)}\n`, "utf8");
  return dir;
}

function readSettings(dataDir) {
  try {
    return readJson(join(dataDir, "settings.json"));
  } catch {
    return null;
  }
}

/** Minimal static fixture server: `/latest.json` plus a fake artifact for the signature-failure path. */
async function startFixtureServer(manifest) {
  const artifact = Buffer.alloc(64 * 1024, 0x5a);
  const requests = [];
  const server = createServer((request, response) => {
    const path = request.url ?? "/";
    requests.push({ path, at: new Date().toISOString() });
    if (path === "/latest.json") {
      const body = `${JSON.stringify(manifest, null, 2)}\n`;
      response.writeHead(200, {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(body),
      });
      response.end(body);
      return;
    }
    if (path === "/Hangline_fixture_x64-setup.exe") {
      // A real-looking download that is deliberately NOT signed: the plugin must reject it before
      // launching anything. It is never executed by this harness.
      response.writeHead(200, {
        "content-type": "application/octet-stream",
        "content-length": artifact.length,
      });
      response.end(artifact);
      return;
    }
    response.writeHead(404, { "content-type": "text/plain" });
    response.end("update-check-test fixture serves only /latest.json and the fake artifact\n");
  });
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  return {
    server,
    requests,
    port: address.port,
    base: `http://127.0.0.1:${address.port}`,
  };
}

/** A port that was just released: connecting to it fails immediately with ECONNREFUSED. */
async function availableClosedPort() {
  const server = createServer(() => {});
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const { port } = server.address();
  await new Promise((resolveClose) => server.close(resolveClose));
  return port;
}

/** Starts the debug app with an isolated profile and the fixture endpoint. */
function launchApp({ profile, endpoint, logFile }) {
  const env = {
    ...process.env,
    LOCALAPPDATA: join(profile, "localappdata"),
    APPDATA: join(profile, "appdata"),
    HANG_UPDATE_ENDPOINT: endpoint,
    HANG_CDP_PORT: String(CDP_PORT),
  };
  const log = createWriteStream(logFile, { flags: "a" });
  const child = spawn(EXE, [], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  child.stdout.pipe(log);
  child.stderr.pipe(log);
  child.once("exit", () => log.end());
  return { child, log };
}

async function stopApp(run) {
  if (!run) return;
  try {
    run.panel?.close();
  } catch {
    /* the socket may already be gone */
  }
  if (run.child.exitCode === null) {
    run.child.kill();
    await Promise.race([once(run.child, "exit").catch(() => undefined), sleep(10000)]);
  }
  // The single-instance mutex is released at process exit; give Windows a moment before relaunching.
  await sleep(900);
}

async function panelReady(panel) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const ready = await evaluate(
      panel,
      `document.readyState === "complete" && !!document.querySelector(".panel")`,
    ).catch(() => false);
    if (ready) return true;
    await sleep(150);
  }
  return false;
}

async function invoke(panel, command, args = {}) {
  return evaluate(
    panel,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

/** Reads the current update notice (or null) plus the install-button presence. */
async function updateNotice(panel) {
  const raw = await evaluate(
    panel,
    `(() => {
      const element = document.querySelector('[data-testid="update-notice"]');
      if (!element) return "null";
      return JSON.stringify({
        kind: element.dataset.kind ?? null,
        version: element.dataset.version ?? null,
        progress: element.dataset.progress ?? null,
        label: element.textContent ?? "",
        installButton: !!element.querySelector('[data-action="install-update"]'),
      });
    })()`,
  );
  return raw === "null" ? null : JSON.parse(raw);
}

async function waitForNotice(panel, kind, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await updateNotice(panel).catch(() => null);
    if (last?.kind === kind) return last;
    await sleep(150);
  }
  throw new Error(`timed out waiting for update notice ${kind} (last=${JSON.stringify(last)})`);
}

/** Captures the panel page over a neutral backdrop (the real window is transparent over desktop). */
async function capturePanel(panel, fileName) {
  await panel
    .send("Emulation.setDefaultBackgroundColorOverride", {
      color: { r: 26, g: 30, b: 35, a: 1 },
    })
    .catch(() => undefined);
  const shot = await panel.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  writeFileSync(join(EVIDENCE_DIR, fileName), Buffer.from(shot.data, "base64"));
  await panel.send("Emulation.setDefaultBackgroundColorOverride", {}).catch(() => undefined);
}

/** Launches one scenario and connects to the panel target. */
async function startScenario({ name, settings, endpoint }) {
  const profile = join(RUN_DIR, `profile-${name}`);
  rmSync(profile, { recursive: true, force: true });
  mkdirSync(profile, { recursive: true });
  const dataDir = settings ? writeSettings(profile, settings) : writeSettings(profile, {});
  const logFile = join(EVIDENCE_DIR, `app-${name}.log`);
  writeFileSync(logFile, "");
  const run = launchApp({ profile, endpoint, logFile });
  const deadline = Date.now() + 40000;
  let panel = null;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      panel = await connectTo("window=panel", 5000);
      if (await panelReady(panel)) break;
    } catch (error) {
      lastError = error;
      panel?.close();
      panel = null;
      if (run.child.exitCode !== null) break;
      await sleep(500);
    }
  }
  if (!panel) {
    throw new Error(
      `app ${name} did not expose the panel over CDP: ${lastError ?? "unknown error"}`,
    );
  }
  return { ...run, panel, profile, dataDir, logFile };
}

function isoLooksValid(value) {
  return typeof value === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/.test(value);
}

function logTail(path, limit = 60) {
  try {
    const lines = readFileSync(path, "utf8").trimEnd().split(/\r?\n/);
    return lines.slice(-limit);
  } catch {
    return [];
  }
}

async function main() {
  if (!existsSync(EXE) || !existsSync(DIST_INDEX)) {
    if (!BUILD) {
      throw new Error(
        `missing ${EXE} or ${DIST_INDEX}; run \`pnpm tauri build --debug --no-bundle\` first or pass --build`,
      );
    }
    step("building the debug app", "pnpm tauri build --debug --no-bundle");
    const built = spawnSync(
      process.platform === "win32" ? "pnpm.cmd" : "pnpm",
      ["tauri", "build", "--debug", "--no-bundle"],
      { cwd: REPO_ROOT, stdio: "inherit", shell: process.platform === "win32" },
    );
    if (built.status !== 0) throw new Error(`debug build failed with exit code ${built.status}`);
  }

  rmSync(RUN_DIR, { recursive: true, force: true });
  mkdirSync(RUN_DIR, { recursive: true });

  // The CDP helper reads the port at module import; set it first.
  process.env.HANG_CDP_PORT = String(CDP_PORT);
  const cdp = await import(`../p2/cdp.mjs?update-test=${Date.now()}`);
  connectTo = cdp.connectTo;
  evaluate = cdp.evaluate;

  const appVersion = readJson(join(REPO_ROOT, "src-tauri", "tauri.conf.json")).version;
  const fixtureVersion = bumpPatch(appVersion);
  const fixture = {
    version: fixtureVersion,
    notes: "P5-2 local updater check fixture (not a published release)",
    pub_date: new Date().toISOString(),
    platforms: {
      "windows-x86_64": {
        signature: "LOCALHOST-FIXTURE-SIGNATURE-NOT-REAL",
        url: "http://127.0.0.1:1/Hangline_fixture_x64-setup.exe",
      },
    },
  };
  results.fixture = { appVersion, fixtureVersion, manifest: fixture };

  const fixtureServer = await startFixtureServer(fixture);
  fixture.manifestUrl = `${fixtureServer.base}/latest.json`;
  fixture.platforms["windows-x86_64"].url = `${fixtureServer.base}/Hangline_fixture_x64-setup.exe`;
  const closedPort = await availableClosedPort();
  const offlineEndpoint = `http://127.0.0.1:${closedPort}/latest.json`;
  results.environment = {
    node: process.version,
    windows: process.env.OS ?? "windows",
    cdpPort: CDP_PORT,
    endpoint: fixture.manifestUrl,
    offlineEndpoint,
    evidenceDir: EVIDENCE_DIR,
  };
  step("fixture server started", { endpoint: fixture.manifestUrl, fixtureVersion });

  // --- Scenario A: automatic check announces an update, persists the check, downloads nothing ---
  let run = null;
  try {
    run = await startScenario({
      name: "a-auto-available",
      settings: { autoCheckUpdates: true, lastUpdateCheck: "", language: "en" },
      endpoint: fixture.manifestUrl,
    });
    const notice = await waitForNotice(run.panel, "available");
    await capturePanel(run.panel, "panel-update-available.png");
    check(
      "automatic startup check announces the fixture version",
      notice.version === fixtureVersion,
      notice,
    );
    check("available notice renders the install action", notice.installButton === true, notice);
    check(
      "available notice text carries the translated label",
      notice.label.includes("Update available") && notice.label.includes(fixtureVersion),
      notice.label,
    );

    const manualResult = await invoke(run.panel, "check_updates", { manual: true });
    check(
      "check_updates returns the update DTO",
      manualResult?.version === fixtureVersion && manualResult?.manual === true,
      manualResult,
    );
    check(
      "check_updates carries notes and pubDate",
      typeof manualResult?.notes === "string" &&
        manualResult.notes.length > 0 &&
        isoLooksValid(manualResult?.pubDate),
      { notes: manualResult?.notes, pubDate: manualResult?.pubDate },
    );

    const settings = readSettings(run.dataDir);
    check(
      "lastUpdateCheck persisted as ISO 8601",
      isoLooksValid(settings?.lastUpdateCheck),
      settings?.lastUpdateCheck,
    );
    check(
      "autoCheckUpdates stays true",
      settings?.autoCheckUpdates === true,
      settings?.autoCheckUpdates,
    );
    check(
      "check performs no download request",
      fixtureServer.requests.every((request) => request.path === "/latest.json"),
      [...fixtureServer.requests],
    );
    results.scenarios.a = {
      notice,
      manualResult,
      settings,
      requests: [...fixtureServer.requests],
    };
    step("scenario A verified", { noticeKind: notice.kind });
  } finally {
    await stopApp(run);
  }

  // --- Scenario B: autoCheckUpdates=false => zero automatic requests; manual still works ---------
  fixtureServer.requests.length = 0;
  try {
    run = await startScenario({
      name: "b-auto-off",
      settings: { autoCheckUpdates: false, lastUpdateCheck: "", language: "en" },
      endpoint: fixture.manifestUrl,
    });
    await sleep(4000);
    check(
      "autoCheckUpdates=false performs no startup request",
      fixtureServer.requests.length === 0,
      [...fixtureServer.requests],
    );
    check("no automatic notice is shown", (await updateNotice(run.panel)) === null, null);

    const manualResult = await invoke(run.panel, "check_updates", { manual: true });
    const notice = await waitForNotice(run.panel, "available");
    check(
      "manual check is allowed while automatic checks are off",
      manualResult?.version === fixtureVersion && notice.version === fixtureVersion,
      { manualResult, notice },
    );
    check(
      "manual check with automatic checks off performs exactly one request",
      fixtureServer.requests.length === 1,
      [...fixtureServer.requests],
    );
    const settings = readSettings(run.dataDir);
    check(
      "manual check persists lastUpdateCheck",
      isoLooksValid(settings?.lastUpdateCheck),
      settings?.lastUpdateCheck,
    );
    check("autoCheckUpdates=false persisted", settings?.autoCheckUpdates === false, settings);
    results.scenarios.b = {
      manualResult,
      notice,
      settings,
      requests: [...fixtureServer.requests],
    };
    step("scenario B verified");
  } finally {
    await stopApp(run);
  }

  // --- Scenario C: manual check against a closed endpoint => clean error + failure notice ------
  try {
    run = await startScenario({
      name: "c-manual-offline",
      settings: { autoCheckUpdates: false, lastUpdateCheck: "", language: "en" },
      endpoint: offlineEndpoint,
    });
    const error = await evaluate(
      run.panel,
      `(async () => {
        try {
          await window.__TAURI_INTERNALS__.invoke("check_updates", { manual: true });
          return "";
        } catch (error) {
          return String(error);
        }
      })()`,
    );
    check(
      "manual offline check returns a clean 'couldn't check' error",
      typeof error === "string" && error.includes("couldn't check for updates"),
      error,
    );
    const notice = await waitForNotice(run.panel, "check-failed");
    check(
      "manual offline check shows the failure notice",
      notice.label.includes("Couldn't check for updates"),
      notice,
    );
    check(
      "failed check does not persist lastUpdateCheck",
      readSettings(run.dataDir)?.lastUpdateCheck === "",
      readSettings(run.dataDir)?.lastUpdateCheck,
    );
    check("app survives the manual offline error", run.child.exitCode === null, run.child.exitCode);
    results.scenarios.c = { error, notice, settings: readSettings(run.dataDir) };
    step("scenario C verified");
  } finally {
    await stopApp(run);
  }

  // --- Scenario D: automatic check against a closed endpoint stays quiet -----------------------
  try {
    run = await startScenario({
      name: "d-auto-offline",
      settings: { autoCheckUpdates: true, lastUpdateCheck: "", language: "en" },
      endpoint: offlineEndpoint,
    });
    await sleep(4000);
    check(
      "automatic offline check shows no notice (quiet)",
      (await updateNotice(run.panel)) === null,
      null,
    );
    check(
      "app survives the automatic offline failure",
      run.child.exitCode === null,
      run.child.exitCode,
    );
    check(
      "automatic offline failure does not persist lastUpdateCheck",
      readSettings(run.dataDir)?.lastUpdateCheck === "",
      readSettings(run.dataDir)?.lastUpdateCheck,
    );
    results.scenarios.d = {
      settings: readSettings(run.dataDir),
      logTail: logTail(run.logFile),
    };
    step("scenario D verified");
  } finally {
    await stopApp(run);
  }

  // --- Scenario E: recent lastUpdateCheck suppresses the startup request; manual bypasses ------
  fixtureServer.requests.length = 0;
  try {
    run = await startScenario({
      name: "e-recent-check",
      settings: {
        autoCheckUpdates: true,
        lastUpdateCheck: new Date().toISOString(),
        language: "en",
      },
      endpoint: fixture.manifestUrl,
    });
    await sleep(4000);
    check(
      "a recent lastUpdateCheck suppresses the startup request",
      fixtureServer.requests.length === 0,
      [...fixtureServer.requests],
    );
    check("no notice for a suppressed check", (await updateNotice(run.panel)) === null, null);
    const manualResult = await invoke(run.panel, "check_updates", { manual: true });
    check(
      "manual check bypasses the 24 h gate",
      manualResult?.version === fixtureVersion,
      manualResult,
    );
    check("manual bypass performs exactly one request", fixtureServer.requests.length === 1, [
      ...fixtureServer.requests,
    ]);
    results.scenarios.e = {
      manualResult,
      settings: readSettings(run.dataDir),
      requests: [...fixtureServer.requests],
    };
    step("scenario E verified");
  } finally {
    await stopApp(run);
  }

  // --- Scenario F: the fake unsigned artifact is rejected; no install/restart is claimed --------
  fixtureServer.requests.length = 0;
  try {
    run = await startScenario({
      name: "f-install-signature",
      settings: { autoCheckUpdates: false, lastUpdateCheck: "", language: "en" },
      endpoint: fixture.manifestUrl,
    });
    await invoke(run.panel, "check_updates", { manual: true });
    await waitForNotice(run.panel, "available");
    const clicked = await evaluate(
      run.panel,
      `(() => {
        const button = document.querySelector('[data-action="install-update"]');
        if (!button) return false;
        button.click();
        return true;
      })()`,
    );
    check("the install button is wired to install_update", clicked === true, clicked);
    const failed = await waitForNotice(run.panel, "install-failed", 30000);
    check(
      "an unsigned fixture artifact fails with the 'Update failed' notice",
      failed.label.includes("Update failed"),
      failed,
    );
    const failedDetail = await evaluate(
      run.panel,
      `document.querySelector('[data-testid="update-notice"]')?.title ?? ""`,
    );
    check(
      "the failure detail is a signature/verification error",
      typeof failedDetail === "string" && /verification|signature/i.test(failedDetail),
      failedDetail,
    );
    const afterState = await invoke(run.panel, "get_app_state");
    check(
      "the app stays on the old version after the rejected install",
      afterState?.version === appVersion && run.child.exitCode === null,
      { version: afterState?.version, expected: appVersion, exit: run.child.exitCode },
    );
    check(
      "the fake artifact download was attempted",
      fixtureServer.requests.some((request) => request.path === "/Hangline_fixture_x64-setup.exe"),
      [...fixtureServer.requests],
    );
    results.scenarios.f = {
      failed,
      failedDetail,
      versionAfter: afterState?.version,
      requests: [...fixtureServer.requests],
    };
    step("scenario F verified (signature rejection, no install)");
  } finally {
    await stopApp(run);
  }

  // --- Scenario G: dead first endpoint falls through to the mirror manifest --------------------
  // This is the mainland fallback shape: the official GitHub endpoint is unreachable, the mirror
  // endpoint (here a second localhost manifest) serves the same signed-manifest contract.
  fixtureServer.requests.length = 0;
  try {
    run = await startScenario({
      name: "g-endpoint-fallback",
      settings: { autoCheckUpdates: true, lastUpdateCheck: "", language: "en" },
      endpoint: `${offlineEndpoint},${fixture.manifestUrl}`,
    });
    const notice = await waitForNotice(run.panel, "available", 30000);
    check(
      "a dead first endpoint falls through to the second (fallback) endpoint",
      notice.version === fixtureVersion,
      notice,
    );
    check(
      "the fallback check reaches the second endpoint exactly once",
      fixtureServer.requests.length === 1,
      [...fixtureServer.requests],
    );
    const settings = readSettings(run.dataDir);
    check(
      "the fallback check persists lastUpdateCheck",
      isoLooksValid(settings?.lastUpdateCheck),
      settings?.lastUpdateCheck,
    );
    results.scenarios.g = { notice, requests: [...fixtureServer.requests], settings };
    step("scenario G verified (multi-endpoint fallback)");
  } finally {
    await stopApp(run);
  }

  await new Promise((resolveClose) => fixtureServer.server.close(resolveClose));

  results.verdict = failures === 0 ? "PASS" : `FAIL (${failures} check(s))`;
  results.checkCount = Object.keys(results.checks).length;
  results.passed = results.checkCount - failures;
  results.failures = failures;
  results.note =
    "Local endpoint evidence. Checks/manifest parsing/signature rejection are exercised; the " +
    "fixture artifact is intentionally unsigned, so a successful download/install/restart of a " +
    "signed release is NOT tested here and remains CI/manual-pending.";
  writeFileSync(
    join(EVIDENCE_DIR, "update-check-test.json"),
    `${JSON.stringify(results, null, 2)}\n`,
    "utf8",
  );
  console.log(
    `verdict: ${results.verdict} (${results.passed}/${results.checkCount} checks passed)`,
  );
  console.log(`evidence: ${join(EVIDENCE_DIR, "update-check-test.json")}`);
  process.exit(failures === 0 ? 0 : 1);
}

try {
  await main();
} catch (error) {
  failures += 1;
  results.verdict = "FAIL (harness error)";
  results.error = error instanceof Error ? `${error.stack ?? error.message}` : String(error);
  console.error("update-check-test: harness error");
  console.error(results.error);
  mkdirSync(EVIDENCE_DIR, { recursive: true });
  writeFileSync(
    join(EVIDENCE_DIR, "update-check-test.json"),
    `${JSON.stringify(results, null, 2)}\n`,
    "utf8",
  );
  process.exit(1);
}
