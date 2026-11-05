#!/usr/bin/env node
/* global fetch, Buffer */
// Dev-only: downloads Tendedero promotional reference assets for local frame comparison.
// The assets are Tendedero's (MIT + name/icon restriction) and are NEVER committed.
import { mkdir, writeFile, access } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../.superpowers/p2-reference");
const base = "https://raw.githubusercontent.com/alejandrobujan/tendedero/main/docs/images/";
const files = [
  "hero-light.png",
  "hero-dark.png",
  "demo-light.gif",
  "demo-dark.gif",
  "bento-light.png",
  "bento-dark.png",
  "icon.png",
];

await mkdir(root, { recursive: true });
for (const name of files) {
  const dest = join(root, name);
  try {
    await access(dest);
    console.log(`skip ${name}`);
    continue;
  } catch {
    // Not downloaded yet: fetch it below.
  }
  let response;
  try {
    response = await fetch(base + name);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`FAIL ${name}: network error (${message})`);
    process.exitCode = 1;
    continue;
  }
  if (!response.ok) {
    console.error(`FAIL ${name}: HTTP ${response.status}`);
    process.exitCode = 1;
    continue;
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  await writeFile(dest, bytes);
  console.log(`ok ${name} ${bytes.length} bytes`);
}
console.log(`reference dir: ${root} (git-ignored)`);
