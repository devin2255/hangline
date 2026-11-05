import { readFileSync, readdirSync } from "node:fs";

const dir = new URL("../src/i18n/", import.meta.url);
const en = JSON.parse(readFileSync(new URL("en.json", dir)));
const langs = readdirSync(dir).filter((f) => f.endsWith(".json") && f !== "en.json");
let bad = 0;
for (const f of langs) {
  const t = JSON.parse(readFileSync(new URL(f, dir)));
  for (const k of Object.keys(en)) {
    if (!(k in t)) {
      console.error(`${f}: missing "${k}"`);
      bad++;
    }
  }
  for (const k of Object.keys(t)) {
    if (!(k in en)) {
      console.error(`${f}: unused "${k}"`);
      bad++;
    }
  }
}
if (bad) process.exit(1);
console.log(`i18n ok: ${Object.keys(en).length} keys, ${langs.length} languages`);
