/**
 * Frontend i18n for the panel and the change-shortcut window (design spec §3.10).
 *
 * Rust is the single source of truth for the active language: every full `state` snapshot carries
 * `resolvedLanguage` (Task 15 review fix), which Rust resolves **once** from the persisted
 * `language` setting — Windows display language first, then locale, then English. `state.ts` calls
 * {@link setResolvedLanguage} before publishing a snapshot, so components re-render with the new
 * table. The tray/menu labels are a separate Rust-side table (`src-tauri/src/i18n.rs`) resolved by
 * the same rule, so the native tray and the panel can never disagree under `language: "system"`.
 */

import enJson from "../i18n/en.json";
import zhHans from "../i18n/zh-Hans.json";

const en = enJson as Record<string, string>;
const tables: Record<string, Record<string, string>> = { en, "zh-Hans": zhHans };

/** A UI language the frontend ships (`src/i18n/<tag>.json`). */
export type Language = "en" | "zh-Hans";

/**
 * Maps a Rust-resolved language tag (`PublicState.resolvedLanguage`) to a shipped table.
 *
 * Rust always sends one of the two supported tags; anything else (an old backend without the field,
 * a hand-built test payload) falls back to English. The frontend deliberately does **not** resolve
 * `"system"` itself: `navigator.language` reports the webview's locale, which can differ from the
 * Windows display language the tray uses.
 */
export function languageForResolvedTag(tag: string): Language {
  const normalized = tag.trim().toLowerCase();
  if (
    normalized === "zh" ||
    normalized === "zh-hans" ||
    normalized.startsWith("zh-") ||
    normalized.startsWith("zh_")
  ) {
    return "zh-Hans";
  }
  return "en";
}

/** The table selected for the current process; English until the first snapshot arrives. */
let activeLanguage: Language = "en";

/**
 * Switches the module-level language table from the Rust-resolved tag.
 *
 * Called by `state.ts` with `AppState.resolvedLanguage`; never derives the language locally.
 * @returns the selected language so callers can set `document.documentElement.lang` etc.
 */
export function setResolvedLanguage(resolvedTag: string): Language {
  activeLanguage = languageForResolvedTag(resolvedTag);
  // `document` is absent under the Node-based vitest environment.
  if (typeof document !== "undefined") document.documentElement.lang = activeLanguage;
  return activeLanguage;
}

/** The currently selected UI language (test/diagnostics helper). */
export function activeLanguageTag(): Language {
  return activeLanguage;
}

/** Translates `key`; an unknown key falls back to English and then to the key itself. */
export function t(key: string): string {
  return tables[activeLanguage]?.[key] ?? en[key] ?? key;
}
