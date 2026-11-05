# Hangline P3 Markup & Recording Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement P3 of Hangline: long-press opens a built-in markup editor for screenshots (pen / rectangle / arrow / text / crop / undo / redo, atomic write-back + thumbnail refresh) and a trim window for Game Bar recordings (first-frame card, `<video>` preview, dual-range trim, WinRT MediaComposition export + atomic replace).

**Architecture:** Two new focusable Tauri windows (`annotate`, `trim`) share the existing window/theme/i18n infrastructure. The annotate frontend is a canvas editor whose pure model (`annotation.ts`) maps display coordinates to image pixels, keeps an undo stack and serializes a full-resolution PNG; Rust validates the data URL, atomically replaces the original file, bumps `Item.updated_at`, rebuilds the thumbnail and broadcasts state. Recording thumbnails come from a small WinRT/Media Composition helper; trim runs the same WinRT stack on a background thread and emits progress.

**Tech Stack:** Tauri 2.12, Rust 1.90 + `windows` 0.62 (`Media_Editing`, `Media_Transcoding`, WinRT), React 19/TS 5, Canvas 2D, HTML5 `<video>`, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` §3.1 (long press), §3.7 (markup editor), §3.8 (recordings/trim), §4.4 (IPC), §7 (M12/M13).

## Global Constraints

- Windows 10 22H2 / 11 x64 only; no new dependencies; app-initiated network stays zero.
- Source comments English; docs and UI strings localized (en / zh-Hans) via `src/i18n/*.json`; new strings must pass `scripts/check-i18n.mjs`.
- Files are only ever replaced atomically (temp file in the same directory + rename/replace); a failed save/trim must leave the original untouched and remove its temp file.
- Never delete or mutate watched-folder/extra-folder files without user action: markup/trim write-back only applies to Hangline-owned items (`kind` = capture/systemScreenshot/clipboard/recording, i.e. items under `%LOCALAPPDATA%\Hangline\Screenshots`). Watched-folder items open read-only in the editor and save is disabled with an explanatory string.
- Manual acceptance items are never marked PASS in docs; they are listed MANUAL-PENDING (M12/M13 and the new checklist).
- Quality gates per task: `pnpm check`, `cargo test`, `cargo check --all-targets` 0 warnings; commit on `feat/core`; clean tree. Do not edit `docs/superpowers/specs|plans` from implementers (controller owns those).
- Evidence privacy: committed screenshots may only contain Hangline UI or neutral synthetic art; no private desktop, no Tendedero assets.

---

## Reference data

| Item | Value | Source |
|---|---|---|
| Long-press trigger | 0.45 s hold (press scale 0.95 already from P2) | spec §3.1/§3.7 |
| Long-press cancel | pointer up/leave before 0.45 s, or movement > 4 dp (same threshold as drag-out) | spec §3.1, `dragThreshold.ts` |
| Editor size | image fit 80% of the work area, max 1400×900, borderless dark window, custom titlebar | spec §3.7 |
| Tools | pen (3 widths, 6 colours), rectangle, arrow, text (multiline), crop, undo/redo ≥ 20, save/cancel | spec §3.7 |
| Shortcuts | Ctrl+Z / Ctrl+Y undo/redo, Ctrl+S save, Esc cancel (dirty → confirm) | spec §3.7 |
| Write-back | composite full-resolution PNG, temp + atomic replace, refresh thumbnail, `onSaved` | spec §3.7 |
| Recording card | first frame + play badge; click copies file (`CF_HDROP`, no PNG); double-click opens; long-press trims | spec §3.8 |
| Trim window | HTML5 `<video>` preview + dual sliders + duration label; ≤ 2 GB / ≤ 60 min else refuse with message | spec §3.8 |
| Trim export | WinRT `Windows.Media.Editing.MediaComposition` + `MediaTranscoder`, temp file, `ReplaceFile`/rename, progress event | spec §3.8 |
| M12 | markup save replaces the original, thumbnail refreshes immediately, Ctrl+Z works in-editor | spec §7.2 |
| M13 | Game Bar recording ingested, trim saves a playable file with the expected duration | spec §7.2 |
| P3 DoD | M12/M13 pass; markup save failure rate 0 (100× stress) | spec §8 |

## File structure

```
src/lib/longPress.ts              # pure 0.45 s hold gesture (new)
src/lib/longPress.test.ts         # tests (new)
src/lib/annotation.ts             # pure editor model: shapes, undo/redo, crop, coord mapping (new)
src/lib/annotation.test.ts        # tests (new)
src/lib/editorKeys.ts             # pure keyboard mapping for the two editors (new)
src/lib/editorKeys.test.ts        # tests (new)
src/lib/ipc.ts                    # open/annotate/trim/save commands + trim-progress listen (modified)
src/lib/types.ts                  # Item.updatedAt, editor targets, trim progress (modified)
src/lib/assetUrl.ts               # convertFileSrc + ?v=<updatedAt> cache busting (new)
src/lib/assetUrl.test.ts          # tests (new)
src/components/GlassCard.tsx      # long-press start/cancel wiring, suppress click after hold (modified)
src/windows/panel/Panel.tsx       # onLongPress → open annotate/trim; pass callbacks (modified)
src/windows/annotate/Annotate.tsx # canvas editor window (new)
src/windows/annotate/Annotate.css # editor materials/dark window (new)
src/windows/trim/Trim.tsx         # video preview + dual sliders (new)
src/windows/trim/Trim.css         # trim window materials (new)
src/main.tsx                      # ?window=annotate|trim routes (modified)
src/i18n/en.json, zh-Hans.json    # editor/trim strings (modified)
src-tauri/src/annotate.rs         # open/save/close annotation, atomic PNG replace, thumb refresh (new)
src-tauri/src/trim.rs             # validation + MediaComposition trim + atomic replace (new)
src-tauri/src/video.rs            # first-frame + duration helper via WinRT (new)
src-tauri/src/item.rs             # updated_at field (modified)
src-tauri/src/app_state.rs        # replace_item/update flow, thumbnail updates (modified)
src-tauri/src/thumbnails.rs       # recordings: first-frame thumbnail + dims; force refresh (modified)
src-tauri/src/lib.rs              # commands, modules, async trim thread registration (modified)
src-tauri/src/panel.rs            # window size helper for editor windows (modified, if needed)
scripts/p3/fixture-video.sh/.ps1  # dev-only: generate tiny test mp4 with ffmpeg (new, not shipped)
docs/experiments/P3-markup-record.md  # runtime evidence + limits (new)
docs/acceptance/P3-core.md        # M12/M13 matrix + manual checklist (new)
README.md, README.zh-Hans.md      # P3 screenshots + status (modified)
```

---

### Task P3-1: Long-press gesture, pure editor helpers, i18n

**Files:** create `src/lib/longPress.ts`, `longPress.test.ts`, `src/lib/editorKeys.ts`, `editorKeys.test.ts`, `docs/experiments/P3-reference.md`; modify `src/i18n/en.json`, `zh-Hans.json` (add keys).

**Interfaces:**
- `createLongPress(options: { holdMs?: number; moveThresholdPx?: number; onLongPress: () => void }): { pointerDown(x:number,y:number): void; pointerMove(x:number,y:number): void; pointerUp(): void; cancel(): void; readonly held: boolean }` — timer-injected through `globalThis.setTimeout/clearTimeout`, so tests use fake timers; a movement > threshold or pointerup before the deadline cancels and never fires; firing the hold sets `held` and cancels future events until `pointerDown` again.
- `editorKeys(key: string, ctrl: boolean, shift: boolean): "undo" | "redo" | "save" | "cancel" | null` — `z`/`Z` with ctrl = undo, ctrl+shift+z or ctrl+y = redo, `s` ctrl = save, `Escape` = cancel; every other key null.
- i18n keys (en → zh): `Markup`, `Pen`, `Rectangle`, `Arrow`, `Text`, `Crop`, `Undo`, `Redo`, `Save`, `Cancel`, `Discard changes?`, `Saving…`, `Save failed`, `Read-only (watched folder)`, `Trim recording`, `Start`, `End`, `Duration`, `Trim`, `Trimming…`, `Trim failed`, `Open in default player`, `File is too large to trim`, `Recording is too long to trim`, `Invalid trim range`, `Loading…`.

- [ ] **Step 1:** write `longPress.test.ts` with fake timers: fires at 450 ms; cancels on movement > 4 px; cancels on early pointerup; a second press after firing starts a new hold; `cancel()` prevents firing; `held` flag transitions.
- [ ] **Step 2:** run `pnpm exec vitest run src/lib/longPress.test.ts` → RED; implement; GREEN.
- [ ] **Step 3:** same for `editorKeys.test.ts` (all shortcuts + ignored keys + no-ctrl z).
- [ ] **Step 4:** add i18n keys to both files and run `pnpm check` (parity check).
- [ ] **Step 5:** create `docs/experiments/P3-reference.md` (Chinese): the reference table above, long-press model (0.45 s, 4 dp cancel), editor window sizing rule, write-back/trim pipeline, source citations (`research/ui/src/{Markup,Trim,GrabArea}.swift`), and the "no mosaic/numbering/highlighter (V2)" exclusions.
- [ ] **Step 6:** commit `feat: P3 long-press gesture and editor helpers`.

---

### Task P3-2: Annotation editor UI (frontend)

> **执行说明（控制器 2026-11-03）**：P3-2 与 P3-3 合并执行，因为窗口创建/`get_editor_target` 本就在 P3-3 接口里，且编辑器只有接上写回+缩略图刷新+版本化 URL 才是独立可验收的交付。顺序：先 Rust 窗口/目标/写回/缩略图/`updatedAt`/assetUrl，再 UI，最后端到端证据；一份报告、可拆两个逻辑提交。

**Files:** create `src/windows/annotate/Annotate.tsx`, `Annotate.css`, `src/lib/annotation.ts`, `annotation.test.ts`; modify `src/main.tsx`, `src/lib/types.ts` (editor target type), `src/lib/ipc.ts` (`getEditorTarget`, `saveAnnotation`, `closeEditor`), `src/lib/assetUrl.ts` (new, `assetUrl(path, updatedAt)`), `assetUrl.test.ts`.

**Interfaces:**
- `type Tool = "pen" | "rect" | "arrow" | "text" | "crop"`.
- `interface Shape { id: string; tool: Exclude<Tool,"crop">; color: string; width: number; points: [number, number][]; text?: string }`; `interface CropRect { x:number; y:number; w:number; h:number }`.
- `createAnnotationState(width, height)`, `addShape(state, shape)`, `undo(state)`, `redo(state)`, `setCrop(state, rect|null)`, `canUndo/canRedo`, `MAX_HISTORY = 24`, `toImagePoint(display, state, view)`, `renderShapes(ctx, shapes, scale)` helper (canvas 2D draw of pen/rect/arrow/text; exported for the component and tests where possible), `cropOutput(state): {width,height,offsetX,offsetY}`.
- Rust command `get_editor_target(id: string) -> Result<EditorTarget>`: `{ item: Item, owned: bool, theme: String }` where `owned` is true when the path is inside Hangline's screenshot dir; `save_annotation(id, pngBase64) -> Result<Item>`; `close_editor()` (destroy the annotate window).
- `Annotate.tsx`: parse `id` from `?id=`, load target, show image scaled to fit (object-fit contain) with an absolutely positioned `<canvas>` at the same displayed size; toolbar (tools, 6 colours, 3 widths, undo/redo, save/cancel) and custom titlebar; pointer events draw; text tool opens a textarea at the click point (Enter/newline, blur commits); crop tool drags a rectangle with handles and an "apply crop" confirm; undo/redo via `editorKeys`; save renders an offscreen canvas at natural image size (image + shapes scaled to image px, crop offset applied), exports `toDataURL("image/png")` base64 and calls `saveAnnotation`; on success calls `closeEditor()` (window closes) and leaves the panel to refresh from `state`; read-only target disables save with the i18n string; dirty + Esc shows a confirm (native `confirm` is acceptable for V1) and closes without saving.
- Theme: window loads `?theme=` like capture/shortcut, `html[data-theme]` before first paint; CSS tokens match the dark custom titlebar.
- `assetUrl.test.ts`: `?v=` appended when updatedAt > 0, absent when 0/missing, keeps URL encoding for spaces/backslashes.

- [ ] Tests: `annotation.test.ts` (undo/redo limit, redo cleared by new shape, crop mapping, toImagePoint with zoom/pan-free view, render? skip canvas in jsdom), `editorKeys` reuse, `assetUrl.test.ts`; run vitest → RED/GREEN.
- [ ] Implement UI; verify via CDP on a generated PNG fixture: open window, switch tools, draw, undo, crop, save; screenshot evidence under `.superpowers/p3-evidence/` (git-ignored).
- [ ] Commit `feat: P3 annotation editor UI`.

---

### Task P3-3: Rust annotation save, versioned assets, thumbnail refresh + 100× stress

**Files:** create `src-tauri/src/annotate.rs` (or `annotate/{mod.rs,window.rs}`), modify `src-tauri/src/{lib.rs,item.rs,app_state.rs,thumbnails.rs}`; frontend `assetUrl` use in `GlassCard.tsx`, `Fly.tsx`, `Panel.tsx`, `Annotate.tsx`.

**Interfaces:**
- `Item.updated_at: i64` serde camelCase `updatedAt`, default 0; set by `Item::new` to `added_at`; migration read keeps 0 for old states; TS `updatedAt?: number`.
- `AppState::replace_item(item) -> anyhow::Result<PublicState>`: find by id, replace fields (path/size/hash/updated_at/thumb_path), persist, revision bump, return snapshot; missing id → Err.
- `annotate::open(app, id) -> Result<()>`: validate item exists, create/reuse `annotate` window (focusable, decorations false, always_on_top false, center on the item's/panel's monitor; inner size from image scaled to 80% work area clamped to 1400×900, at least 480×360), URL `index.html?window=annotate&id=…&theme=…`; the frontend loads pixels via `get_editor_target`.
- `annotate::save(app, id, png_base64) -> Result<Item>`: reject if not owned (`owned=false`); decode base64 → `image::load_from_memory` (RGBA8); limit 12000×12000 and 128 MB decoded; encode PNG; write `<dir>/.hangline-markup-<uuid>.png.tmp` with `sync_all`, rename over the original; compute BLAKE3 hash, regenerate thumbnail (`thumbnails::thumb_from_image`/`attach_from_image` with forced overwrite), set width/height/hash/updated_at = now, `replace_item`, `emit_state`; on any error remove the temp and return Err without touching the original.
- Thumbnail force refresh: `thumbnails::refresh_from_image(cache, source, id, &RgbaImage) -> Option<PathBuf>` writes the 432 px preview atomically even when the old preview is fresh.
- Frontend cache busting: every card/fly image URL becomes `assetUrl(item.thumbPath ?? item.path, item.updatedAt)`; annotations set `updatedAt`, so the panel reloads the new bytes (fixes the P2 byte-only WebView cache known issue). Detail view/editor also use it.

- [ ] **Step 1:** Rust tests: `save_annotation` rejects a watched-folder item; atomic replace writes readable PNG and leaves no `.tmp`; a corrupt/oversized payload returns Err and the original is unchanged; `replace_item` bumps revision/persists; `refresh_from_image` writes 432 px; a 100-iteration save loop on a temp item all succeed and leave exactly one file (0 failures, no temp residue) — the P3 DoD stress test.
- [ ] **Step 2:** run `cargo test` → RED for missing modules; implement; focused GREEN.
- [ ] **Step 3:** runtime E2E via CDP: open annotate for a generated fixture, draw, save, verify panel card refresh (DOM asset URL includes `?v=` and image bytes/dims change), thumbnail mtime/hash updated, no duplicate watcher item.
- [ ] **Step 4:** commit `feat: P3 markup write-back, thumbnails and versioned assets`.

---

### Task P3-4: Long-press wiring + read-only behavior

**Files:** modify `src/components/GlassCard.tsx`, `src/windows/panel/Panel.tsx`, `src/lib/types.ts` (callback types), tests `src/lib/longPress.test.ts` reuse; maybe `cardGesture.ts` (suppress pending copy after hold).

**Behavior:**
- On `<article>` pointerdown (left button only, not the take-down cross, not dragging, not paused/reduced?) start the long-press; pointermove > 4 dp or pointerup/leave/cancel aborts; on hold: cancel the pending single-click copy, clear pressed pose, call `onLongPress(item)`; the following click/up must not copy or open. For recordings the panel opens `open_trim`; images `open_annotate`.
- Opening an editor sets one shared "editor open" flag (Rust tracks the window; panel can `set_panel_busy(true)` while a modal editor is open, mirroring the capture suspension) so the line does not tuck while the user edits, and the annotate/trim window is focused normally.
- Read-only: watched-folder item → editor opens with save disabled and a hint (Rust returns `owned=false`); no write attempt.
- Long press on a card whose image failed to load still opens the editor by path (fallback for HEIC: editor shows the tile/name and Save is disabled with "cannot edit this file" — implement an explicit disabled state).
- Tests: pure long-press already covered; add a component-level guard via existing gesture tests where possible; runtime CDP trusted long-press (mouse down 500 ms then up) opens the window and does not copy.
- Commit `feat: P3 long-press opens markup/trim`.

---

### Task P3-5: Recording first frame and dimensions

**Files:** create `src-tauri/src/video.rs`; modify `src-tauri/src/thumbnails.rs`, `src-tauri/src/lib.rs` (mod), Windows features in `src-tauri/Cargo.toml` if needed (`Foundation`, `Storage`, `Storage_Streams`, `Graphics_Imaging`, `Media_Core` etc.).

**Interfaces:**
- `video::first_frame(path: &Path, max_edge: u32) -> Result<video::VideoInfo>` where `VideoInfo { image: RgbaImage, width: u32, height: u32, duration_ms: u64 }`.
  - Implementation via WinRT `MediaComposition` + `MediaClip::CreateFromFileAsync`, `GetThumbnailAsync(TimeSpan::zero(), max_edge, max_edge, VideoFramePrecision::NearestFrame, ...)`, read the stream bytes and decode with `image`; duration from `clip.OriginalDuration`. Convertible errors (`thiserror`) with context.
- `thumbnails::attach` for `ItemKind::Recording`: call `video::first_frame(source, 432)`, use the returned image as the preview, repair width/height, store `thumb_path`; failures leave None and keep the play-badge fallback. `thumbnails::thumb_for` remains image-only; recording previews go through the video helper.
- Card behavior: when a recording has a preview, `GlassCard` shows the first frame (no filename tile) plus the play badge; no preview → existing fallback.
- Tests: pure aspect/size math; `first_frame` is environment-dependent (mark `#[ignore]` when no media runtime) with a documented command; runtime evidence uses `ffmpeg`-generated neutral mp4 copied into a watched folder (after app start), verifying thumb file (432), item width/height > 0, DOM shows the first-frame image + play badge, and the watcher did not duplicate.
- Commit `feat: P3 recording first-frame previews`.

---

### Task P3-6: Trim window UI

> **执行说明（控制器 2026-11-03）**：P3-6 与 P3-7 合并执行（同一交付：长按录屏 → 裁剪窗口 → WinRT 导出 → 可播放文件），因为窗口/目标命令与导出后端必须一起才能端到端验证；并入 P3-4 的 busy 对账/加载竞态与 P3-5 的录屏缩略图 minor。

**Files:** create `src/windows/trim/Trim.tsx`, `Trim.css`; modify `src/main.tsx`, `src/lib/ipc.ts`, `src/lib/types.ts` (`TrimTarget`, `TrimProgressEvent`).

**Interfaces:**
- Rust commands: `get_trim_target(id) -> Result<TrimTarget>` (`{ item, sizeBytes, durationMs, maxBytes, maxDurationMs, theme }`); `trim_video(id, startMs, endMs) -> Result<Item>` (P3-7); `close_editor()` shared.
- Event `trim-progress { id, progress }` (0–1).
- `Trim.tsx`: `<video controls? no, preview-only>` with `convertFileSrc(assetUrl(path, updatedAt))`; on `loadedmetadata` show duration; dual range inputs (start/end, ms step 10, end > start + 100 ms), labels `mm:ss.S`; disable + message when size > 2 GB or duration > 60 min (Rust values); Save → listen progress, call `trim_video`, close on success; error shows message + "Open in default player" button (`openItem`); Esc/cancel closes; dirty confirm not needed (trim is explicit).
- Window: focusable, decorations false, custom titlebar, theme from `?theme=`, centered, sized to fit 16:9 max 900×600.
- i18n strings from P3-1.
- Evidence: with generated mp4, open window, set range via CDP, save, verify panel card + new duration; screenshots app-UI only.
- Commit `feat: P3 trim window`.

---

### Task P3-7: Rust trim via WinRT MediaComposition

**Files:** create `src-tauri/src/trim.rs`; modify `src-tauri/src/lib.rs`, `src-tauri/src/app_state.rs` (reuse `replace_item`), `src-tauri/Cargo.toml` features if needed.

**Interfaces:**
- Pure `validate_trim(size_bytes, duration_ms, start_ms, end_ms) -> Result<(TimeSpan range), TrimError>` with errors `TooLarge`, `TooLong`, `EmptyRange`, `RangeOutOfBounds`; limits 2 GB / 60 min; minimum 100 ms; `start < end ≤ duration`.
- `trim::trim(app, id, start_ms, end_ms) -> Result<Item>`: item must be `Recording`, path exists, owned; read metadata size; duration from `video::duration`/clip; validate; run on a dedicated named thread (Tauri command `async` + `spawn_blocking`), initialize `RoInitialize(RO_INIT_MULTITHREADED)`; `MediaComposition` + `MediaClip::CreateFromFileAsync`, append clip, set `TrimTimeFromStart`/`TrimTimeFromEnd`, `RenderToFileAsync(temp)`; coarse progress events (0.05 start, 0.6 rendered, 1.0 replaced) through `app.emit("trim-progress")`; atomically replace original (`std::fs::rename` over it after successful render; ensure temp on same volume); regenerate recording preview via `video::first_frame`, bump `updated_at`/dims, persist/emit; remove temp on every failure path; return the updated item.
- Failure UX: return Err with a localized-ish message; frontend shows error + fallback open button; original never mutated.
- Tests: pure validation matrix (all four errors, boundary 100 ms, exact duration, 2 GB/60 min edges); temp naming/dir same as target; runtime E2E: ffmpeg-generated 3 s mp4 in a watched folder, trim to 1.0–2.5 s via CDP, verify `ffprobe` duration ≈1.5 s ±0.2 s, file playable (ffprobe), preview regenerated, item updated, no `.tmp`; >2 GB / >60 min limits unit-tested only.
- Commit `feat: P3 WinRT trim export`.

---

### Task P3-8: P3 acceptance, evidence, README, milestone tag prep

**Files:** create `docs/experiments/P3-markup-record.md`, `docs/acceptance/P3-core.md`; modify `README.md`, `README.zh-Hans.md`, version bumps (`package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`) to **0.3.0**; no tag from implementer.

- **Docs content:** environment/tool versions; P3 DoD matrix M12/M13 (programmatic PASS vs MANUAL-PENDING); stress result (100/100 saves, 0 failures); trim duration/playability evidence; known limitations (WinRT codec support, DRM, >2 GB/60 min refused, watched-folder read-only, no mosaic/numbering/highlighter, first-frame availability, HEIC decode, context-menu alternative not in scope/V1.1); manual checklist (~10 steps: long-press image → draw each tool → undo/redo → crop → save → panel/thumbnail refresh; long-press watched-folder item → read-only; long-press recording → trim 1 s → playable/duration; Esc cancel; oversized/too-long refusal via fixture metadata; 150% DPI editor; theme dark; 100× save soak if desired); README P3 screenshots (annotate editor, trim window; app-UI only; composited if transparency).
- **Evidence:** runtime via CDP + generated fixtures; screenshots committed under `docs/experiments/P3-*.png`; no private desktop; Tendedero assets remain local-only.
- **Gates:** `pnpm check`, `cargo test`, `cargo check --all-targets`, `cargo check --release --all-targets`, `pnpm tauri build --no-bundle`; commit `docs: P3 acceptance, evidence and 0.3.0`; tree clean; **Lead** runs the final whole-branch review and tags `v0.3.0-markup`.

---

## Manual acceptance checklist (user-run; goes into `docs/acceptance/P3-core.md`)

1. 长按一张自有截图卡片 0.45 s：标注编辑器打开；松手不应触发复制/拖拽/双击。
2. 画笔（三种粗细/六色）、矩形、箭头、文字（多行）各画一个；Ctrl+Z / Ctrl+Y 撤销重做；Esc 有改动时二次确认。
3. 裁剪工具框选后保存；关闭编辑器后卡片立即显示新图，缩略图也更新（无需重启）。
4. 标注保存失败场景（只读文件/磁盘满）应提示错误且原图不变（可人工制造只读文件后重试）。
5. 长按 **监听目录**（他人文件夹）的图片：编辑器只读、保存禁用并提示。
6. 用 Win+Alt+R 录制几秒 → 卡片显示首帧 + 播放角标；单击复制文件，双击用播放器打开。
7. 长按录屏卡片 → 裁剪窗口；拖动起止滑杆，保存；用播放器确认时长正确、可播放。
8. 超过 2 GB 或 60 分钟的录屏：裁剪窗口提示不支持并禁用保存。
9. 150% 副屏 + 亮/暗主题下编辑器/裁剪窗口布局与清晰度正常。
10. 取消/关闭编辑器或裁剪窗口后，线恢复正常交互，无残留窗口/进程。

## Self-review checklist (controller)

- **Spec coverage:** §3.1 long press, §3.7 editor tools/size/write-back/shortcuts, §3.8 recording card/trim/limits/fallback, §4.4 IPC (`save_annotation`, `trim_video`, `trim-progress`, plus new `open_*`), §7 M12/M13, §8 P3 DoD — every item maps to a task.
- **No placeholders:** every task names files, interfaces, tests, commands, commits; no TBD.
- **Type consistency:** `Item.updatedAt`, `EditorTarget{items,owned,theme}`, `TrimTarget`, `trim-progress`, `save_annotation`, `trim_video`, `open_annotate/open_trim`, `createLongPress`, `createAnnotationState` used consistently.
- **Risk:** watched-folder safety, atomic replace, WinRT failure fallback, long press vs drag/click arbitration are the main risks and have explicit tests/steps.
