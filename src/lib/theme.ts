/**
 * Shared light/dark resolution for every Hangline window (P2-8).
 *
 * The panel reads `settings.theme` live from the store; the capture and shortcut windows are
 * separate webviews, so Rust passes the raw setting in their URL (`?theme=system|light|dark`) and
 * `themeFromLocation` resolves it against the window's own `prefers-color-scheme`. The fly overlay
 * gets its theme from the existing `fly-geometry` payload and does not use this module.
 */

import { useEffect, useState } from "react";

export type ThemeSetting = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

/** The media query Windows drives from Settings → Personalisation → Colours. */
export const DARK_SCHEME_QUERY = "(prefers-color-scheme: dark)";

/** Resolves the stored setting against the current system preference. */
export function resolveTheme(
  setting: ThemeSetting | string | null | undefined,
  prefersDark: boolean,
): ResolvedTheme {
  if (setting === "light" || setting === "dark") return setting;
  return prefersDark ? "dark" : "light";
}

/** Current system preference (false when there is no window, e.g. unit tests). */
export function systemPrefersDark(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia(DARK_SCHEME_QUERY).matches;
}

/** Raw setting carried by a window URL (`?theme=`), defaulting to `system`. */
export function themeSettingFromLocation(
  search: string = typeof location === "undefined" ? "" : location.search,
): ThemeSetting {
  const raw = new URLSearchParams(search).get("theme");
  return raw === "light" || raw === "dark" ? raw : "system";
}

/**
 * Resolves the theme for a window from `?theme=` in its URL, falling back to the system query.
 *
 * `search` and `prefersDark` are injectable so the pure resolution is testable without a DOM.
 */
export function themeFromLocation(
  search: string = typeof location === "undefined" ? "" : location.search,
  prefersDark: boolean = systemPrefersDark(),
): ResolvedTheme {
  return resolveTheme(themeSettingFromLocation(search), prefersDark);
}

/** Tracks `prefers-color-scheme` so `theme: "system"` follows Windows live. */
export function usePrefersDark(): boolean {
  const [dark, setDark] = useState<boolean>(() => systemPrefersDark());
  useEffect(() => {
    const query = window.matchMedia(DARK_SCHEME_QUERY);
    const onChange = (event: MediaQueryListEvent) => setDark(event.matches);
    setDark(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, []);
  return dark;
}

/** Resolves a window theme from the setting and keeps following the system while "system". */
export function useResolvedTheme(setting: ThemeSetting | string | null | undefined): ResolvedTheme {
  const prefersDark = usePrefersDark();
  return resolveTheme(setting, prefersDark);
}
