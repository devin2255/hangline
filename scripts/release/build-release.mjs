#!/usr/bin/env node
/**
 * Hangline release builder (P5-1).
 *
 * Pipeline:
 *   1. verify package.json / tauri.conf.json version consistency (scripts/version.mjs)
 *   2. pnpm check (lint, typecheck, unit tests, i18n, version) unless --skip-check
 *   3. pnpm tauri build --no-bundle (unsigned release exe) unless --skip-build
 *   4. stage Hangline.exe + Hangline.ico under src-tauri/target/p5-staging
 *   5. scripts/release/fetch-tools.ps1 (official NSIS 3.13 + WebView2 bootstrapper, cached)
 *   6. makensis scripts/release/installer.nsi -> release/Hangline_<version>_x64-setup.exe
 *   7. portable ZIP -> release/Hangline_<version>_x64-portable.zip
 *   8. SHA-256, Authenticode status, hard <=15 MB setup size gate
 *   9. release/latest.json (unsigned placeholder by default; real updater signature with --sign)
 *
 * Usage:
 *   node scripts/release/build-release.mjs [--sign] [--ci] [--skip-check] [--skip-build] [--skip-tools]
 *
 * No new dependencies: Node built-ins, PowerShell (zip + Authenticode) and the cached NSIS
 * toolchain. The release/ directory and src-tauri/target/ are git-ignored; artifacts are local.
 */

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..", "..");

/** Git commit for build provenance (null outside a work tree, e.g. a zip export). */
const GIT_COMMIT = (() => {
  try {
    const result = spawnSync("git", ["rev-parse", "HEAD"], { cwd: repoRoot, encoding: "utf8", windowsHide: true });
    return result.status === 0 ? (result.stdout ?? "").trim() || null : null;
  } catch {
    return null;
  }
})();

const RELEASE_DIR = join(repoRoot, "release");
const NSI_PATH = join(scriptDir, "installer.nsi");
const FETCH_TOOLS_PS1 = join(scriptDir, "fetch-tools.ps1");
const TOOLS_DIR = join(repoRoot, "src-tauri", "target", "p5-tools");
const STAGING_DIR = join(repoRoot, "src-tauri", "target", "p5-staging");
const PORTABLE_DIR = join(repoRoot, "src-tauri", "target", "p5-portable");
const APP_EXE_SOURCE = join(repoRoot, "src-tauri", "target", "release", "hangline.exe");
const APP_ICON_SOURCE = join(repoRoot, "src-tauri", "icons", "icon.ico");

/** Spec §6.1 / P5 DoD: the NSIS setup must stay at or below 15 MB (decimal). */
const SETUP_LIMIT_BYTES = 15_000_000;
/**
 * The portable ZIP is not part of the 15 MB installer budget; it carries the ~20 MB exe and is
 * expected around 7-10 MB with deflate. 25 MB only catches a broken/oversized build.
 */
const PORTABLE_ZIP_LIMIT_BYTES = 25_000_000;

const args = new Set(process.argv.slice(2));
if (args.has("--help") || args.has("-h")) {
  console.log(
    [
      "Usage: node scripts/release/build-release.mjs [options]",
      "",
      "  --sign         Sign the setup with tauri signer and emit a real latest.json signature",
      "  --ci           CI mode (unsigned unless --sign + TAURI_SIGNING_PRIVATE_KEY* are provided)",
      "  --skip-check   Skip pnpm check (fast local iteration; always runs scripts/version.mjs)",
      "  --skip-build   Reuse src-tauri/target/release/hangline.exe",
      "  --skip-tools   Reuse src-tauri/target/p5-tools/tools.json without fetching",
      "  --help, -h     Show this help",
    ].join("\n"),
  );
  process.exit(0);
}
const CI = args.has("--ci");
const SIGN = args.has("--sign");
const SKIP_CHECK = args.has("--skip-check");
const SKIP_BUILD = args.has("--skip-build");
const SKIP_TOOLS = args.has("--skip-tools");

const startedAt = Date.now();

function logStep(step, message) {
  console.log(`\n=== ${step}: ${message} ===`);
}

function elapsed(from) {
  return `${((Date.now() - from) / 1000).toFixed(1)}s`;
}

function fail(message) {
  throw new Error(message);
}

function readJson(file, label) {
  let text = readFileSync(file, "utf8");
  if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
  try {
    return JSON.parse(text);
  } catch (error) {
    fail(`cannot parse ${label} (${file}): ${error.message}`);
  }
}

function run(command, commandArgs, options = {}) {
  let pretty = [command, ...commandArgs].join(" ");
  for (const secret of options.redact ?? []) {
    if (secret) pretty = pretty.split(secret).join("<redacted>");
  }
  console.log(`\n$ ${pretty}`);
  const result = spawnSync(command, commandArgs, {
    cwd: options.cwd ?? repoRoot,
    stdio: options.capture ? "pipe" : "inherit",
    encoding: "utf8",
    env: options.env ?? process.env,
    shell: options.shell ?? false,
    windowsHide: true,
  });
  if (result.error) {
    fail(`failed to start "${pretty}": ${result.error.message}`);
  }
  const stdout = result.stdout ?? "";
  const stderr = result.stderr ?? "";
  if (options.capture) {
    if (stdout) process.stdout.write(stdout);
    if (stderr) process.stderr.write(stderr);
  }
  if (result.status !== 0 && !options.allowFailure) {
    fail(`command failed (exit ${result.status}): ${pretty}`);
  }
  return { status: result.status, stdout, stderr };
}

function runPnpm(pnpmArgs, options = {}) {
  return run("pnpm", pnpmArgs, { ...options, shell: process.platform === "win32" });
}

function findPowerShell() {
  for (const candidate of ["pwsh", "powershell"]) {
    const probe = spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], {
      stdio: "ignore",
      windowsHide: true,
    });
    if (!probe.error && probe.status === 0) return candidate;
  }
  fail("neither pwsh nor powershell was found on PATH");
}

function runPowerShellFile(ps, file, extraArgs = []) {
  return run(ps, ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", file, ...extraArgs]);
}

function runPowerShellCommand(ps, command) {
  return run(ps, ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command], {
    capture: true,
  });
}

function psLiteral(value) {
  // PowerShell single-quoted literal: '' escapes a single quote.
  return `'${String(value).replace(/'/g, "''")}'`;
}

function sha256File(file) {
  const hash = createHash("sha256");
  hash.update(readFileSync(file));
  return hash.digest("hex").toUpperCase();
}

function formatBytes(bytes) {
  return bytes.toLocaleString("en-US");
}

function formatMb(bytes) {
  return (bytes / 1_000_000).toFixed(2);
}

function resetDirectory(directory) {
  const targetRoot = resolve(repoRoot, "src-tauri", "target");
  const resolved = resolve(directory);
  if (!resolved.startsWith(targetRoot + sep)) {
    fail(`refusing to reset a directory outside src-tauri/target: ${resolved}`);
  }
  if (existsSync(resolved)) rmSync(resolved, { recursive: true, force: true });
  mkdirSync(resolved, { recursive: true });
}

function portableReadme(version) {
  return `Hangline ${version} - Portable (Windows x64)
============================================

English
-------
Hangline is a screenshot tool that keeps recent screenshots on a clothesline at the top of
your screen.

How to run
  1. Extract this ZIP to any folder.
  2. Run Hangline.exe.
  No installer is needed. The "portable" marker file identifies this build.

Where is my data?
  Hangline stores settings, state, thumbnails and screenshots in
  %LOCALAPPDATA%\\Hangline (for example C:\\Users\\<you>\\AppData\\Local\\Hangline).
  The V1 portable build uses the same location as the installed build, so deleting that
  folder also deletes your Hangline data. Back up screenshots you want to keep first.
  To remove everything: exit Hangline, delete this folder, then delete
  %LOCALAPPDATA%\\Hangline.

Unsigned build
  This build is not code-signed. Windows SmartScreen may show "Windows protected your PC";
  choose "More info" -> "Run anyway" only if you obtained the ZIP from the official release
  page and its SHA-256 matches the published checksum.

Network
  Hangline's only network request is the signed update check (daily when enabled, or manual
  from the tray). This portable build does not include an uninstaller.

中文
----
Hangline 是一款截图工具，会把最近的截图挂在屏幕顶部的一条“晾衣绳”上。

运行方法
  1. 将 ZIP 解压到任意文件夹。
  2. 运行 Hangline.exe。
  无需安装。“portable” 标记文件用于标识免安装版本。

数据位置
  Hangline 将设置、状态、缩略图和截图保存在 %LOCALAPPDATA%\\Hangline
  （例如 C:\\Users\\<用户名>\\AppData\\Local\\Hangline）。V1 的免安装版与安装版使用同一位置，
  因此删除该文件夹会同时删除 Hangline 数据，请先备份需要保留的截图。
  彻底删除：退出 Hangline，删除本文件夹，再删除 %LOCALAPPDATA%\\Hangline。

未签名说明
  本版本未进行代码签名，Windows SmartScreen 可能提示“Windows 已保护你的电脑”。
  仅当 ZIP 来自官方发布页且 SHA-256 与公布值一致时，才选择“更多信息”->“仍要运行”。

网络
  Hangline 唯一的网络请求是带签名的更新检查（开启时每日一次，或托盘手动检查）。免安装版不包含卸载程序。
`;
}

function authenticodeStatus(ps, file) {
  // Authenticode only applies to PE files: Get-AuthenticodeSignature reports UnknownError for a
  // ZIP, so the portable archive is labelled N/A instead of pretending it is unsigned PE code.
  if (!file.toLowerCase().endsWith(".exe")) {
    return { status: "N/A (not a PE file)", message: "Authenticode applies to PE files" };
  }
  const command = [
    "try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch {}",
    "try {",
    `  $sig = Get-AuthenticodeSignature -LiteralPath ${psLiteral(file)} -ErrorAction Stop;`,
    "  [pscustomobject]@{ status = [string]$sig.Status; message = [string]$sig.StatusMessage }",
    "    | ConvertTo-Json -Compress",
    "} catch {",
    "  [pscustomobject]@{ status = 'CHECK-ERROR'; message = $_.Exception.Message }",
    "    | ConvertTo-Json -Compress",
    "}",
  ].join("\n");
  const result = run(ps, ["-NoProfile", "-NonInteractive", "-Command", command], {
    capture: true,
    allowFailure: true,
  });
  const line = (result.stdout || "")
    .split(/\r?\n/)
    .map((value) => value.trim())
    .filter(Boolean)
    .pop();
  if (!line) return { status: "UNKNOWN", message: result.stderr.trim() };
  try {
    return JSON.parse(line);
  } catch {
    return { status: "UNKNOWN", message: line };
  }
}

function printArtifactTable(artifacts) {
  const headers = ["Artifact", "Bytes", "MB", "SHA-256", "Authenticode"];
  const rows = artifacts.map((artifact) => [
    artifact.name,
    formatBytes(artifact.bytes),
    formatMb(artifact.bytes),
    artifact.sha256,
    artifact.authenticode.status,
  ]);
  const widths = headers.map((header, index) =>
    Math.max(header.length, ...rows.map((row) => row[index].length)),
  );
  const line = (columns) => columns.map((column, index) => column.padEnd(widths[index])).join("  ");
  console.log("");
  console.log("Artifact table");
  console.log(line(headers));
  console.log(widths.map((width) => "-".repeat(width)).join("  "));
  for (const row of rows) console.log(line(row));
}

function main() {
  console.log(`Hangline release build${CI ? " (CI mode)" : ""}`);
  console.log(`Repository: ${repoRoot}`);

  // ---------------------------------------------------------------- signing

/** Private updater keys live outside the repo tree (git-ignored) or in CI secrets. */
const SIGNING_DIR = join(repoRoot, ".superpowers", "release-keys");

function readSignatureFile(setupPath) {
  const sigPath = `${setupPath}.sig`;
  if (existsSync(sigPath)) return readFileSync(sigPath, "utf8").trim();
  return "";
}

function parseSignatureFromStdout(stdout) {
  const lines = stdout.split(/\r?\n/);
  const marker = lines.findIndex((line) => line.trim() === "Public signature:");
  if (marker >= 0) {
    for (const line of lines.slice(marker + 1)) {
      const candidate = line.trim();
      if (candidate) return candidate;
    }
  }
  return "";
}

/**
 * Signs the setup with the Tauri updater key.
 *
 * Sources, in order: TAURI_SIGNING_PRIVATE_KEY (CI secret, key string), 
 * TAURI_SIGNING_PRIVATE_KEY_PATH, then the local .superpowers/release-keys files. The password
 * is never printed (run() redacts it) and never written to the repo.
 */
function signSetup(setupPath, version) {
  // Secrets are passed ONLY through the child environment: pnpm echoes its own argv to the
  // console, so `-p <password>` / `-k <key>` on the command line would leak into logs.
  const childEnv = { ...process.env };
  let source;
  const envKey = (process.env.TAURI_SIGNING_PRIVATE_KEY ?? "").trim();
  const envKeyPath = (process.env.TAURI_SIGNING_PRIVATE_KEY_PATH ?? "").trim();
  const envPassword = process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "";
  if (envKey) {
    childEnv.TAURI_SIGNING_PRIVATE_KEY = envKey;
    childEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = envPassword;
    delete childEnv.TAURI_SIGNING_PRIVATE_KEY_PATH;
    source = "env:TAURI_SIGNING_PRIVATE_KEY";
  } else if (envKeyPath) {
    childEnv.TAURI_SIGNING_PRIVATE_KEY_PATH = envKeyPath;
    childEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = envPassword;
    delete childEnv.TAURI_SIGNING_PRIVATE_KEY;
    source = "env:TAURI_SIGNING_PRIVATE_KEY_PATH";
  } else {
    const keyPath = join(SIGNING_DIR, "hangline.key");
    const passwordPath = join(SIGNING_DIR, "password.txt");
    if (!existsSync(keyPath)) {
      fail(
        `--sign: no updater signing key. Generate one with "pnpm tauri signer generate -w ${keyPath}" ` +
          "or set TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PATH for CI.",
      );
    }
    const password = existsSync(passwordPath) ? readFileSync(passwordPath, "utf8").trim() : "";
    childEnv.TAURI_SIGNING_PRIVATE_KEY_PATH = keyPath;
    childEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = password;
    delete childEnv.TAURI_SIGNING_PRIVATE_KEY;
    source = "local:.superpowers/release-keys";
  }
  const commandArgs = ["tauri", "signer", "sign", "--app-version", version, setupPath];
  const result = runPnpm(commandArgs, { capture: true, env: childEnv });
  const signature = readSignatureFile(setupPath) || parseSignatureFromStdout(result.stdout ?? "");
  if (!signature || signature.length < 100 || !signature.startsWith("dW50cnVzdGVk")) {
    fail("signer did not produce a valid updater signature (expected a long base64 value starting with dW50cnVzdGVk)");
  }
  console.log(`updater signature: ${signature.slice(0, 48)}… (${signature.length} chars, source ${source})`);
  return { signature, source };
}

// ---------------------------------------------------------------- versions
  logStep("1/9", "Version consistency");
  const pkg = readJson(join(repoRoot, "package.json"), "package.json");
  const tauriConf = readJson(join(repoRoot, "src-tauri", "tauri.conf.json"), "tauri.conf.json");
  const version = pkg.version;
  if (tauriConf.version !== version) {
    fail(`version mismatch: package.json=${version} tauri.conf.json=${tauriConf.version}`);
  }
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    fail(
      `version "${version}" is not a plain numeric x.y.z version; ` +
        "the NSIS VIProductVersion block requires one (prerelease suffixes are not supported)",
    );
  }
  run(process.execPath, [join(repoRoot, "scripts", "version.mjs")]);
  console.log(`version: ${version}`);

  // -------------------------------------------------------------- pnpm check
  if (SKIP_CHECK) {
    console.log("SKIP pnpm check (--skip-check)");
  } else {
    logStep("2/9", "pnpm check (lint, typecheck, unit tests, i18n, version)");
    const checkStart = Date.now();
    runPnpm(["check"]);
    console.log(`pnpm check passed in ${elapsed(checkStart)}`);
  }

  // -------------------------------------------------------- release app exe
  if (SKIP_BUILD) {
    console.log("SKIP pnpm tauri build (--skip-build)");
  } else {
    logStep("3/9", "pnpm tauri build --no-bundle");
    const buildStart = Date.now();
    runPnpm(["tauri", "build", "--no-bundle"]);
    console.log(`tauri build finished in ${elapsed(buildStart)}`);
  }
  if (!existsSync(APP_EXE_SOURCE)) {
    fail(`release executable not found: ${APP_EXE_SOURCE}`);
  }
  const appExeBytes = statSync(APP_EXE_SOURCE).size;
  if (appExeBytes < 1_000_000) {
    fail(`release executable looks too small (${appExeBytes} bytes): ${APP_EXE_SOURCE}`);
  }
  if (!existsSync(APP_ICON_SOURCE)) {
    fail(`application icon not found: ${APP_ICON_SOURCE}`);
  }

  // ---------------------------------------------------------------- staging
  logStep("4/9", "Stage installer inputs");
  resetDirectory(STAGING_DIR);
  const stagedExe = join(STAGING_DIR, "Hangline.exe");
  const stagedIcon = join(STAGING_DIR, "Hangline.ico");
  copyFileSync(APP_EXE_SOURCE, stagedExe);
  copyFileSync(APP_ICON_SOURCE, stagedIcon);
  console.log(
    `staged ${relative(repoRoot, stagedExe)} (${formatBytes(statSync(stagedExe).size)} bytes)`,
  );
  console.log(
    `staged ${relative(repoRoot, stagedIcon)} (${formatBytes(statSync(stagedIcon).size)} bytes)`,
  );

  // ------------------------------------------------------------------ tools
  logStep("5/9", "Packaging tools (NSIS 3.13 + WebView2 bootstrapper)");
  const ps = findPowerShell();
  console.log(`PowerShell: ${ps}`);
  if (SKIP_TOOLS) {
    console.log("SKIP fetch-tools.ps1 (--skip-tools)");
  } else {
    runPowerShellFile(ps, FETCH_TOOLS_PS1);
  }
  const toolsJson = join(TOOLS_DIR, "tools.json");
  if (!existsSync(toolsJson)) {
    fail(`tools metadata missing: ${toolsJson} (run without --skip-tools)`);
  }
  const tools = readJson(toolsJson, "tools.json");
  for (const [label, file] of [
    ["makensis", tools.nsis?.exe],
    ["WebView2 bootstrapper", tools.webview2?.path],
  ]) {
    if (!file || !existsSync(file)) fail(`${label} not found: ${file}`);
  }
  console.log(`NSIS ${tools.nsis.version} (${tools.nsis.source}): ${tools.nsis.exe}`);
  console.log(`WebView2 bootstrapper: ${tools.webview2.path}`);

  // -------------------------------------------------------- NSIS installer
  logStep("6/9", "Compile NSIS setup");
  mkdirSync(RELEASE_DIR, { recursive: true });
  const setupName = `Hangline_${version}_x64-setup.exe`;
  const setupPath = join(RELEASE_DIR, setupName);
  const compile = run(
    tools.nsis.exe,
    [
      "/V4",
      "/INPUTCHARSET",
      "UTF8",
      "/OUTPUTCHARSET",
      "UTF8",
      `/DVERSION=${version}`,
      `/DSOURCE_DIR=${STAGING_DIR}`,
      `/DWEBVIEW2_SETUP=${tools.webview2.path}`,
      `/DOUT_FILE=${setupPath}`,
      NSI_PATH,
    ],
    { capture: true, cwd: scriptDir },
  );
  const compileLog = [
    `# makensis ${tools.nsis.version_raw ?? tools.nsis.version}`,
    `# NSIS executable: ${tools.nsis.exe}`,
    `# sha256: ${tools.nsis.sha256}`,
    `# command: makensis /V4 /INPUTCHARSET UTF8 /OUTPUTCHARSET UTF8 /DVERSION=${version} ` +
      `/DSOURCE_DIR=${STAGING_DIR} /DWEBVIEW2_SETUP=${tools.webview2.path} ` +
      `/DOUT_FILE=${setupPath} ${NSI_PATH}`,
    "",
    compile.stdout.trimEnd(),
    compile.stderr.trimEnd(),
    "",
  ].join("\n");
  writeFileSync(join(RELEASE_DIR, "nsis-compile.log"), compileLog, "utf8");
  if (!existsSync(setupPath)) fail(`NSIS did not produce ${setupPath}`);
  const setupBytes = statSync(setupPath).size;

  // ------------------------------------------------------------ portable zip
  logStep("7/9", "Build portable ZIP");
  resetDirectory(PORTABLE_DIR);
  copyFileSync(APP_EXE_SOURCE, join(PORTABLE_DIR, "Hangline.exe"));
  copyFileSync(APP_ICON_SOURCE, join(PORTABLE_DIR, "Hangline.ico"));
  writeFileSync(join(PORTABLE_DIR, "portable"), "", "utf8");
  writeFileSync(
    join(PORTABLE_DIR, "README-portable.txt"),
    portableReadme(version).replace(/\n/g, "\r\n"),
    "utf8",
  );
  const portableName = `Hangline_${version}_x64-portable.zip`;
  const portablePath = join(RELEASE_DIR, portableName);
  const zipCommand = [
    "& {",
    `  $src = ${psLiteral(PORTABLE_DIR)};`,
    `  $dst = ${psLiteral(portablePath)};`,
    "  Add-Type -AssemblyName System.IO.Compression.FileSystem;",
    "  if (Test-Path -LiteralPath $dst) { Remove-Item -LiteralPath $dst -Force };",
    "  [System.IO.Compression.ZipFile]::CreateFromDirectory(",
    "    $src, $dst, [System.IO.Compression.CompressionLevel]::Optimal, $false)",
    "}",
  ].join("\n");
  runPowerShellCommand(ps, zipCommand);
  if (!existsSync(portablePath)) fail(`portable ZIP was not created: ${portablePath}`);
  const portableBytes = statSync(portablePath).size;

  // ------------------------------------------------------------- checksums
  logStep("8/9", "SHA-256, Authenticode and size gate");
  const setupSha = sha256File(setupPath);
  const portableSha = sha256File(portablePath);
  const artifacts = [
    {
      name: setupName,
      path: setupPath,
      bytes: setupBytes,
      sha256: setupSha,
      authenticode: authenticodeStatus(ps, setupPath),
      limitBytes: SETUP_LIMIT_BYTES,
    },
    {
      name: portableName,
      path: portablePath,
      bytes: portableBytes,
      sha256: portableSha,
      authenticode: authenticodeStatus(ps, portablePath),
      limitBytes: PORTABLE_ZIP_LIMIT_BYTES,
    },
  ];
  printArtifactTable(artifacts);

  const setupOk = setupBytes <= SETUP_LIMIT_BYTES;
  const zipOk = portableBytes <= PORTABLE_ZIP_LIMIT_BYTES;
  console.log("");
  console.log(
    `size gate: setup ${formatBytes(setupBytes)} / ${formatBytes(SETUP_LIMIT_BYTES)} bytes ` +
      `(${setupOk ? "PASS" : "FAIL"}); portable ${formatBytes(portableBytes)} / ` +
      `${formatBytes(PORTABLE_ZIP_LIMIT_BYTES)} bytes (${zipOk ? "PASS" : "FAIL"})`,
  );
  if (!setupOk) {
    fail(
      `installer exceeds the 15,000,000-byte budget: ${formatBytes(setupBytes)} bytes ` +
        `(${formatMb(setupBytes)} MB)`,
    );
  }
  if (!zipOk) {
    fail(
      `portable ZIP is unreasonably large: ${formatBytes(portableBytes)} bytes ` +
        `(${formatMb(portableBytes)} MB)`,
    );
  }

  writeFileSync(
    join(RELEASE_DIR, "SHA256SUMS.txt"),
    `${setupSha}  ${setupName}\n${portableSha}  ${portableName}\n`,
    "utf8",
  );

  // --------------------------------------------------------------- manifest
  logStep("9/9", "Updater manifest + signing");
  const pubDate = new Date().toISOString();
  const setupFilePath = join(RELEASE_DIR, setupName);
  const signed = SIGN ? signSetup(setupFilePath, version) : null;
  const signature = signed?.signature ?? "UNSIGNED-PLACEHOLDER";
  const downloadUrl = `https://github.com/hangline-app/hangline/releases/download/v${version}/${setupName}`;
  const platformEntry = { signature, url: downloadUrl };
  // NSIS-installed apps look up "windows-x86_64-nsis" first, then "windows-x86_64"; write both
  // so the manifest works even if the installer detection falls back (same setup file).
  const platforms = SIGN
    ? { "windows-x86_64-nsis": platformEntry, "windows-x86_64": platformEntry }
    : { "windows-x86_64": platformEntry };
  const latest = {
    version,
    notes: SIGN
      ? `Hangline ${version}. See CHANGELOG.md and the release page for details.`
      : `Hangline ${version} packaging validation build (P5-1). Unsigned layout placeholder; ` +
        "build with --sign (scripts/release/update-signing.md) before publishing.",
    pub_date: pubDate,
    platforms,
  };
  writeFileSync(join(RELEASE_DIR, "latest.json"), JSON.stringify(latest, null, 2) + "\n", "utf8");
  if (SIGN) {
    if (signed) console.log(`latest.json: signed for ${Object.keys(platforms).join(", ")} -> ${downloadUrl}`);
  }

  const buildInfo = {
    version,
    built_at: pubDate,
    ci: CI,
    git_commit: GIT_COMMIT,
    host: { platform: process.platform, arch: process.arch, node: process.version },
    app_exe: {
      source: relative(repoRoot, APP_EXE_SOURCE),
      bytes: appExeBytes,
      sha256: sha256File(APP_EXE_SOURCE),
    },
    nsis: {
      version: tools.nsis.version,
      version_raw: tools.nsis.version_raw ?? null,
      source: tools.nsis.source,
      // Relative path only: build-info is uploaded as a CI artifact and must not leak a host path.
      exe: relative(repoRoot, tools.nsis.exe),
      sha256: tools.nsis.sha256,
    },
    webview2: {
      bytes: tools.webview2.bytes,
      sha256: tools.webview2.sha256,
    },
    artifacts: artifacts.map((artifact) => ({
      name: artifact.name,
      bytes: artifact.bytes,
      sha256: artifact.sha256,
      authenticode: artifact.authenticode.status,
    })),
    size_gate: {
      setup_limit_bytes: SETUP_LIMIT_BYTES,
      portable_zip_limit_bytes: PORTABLE_ZIP_LIMIT_BYTES,
      passed: true,
    },
    updater_manifest: {
      signature: SIGN ? "signed" : "UNSIGNED-PLACEHOLDER",
      layout_only: !SIGN,
      platform_keys: Object.keys(platforms),
      url: downloadUrl,
      signature_source: signed?.source ?? null,
    },
  };
  writeFileSync(
    join(RELEASE_DIR, "build-info.json"),
    JSON.stringify(buildInfo, null, 2) + "\n",
    "utf8",
  );

  console.log("");
  console.log("WARNING: the NSIS setup is not Authenticode-signed (expected NotSigned).");
  if (SIGN) {
    console.log("UPDATER: latest.json carries a real Tauri updater signature (keys never printed).");
    console.log("         Authenticode/SmartScreen still reports an unknown publisher; see README.");
  } else {
    console.log("WARNING: release/latest.json is a LAYOUT-ONLY placeholder (signature:");
    console.log('         "UNSIGNED-PLACEHOLDER"). Build with --sign (or CI secrets) before publishing;');
    console.log("         the signing contract is scripts/release/update-signing.md.");
  }
  console.log("");
  console.log("Release artifacts (git-ignored, local only):");
  for (const artifact of artifacts) {
    console.log(`  ${artifact.path}`);
  }
  console.log(`  ${join(RELEASE_DIR, "SHA256SUMS.txt")}`);
  console.log(`  ${join(RELEASE_DIR, "latest.json")}`);
  console.log(`  ${join(RELEASE_DIR, "build-info.json")}`);
  console.log(`  ${join(RELEASE_DIR, "nsis-compile.log")}`);
  console.log(`\nTotal build time: ${elapsed(startedAt)}`);
}

try {
  main();
} catch (error) {
  console.error(`\nRELEASE BUILD FAILED: ${error.message}`);
  process.exit(1);
}
