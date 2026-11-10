# Changelog

## 1.3.0 — 2026-11-10

> Wave 2 (memory & search). Still **unsigned**: see the [Code signing policy](CODE_SIGNING_POLICY.md).

- **Item notes and tags**: every card can carry a plain-text note (≤2000 chars) and up to 20 tags
  (≤32 chars each, trimmed/deduplicated); older `state.json` files load with empty values, and the
  values are written back by `set_item_note` / `set_item_tags` with one revision bump and one save.
- **On-demand search window** (`Ctrl+Alt+F`): opens in ~70 ms, filters by file name, note, tag or
  path as you type, lists results with thumbnails and metadata, and supports ↑/↓, Enter (copy and
  close), Esc and double-click (open). The window is **destroyed on close** — resident memory does
  not grow with use (+31.5 MB while open, back to baseline after close, flat over repeated cycles).
- **Four search entry points**: tray "Search…", the fourth global hotkey (rebindable with the same
  conflict detection/rollback as the other three), the card context menu, and the change-shortcut
  window (`target=search`).
- Notes and tags are edited in the search window's detail pane (the panel never takes focus); all
  values are plain text rendered as React text nodes.
- **Hover preview**: a 300 ms dwell floats the original screenshot inside the panel window
  (bottom-aligned, card-centred, capped by the panel height); leaving, scrolling, dragging or opening
  the menu hides it immediately. No extra window, no focus.
- **Local OCR text index (PoC)**: `Windows.Media.Ocr` indexes text inside screenshots into
  `%LOCALAPPDATA%\Hangline\ocr-index.json` (0-byte install increment, offline, deletable/rebuildable).
  Evaluated on 30 synthetic Chinese screenshots: **81.67% keyword recall** (98/120, 94.6 ms/image);
  the search window merges in-image matches with an "image text" badge and an excerpt.
## 1.2.0 — 2026-11-10

> Wave 1 (card action surface). Still **unsigned**: see the [Code signing policy](CODE_SIGNING_POLICY.md).

- **Card context menu** (right-click): copy image / copy as file / open / show in Explorer /
  markup (or trim recording) / "Take down ▸" with move-to-Recycle-Bin, remove-from-line and
  take-down-everything-to-the-left/right. Rendered inside the panel window (no extra window); the
  menu rect is reported to the click-through hit-test so it is clickable; outside click closes it
  without triggering the card copy gesture; WebView2's default context menu is suppressed.
- **Hover action capsule**: three buttons (copy / markup·trim / more actions) under a hovered card.
- **Annotation tools 5 → 9**: highlight, mosaic, blur and auto-incrementing number (undo-safe),
  with preview and export sharing one renderer; `Shape.label` is backward compatible.
- **Layered Esc/right-click back-out in the editor**: text → crop → tool → dirty-confirm cancel.
- **Rust**: `copy_item_as_file`, `show_in_explorer` (spec-promised, previously missing) and
  `take_down_side` (batch take-down, never deletes files; one persist + one broadcast).
- 51 new frontend tests (210 → 261) and 7 new Rust tests (369 → 376).
- **Capture finish action**: new `captureCopyOnFinish` setting (+ tray check item) copies a confirmed
  screenshot to the clipboard; if saving fails the cropped image is copied instead of being lost.
- **Pressed-at capture** (two-stage fix): the overlay captures the Chromium browser child window
  instead of the Tauri top-level HWND, and Rust forwards cursor samples
  (`capture-pressed-point` / `capture-pressed-up`) so the selection no longer depends on Chromium
  translating synthetic moves into pointer events.
- **Pressed-at capture**: the first mouse press of a hotkey-triggered capture is no longer dropped —
  the overlay takes OS mouse capture and continues the already-started drag (`capture-ready.pressedAt`).
- **Tray explainability**: real disabled explanation rows in both retention submenus, a Yes/No
  confirmation before "Take everything down" (count and file semantics in the text, localized), and
  a one-per-run warning when a global shortcut fails to register.
- **Third global shortcut**: `editLatestHotkey` (default `Ctrl+Alt+E`) opens the newest card in the
  markup editor (recordings in the trim window); tray shortcut row + the change-shortcut window
  support the new target (commands 40 → 43 in total across batches).
- 4 new frontend tests (261 → 270, including the shortcut-target parser) and 6 new Rust tests
  (376 → 382).
## 1.1.1 — 2026-11-09

> Wave 0 debt paydown plus the first real-machine acceptance pass. Still **unsigned**: see the
> [Code signing policy](CODE_SIGNING_POLICY.md).

### Reliability
- Settings and state persistence gained a third fallback layer: every save rotates the previous
  good file to `.bak`, a corrupt main file is recovered from that backup (and written back), and a
  file that exists but **cannot be read is never overwritten** — the write is refused with a
  diagnostic error instead of silently destroying a recoverable file. The corrupt sample is still
  preserved as `<name>.corrupt`.

### Diagnostics
- New `hangline.exe --selftest` / `--selftest --json`: 10 field probes (data directory writability,
  settings/state integrity, free disk, DPI awareness, monitors, WebView2 runtime, process memory,
  autostart, version). No Tauri/window/tray/WebView2 is initialised, the exit code is the number of
  failures, and the report is also appended to `%LOCALAPPDATA%\Hangline\logs\selftest.log`.

### Release discipline
- Installer size gate tightened from 15 MB to **8.5 MB** (1.1.0 measured 7,315,272 B), with a
  four-item size budget report (application exe, NSIS setup, WebView2 bootstrapper, portable ZIP)
  and matching gates in CI, `verify-installer.ps1` and the release docs.

### Internal
- IPC surface trimmed: removed the unconsumed `clipboard-copied`, `second-instance` and
  `fullscreen-state` events and the P0 `drag_demo_file` scaffolding command (events 24 → 21,
  commands 41 → 40). `hotkey-error` is deliberately kept (no consumer yet) for the upcoming
  settings-explainability work.
- 39 new unit tests: 8 for the persistence fallbacks, 31 for the diagnostics module.

### Documentation and process
- Code signing policy and SignPath Foundation application materials (`CODE_SIGNING_POLICY.md`,
  `docs/release/signpath*`); the project is applying for free OSS signing, so releases up to
  1.1.1 remain unsigned.
- Real-machine acceptance record (`docs/acceptance/W0-real-machine-2026-11-09.md`) and a reusable
  acceptance harness; Wave 0 status/runbook and the SnowAir benchmark decision documents under
  `docs/product/`.

## 1.1.0 — 2026-11-05

### Age retention (`keepForDays`)
- Tray → **Keep for** submenu: keep forever (default, unchanged 1.0 behavior) or only the last
  7 / 30 / 90 days. Expired cards leave the line; Hangline-owned screenshots in
  `%LOCALAPPDATA%\Hangline\Screenshots` are recycled (undoable), while watched-folder files,
  system screenshots and recordings are only taken off the line and never deleted.
- The sweep runs once at startup, hourly while the app runs, and immediately after the setting
  changes (shortening the window takes effect at once). `settings.json` gains `keepForDays`
  (`"forever"` or 7/30/90); configs written before this change keep cards forever.

## 1.0.0 — 2026-11-05

First source release of Hangline (晾图绳), a Windows screenshot clothesline.

### Core (P1)
- Four screenshot sources on one line: built-in region capture (`Ctrl+Alt+S`), the Windows
  screenshots folder, clipboard images, and Xbox Game Bar recordings.
- Click to copy (PNG + DIBV5 + CF_HDROP), double-click to open, drag out to apps/folders/Recycle
  Bin, × to take down; content-hash deduplication; tray settings; hotkeys with conflict rollback;
  autostart; sounds; multi-monitor/DPI placement; persistence across restarts.

### Visual soul (P2)
- Final glass cards, aluminium pins and rope geometry, spring arrival/sway, breeze, screen-wide
  fly-in and take-down fall, light/dark materials, 432 px thumbnail cache, 60 fps panel (12 cards)
  and frame-comparison evidence.

### Markup and recordings (P3)
- Long-press markup editor: pen (3 widths/6 colours), rectangle, arrow, multiline text, crop,
  undo/redo, atomic PNG write-back with thumbnail/panel refresh; watched-folder items read-only.
- Game Bar first-frame cards and a WinRT timeline trim window with atomic replace, audio preserved,
  progress and safe failure/fallback.

### Multi-monitor and settings (P4)
- Full-screen suppression (QUERY_USER_NOTIFICATION_STATE + foreground coverage, pinned/override),
  retention cap 25/50/100 (oldest cards leave the line; files are never deleted), 100-card 60 fps
  scroll evidence, hotkey conflict/rebinding, quoted HKCU autostart with reconcile, sound gate,
  release-build instrumentation hygiene and hardening (fly/fall final state, capture preflight,
  corrupt-thumbnail self-heal).

### Release (P5)
- Unsigned per-user NSIS installer (7,315,272 B, ≤15 MB gate) with embedded WebView2 bootstrapper,
  Start-menu shortcut (desktop optional), HKCU uninstall entry, and a keep/delete user-data prompt (M17).
- Portable ZIP with a `portable` marker.
- `tauri-plugin-updater` integration: daily or manual update checks, signature-verified passive
  install and restart; signed `latest.json` generated by the release script. Update checks are the
  only network behavior of the app.
- CI and tag-triggered release workflows, SHA-256 sums, release/press documentation.

### Known limitations
See `docs/acceptance/P1-core.md` … `P5-core.md`. Real clean-VM/SmartScreen installation, real
full-screen games, some real-application drag/paste targets, HEIC decoding, 11-language expansion
and code signing remain manual/deferred. The installer is unsigned in V1. The portable build still
stores data in `%LOCALAPPDATA%\Hangline` (a true portable mode is post-V1). A public GitHub Release
requires a repository + signing secrets; this checkout only builds artifacts locally.
