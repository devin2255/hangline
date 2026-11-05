#!/usr/bin/env node
/**
 * Dev-only static server for the packaged-CSP proof (P3-6 review follow-up).
 *
 * The debug app loads its `devUrl` (`http://localhost:5173`) from whatever serves that port, so this
 * server can reproduce the *packaged* Content-Security-Policy without building an installer: it
 * serves `dist/` with the exact CSP from `src-tauri/tauri.conf.json` as a response header. The
 * packaged build injects the same policy as a `<meta>` tag, so a `media-src` decision taken under
 * this header is the same decision the installed app makes.
 *
 * Usage (from the repository root, after `pnpm build`):
 *
 *   node scripts/p3/csp-static-server.mjs --dist dist --port 5173 --csp "<exact CSP>"
 *
 * `scripts/p3/trim-csp-e2e.mjs` imports `startCspServer` instead of spawning this CLI; the CLI form
 * exists for manual inspection with a browser or `curl -I`.
 */

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".mp4": "video/mp4",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
};

/** Resolves a request path inside `root`, refusing traversal outside the build output. */
function resolveAsset(root, requestUrl) {
  let pathname;
  try {
    pathname = decodeURIComponent(new URL(requestUrl, "http://localhost").pathname);
  } catch {
    return null;
  }
  const candidate = resolve(join(root, pathname === "/" ? "/index.html" : pathname));
  if (candidate !== root && !candidate.startsWith(`${root}${sep}`)) return null;
  if (existsSync(candidate) && statSync(candidate).isFile()) return candidate;
  // SPA fallback: `index.html?window=trim` and any extension-less route render the app.
  const index = join(root, "index.html");
  return existsSync(index) ? index : null;
}

/**
 * Serves `dist` on `port` with `csp` on every response.
 *
 * @param {{dist?: string, port?: number, csp: string, host?: string}} options
 * @returns {Promise<{url: string, port: number, csp: string, close: () => Promise<void>}>}
 */
export function startCspServer({ dist = resolve("dist"), port = 5173, csp, host } = {}) {
  if (typeof csp !== "string" || !csp.includes("default-src")) {
    throw new Error("startCspServer: a complete CSP string is required");
  }
  const root = resolve(dist);
  if (!existsSync(join(root, "index.html"))) {
    throw new Error(`startCspServer: ${root} has no index.html; run \`pnpm build\` first`);
  }

  const server = createServer((request, response) => {
    if (request.method !== "GET" && request.method !== "HEAD") {
      response.writeHead(405, { allow: "GET, HEAD", "content-security-policy": csp });
      response.end();
      return;
    }
    const file = resolveAsset(root, request.url ?? "/");
    if (!file) {
      response.writeHead(404, { "content-type": "text/plain", "content-security-policy": csp });
      response.end("not found");
      return;
    }
    response.writeHead(200, {
      "content-type": MIME[extname(file).toLowerCase()] ?? "application/octet-stream",
      // The exact policy under test: one response header, no meta tag, no Vite dev server.
      "content-security-policy": csp,
      "cache-control": "no-store",
    });
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    createReadStream(file).pipe(response);
  });

  return new Promise((resolvePromise, reject) => {
    server.once("error", reject);
    const listenArgs = host ? [port, host] : [port];
    server.listen(...listenArgs, () => {
      const address = server.address();
      const actualPort = typeof address === "object" && address ? address.port : port;
      resolvePromise({
        url: `http://localhost:${actualPort}`,
        port: actualPort,
        csp,
        close: () =>
          new Promise((done) => {
            server.close(() => done());
            // Node keeps keep-alive sockets open; the harness closes them anyway, but the CLI form
            // must exit on Ctrl+C.
            server.closeAllConnections?.();
          }),
      });
    });
  });
}

/** True when this module is the process entry point (CLI form). */
function isCliEntry() {
  const entry = process.argv[1];
  return Boolean(entry) && import.meta.url === pathToFileURL(resolve(entry)).href;
}

if (isCliEntry()) {
  const args = process.argv.slice(2);
  const option = (name, fallback) => {
    const index = args.indexOf(`--${name}`);
    return index >= 0 ? args[index + 1] : fallback;
  };
  const dist = option("dist", "dist");
  const port = Number(option("port", "5173"));
  const csp = option("csp", "");
  try {
    const served = await startCspServer({ dist, port, csp });
    console.log(`csp-static-server: ${served.url} serving ${resolve(dist)}`);
    console.log(`csp-static-server: content-security-policy: ${served.csp}`);
    const shutdown = () => {
      void served.close().then(() => process.exit(0));
    };
    process.on("SIGINT", shutdown);
    process.on("SIGTERM", shutdown);
  } catch (error) {
    console.error(`csp-static-server: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(2);
  }
}
