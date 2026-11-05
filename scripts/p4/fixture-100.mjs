#!/usr/bin/env node
/* global Buffer */
/**
 * P4-1/P4-5 100-card fixture (dev-only, Node built-ins only).
 *
 * Creates an isolated Hangline profile under `%TEMP%\hangline-p4-100`:
 *
 *   localappdata\Hangline\
 *   ├── settings.json                 keepOnLine=100, cardSize=medium, all watchers off
 *   ├── state.json                    100 items, ascending addedAt (newest on the right)
 *   └── Screenshots\                 100 neutral 640×360 PNGs (synthetic, never user content)
 *
 * and a small summary next to it (`fixture-100.json`, git-ignored because it lives in %TEMP%):
 * `{ profile, localAppData, count, ids, paths }`.
 *
 *   node scripts/p4/fixture-100.mjs
 *   $env:LOCALAPPDATA = "$env:TEMP\hangline-p4-100\localappdata"
 *   $env:HANG_CDP_PORT = "9227"
 *   Start-Process src-tauri\target\debug\hangline.exe
 *   node scripts/p4/perf-100.mjs
 *
 * The app's startup thumbnail backfill generates the previews (the fixture does not write
 * `Thumbnails\`), which also means the run measures the real app path. The helper refuses to touch
 * anything outside `%TEMP%` (or a build `target\` directory) so the user's profile can never be
 * cleared by a mistyped path.
 */

import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const REPO_TARGET = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "src-tauri", "target");

const COUNT = 100;
const CARD_W = 640;
const CARD_H = 360;
/** Bump when the fixture shape changes; the perf harness records it. */
const FIXTURE_VERSION = 1;

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

/**
 * Minimal RGB8 PNG with a neutral slate gradient and an index-dependent light band.
 *
 * The content is purely synthetic (no desktop/user pixels); the band makes neighbouring cards
 * visually distinct so the scroll evidence screenshots show real movement.
 */
function fixturePng(index) {
  const stride = CARD_W * 3 + 1;
  const raw = Buffer.alloc(stride * CARD_H);
  const tint = index % 6;
  const bandCenter = 0.14 + (index % 10) * 0.075;
  for (let y = 0; y < CARD_H; y += 1) {
    const row = y * stride;
    raw[row] = 0; // filter: none
    const t = y / (CARD_H - 1);
    const band = Math.max(0, 1 - Math.abs(t - bandCenter) * 9) * 20;
    const r = Math.min(255, 58 + tint * 7 + Math.round(58 * t) + band);
    const g = Math.min(255, 70 + tint * 5 + Math.round(52 * t) + band);
    const b = Math.min(255, 84 + tint * 3 + Math.round(44 * (1 - t)) + band);
    const rowBytes = raw.subarray(row + 1, row + stride);
    for (let x = 0; x < CARD_W; x += 1) {
      rowBytes[x * 3] = r;
      rowBytes[x * 3 + 1] = g;
      rowBytes[x * 3 + 2] = b;
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(CARD_W, 0);
  header.writeUInt32BE(CARD_H, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 6 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** 32 lowercase hex characters, the backend's opaque item-id shape. */
function itemId(index) {
  return (0x1000_0000 + index).toString(16).padStart(32, "0");
}

/** Refuses to clear a directory that is not inside %TEMP% or this repo's `src-tauri\target`. */
function assertDisposable(dir) {
  const resolved = resolve(dir).toLowerCase();
  const temp = resolve(tmpdir()).toLowerCase();
  const insideTemp = resolved === temp || resolved.startsWith(`${temp}${sep}`);
  const repoTarget = REPO_TARGET.toLowerCase();
  const insideTarget = resolved === repoTarget || resolved.startsWith(`${repoTarget}${sep}`);
  if (!insideTemp && !insideTarget) {
    console.error(
      `fixture-100: refusing to write/clear ${resolve(dir)} (must be under %TEMP% or this repo's src-tauri\\target)`,
    );
    process.exit(2);
  }
}

const base = resolve(process.env.HANG_P4_BASE ?? join(tmpdir(), "hangline-p4-100"));
assertDisposable(base);
if (basename(base) === "Hangline") {
  console.error("fixture-100: HANG_P4_BASE must be the run root, not the Hangline profile dir");
  process.exit(2);
}

const localAppData = join(base, "localappdata");
const profile = join(localAppData, "Hangline");
const screenshots = join(profile, "Screenshots");

// Full re-create: the fixture is disposable by construction (still no `Hangline`-named root).
rmSync(base, { recursive: true, force: true });
mkdirSync(screenshots, { recursive: true });

const now = Date.now();
const ids = [];
const paths = [];
const items = [];
for (let index = 0; index < COUNT; index += 1) {
  const id = itemId(index);
  const name = `hangline-p4-${String(index).padStart(3, "0")}.png`;
  const path = join(screenshots, name);
  // Ascending addedAt, one second apart, oldest first (the newest hangs on the right).
  const addedAt = now - COUNT * 1000 - 60_000 + index * 1000;
  writeFileSync(path, fixturePng(index));
  ids.push(id);
  paths.push(path);
  items.push({
    id,
    path,
    kind: "capture",
    hash: 1000 + index,
    width: CARD_W,
    height: CARD_H,
    createdAt: addedAt,
    addedAt,
    updatedAt: addedAt,
  });
}

const settings = {
  captureHotkey: "Ctrl+Alt+S",
  toggleHotkey: "Ctrl+Alt+T",
  watchSystemScreenshots: false,
  watchClipboard: false,
  watchRecordings: false,
  extraFolders: [],
  revealOnNew: false,
  revealPeekMs: 2500,
  topEdgeReveal: false,
  // Never let a full-screen foreground app suppress the evidence run (P4-2 adds the probe).
  showOverFullScreen: true,
  keepOnLine: COUNT,
  cardSize: "medium",
  sound: false,
  startAtLogin: false,
  language: "en",
  theme: "dark",
  autoCheckUpdates: false,
  lastUpdateCheck: "",
};
writeFileSync(join(profile, "settings.json"), `${JSON.stringify(settings, null, 2)}\n`);
writeFileSync(join(profile, "state.json"), `${JSON.stringify(items, null, 2)}\n`);

const summary = {
  fixtureVersion: FIXTURE_VERSION,
  generatedAt: new Date().toISOString(),
  base,
  localAppData,
  profile,
  count: COUNT,
  cardSize: "medium",
  keepOnLine: COUNT,
  width: CARD_W,
  height: CARD_H,
  ids,
  paths,
};
const summaryPath = join(base, "fixture-100.json");
writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);

console.log(
  `fixture-100: ${COUNT} cards (${CARD_W}x${CARD_H}) ready at ${profile}\n` +
    `fixture-100: summary ${summaryPath}`,
);
