#!/usr/bin/env node
/* global Buffer */
/**
 * P3-5 end-to-end evidence: a recording copied into a watched folder AFTER app start shows its
 * first frame on the line, with natural dimensions and a 432 px preview.
 *
 * Dev-only (Node built-ins + the P2 CDP helper). Runbook:
 *
 *   node scripts/p3/fixture.mjs --profile "$env:TEMP\hangline-p3-recording\localappdata\Hangline" --recordings
 *   .\scripts\p3\fixture-video.ps1 -Out "$env:TEMP\hangline-p3-recording\fixture.mp4"
 *   # start the debug app with HANG_CDP_PORT=9224, isolated LOCALAPPDATA/USERPROFILE
 *   $env:HANG_E2E_DIR = "$env:TEMP\hangline-p3-recording"
 *   node scripts/p3/recording-e2e.mjs
 *
 * The script copies the mp4 into `%USERPROFILE%\Videos\Captures` itself, then verifies: exactly one
 * `recording` item, `width`/`height` = the video's natural size (640x360), `thumbPath` pointing at
 * `Thumbnails/<id>.png` whose longest edge is 432 px, the panel card rendering that preview plus the
 * play badge (no filename fallback), no duplicate item, and no `.tmp` residue.
 *
 * Screenshots and the JSON report land in `.superpowers/p3-evidence/` (git-ignored).
 */

import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { inflateSync } from "node:zlib";
import { connectTo, evaluate, screenshotClip, sleep } from "../p2/cdp.mjs";

const base = process.env.HANG_E2E_DIR;
if (!base) {
  console.error("recording-e2e: HANG_E2E_DIR is required (the temp run directory)");
  process.exit(2);
}
const dataDir = join(base, "localappdata", "Hangline");
const thumbsDir = join(dataDir, "Thumbnails");
const statePath = join(dataDir, "state.json");
const capturesDir = join(base, "userprofile", "Videos", "Captures");
const fixture = join(base, "fixture.mp4");
const evidenceDir = process.env.HANG_EVIDENCE_DIR ?? join(".superpowers", "p3-evidence");
const appExe = resolve(process.env.HANG_APP_EXE ?? join("src-tauri", "target", "debug", "hangline.exe"));
mkdirSync(evidenceDir, { recursive: true });

const results = { checks: {}, steps: [] };
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
      last = await evaluate(cdp, expression);
    } catch (error) {
      last = `error: ${error instanceof Error ? error.message : String(error)}`;
    }
    if (last) return last;
    await sleep(intervalMs);
  }
  throw new Error(`timed out waiting for ${label} (last=${JSON.stringify(last)})`);
}

async function screenshot(cdp, name) {
  const captured = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
  });
  writeFileSync(join(evidenceDir, name), Buffer.from(captured.data, "base64"));
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

/** Shows the click-through panel via the single-instance toggle (same action as the tray). */
async function revealPanel(cdp) {
  const revealed = await evaluate(
    cdp,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
  ).catch(() => false);
  if (!revealed && existsSync(appExe)) {
    const { spawn } = await import("node:child_process");
    try {
      const child = spawn(appExe, [], { detached: true, stdio: "ignore", env: { ...process.env } });
      child.unref();
    } catch (error) {
      console.warn("recording-e2e: cannot spawn the toggle instance", error);
    }
  }
  await waitFor(
    cdp,
    `document.querySelector(".panel")?.dataset.revealed === "true"`,
    "panel reveal",
    10000,
  ).catch(() => undefined);
  await sleep(600);
}

/** Reads a PNG's IHDR without decoding pixels. */
function pngSize(path) {
  const bytes = readFileSync(path);
  if (bytes.length < 24 || bytes.toString("ascii", 1, 4) !== "PNG") return null;
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

function paeth(left, up, upLeft) {
  const estimate = left + up - upLeft;
  const dl = Math.abs(estimate - left);
  const du = Math.abs(estimate - up);
  const dul = Math.abs(estimate - upLeft);
  if (dl <= du && dl <= dul) return left;
  return du <= dul ? up : upLeft;
}

/** Minimal 8-bit non-interlaced RGB/RGBA PNG decoder (the Rust writer emits RGBA8). */
function decodePng(path) {
  const bytes = readFileSync(path);
  let offset = 8;
  let header = null;
  const idat = [];
  while (offset + 8 <= bytes.length) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString("ascii", offset + 4, offset + 8);
    const data = bytes.subarray(offset + 8, offset + 8 + length);
    if (type === "IHDR") {
      header = {
        width: data.readUInt32BE(0),
        height: data.readUInt32BE(4),
        depth: data[8],
        colorType: data[9],
        interlace: data[12],
      };
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    offset += 12 + length;
  }
  const channels = header?.colorType === 6 ? 4 : header?.colorType === 2 ? 3 : 0;
  if (!header || !channels || header.depth !== 8 || header.interlace !== 0) {
    throw new Error(`unsupported PNG shape: ${JSON.stringify(header)}`);
  }
  const raw = inflateSync(Buffer.concat(idat));
  const { width, height } = header;
  const stride = width * channels;
  const pixels = Buffer.alloc(stride * height);
  let position = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[position++];
    const row = y * stride;
    for (let x = 0; x < stride; x += 1) {
      const value = raw[position++];
      const left = x >= channels ? pixels[row + x - channels] : 0;
      const up = y > 0 ? pixels[row - stride + x] : 0;
      const upLeft = y > 0 && x >= channels ? pixels[row - stride + x - channels] : 0;
      let out;
      switch (filter) {
        case 0:
          out = value;
          break;
        case 1:
          out = value + left;
          break;
        case 2:
          out = value + up;
          break;
        case 3:
          out = value + Math.floor((left + up) / 2);
          break;
        case 4:
          out = value + paeth(left, up, upLeft);
          break;
        default:
          throw new Error(`unsupported PNG filter ${filter}`);
      }
      pixels[row + x] = out & 0xff;
    }
  }
  return {
    width,
    height,
    channels,
    pixel(x, y) {
      const index = y * stride + x * channels;
      return [pixels[index], pixels[index + 1], pixels[index + 2]];
    },
  };
}

/** Average RGB over a rectangle inside the fixture's teal cell (away from grid lines). */
function averageColor(png, x0, y0, x1, y1) {
  const sum = [0, 0, 0];
  let count = 0;
  for (let y = y0; y < y1; y += 1) {
    for (let x = x0; x < x1; x += 1) {
      const [r, g, b] = png.pixel(x, y);
      sum[0] += r;
      sum[1] += g;
      sum[2] += b;
      count += 1;
    }
  }
  return sum.map((value) => Math.round(value / count));
}

function residue(dir, suffix = ".tmp") {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).filter((name) => name.toLowerCase().endsWith(suffix));
}

function listVideos(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).filter((name) => /\.(mp4|mov)$/i.test(name));
}

// --- 1. start from an empty line --------------------------------------------------------------

const panel = await connectTo("window=panel");
const before = await invoke(panel, "get_app_state");
const beforeItems = before?.items ?? [];
check("profile starts with an empty line", beforeItems.length === 0, beforeItems.length);
step("panel connected", { dataDir });

// --- 2. copy the fixture AFTER app start ------------------------------------------------------

if (!existsSync(fixture)) {
  throw new Error(`missing fixture mp4 at ${fixture}; run scripts/p3/fixture-video.ps1 first`);
}
mkdirSync(capturesDir, { recursive: true });
const target = join(capturesDir, "p3-recording-fixture.mp4");
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
step("item ingested", { ingestMs, id: item?.id });

// --- 4. preview file --------------------------------------------------------------------------

const thumbPath = item?.thumbPath ?? "";
const expectedThumb = join(thumbsDir, `${item?.id}.png`);
const thumbMatchesId = thumbPath.toLowerCase() === expectedThumb.toLowerCase();
const thumbPng = thumbPath && existsSync(thumbPath) ? pngSize(thumbPath) : null;
const thumbBytes = thumbPath && existsSync(thumbPath) ? statSync(thumbPath).size : 0;
results.preview = { thumbPath, thumbMatchesId, thumbPng, thumbBytes };
check(
  "thumbPath points at Thumbnails/<id>.png",
  thumbMatchesId,
  { thumbPath, expectedThumb },
);
const thumbAspect = thumbPng ? thumbPng.width / thumbPng.height : 0;
check(
  "preview file has a 432 px longest edge and 16:9 aspect",
  thumbPng?.width === 432 && Math.abs(thumbPng.height - 243) <= 1 && Math.abs(thumbAspect - 16 / 9) < 0.02,
  { file: thumbPng, aspect: thumbAspect },
);
check("preview file is a non-empty PNG", thumbBytes > 0, thumbBytes);

const png = thumbPath && existsSync(thumbPath) ? decodePng(thumbPath) : null;
const firstFrameColor = png ? averageColor(png, 20, 20, 61, 51) : null;
results.firstFrameColor = firstFrameColor;
// The fixture is a neutral teal (RGB 30,111,115) grid; yuv420 shifts it slightly.
check(
  "preview pixels are the fixture's teal first frame",
  firstFrameColor !== null &&
    firstFrameColor[0] < 90 &&
    firstFrameColor[1] > 80 &&
    firstFrameColor[2] > 80 &&
    firstFrameColor[1] >= firstFrameColor[0] + 30,
  firstFrameColor,
);

// --- 5. card DOM: preview + play badge, no filename fallback ----------------------------------

await revealPanel(panel);
const dom = JSON.parse(
  await waitFor(
    panel,
    `(() => {
      const slot = document.querySelector('.glass-card-slot[data-item-id="${item.id}"]');
      const image = slot?.querySelector("img.glass-card__image");
      if (!slot || !image || !image.complete || image.naturalWidth === 0) return "";
      const photo = slot.querySelector(".glass-card__photo");
      return JSON.stringify({
        cards: document.querySelectorAll(".glass-card-slot").length,
        kind: slot.dataset.kind,
        src: decodeURIComponent(image.src),
        naturalWidth: image.naturalWidth,
        naturalHeight: image.naturalHeight,
        hasPlayBadge: Boolean(slot.querySelector(".glass-card__play")),
        hasFallback: Boolean(slot.querySelector(".glass-card__fallback")),
        photo: photo
          ? { width: photo.offsetWidth, height: photo.offsetHeight }
          : null,
      });
    })()`,
    "recording card preview",
    15000,
  ),
);
results.dom = dom;
check(
  "panel renders exactly one recording card",
  dom.cards === 1 && dom.kind === "recording",
  { cards: dom.cards, kind: dom.kind },
);
check(
  "card <img> uses the Thumbnails preview",
  dom.src.toLowerCase().includes("thumbnails") &&
    dom.src.split("?")[0].toLowerCase().endsWith(`${item.id}.png`),
  dom.src,
);
check(
  "card <img> decoded at the preview's dimensions",
  dom.naturalWidth === thumbPng?.width && dom.naturalHeight === thumbPng?.height,
  { width: dom.naturalWidth, height: dom.naturalHeight, file: thumbPng },
);
check("card shows the play badge", dom.hasPlayBadge === true, dom.hasPlayBadge);
check("card does not show the filename fallback", dom.hasFallback === false, dom.hasFallback);
const photoRatio = dom.photo ? dom.photo.width / dom.photo.height : 0;
check(
  "card photo keeps the 16:9 video aspect",
  Math.abs(photoRatio - 16 / 9) < 0.05,
  { ratio: photoRatio, photo: dom.photo },
);

// Evidence captures: the panel strip and a panel-only card crop, both over a neutral backdrop.
await capturePanel(panel, "recording-panel.png");
const viewport = JSON.parse(
  await evaluate(
    panel,
    `JSON.stringify({ width: window.innerWidth, height: window.innerHeight })`,
  ),
);
const slotBox = JSON.parse(
  await evaluate(
    panel,
    `(() => {
      const slot = document.querySelector('.glass-card-slot[data-item-id="${item.id}"]');
      const rect = slot.getBoundingClientRect();
      return JSON.stringify({ x: rect.left - 18, y: rect.top - 14, width: rect.width + 36, height: rect.height + 28 });
    })()`,
  ),
);
const clip = {
  x: Math.max(0, slotBox.x),
  y: Math.max(0, slotBox.y),
  width: Math.min(slotBox.width, viewport.width - Math.max(0, slotBox.x)),
  height: Math.min(slotBox.height, viewport.height - Math.max(0, slotBox.y)),
};
await capturePanel(panel, "recording-card.png", clip);
step("panel evidence captured", { panel: "recording-panel.png", card: "recording-card.png" });

// --- 6. idempotence + no residue --------------------------------------------------------------

await sleep(3000);
const settled = await invoke(panel, "get_app_state");
const afterItems = settled?.items ?? [];
results.settled = { items: afterItems };
check(
  "no duplicate item after 3 s",
  afterItems.length === 1 && afterItems[0]?.id === item.id,
  afterItems.map((entry) => entry.id),
);

const persisted = JSON.parse(readFileSync(statePath, "utf8"));
results.stateJson = persisted;
check(
  "state.json persisted one item with dims/thumbPath",
  persisted.length === 1 &&
    persisted[0]?.kind === "recording" &&
    persisted[0]?.width === 640 &&
    persisted[0]?.height === 360 &&
    typeof persisted[0]?.thumbPath === "string" &&
    persisted[0].thumbPath.endsWith(`${item.id}.png`),
  persisted[0],
);

const videos = listVideos(capturesDir);
const thumbResidue = residue(thumbsDir);
const captureResidue = residue(capturesDir);
results.files = { videos, thumbResidue, captureResidue };
check("exactly one recording file in the watched folder", videos.length === 1, videos);
check("no .tmp residue in the thumbnail cache", thumbResidue.length === 0, thumbResidue);
check("no .tmp residue in the watched folder", captureResidue.length === 0, captureResidue);

// --- 7. report --------------------------------------------------------------------------------

results.verdict = failures === 0 ? "PASS" : `FAIL (${failures} check(s))`;
results.checkCount = Object.keys(results.checks).length;
results.passed = results.checkCount - failures;
results.failures = failures;
results.environment = {
  cdpPort: process.env.HANG_CDP_PORT ?? "9222",
  dataDir,
  capturesDir,
  evidenceDir,
  node: process.version,
};
writeFileSync(join(evidenceDir, "recording-e2e.json"), `${JSON.stringify(results, null, 2)}\n`);
panel.close();
console.log(`verdict: ${results.verdict} (${results.passed}/${results.checkCount} checks passed)`);
process.exit(failures === 0 ? 0 : 1);
