import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  // `.research/` holds read-only third-party reference checkouts (e.g. the SnowAir benchmark clone).
  // `.acceptance/` is local real-machine acceptance evidence and harness tooling (git-ignored).
  // Neither is part of the project source, so neither is linted.
  { ignores: ["dist", "src-tauri/target", ".research", ".acceptance"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  { plugins: { "react-hooks": reactHooks }, rules: { ...reactHooks.configs.recommended.rules } },
  {
    files: ["scripts/**/*.mjs"],
    languageOptions: {
      globals: { console: "readonly", process: "readonly", URL: "readonly" },
    },
  },
);
