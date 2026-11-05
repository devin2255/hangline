#!/usr/bin/env node
/* global Buffer */
/**
 * Build-level release-hygiene check (P4-6): the browser-side diagnostic instrumentation must not
 * ship in the production bundle.
 *
 * `src/lib/perf.ts` (rAF frame ring + line-offset trace) and `src/lib/trace.ts` (P2-9 motion trace)
 * are development-only: their install sites are gated on `import.meta.env.DEV`, so Vite drops the
 * `window.__HANG_*` globals and the sampling code from `dist/`. The CDP harnesses
 * (`scripts/p2`, `scripts/p3`, `scripts/p4`) only run against the Vite dev server used by the debug
 * build, so the instrumentation stays complete there.
 *
 * The P4-6 review found that scanning only the seven `__HANG_*` globals passed while the frame-ring
 * constructor and `traceEnabled()` still shipped (they were reachable from ungated call sites but
 * their names are minified, so a global-name scan could not see them). The marker list below
 * therefore also contains property keys that survive esbuild/terser minification (`fpsMean`,
 * `quarterMoves`, ...), source identifiers that catch an unminified bundle, and the minified shape
 * of `createFrameRing`'s object literal.
 *
 * `--self-test` writes a temporary bundle that leaks a global, two non-global markers and the
 * minified frame-ring shape, plus a clean file; it asserts the scanner detects exactly the leaks.
 * It needs no `dist/`. It runs before `vite build` and in
 * `pnpm check`, so a broken detector fails CI instead of silently printing "0 of N".
 *
 * Normal mode runs automatically after `vite build` in `pnpm build` (and therefore in
 * `pnpm tauri build`, whose release exe embeds this same `dist/`). A finding fails the build.
 */

import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const dist = join(repoRoot, "dist");

/**
 * Strings that must not appear in a release bundle.
 *
 * - The first block is the seven debug globals the harnesses install on `window`; they are string
 *   literals, so they would survive minification.
 * - The second block is non-global markers. Object property keys (`fpsMean`, `quarterMoves`, ...)
 *   are not mangled by Vite's esbuild minifier, so they catch the summary/offset code even when
 *   function names are renamed.
 * - The third block catches an unminified (`build.minify: false`) bundle.
 * - The last entry is the minified shape of `createFrameRing`'s return object: it catches a
 *   constructor that stays reachable through an eagerly-evaluated `useRef(...)` argument even
 *   though the function name is mangled to something like `function Zb(i=600){...}`.
 */
const MARKERS = [
  "__HANG_FRAMES__",
  "__HANG_OFFSETS__",
  "__HANG_TRACE__",
  "__HANG_CLEAR_TRACE__",
  "__HANG_TRACE_ON__",
  "__HANG_LONG_TASKS__",
  "__HANG_PERF_MON__",
  "fpsMean",
  "longestBurst",
  "summarizeOffsets",
  "quarterMoves",
  "TRACE_CAPACITY",
  "createFrameRing",
  "traceEnabled",
  "capacity:Math.max(1,Math.floor",
];

/**
 * Release artifacts that can carry executable or inlined code. CSS is scanned too because a marker
 * in a built asset comment/URL would still leak the name. Sourcemaps are deliberately not scanned:
 * they may embed original sources (including dev-history strings) without shipping executable
 * instrumentation, and this build does not emit them.
 */
const SCANNED_EXTENSIONS = new Set([".js", ".mjs", ".cjs", ".html", ".css"]);

function collect(dir) {
  const files = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    const stats = statSync(path);
    if (stats.isDirectory()) files.push(...collect(path));
    else if (SCANNED_EXTENSIONS.has(extname(name))) files.push(path);
  }
  return files;
}

/** Scans every release artifact under `dir`; throws when there is nothing to scan. */
function scan(dir) {
  const files = collect(dir);
  if (files.length === 0) {
    throw new Error(`no bundle files in ${dir}; run \`pnpm build\` first`);
  }
  let bytes = 0;
  const findings = [];
  for (const file of files) {
    // Read the raw bytes: `text.length` counts UTF-16 code units, not the size of the artifact.
    const buffer = readFileSync(file);
    bytes += buffer.length;
    const text = buffer.toString("utf8");
    for (const marker of MARKERS) {
      if (text.includes(marker)) {
        findings.push({ path: relative(repoRoot, file), marker });
      }
    }
  }
  return { files, bytes, findings };
}

function reportFindings(result) {
  for (const finding of result.findings) {
    console.error(
      `release-instrumentation: ${finding.path} still contains the instrumentation marker ` +
        `"${finding.marker}"`,
    );
  }
  console.error(
    "release-instrumentation: FAIL - instrumentation reached the release bundle; gate the install " +
      "site/call site on DEV_INSTRUMENTATION (import.meta.env.DEV) in src/lib/perf.ts, " +
      "src/lib/trace.ts or the component that calls them",
  );
}

function runDistCheck() {
  let result;
  try {
    result = scan(dist);
  } catch (error) {
    console.error(`release-instrumentation: cannot read ${dist}: ${error.message}`);
    console.error("release-instrumentation: run `pnpm build` first (this check runs after vite build)");
    process.exit(1);
  }
  if (result.findings.length > 0) {
    reportFindings(result);
    process.exit(1);
  }
  console.log(
    `release-instrumentation ok: ${result.files.length} bundle file(s), ${result.bytes} bytes, ` +
      `0 of ${MARKERS.length} instrumentation markers present`,
  );
}

/**
 * Proves the detector itself works: a temporary "bundle" leaks a global, a non-global marker and a
 * non-ASCII file (to prove byte counting), while a clean file stays clean.
 */
function runSelfTest() {
  const dir = mkdtempSync(join(tmpdir(), "hangline-release-check-"));
  const fixtures = new Map([
    ["clean.js", 'const label = "héllo";\n'],
    ["leak-frames.js", "window.__HANG_FRAMES__ = { summarize: () => ({}) };\n"],
    ["leak-stats.js", "const summary = { fpsMean: 60, longestBurst: 0 };\n"],
    // The exact minified shape of the P4-6 leak: the frame-ring constructor reachable from an
    // eagerly-evaluated `useRef(...)` argument.
    ["leak-ring.js", "function Zb(i=600){return{capacity:Math.max(1,Math.floor(i)),samples:[]}}\n"],
  ]);
  try {
    for (const [name, contents] of fixtures) writeFileSync(join(dir, name), contents);
    const expectedBytes = [...fixtures.values()].reduce(
      (sum, contents) => sum + Buffer.byteLength(contents, "utf8"),
      0,
    );
    const result = scan(dir);
    const markers = new Set(result.findings.map((finding) => finding.marker));
    const leakingFiles = new Set(result.findings.map((finding) => finding.path));
    const checks = [
      ["detects a debug global", markers.has("__HANG_FRAMES__")],
      ["detects a non-global marker", markers.has("fpsMean")],
      ["detects the minified frame-ring shape", markers.has("capacity:Math.max(1,Math.floor")],
      ["reports every leaking file", leakingFiles.size === 3],
      ["leaves the clean file alone", !result.findings.some((f) => f.path.endsWith("clean.js"))],
      ["counts bytes from the raw buffer", result.bytes === expectedBytes],
    ];
    const failed = checks.filter(([, pass]) => !pass);
    if (failed.length > 0) {
      for (const [name] of failed) console.error(`release-instrumentation self-test: FAIL - ${name}`);
      // Set the exit code instead of calling process.exit: the finally below must remove the temp
      // bundle even when the self-test fails.
      process.exitCode = 1;
      return;
    }
    console.log(
      `release-instrumentation self-test ok: temp bundle detection works ` +
        `(${result.findings.length} findings in ${leakingFiles.size} of ${result.files.length} files, ` +
        `${result.bytes} bytes)`,
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

if (process.argv.includes("--self-test")) runSelfTest();
else runDistCheck();
