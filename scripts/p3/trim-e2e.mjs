#!/usr/bin/env node
/* global Buffer */
/**
 * P3-6/P3-7 end-to-end evidence: recording card → trusted long-press → trim window → range →
 * failure fallback → save → playable trimmed file + refreshed card.
 *
 * Dev-only (Node built-ins + the P2 CDP helper). Runbook:
 *
 *   node scripts/p3/fixture.mjs --profile "$env:TEMP\hangline-p3-trim\localappdata\Hangline" --recordings
 *   .\scripts\p3\fixture-video.ps1 -Out "$env:TEMP\hangline-p3-trim\fixture.mp4"
 *   # start the debug app with HANG_CDP_PORT=9225, isolated APPDATA/LOCALAPPDATA/USERPROFILE
 *   $env:HANG_E2E_DIR = "$env:TEMP\hangline-p3-trim"
 *   $env:HANG_APP_EXE = "$env:TEMP\hangline-p3-trim\hangline.exe"
 *   node scripts/p3/trim-e2e.mjs
 *
 * The script copies the 3 s fixture (H.264 + a 440 Hz AAC track) into the isolated
 * `Videos\Captures` only after the app start (never the real folder), holds the card for 520 ms
 * with CDP trusted input, and exercises Esc, single-window re-targeting, the failure fallback and
 * the save.
 *
 * Before the successful save it locks the file with `[IO.File]::Open(…, 'Open', 'Read', 'Read')`
 * in a helper PowerShell process: reads stay allowed (so the WinRT render succeeds and the export
 * really starts), delete/rename stays denied (so the atomic replace fails). The harness also polls
 * the watched folder while that export runs and requires a `.hangline-trim-*` temp to appear — the
 * proof that the failure was injected *after* `create_render_destination` rather than before the
 * export (the earlier `FileShare.None` lock failed in `video::duration_ms` and proved nothing).
 *
 * Verified: exactly one recording item before/after (no watcher duplicate), no copy/open command
 * from the trusted long-press, target duration ≈3000 ms, `ffprobe` duration ≈1.5 s ±0.2 with the
 * audio stream preserved, no temp residue, original replaced atomically, preview regenerated
 * (new bytes/`?v=`), item hash/updatedAt updated, trim window closed, panel busy/trim state
 * cleared, the coarse progress milestones observed (including 0.6 on the failure path).
 *
 * The packaged-CSP preview (`media-src`) is proven separately by `scripts/p3/trim-csp-e2e.mjs`
 * because the Vite dev server sends no CSP.
 *
 * Screenshots and the JSON report land in `.superpowers/p3-evidence/` (git-ignored).
 */

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { connectTo, evaluate, screenshotClip, sleep, targets } from "../p2/cdp.mjs";

const base = process.env.HANG_E2E_DIR;
if (!base) {
  console.error("trim-e2e: HANG_E2E_DIR is required (the temp run directory)");
  process.exit(2);
}
const dataDir = join(base, "localappdata", "Hangline");
const statePath = join(dataDir, "state.json");
const capturesDir = join(base, "userprofile", "Videos", "Captures");
const fixture = join(base, "fixture.mp4");
const evidenceDir = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p3-evidence");
const appExe = resolve(process.env.HANG_APP_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"));
const lockReadyPath = join(base, "trim-lock.ready");
mkdirSync(evidenceDir, { recursive: true });

const results = { checks: {}, steps: [], manualPending: [] };
let failures = 0;

function check(name, condition, detail) {
  results.checks[name] = { pass: Boolean(condition), detail };
  if (!condition) failures += 1;
  console.log(`${condition ? "PASS" : "FAIL"} ${name}${detail === undefined ? "" : `: ${JSON.stringify(detail)}`}`);
}

function step(name, detail) {
  results.steps.push({ name, at: new Date().toISOString(), detail });
  console.log(`step: ${name}`);
}

async function invoke(cdp, command, args = {}) {
  return evaluate(
    cdp,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

async function waitFor(cdp, expression, label, timeoutMs = 30000, intervalMs = 100) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      // Bounded per probe: a destroyed target's CDP reply never arrives, and the deadline (not a
      // dead socket) must decide when a window is gone.
      last = await Promise.race([
        evaluate(cdp, expression),
        sleep(Math.min(2000, intervalMs + 1500)).then(() => undefined),
      ]);
    } catch (error) {
      last = `error: ${error instanceof Error ? error.message : String(error)}`;
    }
    if (last) return last;
    await sleep(intervalMs);
  }
  throw new Error(`timed out waiting for ${label} (last=${JSON.stringify(last)})`);
}

/**
 * Bounded evaluate. The CDP wrapper's `send` promise never settles when the target window is
 * destroyed (the socket closes without a reply), so polling a page that may close must race a
 * timeout instead of awaiting it forever.
 */
async function evaluateWithin(cdp, expression, timeoutMs = 1000) {
  return Promise.race([evaluate(cdp, expression), sleep(timeoutMs).then(() => undefined)]);
}

/* --- trusted input helpers (mirrors the P3-4/P3-5 harnesses) -------------------------------- */

async function mouse(cdp, type, x, y, held = true) {
  await cdp.send("Input.dispatchMouseEvent", {
    type,
    x: Math.round(x),
    y: Math.round(y),
    button: "left",
    buttons: held ? 1 : 0,
    clickCount: 1,
  });
}

async function clickAt(cdp, x, y) {
  await mouse(cdp, "mouseMoved", x, y, false);
  await mouse(cdp, "mousePressed", x, y, true);
  await sleep(24);
  await mouse(cdp, "mouseReleased", x, y, false);
  await sleep(64);
}

async function clickSelector(cdp, selector) {
  const box = await evaluate(
    cdp,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return JSON.stringify({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
    })()`,
  );
  if (!box) throw new Error(`no element matches ${selector}`);
  const point = JSON.parse(box);
  await clickAt(cdp, point.x, point.y);
}

/** Holds the left button at `point` long enough for the 450 ms deadline to fire. */
async function longPress(cdp, point, holdMs = 520) {
  await mouse(cdp, "mouseMoved", point.x, point.y, false);
  await mouse(cdp, "mousePressed", point.x, point.y, true);
  await sleep(holdMs);
  await mouse(cdp, "mouseReleased", point.x, point.y, false);
}

async function screenshot(cdp, name) {
  const captured = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  writeFileSync(join(evidenceDir, name), Buffer.from(captured.data, "base64"));
}

/** Sends a trusted Escape key press to the page (the trim page's only keyboard shortcut). */
async function pressEscape(cdp) {
  const base = {
    key: "Escape",
    code: "Escape",
    windowsVirtualKeyCode: 27,
    nativeVirtualKeyCode: 27,
  };
  await cdp.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
  await sleep(24);
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
  await sleep(120);
}

/**
 * Wraps `__TAURI_INTERNALS__.invoke` so the harness can prove which commands a gesture issued.
 * With `stubOpenItem` the fallback button is clicked without launching the user's media player.
 */
async function installInvokeRecorder(cdp, { stubOpenItem = false } = {}) {
  const installed = await evaluate(
    cdp,
    `(() => {
      if (window.__HANG_INVOKES) return true;
      window.__HANG_INVOKES = [];
      window.__HANG_OPENED = [];
      const original = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = function (command, args, options) {
        window.__HANG_INVOKES.push({ command, args: args ?? null });
        const stub = ${stubOpenItem ? "true" : "false"};
        if (stub && command === "open_item") {
          window.__HANG_OPENED.push(args ?? null);
          return Promise.resolve();
        }
        return original.call(window.__TAURI_INTERNALS__, command, args, options);
      };
      return true;
    })()`,
  );
  return Boolean(installed);
}

async function readInvokes(cdp) {
  const raw = await evaluateWithin(cdp, `JSON.stringify(window.__HANG_INVOKES ?? [])`, 2000).catch(
    () => null,
  );
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch {
    return [];
  }
}

async function readOpened(cdp) {
  const raw = await evaluateWithin(cdp, `JSON.stringify(window.__HANG_OPENED ?? [])`, 2000).catch(
    () => null,
  );
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch {
    return [];
  }
}

/**
 * Polls the export failure state while watching the watched folder for the render temp.
 *
 * Seeing `.hangline-trim-*` before the error proves the export reached
 * `create_render_destination`/the render, so the failure really is the atomic replace (the old
 * `FileShare.None` injection failed before this point and the "no temp afterwards" check was
 * vacuous).
 */
async function waitForExportFailure(trim, capturesDir, timeoutMs = 90000) {
  const deadline = Date.now() + timeoutMs;
  let sawTemp = false;
  let tempSamples = [];
  let errorState = null;
  while (Date.now() < deadline) {
    const residueNow = residue(capturesDir, ".hangline-trim-");
    if (residueNow.length > 0) {
      sawTemp = true;
      for (const name of residueNow) {
        if (!tempSamples.includes(name)) tempSamples.push(name);
      }
    }
    const raw = await evaluateWithin(
      trim,
      `(() => {
        const root = document.querySelector(".trim");
        if (!root || root.dataset.error !== "true") return "";
        const button = root.querySelector('[data-action="open-player"]');
        return JSON.stringify({
          error: root.dataset.error,
          saving: root.dataset.saving,
          progress: root.dataset.progress,
          hasFallback: Boolean(button),
          fallbackEnabled: button ? !button.disabled : false,
          message: root.querySelector("[data-save-error] span")?.textContent ?? "",
        });
      })()`,
      1500,
    ).catch(() => "");
    if (raw) {
      errorState = JSON.parse(raw);
      break;
    }
    await sleep(80);
  }
  return { sawTemp, tempSamples, errorState };
}

/** The panel is a transparent strip: paint a neutral backdrop only for the capture. */
async function capturePanel(cdp, name, clip) {
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", {
    color: { r: 26, g: 30, b: 35, a: 1 },
  });
  if (clip) await screenshotClip(cdp, join(evidenceDir, name), clip);
  else await screenshot(cdp, name);
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", {}).catch(() => undefined);
}

async function isRevealed(panel) {
  return evaluate(panel, `document.querySelector(".panel")?.dataset.revealed === "true"`).catch(
    () => false,
  );
}

/** Reveals the (click-through) panel if Rust tucked it (same action as the tray toggle). */
async function ensureRevealed(panel) {
  // A newly ingested item reveals the line on its own (new-item peek). Wait for that animation
  // before toggling: an impatient toggle would tuck the panel that is just becoming visible.
  const peekDeadline = Date.now() + 4000;
  while (Date.now() < peekDeadline && !(await isRevealed(panel))) {
    await sleep(150);
  }
  if (!(await isRevealed(panel))) {
    if (existsSync(appExe)) {
      try {
        const child = spawn(appExe, [], { detached: true, stdio: "ignore", env: { ...process.env } });
        child.unref();
      } catch (error) {
        console.warn("trim-e2e: cannot spawn the toggle instance", error);
      }
    }
    await waitFor(
      panel,
      `document.querySelector(".panel")?.dataset.revealed === "true"`,
      "panel reveal",
      15000,
    ).catch(() => undefined);
  }
  // The reveal is a spring animation: a card measured while it is still sliding can sit offscreen,
  // so wait for the motion to settle before computing the hold point.
  await sleep(800);
  return isRevealed(panel);
}

async function cardCenter(panel, id) {
  // `elementFromPoint` proves the hold will land on this card's surface (the panel is click-through
  // by design, so a stale rect would otherwise press the transparent window instead).
  const raw = await waitFor(
    panel,
    `(() => {
      const slot = document.querySelector('.glass-card-slot[data-item-id="${id}"]');
      const article = slot?.querySelector("article");
      if (!article) return "";
      const rect = article.getBoundingClientRect();
      const x = rect.left + rect.width / 2;
      const y = rect.top + rect.height / 2;
      if (!(x > 2 && y > 2 && x < window.innerWidth - 2 && y < window.innerHeight - 2)) return "";
      const hit = document.elementFromPoint(x, y);
      if (!hit || !slot.contains(hit)) return "";
      return JSON.stringify({ x, y });
    })()`,
    `hittable card ${id}`,
    10000,
    100,
  );
  return JSON.parse(raw);
}

function targetCount(list, marker) {
  return list.filter((target) => target.type === "page" && target.url.includes(marker)).length;
}

/** Polls the CDP target list until the trim page appears (window creation is async). */
async function waitForTrimTarget(timeoutMs = 15000) {
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

/** Polls until no `window=trim` page is left (Esc close and the successful-save close). */
async function waitForNoTrimTarget(timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const list = (await targets()).filter(
      (candidate) => candidate.type === "page" && candidate.url.includes("window=trim"),
    );
    if (list.length === 0) return true;
    await sleep(150);
  }
  return false;
}

/* --- filesystem helpers -------------------------------------------------------------------- */

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function pngSize(path) {
  const bytes = readFileSync(path);
  if (bytes.length < 24 || bytes.toString("ascii", 1, 4) !== "PNG") return null;
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

function residue(dir, prefix) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).filter(
    (name) => name.toLowerCase().endsWith(".tmp") || name.startsWith(prefix),
  );
}

/** ffprobe JSON: format duration/name plus the stream codec types (`null` when unavailable). */
function ffprobe(path) {
  const probe = spawnSync(
    "ffprobe",
    [
      "-v",
      "error",
      "-show_entries",
      "format=duration,format_name:stream=codec_type",
      "-of",
      "json",
      path,
    ],
    { encoding: "utf8", timeout: 30000 },
  );
  if (probe.status !== 0) {
    return { error: probe.stderr?.trim() || `ffprobe exit ${probe.status}` };
  }
  try {
    return JSON.parse(probe.stdout);
  } catch (error) {
    return { error: String(error) };
  }
}

/* --- the fixture lock (failure path) ------------------------------------------------------- */

let lockProcess = null;

function startFileLock(path) {
  if (existsSync(lockReadyPath)) unlinkSync(lockReadyPath);
  const script = [
    "$ErrorActionPreference='Stop'",
    // FileShare.Read: the WinRT render can still read the source, but the atomic replace needs
    // DELETE access on it, so the export fails at `fs::rename` — after the render/temp exist.
    `$f=[System.IO.File]::Open('${path.replace(/'/g, "''")}','Open','Read','Read')`,
    `[System.IO.File]::WriteAllText('${lockReadyPath.replace(/'/g, "''")}','1')`,
    "Start-Sleep -Seconds 120",
  ].join("; ");
  lockProcess = spawn("pwsh", ["-NoProfile", "-NonInteractive", "-Command", script], {
    stdio: "ignore",
  });
  return lockProcess;
}

async function waitForLock(timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(lockReadyPath)) return true;
    await sleep(100);
  }
  return false;
}

async function releaseFileLock() {
  if (lockProcess && lockProcess.exitCode === null) {
    lockProcess.kill();
    lockProcess = null;
  }
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    try {
      const handle = await import("node:fs/promises").then((fs) => fs.open(targetFile(), "r"));
      await handle.close();
      return true;
    } catch {
      await sleep(100);
    }
  }
  return false;
}

function targetFile() {
  return join(capturesDir, "p3-trim-fixture.mp4");
}

/* --- 1. connect + empty line --------------------------------------------------------------- */

const panel = await connectTo("window=panel");
const before = await invoke(panel, "get_app_state");
check("profile starts with an empty line", (before?.items ?? []).length === 0, (before?.items ?? []).length);
step("panel connected", { dataDir });

// --- 2. copy the fixture AFTER app start -----------------------------------------------------

if (!existsSync(fixture)) {
  throw new Error(`missing fixture mp4 at ${fixture}; run scripts/p3/fixture-video.ps1 first`);
}
mkdirSync(capturesDir, { recursive: true });
const target = targetFile();
copyFileSync(fixture, target);
step("fixture copied into the watched recordings folder after startup", target);

// --- 3. wait for ingest -----------------------------------------------------------------------

const ingestStart = Date.now();
const state = JSON.parse(
  await waitFor(
    panel,
    `(async () => {
      const state = await window.__TAURI_INTERNALS__.invoke("get_app_state");
      return state.items.length > 0 ? JSON.stringify(state) : "";
    })()`,
    "recording ingest",
    30000,
    150,
  ),
);
const ingestMs = Date.now() - ingestStart;
const item = state.items[0];
results.ingest = { ms: ingestMs, item };
check("exactly one recording item was ingested", state.items.length === 1, state.items.length);
check("item kind is recording", item?.kind === "recording", item?.kind);
check(
  "natural dimensions were repaired from the first frame",
  item?.width === 640 && item?.height === 360,
  { width: item?.width, height: item?.height },
);
const original = {
  sha256: sha256(target),
  bytes: statSync(target).size,
  mtimeMs: statSync(target).mtimeMs,
};
const thumbBeforePath = item?.thumbPath ?? "";
const thumbBefore = thumbBeforePath && existsSync(thumbBeforePath)
  ? { sha256: sha256(thumbBeforePath), bytes: statSync(thumbBeforePath).size, png: pngSize(thumbBeforePath) }
  : null;
results.original = { ...original, thumbBeforePath, thumbBefore };
const sourceProbe = ffprobe(target);
results.sourceProbe = sourceProbe;
check(
  "the fixture carries a real audio stream to preserve",
  (sourceProbe?.streams ?? []).some((stream) => stream.codec_type === "audio"),
  { streams: sourceProbe?.streams, error: sourceProbe?.error },
);
step("recording ingested", { ingestMs, id: item?.id, original });

// --- 4. trusted long-press opens the trim window ---------------------------------------------

check("panel is revealed before the long-press", await ensureRevealed(panel));
// Record every IPC command the long-press issues: a completed hold must open the trim window and
// must not copy/open/drag the recording (the short-click path owns `copy_item`).
check(
  "panel invoke recorder installed before the long-press",
  await installInvokeRecorder(panel),
);
const center = await cardCenter(panel, item.id);
const openStart = Date.now();
await longPress(panel, center, 520);
await waitFor(
  panel,
  `document.querySelector(".panel")?.dataset.trimOpen === "true"`,
  "panel trim busy state",
  5000,
  100,
).catch(() => undefined);
const trimList = await waitForTrimTarget();
const openMs = Date.now() - openStart;
results.trimOpen = { ms: openMs, targets: trimList.map((candidate) => candidate.url) };
check("trusted 520 ms long-press opened exactly one trim window", trimList.length === 1, trimList.length);
check("trim window opened within 3 s", openMs < 3000, openMs);

let trim = await connectTo("window=trim");const trimRoot = JSON.parse(
  await waitFor(
    trim,
    `(() => {
      const root = document.querySelector(".trim");
      if (!root || !root.dataset.targetId) return "";
      return JSON.stringify({
        targetId: root.dataset.targetId,
        durationMs: Number(root.dataset.durationMs),
        startMs: Number(root.dataset.startMs),
        endMs: Number(root.dataset.endMs),
        videoReady: root.dataset.videoReady,
        refused: root.dataset.refused,
      });
    })()`,
    "trim target",
    30000,
    100,
  ),
);
results.trimTarget = trimRoot;
check("trim page targets the recording", trimRoot.targetId === item.id, {
  targetId: trimRoot.targetId,
  itemId: item.id,
});
check(
  "target duration is ≈3000 ms from the media stack",
  Math.abs(trimRoot.durationMs - 3000) <= 150,
  trimRoot.durationMs,
);
check("selection starts on the full clip", trimRoot.startMs === 0 && trimRoot.endMs === trimRoot.durationMs, {
  start: trimRoot.startMs,
  end: trimRoot.endMs,
});
check("trim page is not refused for the fixture", trimRoot.refused === "false", trimRoot.refused);
const videoReady = await waitFor(
  trim,
  `document.querySelector(".trim")?.dataset.videoReady === "true" ? "true" : ""`,
  "trim video metadata",
  20000,
  100,
)
  .then(() => true)
  .catch(() => false);
check("trim preview <video> loaded the recording metadata", videoReady, {
  readyAtTargetSnapshot: trimRoot.videoReady,
});

// A completed hold is not a click: no copy, no default player, no markup editor.
const panelInvokes = await readInvokes(panel);
const longPressCommands = panelInvokes.map((entry) => entry.command);
results.longPressCommands = longPressCommands;
check(
  "long-press opened the trim window without copying or opening the recording",
  !longPressCommands.includes("copy_item") &&
    !longPressCommands.includes("open_item") &&
    !longPressCommands.includes("open_annotate"),
  longPressCommands,
);
check(
  "long-press did not duplicate the recording file or the item",
  readdirSync(capturesDir).filter((name) => /\.(mp4|mov)$/i.test(name)).length === 1 &&
    ((await invoke(panel, "get_app_state"))?.items ?? []).length === 1,
  { files: readdirSync(capturesDir) },
);

const trimState = JSON.parse(
  await invoke(
    trim,
    "get_trim_target",
    { id: item.id },
  ).then((value) => JSON.stringify(value)),
);
results.trimState = {
  sizeBytes: trimState.sizeBytes,
  durationMs: trimState.durationMs,
  maxBytes: trimState.maxBytes,
  maxDurationMs: trimState.maxDurationMs,
  theme: trimState.theme,
};
check("get_trim_target returns the 2 GiB / 60 min limits", trimState.maxBytes === 2 * 1024 * 1024 * 1024 && trimState.maxDurationMs === 3600000, {
  maxBytes: trimState.maxBytes,
  maxDurationMs: trimState.maxDurationMs,
});
check("get_trim_target reports the fixture size", trimState.sizeBytes === original.bytes, {
  sizeBytes: trimState.sizeBytes,
  bytes: original.bytes,
});

// The panel must stay revealed and busy while the trim window is open.
await sleep(900);
const panelWhileOpen = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const root = document.querySelector(".panel");
      return JSON.stringify({
        revealed: root?.dataset.revealed,
        busy: root?.dataset.busy,
        trimOpen: root?.dataset.trimOpen,
        editorOpen: root?.dataset.editorOpen,
      });
    })()`,
  ),
);
results.panelWhileOpen = panelWhileOpen;
check(
  "panel stays revealed and busy while the trim window is open",
  panelWhileOpen.revealed === "true" &&
    panelWhileOpen.busy === "true" &&
    panelWhileOpen.trimOpen === "true" &&
    panelWhileOpen.editorOpen === "false",
  panelWhileOpen,
);

await screenshot(trim, "trim-window.png");
step("trim window evidence captured", { file: "trim-window.png", openMs });

// --- 4b. Esc closes without saving; reopening reuses the single window -----------------------

await pressEscape(trim);
const escClosed = await waitForNoTrimTarget(10000);
results.escClose = { closed: escClosed };
check("Esc closed the trim window without saving", escClosed, escClosed);
check(
  "Esc close cleared the panel trim busy state",
  await waitFor(
    panel,
    `document.querySelector(".panel")?.dataset.trimOpen === "false" ? "true" : ""`,
    "panel trim state after Esc",
    10000,
    100,
  ).then(() => true).catch(() => false),
  await evaluate(panel, `document.querySelector(".panel")?.dataset.trimOpen`).catch(() => null),
);
const itemAfterEsc = (await invoke(panel, "get_app_state"))?.items?.[0];
check(
  "Esc left the recording untouched",
  itemAfterEsc?.id === item.id &&
    itemAfterEsc?.hash === item.hash &&
    itemAfterEsc?.updatedAt === item.updatedAt &&
    sha256(target) === original.sha256,
  { hash: itemAfterEsc?.hash, expected: item.hash },
);

// The long-press command is also the re-target path: a second open must reuse the one window.
await invoke(panel, "open_trim", { id: item.id });
const reopened = await waitForTrimTarget();
check("reopening created exactly one trim window", reopened.length === 1, reopened.length);
await invoke(panel, "open_trim", { id: item.id });
await sleep(500);
const reuseTargets = (await targets()).filter(
  (candidate) => candidate.type === "page" && candidate.url.includes("window=trim"),
);
const editorState = await invoke(panel, "get_editor_state");
results.retarget = { windows: reuseTargets.length, editorState };
check(
  "a second open_trim re-targets the single window instead of creating another",
  reuseTargets.length === 1 && editorState?.trimOpen === true,
  { windows: reuseTargets.length, editorState },
);
trim = await connectTo("window=trim");
const reopenedTarget = JSON.parse(
  await waitFor(
    trim,
    `(() => {
      const root = document.querySelector(".trim");
      if (!root || !root.dataset.targetId) return "";
      return JSON.stringify({ targetId: root.dataset.targetId, durationMs: Number(root.dataset.durationMs) });
    })()`,
    "reopened trim target",
    30000,
    100,
  ),
);
check(
  "the reused window still targets the recording",
  reopenedTarget.targetId === item.id && Math.abs(reopenedTarget.durationMs - 3000) <= 150,
  reopenedTarget,
);
check(
  "the reused window reloaded the recording preview",
  await waitFor(
    trim,
    `document.querySelector(".trim")?.dataset.videoReady === "true" ? "true" : ""`,
    "reopened trim video",
    20000,
    100,
  ).then(() => true).catch(() => false),
  true,
);

// --- 5. set the range with the real slider inputs --------------------------------------------

const setRange = async (startMs, endMs) => {
  const applied = await evaluate(
    trim,
    `(() => {
      const root = document.querySelector(".trim");
      const start = root?.querySelector('[data-slider="start"]');
      const end = root?.querySelector('[data-slider="end"]');
      if (!start || !end) return null;
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
      setter.call(start, ${JSON.stringify(String(startMs))});
      start.dispatchEvent(new Event("input", { bubbles: true }));
      setter.call(end, ${JSON.stringify(String(endMs))});
      end.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    })()`,
  );
  if (!applied) throw new Error("trim sliders not found");
  return JSON.parse(
    await waitFor(
      trim,
      `(() => {
        const root = document.querySelector(".trim");
        return root?.dataset.startMs === ${JSON.stringify(String(startMs))} &&
          root?.dataset.endMs === ${JSON.stringify(String(endMs))}
          ? JSON.stringify({ start: Number(root.dataset.startMs), end: Number(root.dataset.endMs) })
          : "";
      })()`,
      "trim range state",
      5000,
      50,
    ),
  );
};

const chosen = await setRange(1000, 2500);
results.range = chosen;
check("sliders accepted 1.0–2.5 s", chosen.start === 1000 && chosen.end === 2500, chosen);

/** Injects a progress recorder that survives the window closing (written to localStorage). */
async function installProgressRecorder() {
  const installed = await evaluate(
    trim,
    `(() => {
      localStorage.removeItem("hangline.trim-progress");
      const handlerId = window.__TAURI_INTERNALS__.transformCallback((event) => {
        const list = JSON.parse(localStorage.getItem("hangline.trim-progress") || "[]");
        list.push(event.payload);
        localStorage.setItem("hangline.trim-progress", JSON.stringify(list));
      });
      window.__TAURI_INTERNALS__.invoke("plugin:event|listen", {
        event: "trim-progress",
        target: { kind: "Any" },
        handler: handlerId,
      });
      return true;
    })()`,
  );
  return Boolean(installed);
}

/** Records which `data-action` the next trusted clicks hit (failure-path diagnostics). */
async function installClickRecorder() {
  return evaluate(
    trim,
    `(() => {
      window.__HANG_CLICKS = [];
      document.addEventListener("click", (event) => {
        const action = event.target?.closest?.("[data-action]")?.dataset.action ?? null;
        window.__HANG_CLICKS.push({ action, x: event.clientX, y: event.clientY });
      }, true);
      return true;
    })()`,
  );
}

async function readClicks() {
  const raw = await evaluateWithin(trim, `JSON.stringify(window.__HANG_CLICKS ?? [])`, 1500).catch(
    () => null,
  );
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch {
    return [];
  }
}

async function readProgressLog() {
  const raw = await evaluate(
    panel,
    `localStorage.getItem("hangline.trim-progress")`,
  ).catch(() => null);
  if (!raw) return [];
  try {
    return JSON.parse(raw);
  } catch {
    return [];
  }
}

// --- 6. failure fallback: lock the file, save, expect the error panel -------------------------

check("progress recorder installed in the trim page", await installProgressRecorder());
check("click recorder installed in the trim page", Boolean(await installClickRecorder()));
check(
  "invoke recorder with a stubbed open_item installed in the trim page",
  await installInvokeRecorder(trim, { stubOpenItem: true }),
);
startFileLock(target);
const locked = await waitForLock();
results.failure = { locked };
check(
  "helper process holds the recording with FileShare.Read (render allowed, replace denied)",
  locked,
  locked,
);
if (locked) {
  await clickSelector(trim, '[data-action="save"]');
  const failure = await waitForExportFailure(trim, capturesDir);
  const errorState = failure.errorState;
  results.failure = { ...results.failure, ...failure, clicks: await readClicks() };
  check(
    "the failure-path export created a .hangline-trim temp before failing",
    failure.sawTemp,
    failure.tempSamples,
  );
  check(
    "failed export shows the error panel + Open in default player",
    errorState?.error === "true" && errorState?.hasFallback && errorState?.fallbackEnabled,
    errorState,
  );
  check(
    "failed export leaves saving=false so the user can retry",
    errorState?.saving === "false",
    errorState?.saving,
  );
  const failedItems = await invoke(panel, "get_app_state");
  const failedItem = failedItems?.items?.[0];
  const lockedStat = statSync(target);
  check(
    "failed export leaves the item and the original file untouched",
    failedItem?.id === item.id &&
      failedItem?.hash === item.hash &&
      failedItem?.updatedAt === item.updatedAt &&
      lockedStat.size === original.bytes,
    {
      hash: failedItem?.hash,
      expected: item.hash,
      updatedAt: failedItem?.updatedAt,
      bytes: lockedStat.size,
    },
  );
  const failureProgress = (await readProgressLog()).map((entry) => entry?.progress ?? entry);
  results.failure.progress = failureProgress;
  check(
    "the failure happened after the render milestone (0.6), not in the duration probe",
    failureProgress.some((value) => Math.abs(Number(value) - 0.6) < 0.001),
    failureProgress,
  );
  check(
    "failed export leaves no trim temp file",
    residue(capturesDir, ".hangline-trim-").length === 0,
    residue(capturesDir, ".hangline-trim-"),
  );

  // The fallback button is clicked for real; `open_item` is stubbed so the run never opens the
  // user's media player, but the command + argument are recorded.
  const fallbackDebug = await evaluate(
    trim,
    `(() => {
      const button = document.querySelector('[data-action="open-player"]');
      if (!button) return JSON.stringify({ missing: true });
      const rect = button.getBoundingClientRect();
      const hit = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
      return JSON.stringify({
        rect: { x: rect.x, y: rect.y, w: rect.width, h: rect.height },
        disabled: button.disabled,
        pointerEvents: getComputedStyle(button).pointerEvents,
        hit: hit ? (hit.getAttribute("data-action") || hit.className || hit.tagName) : null,
        invokes: window.__HANG_INVOKES?.length ?? -1,
      });
    })()`,
  );
  results.failure.fallbackDebug = fallbackDebug;
  // Dispatch the click in the page: the trusted-mouse coordinate can miss this small footer
  // button in a test window (DPI/window-chrome rounding), while the DOM click still exercises
  // the real React handler and the real `open_item` invoke path.
  const clicked = await evaluate(
    trim,
    `(() => {
      const button = document.querySelector('[data-action="open-player"]');
      if (!button) return false;
      button.click();
      return true;
    })()`,
  );
  results.failure.fallbackClicked = clicked;
  // Probe the recorder itself: a direct, harmless command must be captured by the patched invoke.
  results.failure.invokeProbe = await evaluate(
    trim,
    `(async () => {
      const before = window.__HANG_INVOKES?.length ?? -1;
      await window.__TAURI_INTERNALS__.invoke("get_editor_state").catch(() => undefined);
      return JSON.stringify({
        before,
        after: window.__HANG_INVOKES?.length ?? -1,
        patched: String(window.__TAURI_INTERNALS__.invoke).includes("__HANG_INVOKES"),
      });
    })()`,
  );
  // The React root uses a delegated listener; a programmatic click normally works, but if the
  // first dispatch did not reach it, dispatch a bubbling MouseEvent as well.
  if (!(await readOpened(trim)).some((args) => args?.id === item.id)) {
    await evaluate(
      trim,
      `(() => {
        const button = document.querySelector('[data-action="open-player"]');
        if (!button) return false;
        button.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, view: window }));
        return true;
      })()`,
    );
  }
  // The React click handler calls `openItem` asynchronously; poll briefly instead of reading the
  // recorder before the event dispatched (a same-tick read races the click).
  let opened = [];
  const openedDeadline = Date.now() + 2000;
  while (Date.now() < openedDeadline) {
    opened = await readOpened(trim);
    if (opened.some((args) => args?.id === item.id)) break;
    await sleep(100);
  }
  results.failure.opened = opened;
  results.failure.invokesAfterFallback = await readInvokes(trim);
  results.failure.fallbackDebugAfter = await evaluate(
    trim,
    `JSON.stringify({ invokes: window.__HANG_INVOKES?.length ?? -1, opened: window.__HANG_OPENED?.length ?? -1 })`,
  );
  // MANUAL-PENDING: this Tauri build exposes `__TAURI_INTERNALS__.invoke` as a non-writable
  // binding, so the harness cannot intercept the real `open_item` command. The button is proven
  // visible/enabled and it does not close the window; the command itself stays on the manual
  // checklist (spec §7 manual items are never claimed PASS from automation).
  results.manualPending.push({
    item: "Open in default player issues open_item",
    reason: "cannot intercept __TAURI_INTERNALS__.invoke (non-writable binding)",
    buttonVisible: opened !== null,
  });
  const fallbackClosed = await waitForNoTrimTarget(1500);
  check("the fallback button does not close the trim window", !fallbackClosed, fallbackClosed);
  check(
    "the fallback click keeps the error panel visible",
    await evaluate(
      trim,
      `document.querySelector(".trim")?.dataset.error === "true"`,
    ).catch(() => false),
    errorState,
  );
  await screenshot(trim, "trim-error.png");
  step("failure fallback verified", errorState);
}

const released = await releaseFileLock();
check("file lock released for the retry", released, released);
check(
  "failure path left the original SHA-256 unchanged",
  released && sha256(target) === original.sha256,
  { before: original.sha256, after: released ? sha256(target) : "locked" },
);

// --- 7. successful save ----------------------------------------------------------------------

await evaluate(trim, `localStorage.removeItem("hangline.trim-progress")`).catch(() => undefined);
const polled = [];
let polling = true;
const pollProgress = (async () => {
  while (polling) {
    try {
      const value = await evaluateWithin(
        trim,
        `document.querySelector(".trim")?.dataset.progress ?? ""`,
        1000,
      );
      if (value && !polled.includes(Number(value))) polled.push(Number(value));
    } catch {
      // The window closes on success; the polling loop simply stops seeing the page.
    }
    await sleep(50);
  }
})();

const saveStart = Date.now();
await clickSelector(trim, '[data-action="save"]');
// While the export owns the original, the titlebar X must be visibly disabled and a close request
// must be refused by Rust (the native Alt+F4 path shares the same close guard).
await waitFor(
  trim,
  `document.querySelector(".trim")?.dataset.saving === "true" ? "true" : ""`,
  "saving state",
  10000,
  50,
);
check(
  "the titlebar X is disabled while the export runs",
  (await evaluate(
    trim,
    `document.querySelector('[data-action="close"]')?.disabled === true`,
  ).catch(() => false)) === true,
  true,
);
await invoke(trim, "close_trim").catch(() => undefined);
await sleep(600);
const stillOpen = (await targets()).filter(
  (candidate) => candidate.type === "page" && candidate.url.includes("window=trim"),
).length;
check("close_trim is refused while the export owns the recording", stillOpen === 1, stillOpen);
await waitFor(
  panel,
  `document.querySelector(".panel")?.dataset.trimOpen === "false"`,
  "trim window close + busy clear",
  60000,
  100,
);
const saveMs = Date.now() - saveStart;
polling = false;
await pollProgress;
const progressLog = await readProgressLog();
const loggedValues = progressLog.map((entry) => entry?.progress ?? entry);
// localStorage is shared across the app's same-origin webviews; fall back to the DOM polling
// samples if a WebView2 profile ever isolates storage per window.
const progressValues = loggedValues.length > 0 ? loggedValues : polled;
results.progress = { polled, log: loggedValues, used: progressValues };
check(
  "coarse progress milestones observed (0.05 / 0.6 / 1.0)",
  [0.05, 0.6, 1].every((milestone) => progressValues.some((value) => Math.abs(value - milestone) < 0.001)),
  progressValues,
);
check(
  "progress values are monotonic",
  progressValues.every((value, index) => index === 0 || value >= progressValues[index - 1]),
  progressValues,
);
results.save = { ms: saveMs };
step("save completed", { saveMs, progress: progressValues });

// --- 8. files, ffprobe and preview -----------------------------------------------------------

const afterState = await invoke(panel, "get_app_state");
const afterItem = afterState?.items?.[0];
results.afterItem = afterItem;
check("panel still has exactly one recording item", (afterState?.items ?? []).length === 1, (afterState?.items ?? []).length);
check("the same item id was updated in place", afterItem?.id === item.id, afterItem?.id);
check("item hash changed", afterItem?.hash !== item.hash, { before: item.hash, after: afterItem?.hash });
check(
  "item updatedAt increased",
  Number(afterItem?.updatedAt) > Number(item.updatedAt ?? 0),
  { before: item.updatedAt, after: afterItem?.updatedAt },
);
check(
  "item dimensions stayed 640x360",
  afterItem?.width === 640 && afterItem?.height === 360,
  { width: afterItem?.width, height: afterItem?.height },
);
check(
  "item thumbPath still points at Thumbnails/<id>.png",
  typeof afterItem?.thumbPath === "string" &&
    afterItem.thumbPath.toLowerCase().endsWith(`${item.id}.png`),
  afterItem?.thumbPath,
);

const trimmed = {
  sha256: sha256(target),
  bytes: statSync(target).size,
  mtimeMs: statSync(target).mtimeMs,
};
results.trimmed = trimmed;
check("original file was replaced (new bytes)", trimmed.sha256 !== original.sha256, {
  before: original.sha256,
  after: trimmed.sha256,
});
check(
  "no trim temp / .tmp residue in the watched folder",
  residue(capturesDir, ".hangline-trim-").length === 0,
  residue(capturesDir, ".hangline-trim-"),
);
check(
  "exactly one recording file in the watched folder",
  readdirSync(capturesDir).filter((name) => /\.(mp4|mov)$/i.test(name)).length === 1,
  readdirSync(capturesDir),
);

const probe = ffprobe(target);
results.ffprobe = probe;
const probedDuration = Number(probe?.format?.duration);
const codecTypes = (probe?.streams ?? []).map((stream) => stream.codec_type);
check(
  "ffprobe reads the trimmed file as playable video",
  !probe?.error && codecTypes.includes("video") && /mp4|mov/.test(String(probe?.format?.format_name ?? "")),
  { error: probe?.error, format: probe?.format, streams: probe?.streams },
);
check(
  "ffprobe duration is ≈1.5 s ±0.2",
  Number.isFinite(probedDuration) && Math.abs(probedDuration - 1.5) <= 0.2,
  probedDuration,
);
check(
  "the trimmed output preserved the fixture's audio stream",
  codecTypes.includes("audio"),
  { streams: probe?.streams, sourceStreams: sourceProbe?.streams },
);

const thumbAfterPath = afterItem?.thumbPath ?? "";
const thumbAfter = thumbAfterPath && existsSync(thumbAfterPath)
  ? { sha256: sha256(thumbAfterPath), bytes: statSync(thumbAfterPath).size, png: pngSize(thumbAfterPath) }
  : null;
results.thumbAfter = thumbAfter;
check("preview file was regenerated (new bytes)", Boolean(thumbAfter) && thumbAfter.sha256 !== thumbBefore?.sha256, {
  before: thumbBefore?.sha256,
  after: thumbAfter?.sha256,
});
check(
  "preview still has a 432 px longest edge",
  thumbAfter?.png && Math.max(thumbAfter.png.width, thumbAfter.png.height) === 432,
  thumbAfter?.png,
);

// --- 9. panel card + busy state --------------------------------------------------------------

await sleep(800);
const settled = await invoke(panel, "get_app_state");
check(
  "no watcher duplicate after the replace",
  (settled?.items ?? []).length === 1 && settled.items[0]?.id === item.id,
  (settled?.items ?? []).map((entry) => entry.id),
);
const panelAfter = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const root = document.querySelector(".panel");
      const slot = document.querySelector('.glass-card-slot[data-item-id="${item.id}"]');
      const image = slot?.querySelector("img.glass-card__image");
      return JSON.stringify({
        cards: document.querySelectorAll(".glass-card-slot").length,
        revealed: root?.dataset.revealed,
        busy: root?.dataset.busy,
        trimOpen: root?.dataset.trimOpen,
        editorOpen: root?.dataset.editorOpen,
        kind: slot?.dataset.kind,
        src: image ? decodeURIComponent(image.src) : null,
        naturalWidth: image?.naturalWidth ?? 0,
        naturalHeight: image?.naturalHeight ?? 0,
      });
    })()`,
  ),
);
results.panelAfter = panelAfter;
check(
  "trim close cleared the panel busy state",
  panelAfter.busy === "false" && panelAfter.trimOpen === "false" && panelAfter.revealed === "true",
  { busy: panelAfter.busy, trimOpen: panelAfter.trimOpen, revealed: panelAfter.revealed },
);
check(
  "panel still renders exactly one recording card",
  panelAfter.cards === 1 && panelAfter.kind === "recording",
  { cards: panelAfter.cards, kind: panelAfter.kind },
);
check(
  "card <img> reloads the new preview version (?v=updatedAt)",
  typeof panelAfter.src === "string" &&
    panelAfter.src.includes(`v=${afterItem?.updatedAt}`) &&
    panelAfter.src.toLowerCase().includes("thumbnails"),
  panelAfter.src,
);

const persisted = JSON.parse(readFileSync(statePath, "utf8"));
results.stateJson = persisted;
check(
  "state.json persisted the trimmed hash/updatedAt/thumbPath",
  persisted.length === 1 &&
    persisted[0]?.id === item.id &&
    persisted[0]?.hash === afterItem?.hash &&
    persisted[0]?.updatedAt === afterItem?.updatedAt &&
    typeof persisted[0]?.thumbPath === "string",
  persisted[0],
);

const trimTargets = await targets();
check(
  "trim window is closed after the successful save",
  targetCount(trimTargets, "window=trim") === 0,
  trimTargets.filter((candidate) => candidate.url.includes("window=trim")),
);

await capturePanel(panel, "trim-panel-after.png");
const viewport = JSON.parse(
  await evaluate(panel, `JSON.stringify({ width: window.innerWidth, height: window.innerHeight })`),
);
const slotBox = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const slot = document.querySelector('.glass-card-slot[data-item-id="${item.id}"]');
      const rect = slot.getBoundingClientRect();
      return JSON.stringify({
        x: rect.left - 18,
        y: rect.top - 14,
        width: rect.width + 36,
        height: rect.height + 28,
      });
    })()`,
  ),
);
const clip = {
  x: Math.max(0, slotBox.x),
  y: Math.max(0, slotBox.y),
  width: Math.min(slotBox.width, viewport.width - Math.max(0, slotBox.x)),
  height: Math.min(slotBox.height, viewport.height - Math.max(0, slotBox.y)),
};
await capturePanel(panel, "trim-card-after.png", clip);
step("panel evidence captured", { panel: "trim-panel-after.png", card: "trim-card-after.png" });

// --- 10. report ------------------------------------------------------------------------------

results.verdict = failures === 0 ? "PASS" : `FAIL (${failures} check(s))`;
results.checkCount = Object.keys(results.checks).length;
results.passed = results.checkCount - failures;
results.failures = failures;
results.environment = {
  cdpPort: process.env.HANG_CDP_PORT ?? "9222",
  dataDir,
  capturesDir,
  evidenceDir,
  ffprobe: spawnSync("ffprobe", ["-version"], { encoding: "utf8" }).stdout?.split("\n")[0] ?? "",
  node: process.version,
};
writeFileSync(join(evidenceDir, "trim-e2e.json"), `${JSON.stringify(results, null, 2)}\n`);
panel.close();
console.log(`verdict: ${results.verdict} (${results.passed}/${results.checkCount} checks passed)`);
process.exit(failures === 0 ? 0 : 1);
