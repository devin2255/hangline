#!/usr/bin/env node
/* global Buffer */
/**
 * P2-9 dev fixtures + isolated profile setup (NO dependencies; Node 22 built-ins only).
 *
 * Creates the exact fixture set the perf/compare runbook needs, without touching the real
 * `%LOCALAPPDATA%`:
 *
 *   node scripts/p2/fixtures.mjs --profile "$env:TEMP\hangline-p2-9\localappdata\Hangline" \
 *                                --cards "$env:TEMP\hangline-p2-9\cards"
 *   node scripts/p2/fixtures.mjs --copy-cards "$env:TEMP\hangline-p2-9\cards" \
 *                                --to "$env:TEMP\hangline-p2-9\localappdata\Hangline\Screenshots"
 *
 * The `--profile` mode writes `settings.json` / `state.json` / `Screenshots\`. The `--copy-cards`
 * mode copies the 12 4K fixtures **after** the app is ready, because the folder watcher has no
 * initial scan — files present before launch never produce a notify event.
 *
 * PNGs are generated with the built-in zlib (a vertical gradient; content is synthetic and safe to
 * commit nowhere — only the generated docs/experiments screenshots are ever committed).
 */

import { mkdirSync, copyFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

const CARD_W = 3840;
const CARD_H = 2160;
const ACTION_W = 640;
const ACTION_H = 440;

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

/** Minimal RGB8 PNG writer; `shade(x, y)` returns [r, g, b]. */
function png(width, height, shade) {
  const stride = width * 3 + 1;
  const raw = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y += 1) {
    const row = y * stride;
    raw[row] = 0; // filter: none
    const [r, g, b] = shade(0.5, y / Math.max(1, height - 1));
    const rowBytes = raw.subarray(row + 1, row + stride);
    for (let x = 0; x < width; x += 1) {
      rowBytes[x * 3] = r;
      rowBytes[x * 3 + 1] = g;
      rowBytes[x * 3 + 2] = b;
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

function shadeFor(seed) {
  return (_x, t) => [
    (20 + seed * 17 + Math.round(120 * t)) % 256,
    (40 + seed * 23 + Math.round(150 * t)) % 256,
    (70 + seed * 31 + Math.round(130 * t)) % 256,
  ];
}

function writeCards(dir) {
  mkdirSync(dir, { recursive: true });
  for (let index = 0; index < 12; index += 1) {
    writeFileSync(join(dir, `card-${index}.png`), png(CARD_W, CARD_H, shadeFor(index)));
  }
  for (let index = 0; index < 6; index += 1) {
    writeFileSync(join(dir, `action-${index}.png`), png(ACTION_W, ACTION_H, shadeFor(index + 30)));
  }
  console.log(`fixtures: wrote 12 4K card-*.png and 6 action-*.png into ${dir}`);
}

function writeProfile(profile) {
  const screenshots = join(profile, "Screenshots");
  mkdirSync(screenshots, { recursive: true });
  const settings = {
    captureHotkey: "Ctrl+Alt+S",
    toggleHotkey: "Ctrl+Alt+T",
    watchSystemScreenshots: false,
    watchClipboard: false,
    watchRecordings: false,
    extraFolders: [],
    revealOnNew: true,
    peekDurationMs: 2500,
    language: "en",
    theme: "dark",
    autoCheckUpdates: false,
    lastUpdateCheck: "",
    cardSize: "medium",
    keepOnLine: "fit",
  };
  writeFileSync(join(profile, "settings.json"), JSON.stringify(settings, null, 2) + "\n");
  writeFileSync(join(profile, "state.json"), "[]\n");
  console.log(`fixtures: isolated profile ready at ${profile} (watchRecordings=false, extraFolders=[])`);
}

function copyCards(cardsDir, screenshotsDir) {
  mkdirSync(screenshotsDir, { recursive: true });
  const files = readdirSync(cardsDir).filter((name) => name.startsWith("card-") && name.endsWith(".png"));
  if (files.length < 12) throw new Error(`expected 12 card fixtures in ${cardsDir}, found ${files.length}`);
  for (const name of files) copyFileSync(join(cardsDir, name), join(screenshotsDir, name));
  console.log(`fixtures: copied ${files.length} card fixtures into ${screenshotsDir} (after app start)`);
}

const args = process.argv.slice(2);
const value = (name) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
};
if (args.includes("--profile")) {
  writeProfile(value("--profile"));
} else if (args.includes("--cards")) {
  writeCards(value("--cards"));
} else if (args.includes("--copy-cards")) {
  copyCards(value("--copy-cards"), value("--to"));
} else {
  console.error(
    "usage: fixtures.mjs --profile <dir> | --cards <dir> | --copy-cards <cardsDir> --to <Screenshots>",
  );
  process.exit(2);
}
