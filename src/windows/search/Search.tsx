/**
 * The search window (`index.html?window=search&theme=…&lang=…`, Wave 2.2 + Wave 2.5).
 *
 * Opened by the panel's card menu (`open_search`), destroyed by Rust on close. The page owns the
 * whole search experience locally: it reads the same full `AppState` snapshot as every other window
 * (`initAppState`/`useAppStore`) and filters in the webview — there is no backend search command.
 * Matching is case-insensitive across the file name, note, tags and the full path; an empty query
 * lists everything newest-first.
 *
 * Wave 2.5 adds the local OCR index as an **optional enhancement** on top of that local filter:
 * on open the page asks `ocr_status`, starts one bounded background `build_ocr_index` and then,
 * for queries of at least two characters, debounces `search_ocr` (250 ms). OCR-only hits are
 * appended after the local matches (deduplicated by item id), marked "Contains image text" and
 * expand the recognized excerpt in the detail pane. Every OCR failure is silent: the window just
 * keeps filtering locally.
 *
 * Interaction map (keyboard-first, mouse optional):
 * - the search field is focused on open; ↑/↓ move the active row, Enter copies it and closes,
 *   Esc closes the window, typing filters immediately;
 * - a single click selects a row, a double click opens it with the default application;
 * - the detail pane edits the item's note (textarea) and tags (comma-separated input + removable
 *   chips); Save calls `set_item_note`/`set_item_tags` and the local store picks the persisted
 *   values up from the `state` broadcast (no optimistic local copy).
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { assetUrl } from "../../lib/assetUrl";
import { t } from "../../lib/i18n";
import {
  buildOcrIndex,
  closeSearch,
  copyItem,
  ocrStatus,
  openItem,
  searchOcr,
  setItemNote,
  setItemTags,
} from "../../lib/ipc";
import { fileNameOf, showCardImage } from "../../lib/layout";
import { initAppState, useAppStore } from "../../lib/state";
import { themeSettingFromLocation, useResolvedTheme } from "../../lib/theme";
import type { Item, OcrHit } from "../../lib/types";
import "./Search.css";

/** How long the green "Saved" confirmation stays visible (ms). */
const SAVED_STATUS_MS = 1800;

/**
 * Note character limit, mirrored from Rust `app_state::MAX_NOTE_CHARS` (Wave 2 W2.1).
 *
 * The textarea caps typing at the same limit so the normal path never hits the backend rejection;
 * a pasted/over-limit value still surfaces Rust's user-facing error in the status line.
 */
const MAX_NOTE_CHARS = 2000;

/** Minimum query length before the OCR index is queried (Wave 2.5). */
export const OCR_MIN_QUERY_CHARS = 2;

/** Debounce between the last keystroke and `search_ocr` (ms, Wave 2.5). */
export const OCR_QUERY_DEBOUNCE_MS = 250;

/** OCR attempts per background index build (bounded so opening the window stays responsive). */
const OCR_BUILD_LIMIT = 50;

/** No stable empty array: `useMemo` over a fresh `[]` would re-filter on every render. */
const NO_ITEMS: Item[] = [];

/** Save/copy feedback for the detail pane footer. */
type Status =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved" }
  | { kind: "error"; message: string };

/** Human message for an IPC rejection (falls back to the generic status label). */
function messageOf(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.trim() ? text : t("Search failed");
}

// ---------------------------------------------------------------------------------------------
// Pure helpers (kept exported so the filter/parse rules stay inspectable and reusable)
// ---------------------------------------------------------------------------------------------

/** Keyboard action mapped from a key in the search field (Wave 2.2). */
export type SearchKeyAction = "next" | "previous" | "copy";

/**
 * Maps a `keydown.key` of the search field to its list action.
 *
 * Kept pure (and exported) so the keyboard contract — ↑/↓ move the active row, Enter copies and
 * closes — is covered by the unit tests without a DOM. Every other key returns `null` (typing).
 */
export function searchKeyAction(key: string): SearchKeyAction | null {
  if (key === "ArrowDown") return "next";
  if (key === "ArrowUp") return "previous";
  if (key === "Enter") return "copy";
  return null;
}

/**
 * Case-insensitive match across every searchable field: file name, note, tags and full path.
 *
 * An empty/whitespace-only query matches every item (the window then shows the newest first).
 */
export function matchesQuery(item: Item, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const tags = item.tags ?? [];
  const fields = [fileNameOf(item.path), item.note ?? "", tags.join("\n"), item.path];
  return fields.some((field) => field.toLowerCase().includes(needle));
}

/** Filters the line and sorts the result newest-first (`addedAt` descending). */
export function filterItems(items: readonly Item[], query: string): Item[] {
  return items
    .filter((item) => matchesQuery(item, query))
    .sort((left, right) => (right.addedAt ?? 0) - (left.addedAt ?? 0));
}

/** Parses the comma-separated tag input: trimmed, non-empty, de-duplicated (case-insensitive). */
export function parseTags(raw: string): string[] {
  const seen = new Set<string>();
  const tags: string[] = [];
  for (const part of raw.split(",")) {
    const tag = part.trim();
    if (!tag) continue;
    const key = tag.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    tags.push(tag);
  }
  return tags;
}

/** Formats tags for the comma-separated input. */
export function formatTags(tags: readonly string[]): string {
  return tags.join(", ");
}

/** The containing folder name is a language-neutral "source" for the result row. */
export function sourceLabel(item: Item): string {
  const directory = item.path.replace(/[\\/][^\\/]*$/, "");
  const segment = directory.split(/[\\/]/).filter(Boolean).pop();
  if (segment) return segment;
  return item.kind === "recording" ? t("Trim recording") : t("Capture");
}

/** Short local date/time for the result row; malformed timestamps render as an em dash. */
export function formatAdded(addedAt: number): string {
  if (!Number.isFinite(addedAt) || addedAt <= 0) return "—";
  return new Date(addedAt).toLocaleString();
}

// ---------------------------------------------------------------------------------------------
// Wave 2.5: OCR index integration
// ---------------------------------------------------------------------------------------------

/**
 * Lifecycle of the local OCR index, as shown in the list's status line.
 *
 * - `unknown`: the status query has not answered yet (no line, no OCR queries);
 * - `unavailable`: no Windows OCR engine / no index (line says so, queries stay local-only);
 * - `building`: an index build is running (line says so, queries stay local-only until it ends);
 * - `ready`: the persisted index can be searched.
 */
export type OcrPhase = "unknown" | "unavailable" | "building" | "ready";

/**
 * Whether a query should be sent to `search_ocr`.
 *
 * The OCR index is only queried for a non-trivial query (≥2 trimmed characters) once the index is
 * ready; before that the window degrades silently to local filtering.
 */
export function shouldQueryOcr(query: string, phase: OcrPhase): boolean {
  return phase === "ready" && query.trim().length >= OCR_MIN_QUERY_CHARS;
}

/**
 * One merged result row.
 *
 * `ocrOnly` marks an item that the local filter did not match but whose recognized image text did;
 * `excerpt` carries the OCR context window for the detail pane (hardened to plain text — it is
 * untrusted file content and is only ever rendered as a React text node).
 */
export interface MergedResult {
  item: Item;
  ocrOnly: boolean;
  excerpt: string | null;
}

/**
 * Merges local filter results with OCR index hits (Wave 2.5).
 *
 * Local matches always come first in their existing newest-first order and win a duplicate: an OCR
 * hit for an already-listed item is ignored instead of reordering or duplicating the row. OCR-only
 * hits are resolved through the full item list (the index can contain ids that are no longer on the
 * line — those are dropped silently) and appended newest-first.
 */
export function mergeSearchResults(
  localResults: readonly Item[],
  ocrHits: readonly OcrHit[],
  allItems: readonly Item[],
): MergedResult[] {
  const byId = new Map(allItems.map((item) => [item.id, item] as const));
  const seen = new Set<string>();
  const merged: MergedResult[] = [];
  for (const item of localResults) {
    if (seen.has(item.id)) continue;
    seen.add(item.id);
    merged.push({ item, ocrOnly: false, excerpt: null });
  }
  const ocrOnly: MergedResult[] = [];
  for (const hit of ocrHits) {
    if (!hit || typeof hit.id !== "string" || seen.has(hit.id)) continue;
    const item = byId.get(hit.id);
    if (!item) continue;
    seen.add(hit.id);
    ocrOnly.push({
      item,
      ocrOnly: true,
      excerpt: typeof hit.excerpt === "string" ? hit.excerpt : "",
    });
  }
  ocrOnly.sort((left, right) => (right.item.addedAt ?? 0) - (left.item.addedAt ?? 0));
  return [...merged, ...ocrOnly];
}

// ---------------------------------------------------------------------------------------------
// Thumbnail
// ---------------------------------------------------------------------------------------------

/**
 * Result-row thumbnail with the same fallback chain as the panel cards: cached thumb → original
 * image → extension tile. Recordings only render their first-frame preview.
 */
function ResultThumb({ item }: { item: Item }) {
  const version = item.updatedAt ?? 0;
  const thumbSrc = item.thumbPath ? assetUrl(item.thumbPath, version) : null;
  const [imageSource, setImageSource] = useState<"thumb" | "original" | "failed">(() =>
    item.thumbPath ? "thumb" : item.kind === "recording" ? "failed" : "original",
  );

  useEffect(() => {
    setImageSource(item.thumbPath ? "thumb" : item.kind === "recording" ? "failed" : "original");
  }, [item.kind, item.path, item.thumbPath, item.updatedAt]);

  const imageSrc =
    item.kind === "recording"
      ? (thumbSrc ?? "")
      : imageSource === "thumb" && thumbSrc
        ? thumbSrc
        : assetUrl(item.path, version);
  const showImage =
    item.kind === "recording"
      ? Boolean(thumbSrc) && imageSource !== "failed"
      : showCardImage(item.kind, Boolean(item.thumbPath), imageSource);

  if (!showImage) {
    const extension = /\.([^./\\]+)$/.exec(item.path)?.[1] ?? "FILE";
    return (
      <span className="search__thumb search__thumb--fallback" aria-hidden="true">
        {extension.toUpperCase()}
      </span>
    );
  }
  return (
    <img
      className="search__thumb"
      src={imageSrc}
      alt=""
      loading="lazy"
      decoding="async"
      draggable={false}
      onError={() =>
        setImageSource((current) =>
          current === "thumb" && thumbSrc && item.kind !== "recording" ? "original" : "failed",
        )
      }
    />
  );
}

// ---------------------------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------------------------

export function Search() {
  // Separate webview: theme/language are in the URL for a correct first paint; the state
  // subscription keeps them live if the settings change while the window is open.
  const state = useAppStore((store) => store.state);
  const themeSetting = state?.settings.theme ?? themeSettingFromLocation();
  const theme = useResolvedTheme(themeSetting);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    return () => {
      delete document.documentElement.dataset.theme;
    };
  }, [theme]);

  useEffect(() => {
    initAppState();
  }, []);

  const items = state?.items ?? NO_ITEMS;
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  /** Wave 2.5 OCR index lifecycle + size of the last successful build. */
  const [ocrPhase, setOcrPhase] = useState<OcrPhase>("unknown");
  const [ocrIndexed, setOcrIndexed] = useState(0);
  const [ocrHits, setOcrHits] = useState<OcrHit[]>([]);

  const filtered = useMemo(() => filterItems(items, query), [items, query]);
  /** Local results first, OCR-only hits appended (deduplicated by item id). */
  const merged = useMemo(
    () => mergeSearchResults(filtered, ocrHits, items),
    [filtered, items, ocrHits],
  );
  /** Active row: the explicit selection when it still matches, else the first result. */
  const activeId =
    selectedId && merged.some((result) => result.item.id === selectedId)
      ? selectedId
      : (merged[0]?.item.id ?? null);
  const selectedResult = merged.find((result) => result.item.id === activeId) ?? null;
  const selected = selectedResult?.item ?? null;

  // Wave 2.5: on open, ask for engine availability and kick off one bounded background index
  // build. OCR is an optional enhancement — every failure path degrades to local-only search
  // silently (no dialog, no blocking).
  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const availability = await ocrStatus();
        if (disposed) return;
        if (!availability.available) {
          setOcrPhase("unavailable");
          return;
        }
        setOcrPhase("building");
        try {
          const report = await buildOcrIndex(false, OCR_BUILD_LIMIT);
          if (disposed) return;
          if (!report.available) {
            setOcrPhase("unavailable");
            return;
          }
          // `skipped` counts rows already indexed for an unchanged hash: total usable rows now.
          setOcrIndexed(report.indexed + report.skipped);
          setOcrPhase("ready");
        } catch (error) {
          if (disposed) return;
          // Single-flight rejection means another window is already building: keep searching the
          // rows that exist and flip to ready (the debounced query re-runs on the phase change).
          console.info("Hangline search: OCR index build not started here", error);
          setOcrPhase("ready");
        }
      } catch (error) {
        if (disposed) return;
        console.info("Hangline search: OCR index unavailable; local search only", error);
        setOcrPhase("unavailable");
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  // Wave 2.5: debounce OCR queries (≥2 chars) once the index is ready. Empty / building /
  // unavailable states clear the hits so the list is local-only, with no error surface.
  useEffect(() => {
    if (!shouldQueryOcr(query, ocrPhase)) {
      setOcrHits((current) => (current.length === 0 ? current : []));
      return;
    }
    let cancelled = false;
    const needle = query.trim();
    const timer = window.setTimeout(() => {
      void searchOcr(needle)
        .then((hits) => {
          if (!cancelled) setOcrHits(Array.isArray(hits) ? hits : []);
        })
        .catch((error: unknown) => {
          if (cancelled) return;
          console.info("Hangline search: search_ocr unavailable; local results only", error);
          setOcrHits([]);
        });
    }, OCR_QUERY_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [ocrPhase, query]);

  // Detail drafts, loaded when the active item changes. A `state` broadcast replaces every item
  // object; the `lastLoadedIdRef` guard keeps those identity changes from wiping in-progress edits.
  const [noteDraft, setNoteDraft] = useState("");
  const [tagsDraft, setTagsDraft] = useState("");
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const lastLoadedIdRef = useRef<string | null>(null);
  useEffect(() => {
    if (lastLoadedIdRef.current === activeId) return;
    lastLoadedIdRef.current = activeId;
    setNoteDraft(selected?.note ?? "");
    setTagsDraft(formatTags(selected?.tags ?? []));
    setStatus({ kind: "idle" });
  }, [activeId, selected]);

  // "Saved" is a transient confirmation, not a persistent state.
  useEffect(() => {
    if (status.kind !== "saved") return;
    const timer = window.setTimeout(() => setStatus({ kind: "idle" }), SAVED_STATUS_MS);
    return () => window.clearTimeout(timer);
  }, [status]);

  const searchInputRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    searchInputRef.current?.focus();
  }, []);

  const close = useCallback(() => {
    void closeSearch().catch((error: unknown) => {
      console.warn("Hangline search: close_search failed", error);
    });
  }, []);

  // Esc closes the window from anywhere in the page (search field, note textarea, list).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [close]);

  /** Moves the active row by `delta`, wrapping at both ends (local + OCR-only rows). */
  const moveSelection = useCallback(
    (delta: number) => {
      if (merged.length === 0) return;
      const currentIndex = merged.findIndex((result) => result.item.id === activeId);
      const base = currentIndex < 0 ? (delta > 0 ? -1 : 0) : currentIndex;
      const nextIndex = (base + delta + merged.length) % merged.length;
      setSelectedId(merged[nextIndex].item.id);
    },
    [activeId, merged],
  );

  /** Enter / copy: write the item to the clipboard, then close (the copy is the intent). */
  const copySelectedAndClose = useCallback(() => {
    if (!selected) return;
    void copyItem(selected.id)
      .then(() => closeSearch())
      .catch((error: unknown) => {
        setStatus({ kind: "error", message: messageOf(error) });
      });
  }, [selected]);

  const openSelected = useCallback(() => {
    if (!selected) return;
    void openItem(selected.id).catch((error: unknown) => {
      setStatus({ kind: "error", message: messageOf(error) });
    });
  }, [selected]);

  const saveDetails = useCallback(() => {
    if (!selected) return;
    const tags = parseTags(tagsDraft);
    setStatus({ kind: "saving" });
    void Promise.all([setItemNote(selected.id, noteDraft), setItemTags(selected.id, tags)])
      .then(() => {
        setTagsDraft(formatTags(tags));
        setStatus({ kind: "saved" });
      })
      .catch((error: unknown) => {
        setStatus({ kind: "error", message: messageOf(error) });
      });
  }, [noteDraft, selected, tagsDraft]);

  const onSearchKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLInputElement>) => {
      const action = searchKeyAction(event.key);
      if (!action) return;
      event.preventDefault();
      if (action === "next") moveSelection(1);
      else if (action === "previous") moveSelection(-1);
      else copySelectedAndClose();
    },
    [copySelectedAndClose, moveSelection],
  );

  /** Current tag chips (from the draft, so removing one is reflected before saving). */
  const draftTags = parseTags(tagsDraft);
  const removeTag = useCallback((tag: string) => {
    setTagsDraft((current) =>
      formatTags(parseTags(current).filter((candidate) => candidate !== tag)),
    );
  }, []);

  const activeDescendant = activeId ? `search-result-${activeId}` : undefined;
  const noItems = state !== null && items.length === 0;

  return (
    <div className="search" data-ready={state ? "true" : "false"}>
      <header className="search__head">
        <svg
          className="search__icon"
          viewBox="0 0 16 16"
          aria-hidden="true"
          focusable="false"
        >
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.4 10.4 14 14" />
        </svg>
        <input
          ref={searchInputRef}
          className="search__input"
          type="search"
          role="searchbox"
          autoComplete="off"
          spellCheck={false}
          placeholder={t("Search by file name, note or tag")}
          aria-label={t("Search by file name, note or tag")}
          aria-controls="search-results"
          aria-activedescendant={activeDescendant}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={onSearchKeyDown}
        />
        <button
          type="button"
          className="search__close"
          aria-label={t("Close")}
          title={t("Close")}
          onClick={close}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
            <path d="M3.5 3.5 12.5 12.5M12.5 3.5 3.5 12.5" />
          </svg>
        </button>
      </header>

      <main className="search__body">
        <section className="search__list" aria-label={t("Search screenshots")}>
          {/* Wave 2.5: one line of OCR index status. Unavailable/building never block searching;
              they only explain why image text is not contributing hits yet. */}
          {ocrPhase === "unknown" ? null : (
            <p className="search__index-status" data-phase={ocrPhase}>
              {ocrPhase === "building"
                ? t("Building text index…")
                : ocrPhase === "unavailable"
                  ? t("Text index unavailable")
                  : ocrIndexed > 0
                    ? `${t("Text index ready")} · ${ocrIndexed}`
                    : t("Text index ready")}
            </p>
          )}
          <div className="search__pane-head">
            <h2 className="search__pane-title">{t("Search screenshots")}</h2>
            {/* R4: the window searches the full state, which can hold more items than the line
                shows; the count keeps that difference visible (local + OCR-only hits). */}
            <span className="search__count" aria-label={`${merged.length}`}>
              {merged.length}
            </span>
          </div>
          {state === null ? (
            <p className="search__empty">{t("Loading…")}</p>
          ) : noItems ? (
            <p className="search__empty">{t("Take a screenshot and it will hang here")}</p>
          ) : merged.length === 0 ? (
            <p className="search__empty">{t("No matches")}</p>
          ) : (
            <ul
              id="search-results"
              className="search__results"
              role="listbox"
              aria-label={t("Search screenshots")}
            >
              {merged.map((result) => {
                const item = result.item;
                return (
                  <li
                    key={item.id}
                    id={`search-result-${item.id}`}
                    role="option"
                    aria-selected={item.id === activeId}
                    className="search__row"
                    data-active={item.id === activeId ? "true" : undefined}
                    data-ocr-only={result.ocrOnly ? "true" : undefined}
                    onClick={() => setSelectedId(item.id)}
                    onDoubleClick={openSelected}
                    title={item.path}
                  >
                    <ResultThumb item={item} />
                    <span className="search__row-body">
                      <span className="search__name">{fileNameOf(item.path)}</span>
                      <span className="search__meta">
                        {sourceLabel(item)} · {formatAdded(item.addedAt)}
                      </span>
                      {result.ocrOnly ? (
                        <span className="search__badge" title={t("Contains image text")}>
                          {t("Contains image text")}
                        </span>
                      ) : null}
                      {item.note ? <span className="search__note">{item.note}</span> : null}
                      {item.tags && item.tags.length > 0 ? (
                        <span className="search__tags">
                          {item.tags.map((tag) => (
                            <span key={tag} className="search__tag">
                              {tag}
                            </span>
                          ))}
                        </span>
                      ) : null}
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
        </section>

        <section className="search__details">
          {selected ? (
            <>
              <h2 className="search__detail-title" title={selected.path}>
                {fileNameOf(selected.path)}
              </h2>
              <dl className="search__facts">
                <dt>{t("Source")}</dt>
                <dd>{sourceLabel(selected)}</dd>
                <dt>{t("Added")}</dt>
                <dd>{formatAdded(selected.addedAt)}</dd>
                <dt>{t("Name")}</dt>
                <dd title={selected.path}>{selected.path}</dd>
              </dl>

              {/* OCR-only hit (Wave 2.5): show the recognized context window around the match.
                  The excerpt is untrusted image content and is rendered as a plain React text
                  node — never as HTML. */}
              {selectedResult?.ocrOnly ? (
                <div className="search__field">
                  <span className="search__label">{t("Image text")}</span>
                  <p className="search__ocr-excerpt">{selectedResult.excerpt || "—"}</p>
                </div>
              ) : null}

              <label className="search__field">
                <span className="search__label">{t("Note")}</span>
                <textarea
                  className="search__textarea"
                  rows={4}
                  maxLength={MAX_NOTE_CHARS}
                  placeholder={t("No note")}
                  value={noteDraft}
                  onChange={(event) => setNoteDraft(event.target.value)}
                />
              </label>

              <label className="search__field">
                <span className="search__label">{t("Tags")}</span>
                <input
                  className="search__input search__input--tags"
                  type="text"
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={t("Add tag")}
                  value={tagsDraft}
                  onChange={(event) => setTagsDraft(event.target.value)}
                />
              </label>
              {draftTags.length > 0 ? (
                <ul className="search__chips" aria-label={t("Tags")}>
                  {draftTags.map((tag) => (
                    <li key={tag} className="search__chip">
                      <span>{tag}</span>
                      <button
                        type="button"
                        className="search__chip-remove"
                        aria-label={`${t("Remove tag")}: ${tag}`}
                        title={t("Remove tag")}
                        onClick={() => removeTag(tag)}
                      >
                        ×
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="search__hint">{t("No tags")}</p>
              )}

              <footer className="search__footer">
                <button
                  type="button"
                  className="search__save"
                  onClick={saveDetails}
                  disabled={status.kind === "saving"}
                >
                  {t("Save")}
                </button>
                {status.kind === "saving" ? (
                  <span className="search__status">{t("Saving…")}</span>
                ) : status.kind === "saved" ? (
                  <span className="search__status search__status--saved" role="status">
                    {t("Saved")}
                  </span>
                ) : status.kind === "error" ? (
                  <span className="search__status search__status--error" role="alert">
                    {status.message}
                  </span>
                ) : null}
              </footer>
            </>
          ) : (
            <p className="search__hint search__hint--detail">
              {t("Search by file name, note or tag")}
            </p>
          )}
        </section>
      </main>
    </div>
  );
}
