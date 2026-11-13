#!/usr/bin/env node
/**
 * Consistency gate for the updater endpoint configuration (`pnpm check` runs this).
 *
 * The runtime endpoint list is compiled into `src-tauri/tauri.conf.json`; the release pipeline and
 * the CI verifier derive mirror manifests from `scripts/release/update-mirrors.mjs`. This check
 * keeps the two in sync and enforces the security posture:
 *
 * - endpoint order is official GitHub first, then the three mainland-accessible mirror manifests;
 * - every endpoint is HTTPS (the updater plugin refuses insecure transport in release builds);
 * - `requireSignedVersion` is on, so a mirror cannot pair a newer announced version with the
 *   signature of an older-but-genuine artifact (downgrade attack);
 * - the Windows install mode stays `passive` (the installer restarts the app).
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { MIRRORS, OFFICIAL_MANIFEST_URL, selfTest, updateEndpointUrls } from "./release/update-mirrors.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const confPath = join(repoRoot, "src-tauri", "tauri.conf.json");

function fail(message) {
  throw new Error(message);
}

try {
  selfTest();
  console.log("PASS update-mirrors: manifest rewrite logic self-test");

  const conf = JSON.parse(readFileSync(confPath, "utf8"));
  const updater = conf?.plugins?.updater;
  if (!updater) fail(`tauri.conf.json has no plugins.updater block (${confPath})`);

  const expectedEndpoints = updateEndpointUrls();
  assert.deepEqual(updater.endpoints, expectedEndpoints, "endpoints must match update-mirrors.mjs");
  console.log(
    `PASS update-endpoints: ${updater.endpoints.length} endpoints ` +
      `(official + ${MIRRORS.length} mainland mirrors: ${MIRRORS.map((m) => m.name).join(", ")})`,
  );

  assert.equal(updater.endpoints[0], OFFICIAL_MANIFEST_URL, "official endpoint must be first");
  for (const url of updater.endpoints) {
    assert.match(url, /^https:\/\//, `insecure endpoint ${url}`);
    assert.ok(url.endsWith(".json"), `endpoint is not a JSON manifest: ${url}`);
  }
  console.log("PASS update-endpoints: official first, all endpoints HTTPS");

  assert.equal(updater.requireSignedVersion, true, "requireSignedVersion must be true (downgrade defence)");
  console.log("PASS update-endpoints: requireSignedVersion is on");

  assert.equal(updater.windows?.installMode, "passive", "windows installMode must stay passive");
  console.log("PASS update-endpoints: windows installMode=passive");

  if (!/^dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6/.test(updater.pubkey ?? "")) {
    fail("pubkey is not a minisign public key");
  }
  console.log("PASS update-endpoints: minisign pubkey present");

  console.log("update endpoint configuration ok");
} catch (error) {
  console.error(`FAIL update-endpoints: ${error.message}`);
  process.exitCode = 1;
}
