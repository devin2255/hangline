#!/usr/bin/env node
/* global fetch */
/** Waits until the panel webview is ready for CDP measurements (fresh debug app launch). */

import { connectTo, evaluate, sleep } from "./cdp.mjs";

// 0. The app devUrl is fixed to 5173: wait until the Vite dev server answers before CDP.
const viteDeadline = Date.now() + 60000;
let viteReady = false;
while (Date.now() < viteDeadline) {
  try {
    const response = await fetch("http://localhost:5173/");
    if (response.ok) { viteReady = true; break; }
  } catch {
    // Vite not up yet.
  }
  await sleep(500);
}
if (!viteReady) {
  console.error("wait: Vite on http://localhost:5173/ did not answer within 60 s (start `pnpm dev` first)");
  process.exit(1);
}
console.log("wait: vite ready");

const panel = await connectTo("window=panel");
let reloaded = false;
for (let attempt = 0; attempt < 160; attempt += 1) {
  if (!reloaded && attempt === 20) {
    // A first load racing the Vite dependency optimizer can stay blank; reload once.
    reloaded = true;
    await evaluate(panel, "location.reload()").catch(() => {});
    await sleep(1500);
    continue;
  }
  const state = await evaluate(
    panel,
    "JSON.stringify({ frames: !!window.__HANG_FRAMES__, panel: !!document.querySelector('.panel'), ready: document.readyState })",
  );
  const parsed = JSON.parse(state);
  if (parsed.frames && parsed.panel && parsed.ready === "complete") {
    console.log("wait: panel ready", state);
    panel.close();
    process.exit(0);
  }
  await sleep(500);
}
console.error("wait: panel did not become ready (see docs/experiments/P2-visual.md runbook)");
panel.close();
process.exit(1);
