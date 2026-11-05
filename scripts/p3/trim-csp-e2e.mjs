#!/usr/bin/env node
/* global Buffer */
/**
 * P3-6 review follow-up: prove the trim preview works under the *packaged* CSP.
 *
 * `pnpm dev` serves the page from Vite, which sends no Content-Security-Policy, so the normal
 * `trim-e2e.mjs` run cannot catch a missing `media-src` (the P3-6 critical finding). This harness
 * removes that blind spot:
 *
 *   1. `pnpm build` produces `dist/`;
 *   2. a throwaway static server serves `dist/` on the app's fixed `devUrl` port (5173) and sends
 *      the exact `app.security.csp` value from `src-tauri/tauri.conf.json` as a response header;
 *   3. the debug app is started (debug builds load `devUrl`) with a fresh isolated profile;
 *   4. the trim window is opened and the `<video>` must reach `readyState >= 2` with no
 *      `securitypolicyviolation` for `media-src`, and Save must still export;
 *   5. the negative control repeats the run with the pre-fix CSP (the same policy with the
 *      `media-src` directive removed) and requires the harness to *catch* the blocked video.
 *
 * Usage (from the repository root; stop `pnpm dev` first so port 5173 is free):
 *
 *   pnpm build
 *   .\scripts\p3\fixture-video.ps1 -Out "$env:TEMP\hangline-csp-e2e\fixture.mp4"
 *   $env:HANG_E2E_DIR = "$env:TEMP\hangline-csp-e2e"
 *   node scripts/p3/trim-csp-e2e.mjs
 *
 * The debug app (`src-tauri/target/debug/hangline.exe`, or `HANG_APP_EXE`) must already be built.
 * Evidence: `.superpowers/p3-evidence/trim-csp-e2e.json`.
 */

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";

const base = process.env.HANG_E2E_DIR;
if (!base) {
  console.error("trim-csp-e2e: HANG_E2E_DIR is required (the temp run directory)");
  process.exit(2);
}
const appExe = resolve(process.env.HANG_APP_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"));
const fixture = process.env.HANG_CSP_FIXTURE ?? join(base, "fixture.mp4");
const evidenceDir = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p3-evidence");
const cdpPort = process.env.HANG_CDP_PORT ?? "9226";
// The cdp helper reads the port at module load; set it before the dynamic import below.
process.env.HANG_CDP_PORT = cdpPort;
const { connectTo, evaluate, sleep, targets } = await import("../p2/cdp.mjs");
const { startCspServer } = await import("./csp-static-server.mjs");

if (!existsSync(fixture)) {
  console.error(`trim-csp-e2e: missing fixture at ${fixture}; run scripts/p3/fixture-video.ps1`);
  process.exit(2);
}
if (!existsSync(appExe)) {
  console.error(`trim-csp-e2e: missing debug app at ${appExe}; run \`pnpm tauri build --no-bundle\``);
  process.exit(2);
}
if (!existsSync(join(resolve("dist"), "index.html"))) {
  console.error("trim-csp-e2e: dist/index.html is missing; run `pnpm build` first");
  process.exit(2);
}
mkdirSync(evidenceDir, { recursive: true });

const config = JSON.parse(readFileSync(join("src-tauri", "tauri.conf.json"), "utf8"));
const csp = config?.app?.security?.csp;
if (typeof csp !== "string" || !csp.includes("default-src")) {
  console.error("trim-csp-e2e: src-tauri/tauri.conf.json has no CSP string");
  process.exit(2);
}
if (!csp.includes("media-src")) {
  console.error("trim-csp-e2e: the configured CSP has no media-src; the fix is missing");
  process.exit(2);
}
// Negative control: exactly the pre-fix policy (the only difference was the new media-src clause).
const legacyCsp = csp
  .replace(/;\s*media-src [^;]+/, "")
  .replace(/media-src [^;]+;\s*/, "");
if (legacyCsp.includes("media-src")) {
  console.error(`trim-csp-e2e: cannot derive the legacy CSP from ${JSON.stringify(csp)}`);
  process.exit(2);
}

const results = {
  csp: { configured: csp, legacy: legacyCsp },
  cases: {},
  checks: {},
  steps: [],
};
let failures = 0;

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

const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

async function invoke(cdp, command, args = {}) {
  return evaluate(
    cdp,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

/** Bounded evaluate: a destroyed target's reply never arrives. */
async function evaluateWithin(cdp, expression, timeoutMs = 1500) {
  return Promise.race([evaluate(cdp, expression), sleep(timeoutMs).then(() => undefined)]);
}

async function waitFor(cdp, expression, label, timeoutMs = 30000, intervalMs = 100) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await evaluateWithin(cdp, expression, Math.min(2000, intervalMs + 1500));
    if (last) return last;
    await sleep(intervalMs);
  }
  throw new Error(`timed out waiting for ${label} (last=${JSON.stringify(last)})`);
}

async function waitForTrimTarget(timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  let list = [];
  while (Date.now() < deadline) {
    list = (await targets()).filter(
      (candidate) => candidate.type === "page" && candidate.url.includes("window=trim"),
    );
    if (list.length > 0) return list;
    await sleep(150);
  }
  return list;
}

/** The recorder must exist before the page's own scripts run, so it survives the reload. */
const VIOLATION_RECORDER = `
(() => {
  window.__HANG_CSP = [];
  const record = (event) => {
    window.__HANG_CSP.push({
      directive: event.effectiveDirective || event.violatedDirective,
      blockedURI: event.blockedURI,
    });
    try {
      sessionStorage.setItem("hangline.csp", JSON.stringify(window.__HANG_CSP));
    } catch (_) {}
  };
  document.addEventListener("securitypolicyviolation", record);
})();
`;

async function readCspViolations(cdp) {
  const raw = await evaluateWithin(
    cdp,
    `sessionStorage.getItem("hangline.csp") || "[]"`,
    2000,
  ).catch(() => null);
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch {
    return [];
  }
}

async function readVideoState(cdp) {
  const raw = await evaluateWithin(
    cdp,
    `(() => {
      const video = document.querySelector(".trim__video");
      const root = document.querySelector(".trim");
      if (!video || !root) return "";
      return JSON.stringify({
        readyState: video.readyState,
        networkState: video.networkState,
        error: video.error ? video.error.code : null,
        duration: Number.isFinite(video.duration) ? video.duration : null,
        src: video.currentSrc,
        videoReady: root.dataset.videoReady,
        videoFailed: root.dataset.videoFailed,
        saving: root.dataset.saving,
      });
    })()`,
    1500,
  ).catch(() => "");
  return raw ? JSON.parse(raw) : null;
}

/** Serves `dist` with `policy`, starts the app, returns the panel + resolved item id. */
async function startCase(name, policy) {
  const caseDir = join(base, name);
  rmSync(caseDir, { recursive: true, force: true });
  const localappdata = join(caseDir, "localappdata");
  const userprofile = join(caseDir, "userprofile");
  const capturesDir = join(userprofile, "Videos", "Captures");
  for (const dir of [
    localappdata,
    join(userprofile, "AppData", "Roaming"),
    join(userprofile, "AppData", "Local"),
    capturesDir,
  ]) {
    mkdirSync(dir, { recursive: true });
  }
  const server = await startCspServer({ dist: resolve("dist"), port: 5173, csp: policy });
  const child = spawn(appExe, [], {
    env: {
      ...process.env,
      LOCALAPPDATA: localappdata,
      APPDATA: join(userprofile, "AppData", "Roaming"),
      USERPROFILE: userprofile,
      HANG_CDP_PORT: cdpPort,
      HANG_E2E_DIR: caseDir,
    },
    stdio: "ignore",
  });
  let exitInfo = null;
  const exited = new Promise((done) => child.once("exit", (code, signal) => done({ code, signal })));
  void exited.then((info) => {
    exitInfo = info;
  });
  let panel;
  const shutdown = async () => {
    try {
      panel?.close();
    } catch {
      // the page may already be gone
    }
    child.kill();
    const finished = await Promise.race([exited, sleep(4000).then(() => "timeout")]);
    if (finished === "timeout") {
      spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    }
    await server.close();
  };
  try {
    try {
      panel = await connectTo("window=panel", 45000);
    } catch (error) {
      throw new Error(
        `${error instanceof Error ? error.message : String(error)} (app exit: ${JSON.stringify(exitInfo)})`,
      );
    }
    // The folder watchers are registered during the app setup, slightly after the first webview
    // target exists; a fixture copied into the watched folder before that is never seen.
    await sleep(2500);
    // Copy the fixture only after the app is up: the isolated profile starts empty.
    copyFileSync(fixture, join(capturesDir, "p3-trim-fixture.mp4"));
    const stateRaw = await waitFor(
      panel,
      `(async () => {
        const state = await window.__TAURI_INTERNALS__.invoke("get_app_state");
        return state.items.length === 1 ? JSON.stringify(state) : "";
      })()`,
      "fixture ingest",
      30000,
      150,
    );
    const item = JSON.parse(stateRaw).items[0];
    return { server, child, exited, panel, item, capturesDir, target: join(capturesDir, "p3-trim-fixture.mp4") };
  } catch (error) {
    await shutdown();
    throw error;
  }
}

async function stopCase(running) {
  try {
    running.panel.close();
  } catch {
    // the page may already be gone
  }
  running.child.kill();
  const finished = await Promise.race([running.exited, sleep(4000).then(() => "timeout")]);
  if (finished === "timeout") {
    spawnSync("taskkill", ["/PID", String(running.child.pid), "/T", "/F"], { stdio: "ignore" });
  }
  await running.server.close();
}

/** Opens the trim window and returns the trim CDP page with the violation recorder installed. */
async function openTrimPage(panel, item) {
  await invoke(panel, "open_trim", { id: item.id });
  const list = await waitForTrimTarget();
  if (list.length !== 1) throw new Error(`expected one trim window, got ${list.length}`);
  const trim = await connectTo("window=trim", 20000);
  const root = JSON.parse(
    await waitFor(
      trim,
      `(() => {
        const root = document.querySelector(".trim");
        return root && root.dataset.targetId ? JSON.stringify({ targetId: root.dataset.targetId }) : "";
      })()`,
      "trim page target",
      30000,
      100,
    ),
  );
  if (root.targetId !== item.id) throw new Error(`trim page shows ${root.targetId}, expected ${item.id}`);
  // Reload with the recorder installed so violations from the very first media request are caught
  // (the page fetches the preview immediately on mount).
  await trim.send("Page.addScriptToEvaluateOnNewDocument", { source: VIOLATION_RECORDER });
  await trim.send("Page.reload", { ignoreCache: true });
  await waitFor(
    trim,
    `(() => {
      const root = document.querySelector(".trim");
      return root && root.dataset.targetId ? "ready" : "";
    })()`,
    "trim page after reload",
    30000,
    100,
  );
  return trim;
}

/** Forces a frame fetch so `readyState >= 2` is required, not just metadata (preload=metadata). */
async function forceVideoLoad(trim) {
  await evaluateWithin(
    trim,
    `(() => {
      const video = document.querySelector(".trim__video");
      if (!video) return false;
      video.preload = "auto";
      video.load();
      return true;
    })()`,
    2000,
  );
}

/** The positive assertion: frame data is available and no media-src violation was recorded. */
async function assertVideoLoads(trim, timeoutMs = 20000) {
  await forceVideoLoad(trim);
  const deadline = Date.now() + timeoutMs;
  let state = null;
  while (Date.now() < deadline) {
    state = await readVideoState(trim);
    if (state && state.readyState >= 2) break;
    await sleep(150);
  }
  const violations = await readCspViolations(trim);
  const mediaViolations = violations.filter((entry) => /media-src/.test(entry.directive ?? ""));
  return { state, violations, mediaViolations, loaded: Boolean(state && state.readyState >= 2) };
}

async function setRangeAndSave(trim, startMs, endMs) {
  const applied = await evaluateWithin(
    trim,
    `(() => {
      const root = document.querySelector(".trim");
      const start = root?.querySelector('[data-slider="start"]');
      const end = root?.querySelector('[data-slider="end"]');
      if (!start || !end) return false;
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
      setter.call(start, ${JSON.stringify(String(startMs))});
      start.dispatchEvent(new Event("input", { bubbles: true }));
      setter.call(end, ${JSON.stringify(String(endMs))});
      end.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    })()`,
    3000,
  );
  if (!applied) throw new Error("trim sliders not found");
  await waitFor(
    trim,
    `(() => {
      const root = document.querySelector(".trim");
      return root?.dataset.startMs === ${JSON.stringify(String(startMs))} &&
        root?.dataset.endMs === ${JSON.stringify(String(endMs))}
        ? "range"
        : "";
    })()`,
    "trim range",
    5000,
    50,
  );
  const button = JSON.parse(
    await evaluate(
      trim,
      `(() => {
        const button = document.querySelector('[data-action="save"]');
        if (!button || button.disabled) return "null";
        const rect = button.getBoundingClientRect();
        return JSON.stringify({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
      })()`,
    ),
  );
  if (!button) throw new Error("Save is disabled under the tested CSP");
  for (const type of ["mouseMoved", "mousePressed"]) {
    await trim.send("Input.dispatchMouseEvent", {
      type,
      x: Math.round(button.x),
      y: Math.round(button.y),
      button: "left",
      buttons: type === "mousePressed" ? 1 : 0,
      clickCount: 1,
    });
  }
  await sleep(30);
  await trim.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: Math.round(button.x),
    y: Math.round(button.y),
    button: "left",
    buttons: 0,
    clickCount: 1,
  });
}

async function runPositive() {
  const name = "positive";
  step("positive case: packaged CSP with media-src", { csp });
  const running = await startCase(name, csp);
  try {
    const trim = await openTrimPage(running.panel, running.item);
    step("positive: trim window opened under the packaged CSP");
    const video = await assertVideoLoads(trim);
    check("packaged CSP: trim <video> reaches readyState >= 2", video.loaded, video.state);
    check(
      "packaged CSP: no securitypolicyviolation for media-src",
      video.mediaViolations.length === 0,
      video.mediaViolations,
    );
    // The header actually used in this run (proves the harness served the exact policy).
    const header = await evaluate(
      trim,
      `fetch(location.href).then((response) => response.headers.get("content-security-policy"))`,
    );
    check("the trim page was served with the exact configured CSP header", header === csp, {
      header,
    });
    await trim.send("Page.captureScreenshot", { format: "png", fromSurface: true }).then((shot) => {
      writeFileSync(
        join(evidenceDir, "trim-csp-positive.png"),
        Buffer.from(shot.data, "base64"),
      );
    });
    const before = {
      sha256: sha256(running.target),
      bytes: statSync(running.target).size,
      itemHash: running.item.hash,
    };
    await setRangeAndSave(trim, 1000, 2500);
    await waitFor(
      running.panel,
      `document.querySelector(".panel")?.dataset.trimOpen === "false" ? "closed" : ""`,
      "trim close after the CSP save",
      90000,
      100,
    );
    const state = await invoke(running.panel, "get_app_state");
    const afterItem = state?.items?.[0];
    const after = {
      sha256: sha256(running.target),
      bytes: statSync(running.target).size,
      itemHash: afterItem?.hash,
      itemUpdatedAt: afterItem?.updatedAt,
      items: (state?.items ?? []).length,
    };
    results.cases.positive = { video, before, after, violations: video.violations };
    check(
      "packaged CSP: Save still exports and replaces the recording",
      after.items === 1 && after.sha256 !== before.sha256 && after.itemHash !== before.itemHash,
      { before, after },
    );
    check("packaged CSP: no trim temp residue", readdirSync(running.capturesDir).every(
      (entry) => !entry.startsWith(".hangline-trim-") && !entry.toLowerCase().endsWith(".tmp"),
    ), readdirSync(running.capturesDir));
    step("positive case complete");
    return results.cases.positive;
  } finally {
    await stopCase(running);
  }
}

async function runNegative() {
  const name = "negative";
  step("negative control: pre-fix CSP without media-src", { csp: legacyCsp });
  const running = await startCase(name, legacyCsp);
  try {
    const trim = await openTrimPage(running.panel, running.item);
    // Wait for the blocked media element to settle instead of demanding readyState >= 2.
    const deadline = Date.now() + 15000;
    let state = null;
    while (Date.now() < deadline) {
      state = await readVideoState(trim);
      const violations = await readCspViolations(trim);
      if (
        (state && (state.error !== null || state.videoFailed === "true")) ||
        violations.some((entry) => /media-src/.test(entry.directive ?? ""))
      ) {
        break;
      }
      await sleep(200);
    }
    const violations = await readCspViolations(trim);
    const mediaViolations = violations.filter((entry) => /media-src/.test(entry.directive ?? ""));
    const blocked = Boolean(
      state &&
        (state.error !== null || state.videoFailed === "true" || state.readyState < 2),
    );
    results.cases.negative = { video: state, violations, mediaViolations };
    check(
      "negative control: the pre-fix CSP blocks the trim <video>",
      blocked && mediaViolations.length > 0,
      { state, mediaViolations },
    );
    check(
      "negative control: the positive assertion would have failed",
      !(state && state.readyState >= 2) && mediaViolations.length > 0,
      state,
    );
    step("negative case complete");
    return results.cases.negative;
  } finally {
    await stopCase(running);
  }
}

console.log(`trim-csp-e2e: app ${appExe}`);
console.log(`trim-csp-e2e: configured CSP ${csp}`);
console.log(`trim-csp-e2e: legacy CSP     ${legacyCsp}`);

const positive = await runPositive();
const negative = await runNegative();

results.verdict = failures === 0 ? "PASS" : `FAIL (${failures} check(s))`;
results.checkCount = Object.keys(results.checks).length;
results.passed = results.checkCount - failures;
results.failures = failures;
results.environment = {
  cdpPort,
  appExe,
  fixture,
  evidenceDir,
  positiveLoaded: positive.video.loaded,
  negativeBlocked: negative.mediaViolations.length > 0,
};
writeFileSync(join(evidenceDir, "trim-csp-e2e.json"), `${JSON.stringify(results, null, 2)}\n`);
console.log(`verdict: ${results.verdict} (${results.passed}/${results.checkCount} checks passed)`);
process.exit(failures === 0 ? 0 : 1);
