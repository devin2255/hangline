/**
 * Versioned asset URLs for Hangline windows (P3-3).
 *
 * WebView2 caches asset-protocol responses by URL. A markup save rewrites the screenshot **and** its
 * 432 px preview under the same paths, so the panel would keep showing the pre-edit bytes (the
 * known P2 limitation). Appending `Item.updatedAt` as `?v=` makes every rewrite a new URL while
 * leaving the conversion/encoding of the path (spaces, backslashes, drive letter) to Tauri's
 * `convertFileSrc`. Items that were never rewritten (`updatedAt` 0 or missing) keep the plain URL,
 * so existing caches stay valid.
 */

import { convertFileSrc } from "@tauri-apps/api/core";

/** Converts an absolute path to the asset URL (injectable for the unit tests). */
export type FileSrcConverter = (path: string) => string;

/**
 * Builds the asset URL for `path`, appending `?v=<updatedAt>` when the item was rewritten.
 *
 * `updatedAt` is milliseconds since the Unix epoch; non-finite, zero and negative values are
 * treated as "no version" (legacy items load with `0` after the P3 migration). When the converted
 * URL already carries a query string the version is appended with `&` instead of `?`.
 */
export function assetUrl(
  path: string,
  updatedAt?: number | null,
  convert: FileSrcConverter = convertFileSrc,
): string {
  const base = convert(path);
  const version =
    typeof updatedAt === "number" && Number.isFinite(updatedAt) && updatedAt > 0
      ? Math.trunc(updatedAt)
      : 0;
  if (version <= 0) return base;
  const separator = base.includes("?") ? "&" : "?";
  return `${base}${separator}v=${version}`;
}
