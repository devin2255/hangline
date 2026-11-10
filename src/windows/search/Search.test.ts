/**
 * Pure search rules for the Wave 2.2 + Wave 2.5 search window.
 *
 * The window itself is DOM-only (not rendered under the Node test environment), but every rule
 * that decides what a query matches, how the tag text becomes a tag list, which key does what, and
 * how OCR index hits merge with local results lives in exported helpers of `Search.tsx`. These
 * tests lock the four searchable fields (file name / note / tags / path), the newest-first order,
 * the tag normalization that mirrors Rust's `normalize_tags`, the ↑/↓/Enter keyboard mapping, and
 * the Wave 2.5 OCR merge/gate rules.
 */

import { describe, expect, it } from "vitest";
import type { Item, ItemKind, OcrHit } from "../../lib/types";
import {
  OCR_MIN_QUERY_CHARS,
  OCR_QUERY_DEBOUNCE_MS,
  filterItems,
  formatAdded,
  formatTags,
  matchesQuery,
  mergeSearchResults,
  parseTags,
  searchKeyAction,
  shouldQueryOcr,
  sourceLabel,
} from "./Search";

function makeItem(overrides: Partial<Item> & { id: string }): Item {
  return {
    id: overrides.id,
    path: overrides.path ?? `C:\\Users\\me\\Pictures\\Screenshots\\${overrides.id}.png`,
    kind: (overrides.kind ?? "capture") as ItemKind,
    hash: 1,
    width: 800,
    height: 600,
    createdAt: 1,
    addedAt: overrides.addedAt ?? 1,
    note: overrides.note ?? "",
    tags: overrides.tags ?? [],
    ...(overrides.thumbPath !== undefined ? { thumbPath: overrides.thumbPath } : {}),
    ...(overrides.updatedAt !== undefined ? { updatedAt: overrides.updatedAt } : {}),
  };
}

describe("matchesQuery", () => {
  it("matches the file name, note, tags and full path case-insensitively", () => {
    const item = makeItem({
      id: "a",
      path: "C:\\Users\\me\\Pictures\\Screenshots\\Login-Bug.PNG",
      note: "Login button overflows",
      tags: ["Q4", "release-check"],
    });
    expect(matchesQuery(item, "login-bug")).toBe(true); // file name (case-insensitive)
    expect(matchesQuery(item, "OVERFLOWS")).toBe(true); // note
    expect(matchesQuery(item, "release-CHECK")).toBe(true); // tag
    expect(matchesQuery(item, "pictures")).toBe(true); // path segment
    expect(matchesQuery(item, "payments")).toBe(false);
  });

  it("treats an empty/whitespace query as match-everything", () => {
    const item = makeItem({ id: "b", note: "anything" });
    expect(matchesQuery(item, "")).toBe(true);
    expect(matchesQuery(item, "   ")).toBe(true);
  });
});

describe("filterItems", () => {
  it("filters and sorts newest-first without mutating the source order", () => {
    const items = [
      makeItem({ id: "old", addedAt: 100, note: "alpha" }),
      makeItem({ id: "new", addedAt: 300, note: "alpha" }),
      makeItem({ id: "middle", addedAt: 200, note: "beta" }),
    ];
    const result = filterItems(items, "alpha");
    expect(result.map((item) => item.id)).toEqual(["new", "old"]);
    expect(items.map((item) => item.id)).toEqual(["old", "new", "middle"]);
  });

  it("returns the whole line newest-first for an empty query", () => {
    const items = [
      makeItem({ id: "a", addedAt: 10 }),
      makeItem({ id: "b", addedAt: 30 }),
      makeItem({ id: "c", addedAt: 20 }),
    ];
    expect(filterItems(items, "").map((item) => item.id)).toEqual(["b", "c", "a"]);
  });
});

describe("parseTags / formatTags", () => {
  it("trims, drops empty entries, de-duplicates case-insensitively and keeps order", () => {
    expect(parseTags(" q4 , release ,, Q4,  Review ")).toEqual(["q4", "release", "Review"]);
    expect(parseTags("")).toEqual([]);
    expect(parseTags(" , ,, ")).toEqual([]);
  });

  it("round-trips through the comma-separated editor format", () => {
    expect(parseTags(formatTags(["a", "b"]))).toEqual(["a", "b"]);
    expect(formatTags([])).toBe("");
  });
});

describe("searchKeyAction", () => {
  it("maps ↑/↓ to row movement and Enter to copy", () => {
    expect(searchKeyAction("ArrowDown")).toBe("next");
    expect(searchKeyAction("ArrowUp")).toBe("previous");
    expect(searchKeyAction("Enter")).toBe("copy");
  });

  it("does not intercept typing or Esc (the window handles Esc itself)", () => {
    expect(searchKeyAction("a")).toBeNull();
    expect(searchKeyAction("Escape")).toBeNull();
    expect(searchKeyAction("Tab")).toBeNull();
  });
});

describe("row metadata helpers", () => {
  it("uses the containing folder as the language-neutral source label", () => {
    expect(
      sourceLabel(makeItem({ id: "a", path: "C:\\Users\\me\\Pictures\\Screenshots\\a.png" })),
    ).toBe("Screenshots");
    expect(
      sourceLabel(
        makeItem({
          id: "b",
          kind: "recording",
          path: "C:\\Users\\me\\Videos\\Captures\\clip.mp4",
        }),
      ),
    ).toBe("Captures");
    // A path with no directory (defensive) has no segment to show; the path itself is used.
    expect(sourceLabel(makeItem({ id: "c", path: "file.png" }))).toContain("file.png");
  });

  it("formats valid timestamps and renders an em dash for malformed ones", () => {
    expect(formatAdded(0)).toBe("—");
    expect(formatAdded(Number.NaN)).toBe("—");
    expect(formatAdded(1_700_000_000_000)).not.toBe("—");
  });
});

describe("Wave 2.5 OCR merge", () => {
  const local = makeItem({ id: "local", addedAt: 100 });
  const ocrOld = makeItem({ id: "ocr-old", addedAt: 50 });
  const ocrNew = makeItem({ id: "ocr-new", addedAt: 300 });
  const allItems = [local, ocrOld, ocrNew];

  it("keeps local matches first and appends OCR-only hits newest-first", () => {
    const merged = mergeSearchResults(
      [local],
      [
        { id: "ocr-old", excerpt: "旧图片文字" },
        { id: "ocr-new", excerpt: "新图片文字" },
      ] satisfies OcrHit[],
      allItems,
    );
    expect(merged.map((result) => result.item.id)).toEqual(["local", "ocr-new", "ocr-old"]);
    expect(merged.map((result) => result.ocrOnly)).toEqual([false, true, true]);
    expect(merged[0].excerpt).toBeNull();
    expect(merged[1].excerpt).toBe("新图片文字");
  });

  it("never duplicates an item that both the local filter and OCR matched", () => {
    const merged = mergeSearchResults(
      [local],
      [{ id: "local", excerpt: "图片里的文字" }],
      allItems,
    );
    expect(merged).toHaveLength(1);
    expect(merged[0].ocrOnly).toBe(false);
    expect(merged[0].excerpt).toBeNull();
  });

  it("drops stale index rows (removed items) and malformed hits silently", () => {
    const merged = mergeSearchResults(
      [],
      [
        { id: "gone", excerpt: "deleted item" },
        { id: "local", excerpt: "" },
        { id: 42, excerpt: "not a string id" } as unknown as OcrHit,
        null as unknown as OcrHit,
      ],
      allItems,
    );
    expect(merged.map((result) => result.item.id)).toEqual(["local"]);
    expect(merged[0].ocrOnly).toBe(true);
    expect(merged[0].excerpt).toBe("");
  });

  it("returns an empty merge for no inputs", () => {
    expect(mergeSearchResults([], [], [])).toEqual([]);
  });
});

describe("Wave 2.5 OCR query gate", () => {
  it("only queries the index once it is ready", () => {
    expect(shouldQueryOcr("login", "ready")).toBe(true);
    expect(shouldQueryOcr("login", "building")).toBe(false);
    expect(shouldQueryOcr("login", "unavailable")).toBe(false);
    expect(shouldQueryOcr("login", "unknown")).toBe(false);
  });

  it("requires at least two trimmed characters", () => {
    expect(shouldQueryOcr(" a ", "ready")).toBe(false);
    expect(shouldQueryOcr("  ", "ready")).toBe(false);
    expect(shouldQueryOcr("ab", "ready")).toBe(true);
    expect(shouldQueryOcr("  登录  ", "ready")).toBe(true);
    expect(OCR_MIN_QUERY_CHARS).toBe(2);
    expect(OCR_QUERY_DEBOUNCE_MS).toBe(250);
  });
});
