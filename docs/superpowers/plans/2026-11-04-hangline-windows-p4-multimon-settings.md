# Hangline P4 Multi-Monitor & Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Deliver P4 of Hangline: full-screen suppression (M4), retention/capacity semantics for the line (M14), runtime verification of multi-monitor/DPI/taskbar placement (M2/M3), hotkey-conflict verification (M15), and autostart/sound/settings evidence — then tag `v0.4.0-settings`.

**Architecture:** A new `fullscreen.rs` owns the `SHQueryUserNotificationState` + foreground-window probe and a debounced poll that feeds the reveal controller (`suppressed` gate). `AppState::add_item` gains the retention cap (keep the newest `keepOnLine` cards; never delete files). The existing panel geometry, shortcut window, autostart and sound modules are verified at runtime rather than rebuilt. A CDP evidence suite covers 100-card scrolling, hotkey conflicts (helper `RegisterHotKey` process), fullscreen hide/restore, mixed-DPI placement and settings round-trips.

**Tech Stack:** Tauri 2.12, Rust 1.90 (`windows` crate: `Win32_UI_Shell` `SHQueryUserNotificationState`/`GetForegroundWindow`), React 19/TS, CDP evidence harnesses, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-28-hangline-windows-design.md` §3.2/§3.4 (reveal/tuck), §3.5 (tray menu), §3.6 (settings), §3.9 (multi-monitor/DPI), §4.6 (fullscreen), §4.8 (prune/retention), §7 (M2/M3/M4/M14/M15), §8 (P4).

## Global Constraints

- Windows 10 22H2/11 x64; no new dependencies; no network.
- **Never delete user files from retention or fullscreen paths.** Retention removes cards from the line only (files stay on disk); the existing explicit take-down (`deleteFile:true`) remains the only deletion path for own captures. This deliberately narrows spec §4.8's optional file deletion: there is no “keep files” setting yet, so the safe default is to keep files.
- A full-screen suppression bug must never hide the line permanently: every poll/DB poll exits must restore the pre-suppression reveal state, and panics must not leave the flag stuck.
- Manual acceptance items (M2/M3/M4/M14/M15 real-cursor/taskbar/Game Bar/真实全屏游戏/QQ 抢占) are never claimed PASS; programmatic evidence and user-reported manual results are recorded separately.
- Gates per task: `pnpm check`, `cargo test`, `cargo check --all-targets` (0 warnings), `cargo check --release --all-targets`; commits on `feat/core`; tree clean. Implementers do not edit `docs/superpowers/specs|plans` (controller owns them).
- Evidence privacy: only app-UI/neutral fixtures; the real `Videos\Captures`/desktop are never captured.

## Reference data

| Item | Value | Source |
|---|---|---|
| Fullscreen detection | `SHQueryUserNotificationState` ∈ {`QUNS_RUNNING_D3D_FULL_SCREEN`, `QUNS_PRESENTATION_MODE`, `QUNS_BUSY`}; plus foreground window `GetWindowRect` covering its monitor work area | spec §3.2/§4.6 |
| Fullscreen recheck | 0.8 s after Space/alttab/foreground change; restore ≤1 s | spec §3.2, M4 |
| Fullscreen override | `showOverFullScreen=true` only overrides once the line is pinned/shown by the user (spec: “pinned 且用户显式打开”) | spec §3.2 |
| Retention | `keepOnLine` = `fit` / 25 / 50 / 100; visible window = `max(capacity, keep)`; state must not grow without bound for fixed limits | spec §3.6, §4.8, M14 |
| Capacity | `max(3, min(12, floor((panelWidth-200)/spacing)))`; medium spacing 174 dp | spec §3.3/附录 A |
| Hotkey conflict | second registration fails; old accelerator must stay active and be shown in the remap window | spec §3.5/§4.3, M15 |
| Autostart | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value name “Hangline” | spec §3.5 |
| Sound | `sound=true` plays copy/discard cues through `sound.rs`; no sound when false | spec §3.5 |
| M2 | two monitors 100%+150%, one capture each → correct screen, no offset | spec §7.2 |
| M3 | taskbar top/left → panel hugs the work area, never covered | spec §7.2 |
| M14 | keep 100 + wheel → no jank, newest on the right | spec §7.2 |
| M15 | QQ started first, hotkey conflict → prompt, rollback, editable | spec §7.2 |

## File structure

```
src-tauri/src/fullscreen.rs        # QUNS + foreground probe, debounce, pure decision (new)
src-tauri/src/reveal.rs            # suppressed gate integration (modified)
src-tauri/src/app_state.rs         # retention cap in add_item/set_setting (modified)
src-tauri/src/settings.rs          # helpers: effective_keep(limit, capacity) decision (modified)
src-tauri/src/lib.rs               # spawn fullscreen poller, manage state (modified)
src-tauri/src/panel.rs             # monitor/work-area placement verification helpers (modified if needed)
src/windows/panel/Panel.tsx        # fullscreen/suppressed busy visuals (modified if needed)
scripts/p4/fixture-100.mjs         # 100-card profile + neutral PNGs (new)
scripts/p4/fullscreen-e2e.mjs      # synthetic fullscreen window + hide/restore evidence (new)
scripts/p4/hotkey-conflict-e2e.mjs # helper RegisterHotKey process + app rollback (new)
scripts/p4/settings-e2e.mjs        # autostart/sound/settings round-trips (new)
docs/experiments/P4-multimon-settings.md  # evidence and numbers (new)
docs/acceptance/P4-core.md          # M2/M3/M4/M14/M15 matrix + manual checklist (new)
README.md, README.zh-Hans.md        # P4 sections/screenshots (modified)
```

---

### Task P4-1: Retention cap in state (M14)

**Files:** modify `src-tauri/src/app_state.rs`, `src-tauri/src/settings.rs`, `src-tauri/src/actions.rs`/`ipc.rs` call sites; tests in the same modules; `scripts/p4/fixture-100.mjs`.

**Behavior:**
- `KeepOnLine::Limit(n)` (25/50/100) caps the number of cards in state: after `add_item`, if `items.len() > n`, drop the **oldest** cards until `n` remain. `Fit` leaves state unbounded (it is a view policy; document this in the module comment).
- Reducing `keepOnLine` via `set_setting`/tray immediately trims the state the same way (oldest first), persists, bumps revision once.
- Retention **never deletes files** and never touches watched-folder files; log an info line with the number of cards dropped. The dropped items' thumbnails are left to the existing orphan sweep (id-based) so no new deletion path is introduced.
- Pure helper `retained_len(limit, current, capacity) -> usize`/`drop_oldest(items, limit)` with tests: 101 adds with limit 100 → 100; limit 25 → 25; `Fit` → grows; switching 100→25 trims; newest remains on the right; persistence/revision; no file deletion (assert files still exist); dropping does not touch ids/paths of the kept rows.
- `scripts/p4/fixture-100.mjs`: writes a profile with 100 neutral 640×360 PNGs, settings `keepOnLine=100`, `cardSize=medium`, returns JSON `{profile, ids, paths}` for E2E.
- E2E/evidence: launch with the 100-card profile; assert `state.items.length===100`; set 25 via `set_setting` → 19?/25 (exact 25), files still exist, revision bumped; wheel-scroll panel through the line while a CDP trace samples frames; report fps/long frames (target: no single frame >50 ms, mean ≥58 fps during a 5 s scroll gesture). M14 evidence numbers go into P4 docs; the automated check may be labelled “no jank (programmatic)” with real-mouse manual pending.

**Verification:** `cargo test` retention tests, `pnpm check` untouched, runtime E2E JSON; commit `feat: P4 retention cap and 100-card evidence`.

---

### Task P4-2: Fullscreen suppression (M4)

**Files:** create `src-tauri/src/fullscreen.rs`; modify `src-tauri/src/reveal.rs`, `src-tauri/src/lib.rs`, maybe `src-tauri/src/panel.rs`; tests in `fullscreen.rs`; `scripts/p4/fullscreen-e2e.mjs`.

**Interfaces:**
- `pub enum FullscreenState { Normal, Suppressed(SuppressReason) }`, `SuppressReason { QueryUserNotificationState(u32), ForegroundCoversMonitor }` (serde for events/logs).
- `pub fn query_notification_state() -> Result<u32>` wrapping `SHQueryUserNotificationState`; `pub fn foreground_covers_monitor() -> bool` using `GetForegroundWindow` + `GetWindowRect` + `MonitorFromWindow`/monitor work area; exclude own process windows, the shell/desktop (`Progman`/`WorkerW`/`Shell_TrayWnd`), invisible windows and zero-area rects.
- Pure `pub fn should_suppress(query_state: u32, foreground_covers: bool, pinned: bool, show_over_full_screen: bool) -> Option<SuppressReason>` — QUNS values suppress unless the user has explicitly pinned the line **and** enabled `showOverFullScreen`; foreground-covering suppress unless the same override.
- Polling thread (`fullscreen::start(app)`): every 0.8 s (only while the line is not hidden? keep cheap), emits `fullscreen-state { suppressed }` when changing and calls `reveal::set_fullscreen_suppressed(bool)`. The reveal controller treats `suppressed` like “cannot show”: new-item peeks are deferred (not lost) and a currently visible line is tucked; on exit it restores the pre-suppression wish (if it was pinned/peeked) within ≤1 s. A manual toggle (tray `Ctrl+Alt+T`) sets `pinned=true` and, with `showOverFullScreen`, overrides suppression as spec requires.
- Fail-safe: probe failure => normal (never suppress forever); log errors once per state change.
- Tests: pure matrix for all QUNS values, foreground flag, pinned/override combinations, no-suppress defaults; exclude-window classifier pure helper tests (`class_name_is_own_or_shell`); 0.8 s debounce decision helper (`should_apply(prev, observed, since)`).
- Runtime E2E: helper PowerShell WinForms borderless window covering the primary monitor (allows panel to be underneath), then verify: line visible → within 1.5 s panel hidden and stays hidden on a new fixture item; close helper → within 1.5 s reveal path works again; set `showOverFullScreen=true` + tray toggle while fullscreen → line stays; `showOverFullScreen=false` → hidden. Evidence JSON + screenshots app-UI only.

**Verification:** `cargo test fullscreen`, runtime E2E; commit `feat: P4 fullscreen suppression`.

---

### Task P4-3: Multi-monitor, DPI and taskbar placement (M2/M3)

**Files:** modify `src-tauri/src/geometry.rs`/`panel.rs` only if a gap is found; tests there; `scripts/p4/display-e2e.mjs`; docs evidence.

**Scope:** This task is evidence/hardening, not a rewrite (P1/P2 already place the panel on the pointer monitor and use work areas).
- Pure/work-area tests: top taskbar (work_area.y > 0), left taskbar (work_area.x > 0), negative origins, 150% scaling; panel origin = work area top-left and physical size = logical size × scale; display-change migration when the current monitor disappears (existing logic, add tests if missing).
- Runtime E2E on this machine (1920×1080 @100% + 1920×1080 @150%): capture the panel window rect via CDP + Rust `get_monitors`, move the line to each monitor (top-edge hover or `Ctrl+Alt+T` with cursor there), assert: panel origin equals that monitor's work-area origin, physical size = expected dp × scale, a fixture card's on-screen rect lies within the monitor, no overlap/offset; repeat for light/dark theme. M2's “capture on each screen” remains MANUAL-PENDING (no real capture of the user's desktop); the placement/scale evidence is programmatic.
- Report the measured rects/scales in `docs/experiments/P4-multimon-settings.md`; screenshots are panel-only.
- Commit `test: P4 multi-monitor/DPI placement evidence` (plus any small fix).

---

### Task P4-4: Hotkey conflict, autostart, sound (M15 + P4 deliverables)

**Files:** `scripts/p4/hotkey-conflict-e2e.mjs`, `scripts/p4/settings-e2e.mjs`; small fixes in `src-tauri/src/hotkeys.rs`/`shortcut_window.rs` only if the runtime exposes a defect.

- **M15 runtime:** a helper process registers `Ctrl+Alt+T` (or `Ctrl+Alt+S`) via `RegisterHotKey` and holds it; app startup (or `set_hotkey`) must report the conflict, keep the previous accelerator working (register/unregister sequence), show the conflict in the shortcut remap window, and after the helper releases the key a retry must succeed. Evidence: registration errors, old/new hotkey state, shortcut window text, no duplicate registrations; automate the UI text check via CDP.
- **Autostart:** set `startAtLogin=true` → assert the `HKCU\...\Run` value exists and points at the current exe; `false` → removed; the existing reconcile logic still correct; no leftover when quitting. Test through `set_setting` + registry read.
- **Sound:** with `sound=true`, copy/discard paths call the sound module (observable log/unit seam); with `sound=false` no play. Runtime audio is not captured (no recording of the user's speakers); assert the gate/log only and state manual sound check.
- Record the evidence; commit `test: P4 hotkey conflict, autostart and sound evidence` (+ fixes if needed).

---

### Task P4-5: 100-card panel performance evidence (M14)

**Files:** `scripts/p4/perf-100.mjs` (reuse `scripts/p2/perf.mjs` helpers); docs evidence.

- With the 100-card profile: reveal the line, perform a 5 s horizontal wheel/scroll sweep via CDP trusted input, sample `requestAnimationFrame` timings in the panel webview (reuse `src/lib/perf.ts` ring), assert: ≥58 fps mean, no frame > 50 ms, samples ≥ 200, and the newest card still on the right after scrolling back. Capture long tasks if any and report.
- This complements P4-1's functional cap; M14 manual “real trackpad feels smooth” remains user-run.

**Commit** `test: P4 100-card scroll performance evidence`.

---

### Task P4-6: P4 hardening + backlog (pick only cheap, high-value)

Choose a bounded subset, each with tests/evidence, no structural rewrites:
- Remove the perf sampler from release builds if it still ships (`#[cfg(debug_assertions)]`/feature gate) — verify release bytes/strings.
- Same-id fly/fall render coalescing (P2 backlog): a repeated animation for the same id must not drop the final landed state; test at the render-plan level.
- Preflight (“fly overlay not yet hidden”) one-frame flash: hide the overlay before requesting GDI freeze; verify via trace/log.
- Corrupt-thumbnail self-heal: decode-check the cached preview when considered fresh; regenerate on failure.
- Thumbnail content-hash/versioned URL already partially fixed by `updatedAt`; document remaining edge.
Do not attempt HEIC/EXIF/GIF fidelity, right-click context menu, or Tendedero language ports in P4 (post-V1 backlog).

**Commit** `fix: P4 hardening backlog` with a short note in the report.

---

### Task P4-7: P4 acceptance, evidence, README, 0.4.0, tag prep

- `docs/acceptance/P4-core.md`: environment, gates, M2/M3/M4/M14/M15 matrix (programmatic PASS vs user-reported vs manual), 100-card perf table, fullscreen/hotkey/autostart evidence, known limitations, manual checklist (real dual-monitor capture, real fullscreen game/presentation, taskbar moved top/left, 100-card trackpad, QQ-held hotkey, autostart reboot, sound).
- `docs/experiments/P4-multimon-settings.md`: methods, exact numbers, reproduction commands (isolated profiles; no real desktop capture).
- README EN/ZH: P4 status + evidence links + P4 screenshots (app UI only), manual pending statement.
- Bump version to **0.4.0** (package/tauri/Cargo + Cargo.lock), update README/docs indexes, run all gates, commit `docs: P4 acceptance, evidence and 0.4.0`; **no tag from implementers** — the Lead runs the final review and tags `v0.4.0-settings`.

---

## Manual acceptance checklist (user-run; goes into `docs/acceptance/P4-core.md`)

1. 双屏（100%+150%）各截一张图：截图出现在对应屏幕、卡片无错位；150% 屏卡片/夹子/文字清晰。
2. 把任务栏移到屏幕顶部或左侧：线贴住工作区顶、不被任务栏遮挡；重启后仍正确。
3. 打开全屏游戏/演示（或全屏视频）：线在 1 s 内隐藏、不被全屏内容盖住；退出全屏 1 s 内恢复；托盘打开「全屏应用上」并在全屏时手动唤出 → 线保持显示。
4. 保留数量设为 25/50/100 后连续截图：线只保留最新 N 张，旧卡片下线但**文件仍在截图目录**；滚轮翻阅不卡顿、最新在右侧。
5. 先启动占用 `Ctrl+Alt+T` 的程序（如 QQ）再启动 Hangline/改键：提示冲突、旧热键仍可用、改键窗口可重试成功；释放后改键成功。
6. 托盘打开「开机启动」→ 注销/重启后 Hangline 自动启动；关闭后不再自启。
7. 声音开/关：复制/取下线提示音符合设置。
8. 改尺寸小/中/大、亮/暗主题、缩放窗口：卡片与面板无裁剪、无抖动。
9. 拔掉/扩展一块显示器（或改主屏）：线迁移到可用屏幕，卡片不丢失。
10. 退出重启：保留状态按存在性恢复；P3 标注/裁剪后的文件与缩略图仍正确。

## Self-review checklist (controller)

- Spec coverage: §3.2/§4.6 fullscreen; §3.6/§4.8 retention; §3.9 multi-monitor/DPI; §3.5 M15/autostart/sound; M2/M3/M4/M14/M15; §8 P4 deliverables.
- No placeholders: each task names files/interfaces/tests/commands/commit.
- Consistency: `FullscreenState`, `should_suppress`, `retained_len`, `keepOnLine` limits, evidence file names.
- Safety: no user-file deletion in retention/fullscreen paths; fullscreen never sticks; no real desktop capture.
