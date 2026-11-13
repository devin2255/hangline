#!/usr/bin/env node
/* global Buffer */
/**
 * CI verifier for the mainland mirror manifests (run by the release workflow after
 * `build-release.mjs --ci --sign`, and usable locally on a signed build).
 *
 * It re-establishes the invariants that make a mirror safe to put in front of the updater:
 *
 * 1. `latest.json` is signed with a real (non-placeholder) updater signature;
 * 2. that signature's trusted comment carries `version:<version>` — the value the app's
 *    `requireSignedVersion` check compares against the announced version, so a mirror cannot pair
 *    an older genuine artifact with a newer announced version;
 * 3. one `latest-cn-<mirror>.json` exists per configured mirror, with identical version/notes/
 *    pub_date/platform keys/signatures, and every platform `url` is exactly the official URL with
 *    that mirror's prefix;
 * 4. no stale/extra `latest-cn-*.json` files are lying around (they would be uploaded as assets and
 *    could shadow the intended URLs on the next release).
 *
 * Usage: node scripts/release/verify-cn-manifests.mjs [--dir release]
 */

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { MIRRORS, RELEASE_ASSET_BASE } from "./update-mirrors.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const dirIndex = process.argv.indexOf("--dir");
const releaseDir = dirIndex >= 0 ? resolve(process.argv[dirIndex + 1]) : join(repoRoot, "release");

const failures = [];
function check(name, condition, detail) {
  if (condition) {
    console.log(`PASS ${name}${detail ? `: ${detail}` : ""}`);
  } else {
    failures.push(`${name}${detail ? `: ${detail}` : ""}`);
    console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
  }
}

function decodeSignature(signature) {
  // The updater signature is a base64 blob whose plaintext is a minisign signature; its trusted
  // comment (covered by the signature) carries `timestamp`, `file` and `version`.
  try {
    return Buffer.from(signature, "base64").toString("utf8");
  } catch {
    return "";
  }
}

console.log(`CN mirror manifest verification (${releaseDir})`);

const latestPath = join(releaseDir, "latest.json");
if (!existsSync(latestPath)) {
  console.error(`FAIL latest.json: missing at ${latestPath}`);
  process.exit(1);
}
const latest = JSON.parse(readFileSync(latestPath, "utf8"));

check("latest.json version", /^\d+\.\d+\.\d+$/.test(latest.version ?? ""), latest.version);
const platformKeys = Object.keys(latest.platforms ?? {});
check("latest.json platforms", platformKeys.length > 0, platformKeys.join(", "));

const signatures = new Set();
for (const target of platformKeys) {
  const entry = latest.platforms[target];
  const valid =
    entry &&
    typeof entry.signature === "string" &&
    entry.signature !== "UNSIGNED-PLACEHOLDER" &&
    entry.signature.length >= 100;
  check(`latest.json signature (${target})`, valid, valid ? `${entry.signature.length} chars` : "placeholder/too short");
  if (valid) signatures.add(entry.signature);
  const urlOk = typeof entry.url === "string" && entry.url.startsWith(`${RELEASE_ASSET_BASE}/`);
  check(`latest.json url is a GitHub release asset (${target})`, urlOk, entry.url);
}
check("latest.json signatures agree across platforms", signatures.size === 1, `${signatures.size} distinct`);

for (const signature of signatures) {
  const trusted = decodeSignature(signature);
  check(
    "signature trusted comment carries the version",
    trusted.includes(`version:${latest.version}`),
    trusted.includes(`version:${latest.version}`) ? undefined : "requireSignedVersion would reject the update",
  );
  check("signature trusted comment names the setup file", /file:Hangline_[^\s]+_x64-setup\.exe/.test(trusted));
}

const expectedFiles = new Set(MIRRORS.map((mirror) => `latest-cn-${mirror.name}.json`));
for (const mirror of MIRRORS) {
  const fileName = `latest-cn-${mirror.name}.json`;
  const path = join(releaseDir, fileName);
  if (!existsSync(path)) {
    check(`mirror manifest ${fileName}`, false, "missing");
    continue;
  }
  const manifest = JSON.parse(readFileSync(path, "utf8"));
  check(`mirror ${mirror.name}: version`, manifest.version === latest.version, manifest.version);
  check(`mirror ${mirror.name}: notes`, manifest.notes === latest.notes);
  check(`mirror ${mirror.name}: pub_date`, manifest.pub_date === latest.pub_date);
  check(
    `mirror ${mirror.name}: platform keys`,
    JSON.stringify(Object.keys(manifest.platforms ?? {})) === JSON.stringify(platformKeys),
  );
  for (const target of platformKeys) {
    const mirrored = manifest.platforms?.[target];
    const official = latest.platforms[target];
    if (!mirrored) {
      check(`mirror ${mirror.name}: platform ${target}`, false, "missing");
      continue;
    }
    check(
      `mirror ${mirror.name}: signature (${target})`,
      mirrored.signature === official.signature,
      "must stay byte-identical",
    );
    check(
      `mirror ${mirror.name}: url (${target})`,
      mirrored.url === `${mirror.prefix}${official.url}`,
      mirrored.url,
    );
  }
}

const actualFiles = readdirSync(releaseDir).filter((file) => /^latest-cn-.*\.json$/.test(file));
for (const file of actualFiles) {
  check(`no unexpected mirror manifest ${file}`, expectedFiles.has(file));
}

if (failures.length) {
  console.error(`\nCN mirror manifest verification FAILED (${failures.length} failure(s))`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log(
  `\nCN mirror manifests ok: ${MIRRORS.length} manifests, ` +
    `signatures byte-identical, every url rewritten through its mirror prefix, signatures version-bound`,
);
