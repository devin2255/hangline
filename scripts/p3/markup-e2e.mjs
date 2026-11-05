#!/usr/bin/env node
/* global Buffer */
/**
 * P3-2/P3-3 end-to-end evidence: open → draw (pen/rect/arrow/text) → undo/redo → crop → save.
 *
 * Dev-only (no dependencies). Requires the isolated profile fixture, the debug app and Vite:
 *
 *   node scripts/p3/fixture.mjs --profile "$env:TEMP\hangline-p3\localappdata\Hangline"
 *   $env:LOCALAPPDATA = "$env:TEMP\hangline-p3\localappdata"; $env:HANG_CDP_PORT = "9222"
 *   Start-Process src-tauri\target\debug\hangline.exe
 *   node scripts/p3/markup-e2e.mjs            # HANG_E2E_DIR=$env:TEMP\hangline-p3
 *
 * The script drives the real UI with CDP trusted input (`Input.dispatchMouseEvent` / key events),
 * saves with Ctrl+S, then verifies: the panel card URL carries the new `?v=`, the thumbnail bytes
 * changed, state.json carries the new hash/dimensions/updatedAt, the item count stayed at one (the
 * watcher did not duplicate), the editor window closed and no `.tmp` residue is left.
 *
 * Screenshots and the JSON report land in `.superpowers/p3-evidence/` (git-ignored) by default.
 */

import { spawn } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { connectTo, evaluate, sleep, targets } from "../p2/cdp.mjs";

const base = process.env.HANG_E2E_DIR;
if (!base) {
  console.error("markup-e2e: HANG_E2E_DIR is required (the temp run directory)");
  process.exit(2);
}
const dataDir = join(base, "localappdata", "Hangline");
const screenshotsDir = join(dataDir, "Screenshots");
const thumbsDir = join(dataDir, "Thumbnails");
const statePath = join(dataDir, "state.json");
const evidenceDir = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p3-evidence");
mkdirSync(evidenceDir, { recursive: true });

const results = { checks: {}, steps: [] };
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

function stamp(path) {
  const info = statSync(path);
  return { bytes: info.size, mtimeMs: info.mtimeMs };
}

function residue(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).filter(
    (name) => name.endsWith(".tmp") || name.startsWith(".hangline-markup-"),
  );
}

function folderPngs(dir) {
  return readdirSync(dir).filter((name) => name.toLowerCase().endsWith(".png"));
}

/** Reads the width/height from a PNG's IHDR without decoding the pixels (Node built-ins only). */
function pngSize(path) {
  const bytes = readFileSync(path);
  if (bytes.length < 24 || bytes.toString("ascii", 1, 4) !== "PNG") return null;
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

async function invoke(cdp, command, args = {}) {
  return evaluate(
    cdp,
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}

async function waitFor(cdp, expression, label, timeoutMs = 20000, intervalMs = 100) {
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

/** One raw mouse event; buttons mirrors a real left-button drag. */
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
  await sleep(48);
}

async function drag(cdp, from, to, steps = 10) {
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

/**
 * Shows the (click-through) panel for one screenshot.
 *
 * The panel is a transparent strip that the reveal controller tucks when the pointer is elsewhere;
 * starting a second app instance hits the single-instance handler, which toggles the line — the same
 * user-visible action as the tray/hotkey. A neutral backdrop is set only for the capture (the real
 * window is transparent over the desktop).
 */
async function capturePanel(cdp, name) {
  const alreadyRevealed = await evaluate(
    cdp,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
  ).catch(() => false);
  const executable = join(base, "hangline.exe");
  if (!alreadyRevealed && existsSync(executable)) {
    try {
      const child = spawn(executable, [], {
        detached: true,
        stdio: "ignore",
        env: { ...process.env, LOCALAPPDATA: join(base, "localappdata") },
      });
      child.unref();
    } catch (error) {
      console.warn(`markup-e2e: cannot toggle the panel for ${name}`, error);
    }
  }
  await waitFor(
    cdp,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
    "panel reveal",
    8000,
  ).catch(() => undefined);
  await sleep(500);
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", {
    color: { r: 26, g: 30, b: 35, a: 1 },
  });
  await screenshot(cdp, name);
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", {}).catch(() => undefined);
}

// --- 1. panel + fixture ------------------------------------------------------

const panel = await connectTo("window=panel");
// Reload once so a cold Vite dependency scan / stale page state cannot poison the run.
await panel.send("Page.reload", { ignoreCache: true }).catch(() => undefined);
await waitFor(
  panel,
  `document.readyState === "complete" && !!document.querySelector(".panel")`,
  "panel ready",
  30000,
);
step("panel connected");

let state = await invoke(panel, "get_app_state");
check("fixture has exactly one item", state.items.length === 1, state.items.length);
const itemId = state.items[0]?.id;
const before = state.items[0];
check("fixture item is Hangline-owned", before?.path.startsWith(screenshotsDir), before?.path);
check(
  "fixture starts with a versioned updatedAt",
  Number(before?.updatedAt) > 0,
  before?.updatedAt,
);
results.before = { item: before };
step("state loaded", { id: itemId, path: before.path });

// Wait for the startup thumbnail backfill (the card must switch to the 432 px preview).
await waitFor(
  panel,
  `(async () => {
    const state = await window.__TAURI_INTERNALS__.invoke("get_app_state", {});
    return state.items[0]?.thumbPath ? "yes" : "";
  })()`,
  "thumbnail backfill",
  25000,
);
state = await invoke(panel, "get_app_state");
const beforeItem = state.items[0];
const beforeThumb = stamp(beforeItem.thumbPath);
const beforeCardSrc = await waitFor(
  panel,
  `(() => {
    const image = document.querySelector('.glass-card-slot[data-item-id="${itemId}"] img.glass-card__image');
    return image && image.src ? image.src : "";
  })()`,
  "panel card image",
);
results.before.thumb = { path: beforeItem.thumbPath, ...beforeThumb };
results.before.cardSrc = beforeCardSrc;
check(
  "panel card URL is versioned before the edit",
  beforeCardSrc.includes(`?v=${beforeItem.updatedAt}`),
  beforeCardSrc,
);
await capturePanel(panel, "panel-before-markup.png");
step("panel card visible");

// --- 2. open the editor ------------------------------------------------------

await invoke(panel, "open_annotate", { id: itemId });
const annotate = await connectTo("window=annotate");
await waitFor(
  annotate,
  `!!document.querySelector('.annotate__frame[data-image-ready="true"]') &&
   Number(document.querySelector('.annotate').dataset.imageWidth) > 0`,
  "editor image loaded",
  25000,
);
const editorInfo = JSON.parse(
  await evaluate(
    annotate,
    `(() => {
      const root = document.querySelector(".annotate");
      const canvas = document.querySelector(".annotate__canvas");
      const rect = canvas.getBoundingClientRect();
      return JSON.stringify({
        imageWidth: Number(root.dataset.imageWidth),
        imageHeight: Number(root.dataset.imageHeight),
        display: { x: rect.left, y: rect.top, w: rect.width, h: rect.height },
        viewport: { w: window.innerWidth, h: window.innerHeight },
        theme: document.documentElement.dataset.theme,
        owned: root.dataset.owned,
        tool: root.dataset.tool,
      });
    })()`,
  ),
);
results.editor = { ...editorInfo };
check(
  "editor opened with the item's natural size",
  editorInfo.imageWidth === 1600 && editorInfo.imageHeight === 1000,
  editorInfo,
);
check("editor shows the fixture theme", editorInfo.theme === "dark", editorInfo.theme);
check("owned fixture is editable", editorInfo.owned === "true");
await screenshot(annotate, "annotate-initial.png");
step("editor ready");

const canvas = {
  x: editorInfo.display.x,
  y: editorInfo.display.y,
  w: editorInfo.display.w,
  h: editorInfo.display.h,
};
const at = (fx, fy) => ({ x: canvas.x + canvas.w * fx, y: canvas.y + canvas.h * fy });
const shapeCount = async () =>
  Number(await evaluate(annotate, `document.querySelector(".annotate").dataset.shapes`));

// Empty text entry must not mark the editor dirty, and the textarea reserves two lines (P3 review
// minors). Esc closes only the text entry; the editor itself must stay open.
await clickSelector(annotate, '.annotate__button[data-tool="text"]');
await clickAt(annotate, at(0.5, 0.5).x, at(0.5, 0.5).y);
await waitFor(annotate, `!!document.querySelector(".annotate__text-input")`, "text editor");
const emptyText = JSON.parse(
  await evaluate(
    annotate,
    `(() => {
      const input = document.querySelector(".annotate__text-input");
      const style = getComputedStyle(input);
      return JSON.stringify({
        dirty: document.querySelector(".annotate").dataset.dirty,
        minHeight: parseFloat(style.minHeight),
        lineHeight: parseFloat(style.lineHeight),
      });
    })()`,
  ),
);
check(
  "an empty text entry does not mark the editor dirty",
  emptyText.dirty === "false",
  emptyText,
);
check(
  "the textarea reserves at least two lines",
  emptyText.minHeight >= emptyText.lineHeight * 2,
  emptyText,
);
await pressKey(annotate, { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await waitFor(annotate, `!document.querySelector(".annotate__text-input")`, "text editor closed");
check(
  "Esc on an empty text entry closes only the entry",
  (await evaluate(annotate, `document.querySelector(".annotate").dataset.dirty`)) === "false",
  true,
);
await clickSelector(annotate, '.annotate__button[data-tool="pen"]');

// --- 3. draw with trusted input ---------------------------------------------

await drag(annotate, at(0.16, 0.24), at(0.52, 0.44), 12);
await waitFor(annotate, `document.querySelector(".annotate").dataset.shapes === "1"`, "pen shape");
step("pen stroke drawn");

await clickSelector(annotate, '.annotate__button[data-tool="rect"]');
await drag(annotate, at(0.58, 0.2), at(0.84, 0.42), 8);
await waitFor(annotate, `document.querySelector(".annotate").dataset.shapes === "2"`, "rect shape");

await clickSelector(annotate, '.annotate__button[data-tool="arrow"]');
await drag(annotate, at(0.2, 0.62), at(0.5, 0.78), 8);
await waitFor(
  annotate,
  `document.querySelector(".annotate").dataset.shapes === "3"`,
  "arrow shape",
);

await clickSelector(annotate, '.annotate__button[data-tool="text"]');
await clickAt(annotate, at(0.56, 0.6).x, at(0.56, 0.6).y);
await waitFor(annotate, `!!document.querySelector(".annotate__text-input")`, "text editor");
await annotate.send("Input.insertText", { text: "P3 markup" });
await pressKey(annotate, { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
await waitFor(annotate, `document.querySelector(".annotate").dataset.shapes === "4"`, "text shape");
results.editor.shapesAfterDrawing = await shapeCount();
check(
  "four shapes drawn",
  results.editor.shapesAfterDrawing === 4,
  results.editor.shapesAfterDrawing,
);
step("rect, arrow and multiline text drawn");

// --- 4. undo / redo ----------------------------------------------------------

await pressKey(annotate, { key: "z", code: "KeyZ", windowsVirtualKeyCode: 90, modifiers: 2 });
await waitFor(annotate, `document.querySelector(".annotate").dataset.shapes === "3"`, "undo");
await pressKey(annotate, { key: "y", code: "KeyY", windowsVirtualKeyCode: 89, modifiers: 2 });
await waitFor(annotate, `document.querySelector(".annotate").dataset.shapes === "4"`, "redo");
results.editor.undoRedo = { undoneTo: 3, redoneTo: 4 };
check(
  "Ctrl+Z / Ctrl+Y undo and redo one shape",
  results.editor.undoRedo.undoneTo === 3 && results.editor.undoRedo.redoneTo === 4,
  results.editor.undoRedo,
);
step("undo/redo verified");

// --- 5. crop -----------------------------------------------------------------

await clickSelector(annotate, '.annotate__button[data-tool="crop"]');
await drag(annotate, at(0.1, 0.12), at(0.86, 0.9), 8);
await waitFor(annotate, `!!document.querySelector(".annotate__crop")`, "crop rectangle");
const cropBefore = JSON.parse(
  await evaluate(
    annotate,
    `(() => {
      const crop = document.querySelector(".annotate__crop");
      return JSON.stringify({
        left: parseFloat(crop.style.left),
        top: parseFloat(crop.style.top),
        width: parseFloat(crop.style.width),
        height: parseFloat(crop.style.height),
      });
    })()`,
  ),
);
results.editor.cropDisplay = cropBefore;
await screenshot(annotate, "annotate-crop.png");
await clickSelector(annotate, '[data-action="apply-crop"]');
await waitFor(
  annotate,
  `document.querySelector(".annotate").dataset.crop === "true"`,
  "crop applied",
);
// Undo must resync the overlay with the annotation state: the applied crop is gone, so no stale
// rectangle/Apply button may remain (the P3 review's "undo-cleared crop re-applied" bug).
await pressKey(annotate, { key: "z", code: "KeyZ", windowsVirtualKeyCode: 90, modifiers: 2 });
await waitFor(
  annotate,
  `document.querySelector(".annotate").dataset.crop === "false" &&
   !document.querySelector(".annotate__crop")`,
  "crop undone and overlay cleared",
);
check(
  "undo clears the crop overlay and the Apply button",
  (await evaluate(annotate, `!document.querySelector('[data-action="apply-crop"]')`)) === true,
  true,
);
await pressKey(annotate, { key: "y", code: "KeyY", windowsVirtualKeyCode: 89, modifiers: 2 });
await waitFor(
  annotate,
  `document.querySelector(".annotate").dataset.crop === "true" &&
   !!document.querySelector(".annotate__crop")`,
  "crop restored by redo",
);
// The crop rectangle is movable from its inside; the move is draft-only until Apply.
const cropStyle = JSON.parse(
  await evaluate(
    annotate,
    `(() => {
      const crop = document.querySelector(".annotate__crop");
      return JSON.stringify({ left: parseFloat(crop.style.left), top: parseFloat(crop.style.top) });
    })()`,
  ),
);
const cropCenter = {
  x: canvas.x + cropBefore.left + cropBefore.width / 2,
  y: canvas.y + cropBefore.top + cropBefore.height / 2,
};
await drag(annotate, cropCenter, { x: cropCenter.x + 15, y: cropCenter.y + 10 }, 4);
const movedCropStyle = JSON.parse(
  await evaluate(
    annotate,
    `(() => {
      const crop = document.querySelector(".annotate__crop");
      return JSON.stringify({ left: parseFloat(crop.style.left), top: parseFloat(crop.style.top) });
    })()`,
  ),
);
check(
  "dragging inside the crop rectangle moves it",
  Math.abs(movedCropStyle.left - (cropStyle.left + 15)) <= 3 &&
    Math.abs(movedCropStyle.top - (cropStyle.top + 10)) <= 3,
  { before: cropStyle, after: movedCropStyle },
);
// Re-selecting the crop tool drops the draft move and restores the committed rectangle.
await clickSelector(annotate, '.annotate__button[data-tool="pen"]');
await clickSelector(annotate, '.annotate__button[data-tool="crop"]');
const restoredCropStyle = JSON.parse(
  await evaluate(
    annotate,
    `(() => {
      const crop = document.querySelector(".annotate__crop");
      return JSON.stringify({ left: parseFloat(crop.style.left), top: parseFloat(crop.style.top) });
    })()`,
  ),
);
check(
  "the committed crop survives a cancelled draft move",
  Math.abs(restoredCropStyle.left - cropStyle.left) <= 1 &&
    Math.abs(restoredCropStyle.top - cropStyle.top) <= 1,
  { committed: cropStyle, restored: restoredCropStyle },
);
const saveScale = editorInfo.imageWidth / editorInfo.display.w;
// Crop CSS pixels are inside the displayed image; the saved region is in natural image pixels.
const expected = {
  offsetX: Math.round(cropBefore.left * saveScale),
  offsetY: Math.round(cropBefore.top * saveScale),
  width: Math.round(cropBefore.width * saveScale),
  height: Math.round(cropBefore.height * saveScale),
};
results.editor.expectedOutput = expected;
const intendedCrop = {
  width: Math.round((0.86 - 0.1) * editorInfo.imageWidth),
  height: Math.round((0.9 - 0.12) * editorInfo.imageHeight),
};
check(
  "crop covers the dragged region (pointerup finalizes the rectangle)",
  Math.abs(expected.width - intendedCrop.width) <= 6 &&
    Math.abs(expected.height - intendedCrop.height) <= 6,
  { intendedCrop, expected },
);
await screenshot(annotate, "annotate-annotated.png");
step("crop applied", expected);

// --- 6. Ctrl+S inside the textarea (multiline) saves and closes -------------
// The P3 review minor: Ctrl+S must save/commit even while the textarea has focus, and the
// multi-line text must reach the composited PNG.
await clickSelector(annotate, '.annotate__button[data-tool="text"]');
await clickAt(annotate, at(0.5, 0.5).x, at(0.5, 0.5).y);
await waitFor(annotate, `!!document.querySelector(".annotate__text-input")`, "save text editor");
await annotate.send("Input.insertText", { text: "Saved from\nthe textarea" });
await pressKey(annotate, { key: "s", code: "KeyS", windowsVirtualKeyCode: 83, modifiers: 2 });
const editorClosed = await (async () => {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    const list = await targets();
    if (!list.some((target) => target.type === "page" && target.url.includes("window=annotate"))) {
      return true;
    }
    await sleep(200);
  }
  return false;
})();
check("Ctrl+S in the textarea saves and closes the editor", editorClosed, editorClosed);
if (!editorClosed) {
  await screenshot(annotate, "annotate-save-stuck.png").catch(() => undefined);
}

// --- 7. panel + state + thumbnail refresh -----------------------------------

const afterState = await (async () => {
  const deadline = Date.now() + 20000;
  let latest = null;
  while (Date.now() < deadline) {
    latest = await invoke(panel, "get_app_state");
    if (Number(latest.items[0]?.updatedAt) > Number(beforeItem.updatedAt)) return latest;
    await sleep(200);
  }
  return latest;
})();
const afterItem = afterState?.items?.[0];
results.after = { item: afterItem };
check(
  "state.json item bumped updatedAt",
  Number(afterItem?.updatedAt) > Number(beforeItem.updatedAt),
  { before: beforeItem.updatedAt, after: afterItem?.updatedAt },
);
check("hash changed after the edit", afterItem?.hash !== beforeItem.hash, {
  before: beforeItem.hash,
  after: afterItem?.hash,
});
check(
  "dimensions match the crop output",
  Math.abs(afterItem.width - expected.width) <= 2 &&
    Math.abs(afterItem.height - expected.height) <= 2,
  { expected, actual: { width: afterItem.width, height: afterItem.height } },
);
// The item is metadata; the assertion that matters is the file that was actually replaced.
const savedPng = pngSize(before.path);
results.after.savedPng = savedPng;
check(
  "original file was replaced with the cropped dimensions",
  savedPng?.width === afterItem.width && savedPng?.height === afterItem.height,
  { file: savedPng, item: { width: afterItem.width, height: afterItem.height } },
);
const afterCardSrc = await waitFor(
  panel,
  `(() => {
    const image = document.querySelector('.glass-card-slot[data-item-id="${itemId}"] img.glass-card__image');
    const version = ${JSON.stringify(String(afterItem.updatedAt))};
    return image && image.src.includes("?v=" + version) ? image.src : "";
  })()`,
  "panel card reload with the new version",
);
results.after.cardSrc = afterCardSrc;
check(
  "panel card URL carries the new ?v=",
  afterCardSrc.includes(`?v=${afterItem.updatedAt}`) && afterCardSrc !== beforeCardSrc,
  afterCardSrc,
);
const afterThumb = stamp(afterItem.thumbPath);
results.after.thumb = { path: afterItem.thumbPath, ...afterThumb };
check(
  "432 px thumbnail was regenerated",
  afterThumb.mtimeMs > beforeThumb.mtimeMs || afterThumb.bytes !== beforeThumb.bytes,
  { before: beforeThumb, after: afterThumb },
);
// The preview file itself, not just its mtime/size: 432 px on the long edge, aspect preserved.
const thumbPng = pngSize(afterItem.thumbPath);
const expectedThumbHeight = Math.round((432 * afterItem.height) / afterItem.width);
results.after.thumbPng = thumbPng;
check(
  "thumbnail file has the expected 432 px dimensions",
  thumbPng?.width === 432 && Math.abs(thumbPng.height - expectedThumbHeight) <= 1,
  { file: thumbPng, expected: { width: 432, height: expectedThumbHeight } },
);

const persisted = JSON.parse(readFileSync(statePath, "utf8"));
results.after.stateJson = persisted;
check(
  "state.json has exactly one item (no watcher duplicate)",
  persisted.length === 1,
  persisted.length,
);
check(
  "state.json persisted the new hash/dimensions/updatedAt",
  persisted[0]?.hash === afterItem.hash &&
    persisted[0]?.updatedAt === afterItem.updatedAt &&
    persisted[0]?.width === afterItem.width &&
    persisted[0]?.height === afterItem.height,
  persisted[0],
);
const screenshotFiles = folderPngs(screenshotsDir);
const screenshotResidue = residue(screenshotsDir);
const thumbResidue = residue(thumbsDir);
results.after.files = { screenshots: screenshotFiles, screenshotResidue, thumbResidue };
check(
  "exactly one screenshot file (no duplicate copy)",
  screenshotFiles.length === 1,
  screenshotFiles,
);
check(
  "no .tmp residue in the screenshot folder",
  screenshotResidue.length === 0,
  screenshotResidue,
);
check("no .tmp residue in the thumbnail cache", thumbResidue.length === 0, thumbResidue);
step("panel and disk verified");

// Fly overlay (P3): the panel hands `imageVersion` to `start_fly`, and the overlay renders
// `assetUrl(imagePath, imageVersion)`. The pre-warmed overlay page is already connected, so the
// versioned URL can be observed during the flight without a second window.
const fly = await connectTo("window=fly");
let flyAccepted = false;
for (let attempt = 0; attempt < 20 && !flyAccepted; attempt += 1) {
  flyAccepted = await invoke(panel, "start_fly", {
    id: itemId,
    from: null,
    to: { x: 120, y: 120, w: 288, h: 224 },
    tiltDeg: 2,
    imageVersion: afterItem.updatedAt,
  }).catch(() => false);
  if (!flyAccepted) await sleep(250);
}
const flySrc = flyAccepted
  ? await waitFor(
      fly,
      `(() => {
        const image = document.querySelector(".fly-card__image");
        return image && image.src ? image.src : "";
      })()`,
      "fly overlay image",
      5000,
    ).catch(() => "")
  : "";
check(
  "fly overlay renders the versioned image URL",
  typeof flySrc === "string" && flySrc.includes(`?v=${afterItem.updatedAt}`),
  { accepted: flyAccepted, src: flySrc },
);
await sleep(900); // let the flight finish so the overlay does not straddle the panel capture
fly.close();
step("fly overlay verified");

// Panel evidence: a before/after pair over a neutral backdrop, plus the saved file itself.
await capturePanel(panel, "panel-after-refresh.png");
if (existsSync(before.path)) {
  copyFileSync(before.path, join(evidenceDir, "saved-markup.png"));
}
results.after.dom = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const slot = document.querySelector('.glass-card-slot[data-item-id="${itemId}"]');
      const image = slot?.querySelector("img.glass-card__image");
      return JSON.stringify({
        cards: document.querySelectorAll(".glass-card-slot").length,
        src: image?.src ?? null,
        naturalWidth: image?.naturalWidth ?? 0,
        naturalHeight: image?.naturalHeight ?? 0,
      });
    })()`,
  ),
);
check(
  "panel card shows the refreshed preview bytes",
  results.after.dom.cards === 1 &&
    results.after.dom.naturalWidth === thumbPng?.width &&
    results.after.dom.naturalHeight === thumbPng?.height &&
    results.after.dom.src?.includes(`?v=${afterItem.updatedAt}`),
  results.after.dom,
);

// --- 8. report ---------------------------------------------------------------

results.verdict = failures === 0 ? "PASS" : `FAIL (${failures} check(s))`;
results.checkCount = Object.keys(results.checks).length;
results.passed = results.checkCount - failures;
results.failures = failures;
results.environment = {
  cdpPort: process.env.HANG_CDP_PORT ?? "9222",
  dataDir,
  evidenceDir,
  node: process.version,
};
writeFileSync(join(evidenceDir, "markup-e2e.json"), `${JSON.stringify(results, null, 2)}\n`);
panel.close();
console.log(`verdict: ${results.verdict} (${results.passed}/${results.checkCount} checks passed)`);
process.exit(failures === 0 ? 0 : 1);
