import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url)));
const conf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url)));
const a = pkg.version;
const b = conf.version;
if (a !== b) {
  console.error(`version mismatch: package.json=${a} tauri.conf.json=${b}`);
  process.exit(1);
}
console.log(`version ok: ${a}`);
