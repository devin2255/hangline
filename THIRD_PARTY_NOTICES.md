# Third-Party Notices

Hangline is licensed under the MIT License (see [`LICENSE`](LICENSE)); the app is not released yet.

This file records third-party projects and inspiration. It is not a complete
dependency inventory: the authoritative lists are `src-tauri/Cargo.lock` and
`pnpm-lock.yaml`. Every dependency remains under its own license and copyright,
and those licenses govern that dependency's code.

## Design inspiration: Tendedero

- **Project:** Tendedero — <https://github.com/alejandrobujan/tendedero>
- **License:** MIT with an additional name/icon restriction, © 2026 Alejandro Buján
  (the upstream license reserves Tendedero's name and icon; Hangline does not use
  them, so no permission for Tendedero assets is required)
- **Relationship to Hangline:** design inspiration for the product concept only
  (a screenshot "clothesline" along the top of the screen).

Hangline is an independent implementation for Windows:

- No Tendedero source code, artwork, translation text or assets were copied into
  this repository. All Hangline code and documentation in this repository is
  original work unless noted otherwise.
- The **Hangline** name, the Chinese display name **晾图绳**, the tray icon and
  all UI art in this repository are original Hangline assets, drawn for this
  project. They are not Tendedero's name, icon or art.
- This project is **not affiliated with, endorsed by, or sponsored by** the
  Tendedero project or its authors. References to Tendedero are attribution for
  inspiration, not a claim of association.
- The Tendedero name and icon are not used as trademarks of this project, and
  this project does not use "Tendedero" as part of its product name or branding.

## Major dependencies

Hangline ships as a Rust (Tauri) desktop application with a React frontend.
Representative direct dependencies and their licenses:

| Dependency                 | Version (lockfile) | License                          |
| -------------------------- | ------------------ | -------------------------------- |
| Tauri (`tauri`)            | 2.12.5             | MIT OR Apache-2.0                |
| `tauri-plugin-autostart`   | 2.7.0              | MIT OR Apache-2.0                |
| `tauri-plugin-global-shortcut` | 2.4.1          | MIT OR Apache-2.0                |
| `tauri-plugin-log`         | 2.10.1             | MIT OR Apache-2.0                |
| `tauri-plugin-single-instance` | 2.5.5          | MIT OR Apache-2.0                |
| `serde` / `serde_json`     | 1.0.x              | MIT OR Apache-2.0                |
| `image`                    | 0.25.10            | MIT OR Apache-2.0                |
| `notify`                   | 8.2.1              | CC0-1.0                          |
| `blake3`                   | 1.8.7              | CC0-1.0 OR Apache-2.0 (OR Apache-2.0 WITH LLVM-exception) |
| `windows` (Win32 bindings) | 0.62.2             | MIT OR Apache-2.0                |
| `anyhow` / `thiserror` / `log` / `base64` | 1.x / 2.x / 0.4.x / 0.23.x | MIT OR Apache-2.0 |
| React / React DOM          | 19.3.1             | MIT                              |
| Zustand                    | 5.0.16             | MIT                              |
| `@tauri-apps/api` / CLI    | 2.12.4             | MIT OR Apache-2.0                |
| Vite                       | 7.3.7              | MIT                              |
| TypeScript                 | 5.9.3              | Apache-2.0                       |
| Vitest                     | 3.2.7              | MIT                              |
| ESLint                     | 9.39.5             | MIT                              |
| Prettier                   | 3.9.10             | MIT                              |

The Windows build additionally links the Microsoft WebView2 runtime, which is
installed on Windows 10/11 and distributed under Microsoft's own terms; Hangline
does not redistribute WebView2 binaries in this repository. Debug builds and the
`--no-bundle` release build do not bundle any installer.

If a dependency is used under the Apache-2.0 option, the Apache License 2.0 text
applies to that dependency; the MIT option applies where the MIT license is
chosen. Copyright notices remain with the respective dependency authors.
