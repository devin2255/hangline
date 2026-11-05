#!/usr/bin/env node
/* global Buffer */
/**
 * P3-2/P3-3 dev fixture: one isolated Hangline profile with synthetic screenshots.
 *
 *   node scripts/p3/fixture.mjs --profile "$env:TEMP\hangline-p3\localappdata\Hangline"
 *   node scripts/p3/fixture.mjs --profile … --watched   # adds a read-only watched-folder item
 *   node scripts/p3/fixture.mjs --profile … --recordings # empty line + Videos\Captures watcher
 *
 * Writes `settings.json` (every watcher off, English, dark), `Screenshots/p3-markup-fixture.png`
 * (1600×1000 vertical gradient; neutral synthetic art) and `state.json` with one item. With
 * `--watched` it also writes `Watched/p3-watched-fixture.png` (1200×800, `kind: watchedFolder`, a
 * configured extra folder) as the second item, used by the P3-4 read-only long-press E2E. With
 * `--recordings` it writes no items (an empty line), enables the recordings folder watcher and
 * `revealOnNew` with a one-minute peek, so the P3-5 E2E can copy a generated mp4 into
 * `%USERPROFILE%\Videos\Captures` after startup. The app's screenshot-folder watcher has no
 * initial scan, so the items are simply loaded at startup; the startup thumbnail backfill
 * generates their 432 px previews. NO dependencies beyond Node built-ins.
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

const WIDTH = 1600;
const HEIGHT = 1000;
/** Fixed item id (32 lowercase hex, the backend's opaque-id shape). */
const ITEM_ID = "0123456789abcdef0123456789abcdef";
const FILE_NAME = "p3-markup-fixture.png";
const WATCHED_WIDTH = 1200;
const WATCHED_HEIGHT = 800;
const WATCHED_ID = "fedcba9876543210fedcba9876543210";
const WATCHED_FILE_NAME = "p3-watched-fixture.png";

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

/** Minimal RGB8 PNG: a vertical teal→indigo gradient with a soft diagonal band. */
function fixturePng(width = WIDTH, height = HEIGHT) {
  const stride = width * 3 + 1;
  const raw = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y += 1) {
    const row = y * stride;
    raw[row] = 0; // filter: none
    const t = y / (height - 1);
    for (let x = 0; x < width; x += 1) {
      const band = Math.max(0, 1 - Math.abs(x / width - t) * 6) * 18;
      const index = row + 1 + x * 3;
      raw[index] = Math.round(18 + 42 * t + band);
      raw[index + 1] = Math.round(120 + 70 * (1 - t) + band);
      raw[index + 2] = Math.round(140 + 90 * (1 - t) + band);
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 6 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const args = process.argv.slice(2);
const value = (name) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
};
const profile = value("--profile");
if (!profile) {
  console.error("usage: fixture.mjs --profile <Hangline data dir>");
  process.exit(2);
}
const screenshots = join(profile, "Screenshots");
mkdirSync(screenshots, { recursive: true });
const recordings = args.includes("--recordings");
const imagePath = join(screenshots, FILE_NAME);
if (!recordings) {
  writeFileSync(imagePath, fixturePng());
}

const watchedEnabled = args.includes("--watched");
const watchedDir = join(profile, "Watched");
const watchedPath = join(watchedDir, WATCHED_FILE_NAME);
if (watchedEnabled) {
  mkdirSync(watchedDir, { recursive: true });
  writeFileSync(watchedPath, fixturePng(WATCHED_WIDTH, WATCHED_HEIGHT));
}

const settings = {
  captureHotkey: "Ctrl+Alt+S",
  toggleHotkey: "Ctrl+Alt+T",
  watchSystemScreenshots: false,
  watchClipboard: false,
  watchRecordings: recordings,
  extraFolders: watchedEnabled ? [watchedDir] : [],
  revealOnNew: recordings,
  revealPeekMs: recordings ? 60000 : 2500,
  topEdgeReveal: false,
  showOverFullScreen: true,
  keepOnLine: "fit",
  cardSize: "medium",
  sound: false,
  startAtLogin: false,
  language: "en",
  theme: "dark",
  autoCheckUpdates: false,
  lastUpdateCheck: "",
};
writeFileSync(join(profile, "settings.json"), `${JSON.stringify(settings, null, 2)}\n`);

const now = Date.now();
const items = [];
if (!recordings) {
  items.push({
    id: ITEM_ID,
    path: imagePath,
    kind: "capture",
    hash: 42,
    width: WIDTH,
    height: HEIGHT,
    createdAt: now - 5000,
    addedAt: now - 4000,
    updatedAt: now - 4000,
  });
}
if (watchedEnabled && !recordings) {
  items.push({
    id: WATCHED_ID,
    path: watchedPath,
    kind: "watchedFolder",
    hash: 43,
    width: WATCHED_WIDTH,
    height: WATCHED_HEIGHT,
    createdAt: now - 3000,
    addedAt: now - 2000,
    updatedAt: now - 2000,
  });
}
writeFileSync(join(profile, "state.json"), `${JSON.stringify(items, null, 2)}\n`);
console.log(
  `fixture: profile ready at ${profile} (` +
    `${recordings ? "empty line, recordings watcher on" : `${items.length} item(s): ${WIDTH}x${HEIGHT} owned${watchedEnabled ? `, ${WATCHED_WIDTH}x${WATCHED_HEIGHT} watched` : ""}`})`,
);
