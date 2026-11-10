/**
 * The search window (`index.html?window=search&theme=…&lang=…`, Wave 2.2).
 *
 * Opened by the panel's card menu (`open_search`), destroyed by Rust on close. The page owns the
 * whole search experience locally: it reads the same full `AppState` snapshot as every other window
 * (`initAppState`/`useAppStore`) and filters in the webview — there is no backend search command.
 * Matching is case-insensitive across the file name, note, tags and the full path; an empty query
 * lists everything newest-first.
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
  closeSearch,
  copyItem,
  openItem,
  setItemNote,
  setItemTags,
} from "../../lib/ipc";
import { fileNameOf, showCardImage } from "../../lib/layout";
import { initAppState, useAppStore } from "../../lib/state";
import { themeSettingFromLocation, useResolvedTheme } from "../../lib/theme";
import type { Item } from "../../lib/types";
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
  const filtered = useMemo(() => filterItems(items, query), [items, query]);
  /** Active row: the explicit selection when it still matches, else the first result. */
  const activeId =
    selectedId && filtered.some((item) => item.id === selectedId)
      ? selectedId
      : (filtered[0]?.id ?? null);
  const selected = filtered.find((item) => item.id === activeId) ?? null;

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

  /** Moves the active row by `delta`, wrapping at both ends. */
  const moveSelection = useCallback(
    (delta: number) => {
      if (filtered.length === 0) return;
      const currentIndex = filtered.findIndex((item) => item.id === activeId);
      const base = currentIndex < 0 ? (delta > 0 ? -1 : 0) : currentIndex;
      const nextIndex = (base + delta + filtered.length) % filtered.length;
      setSelectedId(filtered[nextIndex].id);
    },
    [activeId, filtered],
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
          <div className="search__pane-head">
            <h2 className="search__pane-title">{t("Search screenshots")}</h2>
            {/* R4: the window searches the full state, which can hold more items than the line
                shows; the count keeps that difference visible instead of looking broken. */}
            <span className="search__count" aria-label={`${filtered.length}`}>
              {filtered.length}
            </span>
          </div>
          {state === null ? (
            <p className="search__empty">{t("Loading…")}</p>
          ) : noItems ? (
            <p className="search__empty">{t("Take a screenshot and it will hang here")}</p>
          ) : filtered.length === 0 ? (
            <p className="search__empty">{t("No matches")}</p>
          ) : (
            <ul
              id="search-results"
              className="search__results"
              role="listbox"
              aria-label={t("Search screenshots")}
            >
              {filtered.map((item) => (
                <li
                  key={item.id}
                  id={`search-result-${item.id}`}
                  role="option"
                  aria-selected={item.id === activeId}
                  className="search__row"
                  data-active={item.id === activeId ? "true" : undefined}
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
              ))}
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
