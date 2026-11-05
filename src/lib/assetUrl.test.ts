/**
 * Tests for the versioned asset URL helper (P3-3, plan Task P3-3).
 *
 * WebView2 caches asset-protocol bytes by URL; a markup save rewrites the original and its 432 px
 * preview under the same paths, so the panel must append `?v=<Item.updatedAt>` to force a reload.
 * The helper must not disturb the conversion/encoding done by `convertFileSrc` (spaces and
 * backslashes arrive percent-encoded).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assetUrl } from "./assetUrl";

/** The conversion WebView2 receives from Tauri: `http://asset.localhost/<encoded path>`. */
const convertStub = (path: string) => `http://asset.localhost/${encodeURIComponent(path)}`;

beforeEach(() => {
  // `convertFileSrc` calls `window.__TAURI_INTERNALS__.convertFileSrc`; provide just enough of the
  // injected object for the Node-based test environment.
  vi.stubGlobal("window", {
    __TAURI_INTERNALS__: { convertFileSrc: convertStub },
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("assetUrl", () => {
  it("appends ?v= when the item was rewritten", () => {
    const url = assetUrl("C:\\Users\\Example\\AppData\\Local\\Hangline\\Screenshots\\shot.png", 1);
    expect(url.startsWith("http://asset.localhost/")).toBe(true);
    expect(url.endsWith("?v=1")).toBe(true);
  });

  it("leaves the converted URL untouched for 0, missing, negative or non-finite versions", () => {
    const path = "C:\\shots\\a.png";
    const base = convertStub(path);
    for (const version of [0, undefined, null, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(assetUrl(path, version)).toBe(base);
    }
  });

  it("keeps the percent-encoding of spaces and backslashes", () => {
    const path = "C:\\Users\\Example\\My Shots\\shot 1.png";
    const url = assetUrl(path, 1_700_000_000_900);
    expect(url).not.toContain(" ");
    expect(url).toContain("%20");
    expect(url).toContain("%5C");
    expect(url).toContain("?v=1700000000900");
    const parsed = new URL(url);
    expect(parsed.searchParams.get("v")).toBe("1700000000900");
    // The path portion must be byte-identical to what convertFileSrc produced.
    expect(url.slice(0, url.indexOf("?"))).toBe(convertStub(path));
  });

  it("uses & when the converted URL already carries a query string", () => {
    const convert = (path: string) => `http://asset.localhost/${encodeURIComponent(path)}?raw=1`;
    expect(assetUrl("C:\\a.png", 7, convert)).toBe("http://asset.localhost/C%3A%5Ca.png?raw=1&v=7");
    expect(assetUrl("C:\\a.png", 0, convert)).toBe("http://asset.localhost/C%3A%5Ca.png?raw=1");
  });

  it("truncates a fractional version to the integer the backend produced", () => {
    expect(assetUrl("C:\\a.png", 1234.9)).toMatch(/\?v=1234$/);
  });
});
