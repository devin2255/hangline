#!/usr/bin/env node
/* global Buffer */
/**
 * P3-4 end-to-end evidence: trusted long press → markup editor, click/drag arbitration, read-only,
 * editor busy state, single-window reuse and dirty-close guards.
 *
 * Dev-only (no dependencies). Requires the isolated profile fixture, the debug app and Vite:
 *
 *   node scripts/p3/fixture.mjs --profile "$env:TEMP\hangline-p4\localappdata\Hangline" --watched
 *   $env:LOCALAPPDATA = "$env:TEMP\hangline-p4\localappdata"; $env:HANG_CDP_PORT = "9223"
 *   Start-Process ...\hangline.exe
 *   $env:HANG_E2E_DIR = "$env:TEMP\hangline-p4"
 *   node scripts/p3/longpress-e2e.mjs
 *
 * The script drives the real panel/editor with CDP trusted mouse and key events. It verifies:
 * a 520 ms hold opens the editor without copying; a short click still copies; a 5 px move cancels
 * the hold; a watched-folder item opens read-only and never writes; the panel reports busy while
 * the editor is open; `open_annotate` re-targets a clean editor but keeps a dirty one; Esc and the
 * window close-request both run the dirty confirmation.
 *
 * The WebView2 dirty confirmation is a native dialog that CDP cannot drive, so the script replaces
 * `window.confirm` with a counting stub for the two confirm paths (the same limitation the P3-2/3
 * report documented). Everything else is trusted input.
 *
 * Screenshots and the JSON report land in `.superpowers/p3-evidence/` (git-ignored) by default.
 */

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { connectTo, evaluate, sleep, targets } from "../p2/cdp.mjs";

const base = process.env.HANG_E2E_DIR;
if (!base) {
  console.error("longpress-e2e: HANG_E2E_DIR is required (the temp run directory)");
  process.exit(2);
}
const dataDir = join(base, "localappdata", "Hangline");
const screenshotsDir = join(dataDir, "Screenshots");
const watchedDir = join(dataDir, "Watched");
const evidenceDir = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p3-evidence");
mkdirSync(evidenceDir, { recursive: true });

const OWNED_ID = "0123456789abcdef0123456789abcdef";
const WATCHED_ID = "fedcba9876543210fedcba9876543210";
const OWNED_NAME = "p3-markup-fixture.png";
const WATCHED_NAME = "p3-watched-fixture.png";
const ownedPath = join(screenshotsDir, OWNED_NAME);
const watchedPath = join(watchedDir, WATCHED_NAME);

const results = { checks: {}, steps: [] };
let failures = 0;

/**
 * The automation session cannot use the Windows clipboard (OpenClipboard fails with access denied
 * for both PowerShell and this app), so the copy/no-copy checks observe the card's copy path: the
 * panel logs a warning when `copy_item` rejects, and a successful copy would show the Copied pill.
 * A hold must produce neither; a short click must reach the copy path exactly once.
 */
function watchCopyAttempts(panel) {
  const attempts = [];
  panel.on("Runtime.consoleAPICalled", (event) => {
    const text = (event.args ?? [])
      .map((arg) => arg.value ?? arg.description ?? "")
      .join(" ");
    if (text.includes("Hangline card: copy of")) {
      attempts.push({ text, at: new Date().toISOString() });
    }
  });
  return attempts;
}

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

async function invoke(cdp, command, args = {}) {
  return evaluate(
    cdp,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

async function waitFor(cdp, expression, label, timeoutMs = 15000, intervalMs = 100) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await evaluate(cdp, expression);
    } catch (error) {
      last = `error: ${error instanceof Error ? error.message : String(error)}`;
    }
    if (last) return last;
    await sleep(intervalMs);
  }
  throw new Error(`timed out waiting for ${label} (last=${JSON.stringify(last)})`);
}

/** One raw mouse event; `buttons` mirrors a real left-button drag. */
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

/** Holds the left button at `point` long enough for the 450 ms deadline to fire. */
async function longPress(cdp, point, holdMs = 520) {
  await mouse(cdp, "mouseMoved", point.x, point.y, false);
  await mouse(cdp, "mousePressed", point.x, point.y, true);
  await sleep(holdMs);
  await mouse(cdp, "mouseReleased", point.x, point.y, false);
}

async function drag(cdp, from, to, steps = 8) {
  await mouse(cdp, "mouseMoved", from.x, from.y, false);
  await mouse(cdp, "mousePressed", from.x, from.y, true);
  await sleep(24);
  for (let index = 1; index <= steps; index += 1) {
    const k = index / steps;
    await mouse(
      cdp,
      "mouseMoved",
      from.x + (to.x - from.x) * k,
      from.y + (to.y - from.y) * k,
      true,
    );
    await sleep(16);
  }
  await mouse(cdp, "mouseReleased", to.x, to.y, false);
  await sleep(64);
}

async function pressKey(cdp, { key, code, windowsVirtualKeyCode, modifiers = 0 }) {
  const common = {
    key,
    code,
    windowsVirtualKeyCode,
    nativeVirtualKeyCode: windowsVirtualKeyCode,
    modifiers,
  };
  await cdp.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...common });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", ...common });
  await sleep(90);
}

async function screenshot(cdp, name) {
  const captured = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  writeFileSync(join(evidenceDir, name), Buffer.from(captured.data, "base64"));
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/** Spawns a second app instance, which toggles the line (the documented user action). */
function togglePanel() {
  const executable = join(base, "hangline.exe");
  if (!existsSync(executable)) return false;
  try {
    const child = spawn(executable, [], {
      detached: true,
      stdio: "ignore",
      env: { ...process.env, LOCALAPPDATA: join(base, "localappdata") },
    });
    child.unref();
    return true;
  } catch (error) {
    console.warn("longpress-e2e: cannot toggle the panel", error);
    return false;
  }
}

async function isRevealed(panel) {
  return evaluate(
    panel,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
  ).catch(() => false);
}

/** Reveals the (click-through) panel if Rust tucked it; safe to call before every card gesture. */
async function ensureRevealed(panel) {
  if (await isRevealed(panel)) return true;
  if (!togglePanel()) return false;
  await waitFor(
    panel,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
    "panel reveal",
    8000,
  ).catch(() => undefined);
  return isRevealed(panel);
}

async function cardCenter(panel, id) {
  const raw = await evaluate(
    panel,
    `(() => {
      const slot = document.querySelector('.glass-card-slot[data-item-id="${id}"]');
      const article = slot?.querySelector("article");
      if (!article) return null;
      const rect = article.getBoundingClientRect();
      return JSON.stringify({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
    })()`,
  );
  if (!raw) throw new Error(`no card for item ${id}`);
  return JSON.parse(raw);
}

async function annotateTargetCount() {
  const list = await targets();
  return list.filter(
    (target) => target.type === "page" && target.url.includes("window=annotate"),
  ).length;
}

async function waitForEditorClosed(label = "editor close", timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if ((await annotateTargetCount()) === 0) return true;
    await sleep(150);
  }
  console.warn(`longpress-e2e: timed out waiting for ${label}`);
  return false;
}

async function editorDom(editor) {
  const raw = await evaluate(
    editor,
    `(() => {
      const root = document.querySelector(".annotate");
      const image = document.querySelector(".annotate__image");
      const save = document.querySelector('[data-action="save"]');
      const readOnly = document.querySelector('[data-read-only="true"]');
      const cannotEdit = document.querySelector('[data-cannot-edit="true"]');
      return JSON.stringify({
        ready: document.querySelector('.annotate__frame')?.dataset.imageReady === "true",
        dirty: root?.dataset.dirty ?? null,
        owned: root?.dataset.owned ?? null,
        saveDisabled: save?.disabled ?? null,
        readOnlyText: readOnly?.textContent ?? null,
        cannotEditText: cannotEdit?.textContent ?? null,
        imagePath: image?.dataset.imagePath ?? null,
        shapes: Number(root?.dataset.shapes ?? 0),
        imageWidth: Number(root?.dataset.imageWidth ?? 0),
        imageHeight: Number(root?.dataset.imageHeight ?? 0),
      });
    })()`,
  );
  return JSON.parse(raw);
}

async function waitForEditor(label, timeoutMs = 20000) {
  const editor = await connectTo("window=annotate", timeoutMs);
  await waitFor(
    editor,
    `!!document.querySelector('.annotate__frame[data-image-ready="true"]')`,
    label,
    timeoutMs,
  );
  return editor;
}

async function stubConfirm(editor, accept) {
  await evaluate(
    editor,
    `(() => {
      window.__HANG_CONFIRM__ = window.__HANG_CONFIRM__ ?? { calls: 0, accept: false };
      window.__HANG_CONFIRM__.accept = ${accept ? "true" : "false"};
      window.confirm = () => {
        window.__HANG_CONFIRM__.calls += 1;
        return window.__HANG_CONFIRM__.accept;
      };
      return true;
    })()`,
  );
}

async function confirmCalls(editor) {
  return Number(await evaluate(editor, `window.__HANG_CONFIRM__?.calls ?? 0`));
}

async function canvasRect(editor) {
  const raw = await evaluate(
    editor,
    `(() => {
      const canvas = document.querySelector(".annotate__canvas");
      if (!canvas) return null;
      const rect = canvas.getBoundingClientRect();
      return JSON.stringify({ x: rect.left, y: rect.top, w: rect.width, h: rect.height });
    })()`,
  );
  if (!raw) throw new Error("the editor canvas is not rendered");
  return JSON.parse(raw);
}

async function drawShape(editor, label) {
  const rect = await canvasRect(editor);
  const at = (fx, fy) => ({ x: rect.x + rect.w * fx, y: rect.y + rect.h * fy });
  await drag(editor, at(0.2, 0.25), at(0.6, 0.55), 8);
  await waitFor(
    editor,
    `document.querySelector(".annotate")?.dataset.shapes !== "0"`,
    `${label} shape`,
  );
  await sleep(250); // let set_editor_dirty reach Rust before a retarget request
}

/** Captures the panel over a neutral backdrop (the real window is transparent). */
async function capturePanel(panel, name) {
  await ensureRevealed(panel);
  await sleep(250);
  await panel.send("Emulation.setDefaultBackgroundColorOverride", {
    color: { r: 26, g: 30, b: 35, a: 1 },
  });
  await screenshot(panel, name);
  await panel.send("Emulation.setDefaultBackgroundColorOverride", {}).catch(() => undefined);
}

// --- 1. panel + fixture ------------------------------------------------------

const panel = await connectTo("window=panel");
// Reload once: a cold Vite dependency scan can leave the first page load half-initialized, and a
// reload also drops any instrumentation left by a previous harness run in this app instance.
await panel.send("Page.reload", { ignoreCache: true }).catch(() => undefined);
await waitFor(
  panel,
  `document.readyState === "complete" && !!document.querySelector(".panel")`,
  "panel ready",
  30000,
);
step("panel connected");
const copyAttempts = watchCopyAttempts(panel);
await evaluate(
  panel,
  `(() => {
    window.__HANG_CLICKS__ = [];
    const viewport = document.querySelector(".panel__viewport");
    viewport.addEventListener("pointerdown", (event) => {
      window.__HANG_CLICKS__.push({
        type: "pointerdown",
        at: performance.now(),
        x: event.clientX,
        y: event.clientY,
        target: String(event.target?.className ?? ""),
      });
    }, true);
    viewport.addEventListener("click", (event) => {
      window.__HANG_CLICKS__.push({
        type: "click",
        at: performance.now(),
        detail: event.detail,
        x: event.clientX,
        y: event.clientY,
        target: String(event.target?.className ?? ""),
      });
    }, true);
    return true;
  })()`,
);
await waitFor(
  panel,
  `(async () => {
    const state = await window.__TAURI_INTERNALS__.invoke("get_app_state", {});
    return state.items.length === 2 && state.items.every((item) => item.thumbPath) ? "yes" : "";
  })()`,
  "two fixture items with thumbnails",
  30000,
);
const beforeState = await invoke(panel, "get_app_state");
const beforeItem = beforeState.items.find((item) => item.id === OWNED_ID);
const beforeWatched = beforeState.items.find((item) => item.id === WATCHED_ID);
const ownedHashBefore = sha256(ownedPath);
const watchedHashBefore = sha256(watchedPath);
const doubleClickMs = await invoke(panel, "get_double_click_interval_ms").catch(() => 500);
results.before = {
  items: beforeState.items,
  ownedHash: ownedHashBefore,
  watchedHash: watchedHashBefore,
  doubleClickMs,
};
check(
  "fixture has the owned and watched items",
  beforeState.items.length === 2 && !!beforeItem && !!beforeWatched,
  { count: beforeState.items.length, owned: !!beforeItem, watched: !!beforeWatched },
);
check(
  "watched item is outside the screenshot folder",
  beforeWatched && !beforeWatched.path.toLowerCase().startsWith(screenshotsDir.toLowerCase()),
  beforeWatched?.path,
);
step("state loaded");

// --- 2. trusted long press on an owned image --------------------------------

if (!(await ensureRevealed(panel))) {
  console.warn("longpress-e2e: the panel did not reveal; card gestures may not be hittable");
}
await sleep(150);
const ownedPoint = await cardCenter(panel, OWNED_ID);
const pressStart = Date.now();
await longPress(panel, ownedPoint, 520);
const editor = await waitForEditor("editor after long press");
const openLatencyMs = Date.now() - pressStart;
results.editorOpenLatencyMs = openLatencyMs;
check(
  "trusted long press opens the editor promptly",
  openLatencyMs < 3000,
  { latencyMs: openLatencyMs },
);
let dom = await editorDom(editor);
results.editor = dom;
check(
  "editor shows the long-pressed owned item",
  dom.imagePath === ownedPath && dom.owned === "true",
  { imagePath: dom.imagePath, owned: dom.owned },
);
const holdCopies = copyAttempts.length;
results.holdCopyCalls = holdCopies;
check(
  "the hold reaches no copy path and shows no Copied pill",
  holdCopies === 0 &&
    (await evaluate(
      panel,
      `document.querySelector('.glass-card-slot[data-item-id="${OWNED_ID}"] article')?.dataset.copied ?? "missing"`,
    )) === "false",
  { copyAttempts: holdCopies },
);
// The editor's own `editor-state` event must make the panel busy, and the line must stay revealed.
await waitFor(
  panel,
  `document.querySelector(".panel")?.dataset.editorOpen === "true"`,
  "panel editor-open mirror",
  8000,
);
await waitFor(
  panel,
  `document.querySelector(".panel")?.dataset.busy === "true"`,
  "panel busy while editing",
  8000,
);
await sleep(1200);
const panelWhileEditing = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const root = document.querySelector(".panel");
      return JSON.stringify({
        revealed: root?.dataset.revealed,
        editorOpen: root?.dataset.editorOpen,
        busy: root?.dataset.busy,
      });
    })()`,
  ),
);
results.panelWhileEditing = panelWhileEditing;
check(
  "line stays revealed and busy while the editor is open",
  panelWhileEditing.revealed === "true" &&
    panelWhileEditing.editorOpen === "true" &&
    panelWhileEditing.busy === "true",
  panelWhileEditing,
);
check(
  "state did not change during the hold",
  (await invoke(panel, "get_app_state")).items.find((item) => item.id === OWNED_ID)?.hash ===
    beforeItem?.hash,
  { hash: beforeItem?.hash },
);
await screenshot(editor, "longpress-editor.png");
await capturePanel(panel, "longpress-panel-busy.png");
step("long press opened the editor without copying");

// --- 3. close clean, short click still copies --------------------------------

await pressKey(editor, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
check("Esc closes a clean editor", await waitForEditorClosed("clean Esc close"), true);
await waitFor(
  panel,
  `document.querySelector(".panel")?.dataset.editorOpen === "false"`,
  "panel editor-open cleared",
  8000,
);
check(
  "editor close emits editor-state false",
  (await evaluate(panel, `document.querySelector(".panel")?.dataset.editorOpen`)) === "false",
  true,
);
await sleep(1000); // past the post-hold guard
if (!(await ensureRevealed(panel))) throw new Error("panel did not reveal for the click test");
const copiesBeforeClick = copyAttempts.length;
const clickEventsBefore = Number(
  await evaluate(panel, `window.__HANG_CLICKS__?.length ?? 0`),
);
const clickPoint = await cardCenter(panel, OWNED_ID);
const clickProbe = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const element = document.elementFromPoint(${Math.round(clickPoint.x)}, ${Math.round(clickPoint.y)});
      return JSON.stringify({
        hidden: document.hidden,
        hit: element?.closest("[data-item-id]")?.dataset.itemId ?? null,
        revealed: document.querySelector(".panel")?.dataset.revealed,
        editorOpen: document.querySelector(".panel")?.dataset.editorOpen,
      });
    })()`,
  ),
);
results.shortClickProbe = clickProbe;
check(
  "the click point hits the owned card",
  clickProbe.hit === OWNED_ID && clickProbe.hidden === false,
  clickProbe,
);
await clickAt(panel, clickPoint.x, clickPoint.y);
// The panel can be a background/occluded webview right after the editor closes, where Chromium
// throttles the single-click timer; wait for the copy path (warning or Copied pill) instead of a
// fixed short sleep.
const copiedPillSelector = `document.querySelector('.glass-card-slot[data-item-id="${OWNED_ID}"] article')?.dataset.copied ?? "missing"`;
const copyDeadline = Date.now() + 8000;
let copiedPill = "missing";
while (Date.now() < copyDeadline) {
  if (copyAttempts.length > copiesBeforeClick) break;
  copiedPill = await evaluate(panel, copiedPillSelector);
  if (copiedPill === "true") break;
  await sleep(100);
}
await sleep(120);
if (copyAttempts.length === copiesBeforeClick) {
  copiedPill = await evaluate(panel, copiedPillSelector);
}
const clickAttempts = copyAttempts.slice(copiesBeforeClick);
const clickEvents = (
  await evaluate(panel, `window.__HANG_CLICKS__ ?? []`)
).slice(clickEventsBefore);
/**
 * A reachable copy path shows either the `Copied` pill (the clipboard worked in this session) or
 * the panel's `Hangline card: copy of … failed` warning (the automation desktop's clipboard is
 * access-denied). Requiring only the warning made the check depend on the session's clipboard
 * permissions; both outcomes prove the click reached the copy path exactly once.
 */
results.shortClickCopyAttempts = clickAttempts;
results.shortClickEvents = clickEvents;
results.shortClickCopiedPill = copiedPill;
check(
  "a short click still reaches the copy path exactly once",
  (clickAttempts.length === 1 && clickAttempts[0]?.text.includes(OWNED_NAME)) ||
    (clickAttempts.length === 0 && copiedPill === "true"),
  {
    before: copiesBeforeClick,
    after: copyAttempts.length,
    attempts: clickAttempts,
    copiedPill,
    events: clickEvents,
    // The automation session's clipboard may be access-denied: then Rust's copy failure warning is
    // the evidence, otherwise the card's Copied pill is.
  },
);
check("the short click did not open the editor", (await annotateTargetCount()) === 0, true);
step("short click copied without opening the editor");

// --- 4. a 5 px move during the hold cancels it -------------------------------

const copiesBeforeMove = copyAttempts.length;
await sleep(1000);
if (!(await ensureRevealed(panel))) throw new Error("panel did not reveal for the move test");
const movePoint = await cardCenter(panel, OWNED_ID);
await mouse(panel, "mouseMoved", movePoint.x, movePoint.y, false);
await mouse(panel, "mousePressed", movePoint.x, movePoint.y, true);
await sleep(120);
await mouse(panel, "mouseMoved", movePoint.x + 5, movePoint.y, true);
await sleep(550);
await mouse(panel, "mouseReleased", movePoint.x + 5, movePoint.y, false);
await sleep(1400);
check(
  "a 5 px move cancels the hold (no editor)",
  (await annotateTargetCount()) === 0,
  { editorTargets: await annotateTargetCount() },
);
check(
  "the cancelled hold did not copy",
  copyAttempts.length === copiesBeforeMove,
  { before: copiesBeforeMove, after: copyAttempts.length },
);
step("5 px movement cancelled the hold");

// --- 5. watched-folder item opens read-only and never writes -----------------

if (!(await ensureRevealed(panel))) throw new Error("panel did not reveal for the read-only test");
await sleep(150);
const watchedPoint = await cardCenter(panel, WATCHED_ID);
const watchedHashAfterOpen = sha256(watchedPath);
await longPress(panel, watchedPoint, 520);
const readOnlyEditor = await waitForEditor("editor after watched long press");
dom = await editorDom(readOnlyEditor);
results.readOnlyEditor = dom;
check(
  "watched-folder editor is read-only with a disabled Save and hint",
  dom.owned === "false" &&
    dom.imagePath === watchedPath &&
    dom.saveDisabled === true &&
    typeof dom.readOnlyText === "string" &&
    dom.readOnlyText.length > 0,
  {
    owned: dom.owned,
    imagePath: dom.imagePath,
    saveDisabled: dom.saveDisabled,
    readOnlyText: dom.readOnlyText,
  },
);
await screenshot(readOnlyEditor, "longpress-readonly.png");
await pressKey(readOnlyEditor, {
  key: "s",
  code: "KeyS",
  windowsVirtualKeyCode: 83,
  modifiers: 2,
});
await sleep(900);
check(
  "Ctrl+S on a read-only item neither writes nor closes",
  sha256(watchedPath) === watchedHashAfterOpen &&
    sha256(watchedPath) === watchedHashBefore &&
    (await annotateTargetCount()) === 1,
  { hashUnchanged: sha256(watchedPath) === watchedHashBefore },
);
step("watched-folder item opened read-only; no write occurred");
await pressKey(readOnlyEditor, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
check("read-only editor closes without a prompt", await waitForEditorClosed(), true);

// --- 6. single-window reuse: clean retarget, then dirty guard ---------------

await invoke(panel, "open_annotate", { id: OWNED_ID });
const reused = await waitForEditor("reused editor");
await waitFor(
  reused,
  `document.querySelector(".annotate__image")?.dataset.imagePath?.endsWith(${JSON.stringify(OWNED_NAME)}) === true`,
  "owned target loaded",
);
await invoke(panel, "open_annotate", { id: WATCHED_ID });
await waitFor(
  reused,
  `document.querySelector(".annotate__image")?.dataset.imagePath?.endsWith(${JSON.stringify(WATCHED_NAME)}) === true`,
  "clean retarget to the watched item",
);
check(
  "a clean editor re-targets the second item in one window",
  (await annotateTargetCount()) === 1,
  { windows: await annotateTargetCount() },
);
await pressKey(reused, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await waitForEditorClosed();
step("clean single-window retarget works");

// Open twice back-to-back: the second target may arrive before the page's listener is ready and
// must be replayed by the ready handshake.
await invoke(panel, "open_annotate", { id: OWNED_ID });
await invoke(panel, "open_annotate", { id: WATCHED_ID });
const replayed = await connectTo("window=annotate");
await waitFor(
  replayed,
  `document.querySelector(".annotate__image")?.dataset.imagePath?.endsWith(${JSON.stringify(WATCHED_NAME)}) === true`,
  "replayed target after a back-to-back open",
  20000,
);
check(
  "a target requested before the page listener is replayed",
  (await annotateTargetCount()) === 1,
  { windows: await annotateTargetCount() },
);
await pressKey(replayed, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await waitForEditorClosed();
step("pre-listener target replay works");

// Dirty editor: the second open must focus the existing editor instead of discarding the drawing.
await invoke(panel, "open_annotate", { id: OWNED_ID });
const dirtyEditor = await waitForEditor("dirty editor");
await drawShape(dirtyEditor, "dirty retarget");
check(
  "drawing marks the editor dirty",
  (await editorDom(dirtyEditor)).dirty === "true",
  (await editorDom(dirtyEditor)).dirty,
);
await invoke(panel, "open_annotate", { id: WATCHED_ID });
await sleep(800);
dom = await editorDom(dirtyEditor);
check(
  "a dirty editor refuses a silent retarget",
  dom.imagePath === ownedPath && (await annotateTargetCount()) === 1,
  { imagePath: dom.imagePath, windows: await annotateTargetCount() },
);
await pressKey(dirtyEditor, {
  key: "z",
  code: "KeyZ",
  windowsVirtualKeyCode: 90,
  modifiers: 2,
});
await waitFor(
  dirtyEditor,
  `document.querySelector(".annotate")?.dataset.dirty === "false"`,
  "dirty cleared by undo",
);
await sleep(300);
await invoke(panel, "open_annotate", { id: WATCHED_ID });
await waitFor(
  dirtyEditor,
  `document.querySelector(".annotate__image")?.dataset.imagePath?.endsWith(${JSON.stringify(WATCHED_NAME)}) === true`,
  "retarget after the editor became clean",
);
check(
  "an undone editor becomes retargetable again",
  (await editorDom(dirtyEditor)).imagePath === watchedPath,
  (await editorDom(dirtyEditor)).imagePath,
);
await pressKey(dirtyEditor, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await waitForEditorClosed();
step("dirty editor guard works");

// --- 7. Esc dirty confirmation ----------------------------------------------

await invoke(panel, "open_annotate", { id: OWNED_ID });
let confirmEditor = await waitForEditor("editor for Esc confirmation");
await drawShape(confirmEditor, "esc confirmation");
await stubConfirm(confirmEditor, false);
await pressKey(confirmEditor, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await sleep(500);
check(
  "Esc on a dirty editor asks before discarding (cancel keeps it open)",
  (await confirmCalls(confirmEditor)) === 1 && (await annotateTargetCount()) === 1,
  { confirmCalls: await confirmCalls(confirmEditor), windows: await annotateTargetCount() },
);
await stubConfirm(confirmEditor, true);
await pressKey(confirmEditor, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
check(
  "confirming the Esc prompt closes the editor",
  await waitForEditorClosed("Esc confirm close"),
  true,
);
step("Esc dirty confirmation works");

// --- 8. window close-request (Alt+F4 path) dirty confirmation ---------------

await invoke(panel, "open_annotate", { id: OWNED_ID });
confirmEditor = await waitForEditor("editor for close-request confirmation");
await drawShape(confirmEditor, "close-request confirmation");
await stubConfirm(confirmEditor, false);
await invoke(panel, "close_editor");
await sleep(700);
check(
  "a close request on a dirty editor asks first (Alt+F4 path)",
  (await confirmCalls(confirmEditor)) === 1 && (await annotateTargetCount()) === 1,
  { confirmCalls: await confirmCalls(confirmEditor), windows: await annotateTargetCount() },
);
await stubConfirm(confirmEditor, true);
await invoke(panel, "close_editor");
check(
  "confirming the close request closes the editor",
  await waitForEditorClosed("close-request confirm close"),
  true,
);
step("window close-request dirty confirmation works");

// --- 9. report ---------------------------------------------------------------

const afterState = await invoke(panel, "get_app_state");
results.after = {
  items: afterState.items,
  ownedHash: sha256(ownedPath),
  watchedHash: sha256(watchedPath),
};
check(
  "the watched file stayed byte-identical for the whole run",
  results.after.watchedHash === watchedHashBefore,
  { before: watchedHashBefore, after: results.after.watchedHash },
);
check(
  "the owned file was not rewritten by long press or read-only paths",
  results.after.ownedHash === ownedHashBefore &&
    afterState.items.find((item) => item.id === OWNED_ID)?.hash === beforeItem?.hash,
  { before: ownedHashBefore, after: results.after.ownedHash },
);
check(
  "state still has both items (no duplicates, no removals)",
  afterState.items.length === 2,
  afterState.items.length,
);
results.verdict = failures === 0 ? "PASS" : `FAIL (${failures} check(s))`;
results.checkCount = Object.keys(results.checks).length;
results.passed = results.checkCount - failures;
results.failures = failures;
results.environment = {
  cdpPort: process.env.HANG_CDP_PORT ?? "9223",
  dataDir,
  evidenceDir,
  node: process.version,
};
writeFileSync(join(evidenceDir, "longpress-e2e.json"), `${JSON.stringify(results, null, 2)}\n`);
panel.close();
console.log(`verdict: ${results.verdict} (${results.passed}/${results.checkCount} checks passed)`);
process.exit(failures === 0 ? 0 : 1);
