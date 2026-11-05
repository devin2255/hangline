# P4 multi-monitor, full-screen and settings evidence (P4-1..P4-6)

Programmatic placement/DPI evidence for M2/M3 on the real dual-monitor setup. M2's “one real
screenshot on each screen” and M3's real top/left taskbar remain **MANUAL-PENDING**: this run
uses synthetic neutral fixtures and the machine's real (bottom) taskbars, and only ever captures
the panel webview itself (never the desktop or another application's pixels).

- Run date: 2026-11-04 (Asia/Shanghai)
- OS: Windows 11 Home 10.0.26300
- Build: debug `hangline.exe` + Vite dev server on 127.0.0.1:5173 + `HANG_CDP_PORT=9229`
- Profile: isolated `%TEMP%\hangline-p4-display\localappdata\Hangline`, two synthetic 640×360
  cards, `cardSize=medium`, all watchers off, `keepOnLine=fit`
- Evidence: `.superpowers/p4-evidence/display/display-e2e.json` + four panel-only PNGs
  (`panel-dark-100.png`, `panel-dark-150.png`, `panel-light-100.png`, `panel-light-150.png`);
  `.superpowers/` is git-ignored.
- Verdict of the recorded run: **PASS (placement/DPI), 85 checks / 0 failed on an unlocked desktop**
  (`workstationLocked=false`, final run 2026-11-04 15:0x). An earlier locked-desktop run also passed
  85/85; the final unlocked JSON supersedes it.

## Environment

| Monitor | Role | rect (physical) | Work area (taskbar excluded) | Scale | Window DPI |
|---|---|---|---|---|---|
| `\\.\DISPLAY2` | primary | (0, 0) 1920×1080 | (0, 0) 1920×1032 (bottom taskbar) | 100% | 96 |
| `\\.\DISPLAY1` | secondary (left) | (−1920, 0) 1920×1080 | (−1920, 0) 1920×1008 (bottom taskbar) | 150% | 144 |

No 125% display is attached on this machine; the 125% rules are covered by unit tests.

## Method

`node scripts/p4/display-e2e.mjs`:

1. Preflight: refuses to start if another `hangline.exe` holds the single-instance lock; probes the
   workstation lock state; reads both monitors with `scripts/p4/display-probe.ps1`.
2. Launches the debug app with the isolated profile and `HANG_CDP_PORT=9229`; connects to the
   `?window=panel` webview over CDP.
3. Compares the app's `get_monitors` IPC with the independent Win32 enumeration (id, rect, work
   area and scale factor must match).
4. Pins the line (second instance = the documented toggle) and moves the real pointer to each
   monitor's centre with `SetCursorPos`; the pinned line follows after the spec's 500 ms dwell
   (200 ms poll).
5. For each monitor and theme, waits for the real window rectangle (`GetWindowRect` of the
   `Tauri Window` titled `Hangline`, measured from a Per-Monitor-V2 PowerShell process) to equal the
   work-area origin and the expected physical size, then measures the webview over CDP:
   `innerWidth`/`innerHeight`, `devicePixelRatio`, `get_scale_factor` IPC, `data-theme`,
   `data-card-size`, and every card's DOM rect.
6. Maps each card DOM rect to physical pixels (`window origin + CSS px × dpr`) and asserts it lies
   inside the target monitor; predicts the slot layout from spec §3.3 and asserts the measured CSS
   position matches it; asserts the card AABB and take-down chrome are inside the viewport.
7. Switches the theme via `set_setting("theme", …)` and repeats on both monitors; a theme switch
   must not change the window origin or size. Captures a panel-only screenshot per case
   (`Page.captureScreenshot`).

The expected physical height is `round(210 dp × scale)` (medium card size: `photoMaxHeight` 104 +
106 chrome). Width is the work-area width, unchanged: the panel spans the monitor's work area and
never overhangs it.

## Measured results

All values are physical pixels unless marked CSS/dp.

| Theme | Monitor | Work area | Expected origin | Measured `GetWindowRect` | Expected size | CSS viewport | dpr / IPC scale | Window DPI | Result |
|---|---|---|---|---|---|---|---|---|---|
| dark | `DISPLAY2` 100% | (0, 0) 1920×1032 | (0, 0) | (0, 0) | 1920×210 | 1920×210 | 1.0 / 1.0 | 96 | PASS |
| dark | `DISPLAY1` 150% | (−1920, 0) 1920×1008 | (−1920, 0) | (−1920, 0) | 1920×315 | 1280×210 | 1.5 / 1.5 | 144 | PASS |
| light | `DISPLAY1` 150% | (−1920, 0) 1920×1008 | (−1920, 0) | (−1920, 0) | 1920×315 | 1280×210 | 1.5 / 1.5 | 144 | PASS |
| light | `DISPLAY2` 100% | (0, 0) 1920×1032 | (0, 0) | (0, 0) | 1920×210 | 1920×210 | 1.0 / 1.0 | 96 | PASS |

Per case, additionally:

- The undecorated window's client rectangle equals its outer rectangle at the same origin
  (`1920×210` at 100%, `1920×315` at 150%).
- Two fixture cards render; their thumbnail images decode (432×243 preview, 16:9).
- Card DOM rects mapped to physical pixels (first / second slot):
  - 100%: slots `(798, 30) 150×99` and `(972, 30) 150×99`; card AABBs `(801, 42) 147×89` and
    `(975, 42) 147×89`.
  - 150%: slots `(−1203, 35) 225×147` and `(−942, 35) 225×147`; card AABBs `(−1198, 52) 220×134`
    and `(−937, 53) 220×132`.
  All lie inside the monitor's work area with room to spare; the card bottom is ~130 CSS px in a
  210 CSS px panel, so nothing is clipped at 150%.
- The measured slot position matches the spec layout (`cardWidth 150`, gap 24, rope sag
  `min(30, width × 0.018)`) with a maximum offset of 0.01 CSS px; the panel origin adds no offset
  (window origin = work-area origin exactly).
- A theme switch leaves the window rectangle unchanged at both monitors
  (dark `0,0 1920×210` → light `0,0 1920×210`; dark `−1920,0 1920×315` → light `−1920,0 1920×315`).
- The line height is `210 dp` in CSS on both monitors and `210 / 315` physical pixels at
  100% / 150% (medium card size at that scale).
- Observed follow latency to the settled rectangle: 1.48–2.13 s from `SetCursorPos`, which includes
  the spec's 500 ms dwell, the 200 ms follow poll, the window move and the DPI change.

## Unit tests added (Rust)

`src-tauri/src/panel.rs`:

- `panel_origin_and_pixel_size_follow_the_work_area_across_taskbars_and_dpi` — seven cases:
  bottom/top/left taskbars on 100%, a negative-origin 150% secondary (bottom/top/left taskbar),
  and a negative-origin 125% monitor above the primary. Each asserts origin = work-area top-left,
  width = work-area width, height = `panel_height(card_size) × scale` rounded, and that the
  physical height maps back to the spec's dp height within half a device pixel.
- `monitor_signature_tracks_a_taskbar_move_with_an_unchanged_screen_rect` — moving the taskbar
  changes only the work area; the signature must change so the reveal loop re-applies the origin.
- `display_change_migrates_the_panel_when_its_monitor_disappears` — a window rectangle that still
  claims the vanished 150% monitor resolves to no monitor, the primary is the fallback target, and
  the panel geometry becomes the primary's `(0, 0) 1920×210`; the cursor path has the same
  fallback.

Existing tests already covered the pixel-size/DPI table, negative origins, `monitor_at_point`
gaps, top-edge per-monitor hits and the `MonitorFollow` debounce/display-change state.

## Taskbar (M3)

Real taskbar relocation is not safe to automate on the user's desktop, so M3 is split:

- **Programmatic**: the panel geometry never reads the full monitor `rect`; origin and width come
  from `work_area` (unit-tested for top/left taskbars, negative origins and 125/150% scales). The
  real run confirms the work-area origin on both monitors (bottom taskbar: work-area heights
  1032 px and 1008 px are correctly excluded from the panel height, which is card-size driven).
- **MANUAL-PENDING**: move the taskbar to the top and then the left of a monitor, restart Hangline,
  and confirm the line hugs the work-area top and is never covered. No PASS is claimed for this.

## M2 manual item

M2's acceptance wording is “take one real screenshot on each screen and confirm it lands on the
correct screen without offset”. This task proves the placement/scale half programmatically
(panel origin/size and card rects per monitor, both themes). The real screenshot-capture half is
**MANUAL-PENDING** because the evidence harness must not capture the user's desktop. The
functional capture-path placement for real captures is covered by P1/P3 evidence and the manual
checklist.

## Reproduction

```powershell
# one Hangline at a time; the single-instance plugin would otherwise absorb the launch
Get-Process hangline -ErrorAction SilentlyContinue | Stop-Process -Force
pnpm dev                                   # terminal 1, Vite on 127.0.0.1:5173
cd src-tauri; cargo build                   # debug exe
cd ..; node scripts/p4/display-e2e.mjs      # terminal 2
```

On a locked workstation (QUNS=1, `LockApp` foreground) the harness refuses a clean PASS; for a
placement-only run with the lock recorded use `node scripts/p4/display-e2e.mjs --allow-locked`.
Evidence is written to `.superpowers/p4-evidence/display/` (git-ignored). The script never captures
the desktop: all PNGs are `Page.captureScreenshot` output of the panel webview.

## Environment note for the recorded run

The **final** recorded run was unlocked (`workstationLocked=false`, 2026-11-04 15:03–15:04 local);
its JSON is the one cited above (85/85, `verdict: PASS`). An **earlier** run was captured while the
workstation was locked (`QUNS=1`, foreground `LockApp` / “Windows 默认锁屏界面”, `LogOnUI` running)
and also passed 85/85 — placement APIs and the panel-webview CDP do not need the foreground, and
the app log shows the pointer dwell/follow (`panel: pointer dwelt on … for 500 ms; following it`).
A subsequent clean-rerun attempt at 14:07 failed because the user was moving the mouse
(`SetCursorPos` not observable); that failed JSON was overwritten by the final clean PASS above.
M2/M3 manual items remain pending in either case.


---

# P4-2 full-screen suppression evidence (M4)

- Run: 2026-11-04, **unlocked** workstation, debug app + Vite 5173 + `HANG_CDP_PORT=9228`, isolated
  `%TEMP%\hangline-p4-fullscreen` profile with neutral cards; evidence
  `.superpowers/p4-evidence/fullscreen/fullscreen-e2e.json` (git-ignored).
- Verdict: **PASS 42/42** checks.
- Numbers: synthetic borderless topmost full-screen window (also sets QUNS=2 BUSY) → line tuck
  reason `fullscreen` and panel hidden in 643 / 995 ms; suppression log
  `reason=queryUserNotificationState, quns=2, foreground_covers=true`; a new item ingested while
  full-screen stays hidden (2 cards in the webview, no reveal); closing the helper restores the
  line in **779 ms** (earlier precise runs 835/838 ms) against the M4 ≤1 s bound; override with
  `showOverFullScreen` + pinned toggle works (110/82 ms; pinned toggle 124 ms); plain maximized
  framed windows on the primary and on the secondary, a framed window exactly covering the
  secondary, and a borderless-maximized window covering the secondary do **not** suppress (style
  classifier: no `WS_CAPTION`/`WS_THICKFRAME`, not `SW_SHOWMAXIMIZED`).
- Poller: 100 ms cadence, 800 ms decision-bit stability (≤900 ms worst before the controller sees
  the exit), probe failures fall back to Normal, panic/stop fail-safes, `fullscreen-state` event.
- Measurement origin: the 779 ms M4 number is measured from the helper's `.closed` marker (written
  after the native full-screen window is destroyed) to the panel's `data-revealed=true` DOM flip;
  the JSON's `restoreFromStopRequestMs` (1042 ms) additionally includes the helper stop request
  and window-destruction time, so it is not the M4 metric.
- Manual/MANUAL-PENDING: real D3D exclusive games, presentation mode, DRM/protected content,
  Space/Alt+Tab hand-off, capture-overlay over a real game.

# P4-1/P4-5 retention and 100-card performance evidence (M14)

- Retention unit tests (Rust, 6 new): 101st add with Limit(100) drops the oldest; reduce via the
  real `set_setting`/tray path trims immediately, persists settings-then-state and bumps the
  revision once; `Fit` stays unbounded; startup enforces the persisted cap; dropped files are never
  deleted; kept rows keep id/path/order.
- `scripts/p4/fixture-100.mjs`: isolated profile with 100 synthetic 640×360 cards, `keepOnLine=100`,
  medium cards. `node scripts/p4/perf-100.mjs` (debug + Vite + `HANG_CDP_PORT=9227`):
  **PASS** — 302 rAF samples over a 5.07 s trusted CDP wheel sweep, mean **60.00 fps**, p5
  **59.52 fps**, worst **17.0 ms**, long frames (>20 ms) **0**, longest burst **0**; the dev-only
  per-frame offset trace proves real scroll movement (120 moves, 29/30/30/31 per quarter, mid
  offset 20 > start 0; capacity max offset reported as information only); after scrolling back the
  newest card is on the right (offset 0, range 20..100).
- `set_setting {key:"keepOnLine", value:25}` through the real IPC: state **100→25**, revision
  **1→2 exactly once**, kept ids exactly the newest 25 with unchanged paths, dropped exactly the
  oldest 75, **75/75 dropped files still exist**, `state.json`/`settings.json` persisted; a short
  post-trim sweep still scrolls.
- A first run on an active desktop showed 55 fps / 23 long frames (external system load) and
  **failed** the harness; the clean rerun above passed. Evidence: `.superpowers/p4-evidence/` —
  `perf-100.json`, `p4-100-cards-mid-scroll.png`, `p4-25-cards-newest-right.png` (git-ignored).
- Manual/MANUAL-PENDING: real trackpad feel, 100-card desktop with other apps running.

# P4-4 hotkey conflict, autostart and sound evidence (M15 + P4 deliverables)

- `scripts/p4/hotkey-conflict-e2e.mjs` + helper `RegisterHotKey` process: **59/59 PASS** — a real
  startup conflict on `Ctrl+Alt+T` is logged and kept in settings while capture still registers;
  a same-value retry re-registers after the helper releases; a change to a helper-owned
  `Ctrl+Alt+Shift+F9` is rejected in **105 ms** with the old hotkey still owned and `settings.json`
  unchanged (rollback logged); retry after release succeeds in **42 ms**, old released/new owned,
  no duplicate registration; the real shortcut page (CDP) shows both accelerators and an inline
  conflict, captures `Ctrl+Alt+Shift+F5`, and stays open.
- `scripts/p4/autostart-e2e.mjs` + `registry-state.ps1`: **55/55 PASS** and the spaced-path run
  **55/55 PASS** (`%TEMP%\hangline-p4-autostart\Hangline Test\hangline.exe`): the `Run` value is
  normalized to `"<exe>" <args>` and parsed with `CommandLineToArgvW`; enable/disable/reconcile
  both directions; stale exe path repaired with args preserved; a Task-Manager-disabled
  `StartupApproved\Run` byte `0x03` is detected and left disabled (logged); tray checked state
  reflects the effective registration and the first click removes a manual entry; registry
  snapshots restored, independent `reg query` shows no Hangline Run value in HKCU or HKLM.
- `scripts/p4/autostart-hkcu-probe.mjs`: **15/15 PASS** via a normal-user token (`runas`), proving
  the spec's `HKCU\...\Run` location even though this agent session is elevated.
- Sound: pure `should_play` gate (missing state = on) + unit test; `sound=false` logs the skip and
  never reaches `MessageBeep`; `sound=true` logs `sound: played the discarded beep` at runtime.
  Audible feedback is **MANUAL-PENDING** (no WAV assets by design).
- Deviation recorded: the startup hotkey-conflict prompt has no balloon; the app shows an inline
  shortcut-window error plus a tray hint row instead.

# P4-6 hardening evidence

- Release hygiene: debug instrumentation (`__HANG_*` globals, frame ring, offset/trace) is gated on
  `import.meta.env.DEV` and lazily allocated inside the dev effect; `pnpm build` runs
  `scripts/check-release-instrumentation.mjs` (15 markers + `--self-test`) and reports
  **0 of 15** markers in `dist`; the previous stale bundle failed the check (exit 1), proving it
  catches the leak. Debug/Vite pages still install the instrumentation (49 frame / 50 offset
  samples observed). Evidence: `.superpowers/p4-evidence/release-instrumentation.txt`.
- Same-id fly/fall final state: new pure `flyPlan.ts` (payload-derived render key + merge rules),
  `Fly.tsx` renders keyed entries and removes exactly the finished copy; 8 tests (identical re-emit
  reuses the running copy, changed payload remounts, a fall during the fly handoff is no longer
  swallowed, superseded fading copies removed, final landed state kept).
- Capture preflight: `request_capture` suspends the line, hides the pre-warmed fly overlay, waits
  for both windows to be off screen (bounded wait, abort on still-visible), then GDI-grabs; logs
  `capture: fly overlay preflight` / `capture: preflight complete`; 4 unit tests; runtime log
  assertion is honest BLOCKED (no live freeze captured).
- Corrupt thumbnail self-heal: a mtime-fresh but undecodable preview is regenerated in place once
  (atomic writer); failure returns `None` with no temp residue; 5 tests including zero-byte/text/
  truncated previews and startup backfill. Startup backfill now fully decodes 100 fresh previews
  (~13 s observed for 100 synthetic cards on this machine) — recorded cost, see limitations.
