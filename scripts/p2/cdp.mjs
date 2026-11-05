/* global fetch, WebSocket, Buffer, setTimeout */
// P2-9 dev-only CDP helper (no dependencies): Node 22 global fetch + WebSocket.
const PORT = Number(process.env.HANG_CDP_PORT ?? 9222);

export class Cdp {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    /** CDP event handlers keyed by method (dev harness only; no dependency on the page). */
    this.handlers = new Map();
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.method) {
        for (const handler of this.handlers.get(message.method) ?? []) handler(message.params);
        return;
      }
      if (!message.id || !this.pending.has(message.id)) return;
      const { resolve, reject } = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result);
    });
  }
  ready() {
    return new Promise((resolve, reject) => {
      this.socket.addEventListener("open", () => resolve(this), { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
  }
  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  /** Subscribes to one CDP event; returns the unsubscribe function. */
  on(method, handler) {
    const handlers = this.handlers.get(method) ?? [];
    handlers.push(handler);
    this.handlers.set(method, handlers);
    return () => {
      const current = this.handlers.get(method);
      if (!current) return;
      const index = current.indexOf(handler);
      if (index >= 0) current.splice(index, 1);
      if (current.length === 0) this.handlers.delete(method);
    };
  }
  close() {
    this.socket.close();
  }
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function fetchTargets(timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  let delay = 250;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/json/list`);
      if (response.ok) return await response.json();
      lastError = new Error(`HTTP ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await sleep(delay);
    delay = Math.min(Math.round(delay * 1.5), 1000);
  }
  throw new Error(
    `CDP not reachable at 127.0.0.1:${PORT} within ${timeoutMs} ms (${lastError}); ` +
      `start the debug app with HANG_CDP_PORT=${PORT} (see docs/experiments/P2-visual.md runbook)`,
  );
}

export async function targets() {
  return fetchTargets();
}

export async function connectTo(match, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const list = await targets();
    const page = list.find((target) => target.type === "page" && target.url.includes(match));
    if (page) {
      const cdp = await new Cdp(page.webSocketDebuggerUrl).ready();
      await cdp.send("Runtime.enable");
      await cdp.send("Page.enable");
      let navigated = false;
      for (let attempt = 0; attempt < 90; attempt += 1) {
        const ready = await evaluate(cdp, `typeof window.__TAURI_INTERNALS__ === "object" && !!document.body`).catch(() => false);
        if (ready) return cdp;
        if (!navigated && attempt === 6) {
          navigated = true;
          await cdp.send("Page.navigate", { url: page.url }).catch(() => {});
        }
        await sleep(200);
      }
    }
    await sleep(250);
  }
  throw new Error(`no CDP target matching ${match}`);
}

export async function evaluate(cdp, expression) {
  const result = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result?.value;
}

export async function screenshotClip(cdp, filePath, clip) {
  const { writeFileSync } = await import("node:fs");
  const captured = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
    captureBeyondViewport: false,
    clip: { ...clip, scale: 1 },
  });
  writeFileSync(filePath, Buffer.from(captured.data, "base64"));
}
