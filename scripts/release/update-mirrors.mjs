#!/usr/bin/env node
/**
 * China-accessible updater mirrors: one source of truth for the release builder, the runtime
 * endpoint configuration check and the CI manifest verifier.
 *
 * The official manifest (`latest.json`) is served by GitHub, whose release download host is not
 * reachable from mainland China on many home/office networks. The release pipeline therefore also
 * publishes one manifest per mirror, where every platform `url` is rewritten through that mirror's
 * prefix (the updater signature stays byte-identical, and the signature's trusted comment carries
 * the version, so a mirror cannot pair an old artifact with a newer version).
 *
 * All three mirrors were probed from a mainland network without a VPN: the manifest and the 7.3 MB
 * setup both download (`206`, PE `MZ` header). See docs/product/2026-11-13-v1.4.1-cn-update.md.
 *
 * Usage:
 *   node scripts/release/update-mirrors.mjs --self-test
 */

import assert from "node:assert/strict";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** GitHub repository that owns the releases. */
export const GITHUB_REPO = "devin2255/hangline";

/** Base URL of the GitHub release asset download host (blocked from mainland China on some ISPs). */
export const RELEASE_ASSET_BASE = `https://github.com/${GITHUB_REPO}/releases/download`;

/** The official updater manifest (Tauri `latest.json`). Tried first for trust. */
export const OFFICIAL_MANIFEST_URL = `https://github.com/${GITHUB_REPO}/releases/latest/download/latest.json`;

/**
 * Public GitHub release proxies, in the fallback order used by the app. Each one serves both the
 * mirrored manifest and the setup download without redirecting to a blocked host.
 */
export const MIRRORS = [
  { name: "ghfast", prefix: "https://ghfast.top/" },
  { name: "gh-proxy", prefix: "https://gh-proxy.com/" },
  { name: "ghproxy-net", prefix: "https://ghproxy.net/" },
];

/** Release asset file name of one mirror manifest. */
export function mirrorAssetName(name) {
  return `latest-cn-${name}.json`;
}

/** Updater endpoint URL that fetches one mirror manifest through its proxy. */
export function mirrorEndpointUrl(mirror) {
  const fileName = mirrorAssetName(mirror.name);
  return `${mirror.prefix}${OFFICIAL_MANIFEST_URL.replace(/latest\.json$/, fileName)}`;
}

/** Every updater endpoint in the exact order the app tries them: official first, then mirrors. */
export function updateEndpointUrls() {
  return [OFFICIAL_MANIFEST_URL, ...MIRRORS.map((mirror) => mirrorEndpointUrl(mirror))];
}

/**
 * Builds one mirrored manifest per [`MIRRORS`] entry from the official `latest` object.
 *
 * The mirror manifests are byte-identical to the official one except for the platform `url`
 * fields, which gain the mirror prefix. Version, notes, pub_date and signatures are preserved
 * exactly; those invariants are what `verify-cn-manifests.mjs` re-checks in CI.
 */
export function buildMirrorManifests(latest) {
  if (!latest || typeof latest !== "object") {
    throw new Error("buildMirrorManifests: latest manifest must be an object");
  }
  if (typeof latest.version !== "string" || !/^\d+\.\d+\.\d+$/.test(latest.version)) {
    throw new Error(`buildMirrorManifests: invalid version ${JSON.stringify(latest.version)}`);
  }
  const platforms = latest.platforms;
  if (!platforms || typeof platforms !== "object" || Array.isArray(platforms) || !Object.keys(platforms).length) {
    throw new Error("buildMirrorManifests: latest manifest has no platforms");
  }

  return MIRRORS.map((mirror) => {
    const manifest = JSON.parse(JSON.stringify(latest));
    for (const [target, entry] of Object.entries(manifest.platforms)) {
      if (!entry || typeof entry.url !== "string") {
        throw new Error(`buildMirrorManifests: platform ${target} has no url`);
      }
      if (!entry.url.startsWith(`${RELEASE_ASSET_BASE}/`)) {
        throw new Error(
          `buildMirrorManifests: platform ${target} url is not a GitHub release asset: ${entry.url}`,
        );
      }
      entry.url = `${mirror.prefix}${entry.url}`;
    }
    return { name: mirror.name, fileName: mirrorAssetName(mirror.name), manifest };
  });
}

/**
 * Self-test for the pure manifest logic (run from `pnpm check` via `check-update-endpoints.mjs`).
 * Throws on the first failed assertion.
 */
export function selfTest() {
  const latest = {
    version: "9.8.7",
    notes: "test notes",
    pub_date: "2026-11-13T00:00:00Z",
    platforms: {
      "windows-x86_64-nsis": {
        signature: "c2lnbmF0dXJl",
        url: `${RELEASE_ASSET_BASE}/v9.8.7/Hangline_9.8.7_x64-setup.exe`,
      },
      "windows-x86_64": {
        signature: "c2lnbmF0dXJl",
        url: `${RELEASE_ASSET_BASE}/v9.8.7/Hangline_9.8.7_x64-setup.exe`,
      },
    },
  };

  assert.equal(MIRRORS.length, 3, "three mirrors are configured");
  const endpoints = updateEndpointUrls();
  assert.equal(endpoints.length, 4, "official + three mirror endpoints");
  assert.equal(endpoints[0], OFFICIAL_MANIFEST_URL, "official endpoint is tried first");
  for (const url of endpoints) {
    assert.match(url, /^https:\/\//, `endpoint must be https: ${url}`);
  }
  assert.equal(new Set(endpoints).size, endpoints.length, "endpoints must be unique");

  const manifests = buildMirrorManifests(latest);
  assert.equal(manifests.length, MIRRORS.length, "one manifest per mirror");
  for (const [index, mirror] of MIRRORS.entries()) {
    const built = manifests[index];
    assert.equal(built.name, mirror.name);
    assert.equal(built.fileName, `latest-cn-${mirror.name}.json`);
    assert.deepEqual(Object.keys(built.manifest.platforms), Object.keys(latest.platforms), "platforms preserved");
    assert.equal(built.manifest.version, latest.version, "version preserved");
    assert.equal(built.manifest.notes, latest.notes, "notes preserved");
    assert.equal(built.manifest.pub_date, latest.pub_date, "pub_date preserved");
    for (const target of Object.keys(latest.platforms)) {
      const official = latest.platforms[target];
      const mirrored = built.manifest.platforms[target];
      assert.equal(mirrored.signature, official.signature, "signature preserved");
      assert.equal(mirrored.url, `${mirror.prefix}${official.url}`, "url rewritten through the mirror");
    }
    assert.ok(
      mirrorEndpointUrl(mirror).includes(built.fileName),
      "endpoint URL points at the mirror manifest asset",
    );
    // The input must not be mutated.
    assert.ok(
      latest.platforms["windows-x86_64"].url.startsWith(`${RELEASE_ASSET_BASE}/`),
      "official manifest is not mutated",
    );
  }

  assert.throws(
    () =>
      buildMirrorManifests({
        version: "1.0.0",
        platforms: { "windows-x86_64": { signature: "x", url: "https://example.com/setup.exe" } },
      }),
    /not a GitHub release asset/,
    "non-GitHub artifact urls are rejected",
  );
  assert.throws(() => buildMirrorManifests({ version: "1.0.0" }), /no platforms/);
  assert.throws(() => buildMirrorManifests({ version: "latest", platforms: {} }), /invalid version/);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  // Direct invocation (the module is also imported by the build/check scripts).
  if (process.argv.includes("--self-test")) {
    selfTest();
    console.log("update-mirrors self-test ok: 3 mirrors, official-first endpoint order, urls rewritten, signatures preserved");
  } else {
    console.log(updateEndpointUrls().join("\n"));
  }
}
