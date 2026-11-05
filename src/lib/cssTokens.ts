/**
 * Test-only helpers for asserting against the **real** CSS token values (P2-8/P2-9).
 *
 * Kept out of the runtime bundle (only imported by `*.test.ts`): visual-token tests must fail when
 * a CSS variable is reverted, so no duplicated colour literals are allowed in the assertions.
 */

import { readFileSync } from "node:fs";

/** Reads a repo-relative CSS file (URL resolved from this module). */
export function readCss(relativePath: string): string {
  return readFileSync(new URL(relativePath, import.meta.url), "utf8");
}

/** Extracts `--name: value;` declarations from one selector block. */
export function tokensOf(css: string, selector: string): Map<string, string> {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(`${escaped}\\s*\\{`).exec(css);
  if (!match) throw new Error(`selector not found: ${selector}`);
  const start = match.index + match[0].length;
  const end = css.indexOf("}", start);
  if (end < 0) throw new Error(`unterminated block: ${selector}`);
  const block = css.slice(start, end);
  const tokens = new Map<string, string>();
  const declaration = /--([\w-]+)\s*:\s*([^;]+);/g;
  for (let item = declaration.exec(block); item; item = declaration.exec(block)) {
    tokens.set(item[1], item[2].trim().replace(/\s+/g, " "));
  }
  return tokens;
}

/** Reads one token from a selector block, throwing a useful message when it is missing. */
export function token(tokens: Map<string, string>, name: string): string {
  const value = tokens.get(name);
  if (!value) throw new Error(`missing token --${name}`);
  return value;
}
