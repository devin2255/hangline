use std::os::windows::process::CommandExt;

use tauri::Manager;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE};

mod app_state;
mod actions;
mod annotate;
mod autostart;
mod capture;
mod clipboard_io;
mod dedupe;
mod devtools;
mod dragout;
mod editor_window;
mod fly;
mod folder_picker;
mod fullscreen;
mod geometry;
mod hittest;
mod hotkeys;
mod i18n;
mod ipc;
mod item;
// Wave 2.5: public so the binary crate can run the offline `--ocr-probe <dir>` evaluation probe
// without starting Tauri (see `main.rs` and `ocr::run_probe`).
pub mod ocr;
mod panel;
mod retention;
mod reveal;
mod search_window;
mod settings;
mod shortcut_window;
mod sound;
mod store;
mod thumbnails;
mod tray;
mod trash;
mod trim;
mod updater;
mod video;
mod watchers;

use dragout::{after_drag_action, AfterDragAction, DragEffect};
use geometry::PhysicalRect;

/// Updates the card rects used by the 30 Hz click-through hit-test.
///
/// The frontend reports physical pixels relative to the panel webview's client area. The first rect
/// is logged too, so a wheel step (offset change) can be checked against the physical hit area
/// without exposing the internal hit-rect store (Task 14 runtime evidence).
#[tauri::command]
fn set_hit_rects(rects: Vec<PhysicalRect>, state: tauri::State<'_, hittest::HitRects>) {
    match rects.first() {
        Some(first) => log::debug!(
            "panel hit rects updated: {} rect(s), first at ({}, {}) {}x{}",
            rects.len(),
            first.x,
            first.y,
            first.w,
            first.h
        ),
        None => log::debug!("panel hit rects updated: 0 rect(s)"),
    }
    state.set(rects);
}

/// Returns the panel's DPI scale factor (physical pixels per logical pixel).
#[tauri::command]
fn get_scale_factor(app: tauri::AppHandle) -> Result<f64, String> {
    let panel = app
        .get_webview_window(panel::LABEL)
        .ok_or_else(|| "panel window is not open".to_string())?;
    panel.scale_factor().map_err(|err| err.to_string())
}

/// Native message box seam for the W1.7 explanations (no new dependency: `MessageBoxW`).
///
/// Callers pass an owner-less box (the app has no focused window, or the action runs on a worker
/// thread, exactly like the capture error box) and must run it on a thread that may block until the
/// user answers — never the main event loop.
pub(crate) fn message_box(
    title: &str,
    body: &str,
    flags: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let body: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both buffers are NUL-terminated and outlive the call; the box is owner-less on
    // purpose (the same pattern as the capture error box in `capture::show_capture_error`).
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            flags,
        )
    }
}

/// Starts Hangline's own freeze-frame region capture (design spec §3.2 A, `Ctrl+Alt+S`).
///
/// The command returns once the frozen frame is saved and the overlay window is up; the selection
/// is confirmed later by [`finish_capture`] or discarded by [`cancel_capture`]. Reentry-safe: a
/// request while a capture is already in flight is a logged no-op, not an error (a hotkey burst
/// must not open two overlays).
///
/// Deliberately `async`: window creation dispatches to the Tauri event loop and waits for it, so a
/// synchronous command (which runs on the main thread) would deadlock creating the overlay.
#[tauri::command]
async fn request_capture(app: tauri::AppHandle) -> Result<(), String> {
    capture::request_capture(&app).map_err(|error| error.to_string())
}

/// Capture-overlay handshake: Rust emits `capture-ready` once the overlay frontend listens.
///
/// The overlay webview needs a moment to load after the window is created; emitting the frozen
/// frame path/geometry only after this command guarantees the listener exists (mirrors the panel's
/// `report_panel_ready` handshake). Async for the same event-loop reason as [`request_capture`].
#[tauri::command]
async fn report_capture_ready(app: tauri::AppHandle) -> Result<(), String> {
    capture::report_overlay_ready(&app).map_err(|error| error.to_string())
}

/// Confirms the overlay drag selection and hangs the capture on the line (Task 16).
///
/// `rect` is in virtual-desktop physical pixels (CSS selection × `devicePixelRatio` + the virtual
/// origin the `capture-ready` payload carries); Rust clamps it to the frozen frame. Async because
/// cleanup destroys/moves windows through the event loop.
///
/// W1.5 reply contract: `{ saved, copiedFallback }` (serde camelCase). `saved` means the file was
/// written and hung; `copiedFallback` means the save failed and the crop was copied to the
/// clipboard instead, so the panel must not report a lost capture.
#[tauri::command]
async fn finish_capture(
    app: tauri::AppHandle,
    rect: PhysicalRect,
) -> Result<capture::CaptureOutcome, String> {
    capture::finish_capture(&app, rect).map_err(|error| error.to_string())
}

/// Cancels the capture overlay (Esc / right click): no file, no item, line restored (Task 16).
#[tauri::command]
async fn cancel_capture(app: tauri::AppHandle) -> Result<(), String> {
    capture::cancel_capture(&app);
    Ok(())
}

/// Called by the panel frontend once after its first layout.
///
/// The panel window is created hidden (design spec 4.2), so the first frame is painted before
/// the window becomes visible (no first-paint flash). The reveal controller is told about the
/// startup show so its empty-line timeout and pointer polling start from the same moment; toggles
/// requested before this handshake are queued by the controller and flushed now (Task 15 READY
/// gating), so an early tray/hotkey/second-instance action is never lost or replayed twice.
/// Any other label is rejected.
#[tauri::command]
fn report_panel_ready(app: tauri::AppHandle, label: String) -> Result<(), String> {
    if label != panel::LABEL {
        return Err(format!("unknown panel label: {label}"));
    }
    if fullscreen::is_suppressed() {
        // The app was launched while a full-screen app owns the screen: paint ready, but keep the
        // window hidden. The reveal controller records the gate and shows the line only after the
        // full-screen app is gone, so no line flashes over the game.
        log::info!(
            "panel: frontend reported ready during full-screen suppression; keeping the panel hidden"
        );
    } else {
        log::info!("panel: frontend reported ready, showing the panel");
        panel::show(&app);
    }
    reveal::on_ready_request(&app);
    tray::sync_state(&app);
    Ok(())
}

/// Marks whether the panel frontend is inside a menu/press/drag gesture.
///
/// While busy the reveal controller must not tuck the line (design spec §3.4: an open menu,
/// press or drag keeps the line visible). The frontend sets it on pointerdown and clears it on
/// pointerup/cancel/blur; a native drag-out is detected separately by `dragout::is_dragging`.
#[tauri::command]
fn set_panel_busy(busy: bool) {
    reveal::set_busy(busy);
}

/// Returns the system double-click interval in milliseconds (`GetDoubleClickTime`).
///
/// The panel delays a single-card copy by this interval so the first click of a double-click
/// never pollutes the clipboard (design spec §3.1).
#[tauri::command]
fn get_double_click_interval_ms() -> u32 {
    // SAFETY: GetDoubleClickTime reads a user preference and has no arguments.
    unsafe { GetDoubleClickTime() }
}

/// Looks up one item by its opaque id (the commands below never mutate through a stale snapshot).
fn item_by_id(app: &tauri::AppHandle, id: &str) -> Result<item::Item, String> {
    app.state::<app_state::AppState>()
        .item(id)
        .ok_or_else(|| format!("item not found: {id}"))
}

/// How long `after_drag` waits before its first existence check, in milliseconds.
///
/// A shell drop target (Explorer, the Recycle Bin) completes its move/delete just after OLE reports
/// the effect, so probing immediately could still see the source file. 600 ms is the spec's settle
/// window; the command itself returns before the wait because the sleep runs on a background
/// thread.
const AFTER_DRAG_PRUNE_DELAY: std::time::Duration = std::time::Duration::from_millis(600);

/// Re-check cadence while a `MOVE` source file still exists (slow/cross-volume shell move).
///
/// The OLE effect can arrive before Explorer has finished removing the source path; polling gives
/// the shell time to complete instead of keeping a card whose file is already on its way out.
const AFTER_DRAG_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Maximum number of re-checks for a `MOVE` whose source still exists (≈2 s after the first probe).
///
/// The wait is bounded on purpose: beyond it, the shell move is simply still pending, the card stays
/// (a readable file is never hidden) and the folder watcher prunes later if the file disappears.
const AFTER_DRAG_MAX_RETRIES: u32 = 5;

/// Name of the thread that waits out [`AFTER_DRAG_PRUNE_DELAY`] and prunes.
const AFTER_DRAG_THREAD_NAME: &str = "hangline-after-drag";

/// Final shape of the post-drag wait, before the command logs and prunes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterDragOutcome {
    /// The item left the line while waiting; nothing to do.
    ItemGone,
    /// The card's file disappeared: prune.
    Prune,
    /// The effect and existence agree that the card stays on the line.
    Done,
    /// A `MOVE` whose source still exists after the retry budget: keep the card and log it.
    StillPending,
}

/// Resolves the item a `drag-ended` event refers to; an unknown id is a logged no-op.
///
/// The line may have changed while the frontend held the id (another path removed the card), so
/// an id that is no longer present is normal, not an error.
fn after_drag_target(state: &app_state::AppState, id: &str) -> Option<item::Item> {
    let target = state.item(id);
    if target.is_none() {
        log::info!("after_drag: item {id} is no longer on the line");
    }
    target
}

/// Parses the effect string carried by `drag-ended`; an unknown value is logged and treated as
/// [`DragEffect::None`] (the conservative "no prune signal unless the file is gone" policy).
fn parse_drag_effect(name: &str) -> DragEffect {
    match DragEffect::from_name(name) {
        Some(effect) => effect,
        None => {
            log::warn!("after_drag: unknown drag effect {name:?}; treating it as none");
            DragEffect::None
        }
    }
}

/// Runs the effect-driven wait policy: one initial probe, then bounded `MOVE` retries.
///
/// The decision comes from [`dragout::after_drag_action`]: a `MOVE` whose source still exists is
/// re-probed up to [`AFTER_DRAG_MAX_RETRIES`] times at [`AFTER_DRAG_RETRY_INTERVAL`] so a slow shell
/// move still drops its card; every effect whose file disappeared prunes; a `COPY`/`LINK`/`NONE`
/// whose file remains is a no-op. `probe` returns `None` when the item left the line meanwhile.
fn wait_for_after_drag(
    effect: DragEffect,
    interval: std::time::Duration,
    max_retries: u32,
    mut probe: impl FnMut() -> Option<bool>,
) -> AfterDragOutcome {
    let Some(exists) = probe() else {
        return AfterDragOutcome::ItemGone;
    };
    let mut action = after_drag_action(effect, exists);
    let mut retries = 0;
    while action == AfterDragAction::KeepWaiting && retries < max_retries {
        std::thread::sleep(interval);
        retries += 1;
        let Some(exists) = probe() else {
            return AfterDragOutcome::ItemGone;
        };
        action = after_drag_action(effect, exists);
    }
    match action {
        AfterDragAction::Prune => AfterDragOutcome::Prune,
        AfterDragAction::KeepWaiting => AfterDragOutcome::StillPending,
        AfterDragAction::Done => AfterDragOutcome::Done,
    }
}

/// Applies one post-drag outcome: re-check the item before logging, prune only when needed.
fn apply_after_drag(app: &tauri::AppHandle, id: &str, effect: DragEffect, outcome: AfterDragOutcome) {
    let state = app.state::<app_state::AppState>();
    match outcome {
        AfterDragOutcome::ItemGone => {
            log::info!("after_drag: item {id} is no longer on the line");
        }
        // The item may have left the line while the retries ran; never log a stale "still has".
        AfterDragOutcome::StillPending => {
            if state.item(id).is_some() {
                log::info!(
                    "after_drag: item {id} still has its file after the retry window \
                     (effect={}); the shell move is still pending, keeping the card (watcher \
                     events will prune later)",
                    effect.as_str()
                );
            } else {
                log::info!("after_drag: item {id} is no longer on the line");
            }
        }
        AfterDragOutcome::Done => {
            if state.item(id).is_some() {
                log::debug!(
                    "after_drag: no card dropped for {id} (effect={}); its file still exists",
                    effect.as_str()
                );
            } else {
                log::info!("after_drag: item {id} is no longer on the line");
            }
        }
        AfterDragOutcome::Prune => match state.prune() {
            Ok(removed) if !removed.is_empty() => {
                log::info!(
                    "after_drag: dropped {} card(s) whose files are gone: {removed:?}",
                    removed.len()
                );
                // prune already persisted and bumped the revision; emit that snapshot only.
                let _ = app_state::emit_state(app);
            }
            Ok(_) => {
                // The item vanished while the prune ran; nothing was removed or persisted.
                log::info!("after_drag: item {id} is no longer on the line");
            }
            Err(error) => {
                log::error!("after_drag: prune failed: {error:#}");
                // prune mutates in memory before persisting; publish so the UI matches.
                let _ = app_state::emit_state(app);
            }
        },
    }
}

/// Runs one `after_drag` policy pass after `delay`; the caller owns the background thread.
///
/// Factored separately from the command so the wait policy can be exercised with short delays in
/// tests (via [`wait_for_after_drag`]) without a Tauri application handle.
fn after_drag_with_delay(
    app: tauri::AppHandle,
    id: String,
    effect: DragEffect,
    delay: std::time::Duration,
) {
    std::thread::sleep(delay);
    let state = app.state::<app_state::AppState>();
    let mut probes = 0u32;
    let outcome = wait_for_after_drag(
        effect,
        AFTER_DRAG_RETRY_INTERVAL,
        AFTER_DRAG_MAX_RETRIES,
        || {
            probes += 1;
            let exists = state.item(&id).map(|item| item.path.exists());
            if probes > 1 {
                log::debug!(
                    "after_drag: retry {}/{} for {id} (effect={}): file exists={:?}",
                    probes - 1,
                    AFTER_DRAG_MAX_RETRIES,
                    effect.as_str(),
                    exists
                );
            }
            exists
        },
    );
    drop(state);
    apply_after_drag(&app, &id, effect, outcome);
}

/// Applies the effect-driven post-drag policy for a native drag-out (Task 13).
///
/// The shell, not Hangline, performs the drop: a folder/desktop drop is an Explorer MOVE and a
/// Recycle Bin drop is a shell delete, so the source path disappears without this app touching the
/// file. After [`AFTER_DRAG_PRUNE_DELAY`] the command runs [`dragout::after_drag_action`] and, for
/// a `MOVE` whose source still exists, re-checks it up to [`AFTER_DRAG_MAX_RETRIES`] times (a slow
/// shell move can report the effect before the path disappears). A prune removes the cards whose
/// files are gone (persisting and bumping the revision) and broadcasts only when something was
/// removed. It returns immediately so the IPC thread never blocks, it is safe to call for every
/// drag ending, including cancels, and an unknown id is a no-op before any thread is spawned.
#[tauri::command]
fn after_drag(app: tauri::AppHandle, id: String, effect: String) -> Result<(), String> {
    let effect = parse_drag_effect(&effect);
    let known = {
        let state = app.state::<app_state::AppState>();
        after_drag_target(&state, &id).is_some()
    };
    if !known {
        return Ok(());
    }

    std::thread::Builder::new()
        .name(AFTER_DRAG_THREAD_NAME.to_owned())
        .spawn(move || after_drag_with_delay(app, id, effect, AFTER_DRAG_PRUNE_DELAY))
        .map_err(|error| format!("failed to start the after-drag prune thread: {error}"))?;
    Ok(())
}

/// Returns the panel window as the clipboard owner, when its native handle is available.
///
/// `OpenClipboard(NULL)` associates the clipboard with the calling task and is known to fail
/// intermittently with `ERROR_CLIPBOARD_NOT_OPEN`. Passing the panel HWND gives Windows a stable
/// owner for the open/empty/set/verify/close sequence. If the window is gone (shutdown, a failed
/// creation) the caller falls back to the NULL owner, which is logged because it is the flaky path.
fn clipboard_owner(app: &tauri::AppHandle) -> Option<HWND> {
    let Some(panel) = app.get_webview_window(panel::LABEL) else {
        log::warn!("clipboard: panel window is not open; falling back to a NULL clipboard owner");
        return None;
    };
    match panel.hwnd() {
        Ok(hwnd) => Some(hwnd),
        Err(error) => {
            log::warn!(
                "clipboard: cannot resolve the panel HWND ({error}); \
                 falling back to a NULL clipboard owner"
            );
            None
        }
    }
}

/// Copies one card: images publish PNG + DIBV5 + HDROP, recordings only HDROP (spec §4.7).
///
/// The file itself is only read. A successful return makes the frontend show the `Copied` pill.
#[tauri::command]
fn copy_item(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let item = item_by_id(&app, &id)?;
    clipboard_io::copy_for_kind(item.kind, &item.path, clipboard_owner(&app)).map_err(|error| {
        log::warn!(
            "copy_item: cannot copy {} ({:?}): {error:#}",
            item.path.display(),
            item.kind
        );
        error.to_string()
    })?;
    // Design spec §3.5 "声音": copy/discard feedback, gated by the `sound` setting.
    sound::copied(&app);
    Ok(())
}

/// Copies one card's file as a file drop only (`CF_HDROP`, no PNG/DIBV5), for "copy as file".
///
/// This is the design-spec §4.7 fallback `copy_item` already uses for recordings: chat apps that
/// paste `CF_HDROP` as the file itself instead of the pixels get exactly that. The clipboard writer
/// is the same verified path ([`clipboard_io::copy_file`]); a failure returns its diagnostic error
/// and the panel shows no `Copied` pill.
#[tauri::command]
fn copy_item_as_file(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let item = item_by_id(&app, &id)?;
    clipboard_io::copy_file(&item.path, clipboard_owner(&app)).map_err(|error| {
        log::warn!(
            "copy_item_as_file: cannot copy {} as a file: {error:#}",
            item.path.display()
        );
        error.to_string()
    })?;
    // Same copy feedback as `copy_item` (design spec §3.5 "声音").
    sound::copied(&app);
    Ok(())
}

/// Opens one card's file with its default application (`ShellExecuteW "open"`).
#[tauri::command]
fn open_item(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let item = item_by_id(&app, &id)?;
    trash::open_path(&item.path).map_err(|error| {
        log::warn!(
            "open_item: cannot open {}: {error:#}",
            item.path.display()
        );
        error.to_string()
    })
}

/// Returns the single command-line argument that makes Explorer reveal `path`.
///
/// Explorer expects `/select,` immediately followed by the path. A Windows file name can never
/// contain `"`, so wrapping the path in quotes is the whole escaping: spaces, commas, parentheses
/// and non-ASCII characters stay inside the quoted path and the `/select,` prefix stays outside.
/// The argument is passed verbatim to the command line (`raw_arg`), so Explorer receives exactly
/// `/select,"<path>"` (design spec §4.4 `show_in_explorer`).
fn explorer_select_argument(path: &std::path::Path) -> String {
    format!("/select,\"{}\"", path.display())
}

/// Whether `path` can be revealed in Explorer: it must be an existing regular file.
///
/// Explorer runs as a separate process, so without this check a deleted file would only open a
/// folder with nothing selected. The command returns a concrete, path-bearing error instead.
fn validate_explorer_target(path: &std::path::Path) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!("cannot reveal a non-file: {}", path.display())),
        Err(error) => Err(format!(
            "cannot reveal {}: the file is not accessible: {error}",
            path.display()
        )),
    }
}

/// Reveals one card's file in Explorer (`explorer.exe /select,"<path>"`).
///
/// The file is validated first (a card can outlive its file for a few polls), then Explorer is
/// spawned and never waited on: the shell owns its lifetime and the IPC thread must not block.
#[tauri::command]
fn show_in_explorer(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let item = item_by_id(&app, &id)?;
    validate_explorer_target(&item.path).map_err(|error| {
        log::warn!("show_in_explorer: {error}");
        error
    })?;
    let argument = explorer_select_argument(&item.path);
    std::process::Command::new("explorer.exe")
        .raw_arg(argument)
        .spawn()
        .map_err(|error| {
            log::warn!(
                "show_in_explorer: cannot launch Explorer for {}: {error}",
                item.path.display()
            );
            format!("cannot open Explorer: {error}")
        })?;
    log::info!(
        "show_in_explorer: revealing {} in Explorer",
        item.path.display()
    );
    Ok(())
}

/// Takes one card off the line, optionally recycling its file.
///
/// `delete_file` is only a request: [`trash::take_down_action`] downgrades it to "off the line"
/// for watched-folder items and for files outside Hangline's own screenshot directory. The card is
/// removed only after a requested recycle succeeded, so a failed recycle never hides the item.
#[tauri::command]
fn take_down(app: tauri::AppHandle, id: String, delete_file: bool) -> Result<(), String> {
    let state = app.state::<app_state::AppState>();
    let Some(item) = state
        .snapshot()
        .items
        .into_iter()
        .find(|item| item.id == id)
    else {
        // The card may already be gone (double click on the cross, a watcher prune): idempotent.
        log::info!("take_down: item {id} is no longer on the line");
        return Ok(());
    };

    if delete_file {
        let requested = match capture::screenshots_dir() {
            Ok(dir) => trash::take_down_action(item.kind, &item.path, &dir),
            Err(error) => {
                log::warn!(
                    "take_down: cannot resolve the screenshot directory ({error:#}); \
                     keeping {}",
                    item.path.display()
                );
                trash::TakeDownAction::OffLineOnly
            }
        };
        // A file that disappeared since the card was hung has nothing to recycle: take the card
        // off the line instead of failing and leaving it stuck.
        let action = trash::resolve_action(requested, &item.path);
        match action {
            trash::TakeDownAction::Recycle => {
                trash::recycle(&item.path).map_err(|error| {
                    log::warn!(
                        "take_down: cannot recycle {}: {error:#}",
                        item.path.display()
                    );
                    error.to_string()
                })?;
            }
            trash::TakeDownAction::OffLineOnly => {
                let reason = if item.kind == item::ItemKind::WatchedFolder {
                    "watched-folder item"
                } else if requested == trash::TakeDownAction::Recycle {
                    "the file is already gone"
                } else {
                    "outside the Hangline screenshot directory"
                };
                log::info!(
                    "take_down: {} only leaves the line ({reason}); the file is kept",
                    item.path.display()
                );
            }
        }
    } else {
        log::info!(
            "take_down: {} only leaves the line (deleteFile=false)",
            item.path.display()
        );
    }

    match state.remove(&id) {
        Ok(Some(removed)) => {
            if removed.path != item.path {
                log::warn!(
                    "take_down: removed item {} had path {} but {} was requested",
                    removed.id,
                    removed.path.display(),
                    item.path.display()
                );
            }
        }
        Ok(None) => log::warn!("take_down: item {id} vanished before it could be removed"),
        Err(error) => {
            log::error!("take_down: cannot persist removal of {id}: {error:#}");
            // `remove` mutates in memory before persisting; publish the new list so the UI matches.
            let _ = app_state::emit_state(&app);
            return Err(error.to_string());
        }
    }
    let _ = app_state::emit_state(&app);
    sound::discarded(&app);
    log::info!("take_down: item {id} is off the line");
    Ok(())
}

/// Takes every card on one side of `id` off the line, never deleting a file.
///
/// `side` is `"before"` (the older cards to the left) or `"after"` (the newer cards to the right);
/// the line is ordered by `added_at` ascending (design spec §4.5), so the selection is the prefix or
/// suffix around the target. The target card itself stays on the line and the removed cards' files
/// stay on disk (the `take_down(deleteFile = false)` contract). The whole batch is one state
/// mutation: one `state.json` write and one `state` broadcast. A stale menu on a card that already
/// left the line is an idempotent no-op returning `0`.
#[tauri::command]
fn take_down_side(app: tauri::AppHandle, id: String, side: String) -> Result<u32, String> {
    let side = app_state::Side::parse(&side).ok_or_else(|| {
        format!("unknown take-down side: {side:?} (expected \"before\" or \"after\")")
    })?;
    let state = app.state::<app_state::AppState>();
    let Some(ids) = app_state::side_ids(&state.snapshot().items, &id, side) else {
        log::info!("take_down_side: item {id} is no longer on the line; nothing to remove");
        return Ok(0);
    };
    let removed = state.remove_many(&ids).map_err(|error| {
        log::error!(
            "take_down_side: cannot persist the {} side take-down of {id}: {error:#}",
            side.as_str()
        );
        // `remove_many` mutates in memory before persisting; publish so the UI matches.
        let _ = app_state::emit_state(&app);
        error.to_string()
    })?;
    let count = u32::try_from(removed.len()).unwrap_or(u32::MAX);
    let _ = app_state::emit_state(&app);
    if count > 0 {
        sound::discarded(&app);
    }
    log::info!(
        "take_down_side: {count} card(s) on the {} side of {id} are off the line (files kept)",
        side.as_str()
    );
    Ok(count)
}

/// Changes one global shortcut.
///
/// Task 15 routes this through the shared settings path: the accelerator is persisted in
/// `settings.captureHotkey`/`toggleHotkey`/`editLatestHotkey`/`searchHotkey` (the shortcut window's
/// save), re-registered with the conflict-safe rollback in [`hotkeys::register`], broadcast as a
/// new full state and reflected in the tray. On a conflict the previous shortcut stays live and the
/// previous setting is restored; the error string is returned to the caller (design spec 4.4
/// `set_hotkey` / §3.11). W1.8 adds the third `"editLatest"` target and W2.3 the fourth `"search"`
/// target on exactly the same contract.
///
/// Saving the accelerator that is already persisted is handled here as a registration *retry*
/// (M15): [`actions::apply`] only schedules a re-registration when the setting changes, so after a
/// startup conflict (another application owned the key) the user could never make the same
/// combination work from the shortcut window. A retry needs no rollback: the persisted value
/// already is the requested one, and a failure leaves whatever is registered untouched.
#[tauri::command]
fn set_hotkey(
    app: tauri::AppHandle,
    target: String,
    accelerator: String,
) -> Result<(), String> {
    let target = hotkeys::Target::parse(&target)
        .ok_or_else(|| format!("unknown hotkey target: {target}"))?;
    let key = match target {
        hotkeys::Target::Capture => "captureHotkey",
        hotkeys::Target::Toggle => "toggleHotkey",
        hotkeys::Target::EditLatest => "editLatestHotkey",
        hotkeys::Target::Search => "searchHotkey",
    };
    debug_assert_eq!(actions::hotkey_target_for_key(key), Some(target));

    let persisted = app
        .try_state::<app_state::AppState>()
        .map(|state| match target {
            hotkeys::Target::Capture => state.snapshot().settings.capture_hotkey,
            hotkeys::Target::Toggle => state.snapshot().settings.toggle_hotkey,
            hotkeys::Target::EditLatest => state.snapshot().settings.edit_latest_hotkey,
            hotkeys::Target::Search => state.snapshot().settings.search_hotkey,
        })
        .unwrap_or_default();
    if hotkeys::is_registration_retry(&persisted, &accelerator) {
        log::info!(
            "set_hotkey: {} shortcut {accelerator} is already persisted; retrying registration",
            target.as_str()
        );
        hotkeys::register(&app, target, &accelerator).map_err(|error| {
            log::warn!(
                "set_hotkey: {} shortcut retry rejected: {error}",
                target.as_str()
            );
            error.to_string()
        })?;
        // Drop the tray's disabled hint item for a shortcut that is live again.
        tray::sync_state(&app);
        return Ok(());
    }

    actions::apply(&app, key, serde_json::json!(accelerator))
        .map(|_| ())
        .map_err(|error| {
            log::warn!(
                "set_hotkey: {} shortcut rejected: {error}",
                target.as_str()
            );
            error
        })
}

/// Closes the change-shortcut window (Esc/Cancel/Save in the shortcut frontend).
#[tauri::command]
fn close_shortcut_window(app: tauri::AppHandle) {
    shortcut_window::close(&app);
}

/// Opens the on-demand search window (Wave 2 W2.2).
///
/// Deliberately `async`: window creation dispatches to the Tauri event loop and waits, so a
/// synchronous command on the main thread would deadlock. The window is destroyed on close, so
/// every call after a close creates a fresh page (see [`search_window`]).
#[tauri::command]
async fn open_search(app: tauri::AppHandle) -> Result<(), String> {
    search_window::open(&app).map_err(|error| {
        log::warn!("open_search: cannot open the search window: {error}");
        error.to_string()
    })
}

/// Closes (and destroys) the search window; a missing window is a logged no-op.
#[tauri::command]
fn close_search(app: tauri::AppHandle) {
    search_window::close(&app);
}

/// Opens the P3 markup editor for item `id` (plan Task P3-2/P3-3).
///
/// Deliberately `async`: window creation dispatches to the Tauri event loop and waits for it, so a
/// synchronous command (which runs on the main thread) would deadlock creating the window. An
/// already-open editor is re-targeted in place (see `annotate::TARGET_EVENT`).
#[tauri::command]
async fn open_annotate(app: tauri::AppHandle, id: String) -> Result<(), String> {
    annotate::open(&app, &id).map_err(|error| {
        log::warn!("open_annotate: cannot open {}: {error:#}", id);
        error.to_string()
    })
}

/// Returns the item + ownership + theme the markup editor page needs (plan Task P3-2).
///
/// A watched-folder item is returned with `owned: false`; the page disables Save and
/// `save_annotation` rejects the write again (the UI is not the security boundary).
#[tauri::command]
fn get_editor_target(
    app: tauri::AppHandle,
    id: String,
) -> Result<annotate::EditorTarget, String> {
    annotate::target(&app, &id).map_err(|error| {
        log::warn!("get_editor_target: {error:#}");
        error.to_string()
    })
}

/// Atomically writes the editor's composited PNG over the item's file (plan Task P3-3).
///
/// Async + `spawn_blocking`: decoding/encoding a 4K PNG and syncing it to disk must never run on
/// the main thread (or block the async runtime's reactor). The command resolves with the updated
/// item; the frontend closes the editor and the panel re-renders from the `state` broadcast.
#[tauri::command]
async fn save_annotation(
    app: tauri::AppHandle,
    id: String,
    png_base64: String,
) -> Result<item::Item, String> {
    let handle = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        annotate::save(&handle, &id, &png_base64)
    })
    .await
    .map_err(|error| format!("save_annotation task failed: {error}"))?;
    joined.map_err(|error| {
        log::warn!("save_annotation: {error:#}");
        error.to_string()
    })
}

/// Closes the markup editor (Esc/Cancel and after a successful save).
#[tauri::command]
async fn close_editor(app: tauri::AppHandle) {
    annotate::close(&app);
}

/// Mirrors the markup editor's dirty state so `open_annotate` can refuse to discard unsaved edits.
#[tauri::command]
fn set_editor_dirty(dirty: bool) {
    annotate::set_dirty(dirty);
}

/// Editor ready handshake: returns the last requested target so a lost `annotate-target` is
/// replayed instead of leaving the window on a stale item.
#[tauri::command]
fn report_annotate_ready(app: tauri::AppHandle) -> Option<String> {
    annotate::report_ready(&app)
}

/// Opens the P3-6 trim window for recording `id` (plan P3-6).
///
/// Deliberately `async`: resolving the target blocks on the WinRT media stack (duration) and
/// window creation dispatches to the Tauri event loop and waits, so neither may run on the main
/// thread. An already-open trim window is re-targeted in place.
#[tauri::command]
async fn open_trim(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let handle = app.clone();
    let requested = id.clone();
    let target = tauri::async_runtime::spawn_blocking(move || trim::target(&handle, &requested))
        .await
        .map_err(|error| format!("open_trim task failed: {error}"))?
        .map_err(|error| {
            log::warn!("open_trim: cannot resolve {id}: {error:#}");
            error.to_string()
        })?;
    let lang = app
        .try_state::<app_state::AppState>()
        .map(|state| state.snapshot().resolved_language)
        .unwrap_or_else(|| "en".to_string());
    trim::open(&app, target, &lang).map_err(|error| {
        log::warn!("open_trim: cannot open the trim window for {id}: {error:#}");
        error.to_string()
    })
}

/// Returns the item, limits and theme the trim page needs (plan P3-6).
///
/// Runs on a blocking worker because the duration comes from `MediaClip::OriginalDuration`.
#[tauri::command]
async fn get_trim_target(
    app: tauri::AppHandle,
    id: String,
) -> Result<trim::TrimTarget, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || trim::target(&handle, &id))
        .await
        .map_err(|error| format!("get_trim_target task failed: {error}"))?
        .map_err(|error| {
            log::warn!("get_trim_target: {error:#}");
            error.to_string()
        })
}

/// Exports `[start_ms, end_ms)` of recording `id` (plan P3-7).
///
/// Async + `spawn_blocking`: the WinRT render, the streamed hash, the atomic replace and the
/// first-frame preview regeneration must never run on the main thread or block the async reactor.
/// Progress arrives as `trim-progress` events; on success the state broadcast refreshes the panel.
#[tauri::command]
async fn trim_video(
    app: tauri::AppHandle,
    id: String,
    start_ms: i64,
    end_ms: i64,
) -> Result<item::Item, String> {
    let handle = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        trim::trim(&handle, &id, start_ms, end_ms)
    })
    .await
    .map_err(|error| format!("trim_video task failed: {error}"))?;
    joined.map_err(|error| {
        log::warn!("trim_video: {error:#}");
        error.to_string()
    })
}

/// Closes the trim window (Esc/Cancel and after a successful export).
#[tauri::command]
async fn close_trim(app: tauri::AppHandle) {
    trim::close(&app);
}

/// Trim ready handshake: returns the last requested target so a lost `trim-target` is replayed.
#[tauri::command]
fn report_trim_ready(app: tauri::AppHandle) -> Option<String> {
    trim::report_ready(&app)
}

/// Both modal editors' window state (P3-4 review follow-up: panel busy reconciliation).
///
/// The panel calls this once after registering the `editor-state`/`trim-state` listeners: a panel
/// webview that reloaded while an editor was already open would otherwise never learn about it and
/// could let the line tuck under the editor. The command reads the live window registry, so it also
/// recovers from a lost close event (the next reconciliation reports `false`).
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorStateSnapshot {
    /// The markup editor window exists.
    annotate_open: bool,
    /// The trim window exists.
    trim_open: bool,
}

/// Reads the live editor window state (see [`EditorStateSnapshot`]).
#[tauri::command]
fn get_editor_state(app: tauri::AppHandle) -> EditorStateSnapshot {
    EditorStateSnapshot {
        annotate_open: annotate::is_open(&app),
        trim_open: trim::is_open(&app),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // single-instance 必须最先注册：第二个实例要在其余插件 setup 之前被检测并退出
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 二次启动 = 唤起已有实例并切换线（design spec 4.2 / E6）。Task 15: requested
            // before the panel frontend reports ready, the reveal controller queues the toggle.
            log::info!("single-instance: second launch detected, toggling the panel");
            reveal::toggle(app, reveal::RevealReason::SecondInstance);
        }))
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            ipc::get_app_state,
            ipc::get_monitors,
            ipc::sync_panel_geometry,
            ipc::set_setting,
            ipc::set_item_note,
            ipc::set_item_tags,
            ipc::ocr_status,
            ipc::build_ocr_index,
            ipc::search_ocr,
            set_hit_rects,
            get_scale_factor,
            request_capture,
            report_capture_ready,
            finish_capture,
            cancel_capture,
            report_panel_ready,
            set_panel_busy,
            get_double_click_interval_ms,
            copy_item,
            copy_item_as_file,
            open_item,
            show_in_explorer,
            take_down,
            take_down_side,
            after_drag,
            set_hotkey,
            close_shortcut_window,
            open_search,
            close_search,
            open_annotate,
            get_editor_target,
            save_annotation,
            close_editor,
            set_editor_dirty,
            report_annotate_ready,
            open_trim,
            get_trim_target,
            trim_video,
            close_trim,
            report_trim_ready,
            get_editor_state,
            updater::check_updates,
            updater::install_update,
            updater::get_pending_update,
            dragout::start_drag,
            fly::start_fly,
            fly::start_fall,
            fly::fly_done,
            fly::report_fly_ready,
            fly::take_capture_origin
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // The data layer is the single source of truth (design spec 4.4): settings and items are
            // loaded and normalized before any window can ask for them.
            let data_dir = settings::data_dir()?;
            let state = app_state::AppState::load(data_dir);
            log::info!(
                "hangline: data layer ready with {} item(s), version {}",
                state.item_count(),
                app_state::version()
            );
            app.manage(state);

            // P2-7: rebuild missing/stale thumbnails for the loaded items on a named background
            // thread (serial, bounded); it emits one state snapshot only when anything changed, so
            // startup is never blocked by a large 4K backlog.
            thumbnails::allow_asset_scope(&handle);
            thumbnails::backfill(&handle);

            // P3: a crash during an annotation save can strand `.hangline-markup-*.png.tmp` in the
            // screenshot folder. No save can be in flight before the editor window exists, so the
            // startup sweep only removes old residue (thumbnail-sweep parity).
            annotate::sweep_stale_temp_files();
            // A killed trim export can strand `.hangline-trim-*` next to the recording; no export
            // can be in flight before the app is up, so only old residue is removed.
            trim::sweep_stale_temp_files();

            // Folder watchers (Task 8), the clipboard listener (Task 9) and the capture overlay
            // (Task 16) share one content-hash dedupe instance.
            app.manage(dedupe::Deduper::default());

            // Task 16: the capture reentry guard + frozen-frame session. Managed before the
            // hotkeys/tray so the first Ctrl+Alt+S or tray "Take screenshot…" always finds it.
            app.manage(capture::CaptureCoordinator::default());

            // Crash residue: a full-desktop frozen frame must never survive a restart in the asset
            // scope (the overlay deletes its own temp file on finish/cancel; this covers kills).
            capture::remove_stale_frozen_frame();

            panel::create(&handle)?;

            // P2-5: the transparent screen-wide fly overlay is created hidden ("pre-warmed") and
            // reused; its page reports ready from `report_fly_ready` once it listens for animations.
            app.manage(fly::FlyManager::default());
            if let Err(error) = fly::prewarm(&handle) {
                // A broken overlay must never block the app: flights are skipped (`start_fly` false)
                // and cards simply fall back to the normal arrival drop-in.
                log::warn!("fly: cannot pre-warm the overlay window: {error:#}");
            }

            // The hit-test thread and the IPC command share this rect list.
            let rects = hittest::HitRects::default();
            app.manage(rects.clone());
            hittest::spawn(handle.clone(), rects);

            // One thread owns every reveal/tuck transition (spec §3.4). It is started before the
            // tray/hotkeys/watchers so the first new-item request and the first hotkey toggle
            // always find a live controller; toggles that arrive before the frontend ready
            // handshake are queued by the controller (Task 15 READY gating).
            app.manage(reveal::start(&handle));

            // P4-2/M4: a second named thread probes `SHQueryUserNotificationState` and the
            // foreground window (0.8 s debounced) and feeds the reveal controller's full-screen
            // gate. Started after the controller so the first change always has somewhere to go.
            app.manage(fullscreen::start(handle.clone()));

            // Task 15: register the *persisted* accelerators (Task 3 only registered defaults),
            // then build the full tray menu so an initial registration failure is visible in it.
            hotkeys::init(&handle);
            // W1.7: the one-shot proactive warning for a startup registration conflict (the tray's
            // disabled hint row/tooltip stays the persistent surface). Shows on a worker thread.
            hotkeys::notify_startup_failures(&handle);
            tray::create(&handle)?;

            // P5-2: the updater keeps the last announcement so a panel that loads later can query
            // it (`get_pending_update`), then starts the one-per-launch daily check. The gating
            // inside `spawn_startup_check` makes this a no-op when `autoCheckUpdates` is off or
            // the last successful check is younger than 24 h.
            app.manage(updater::PendingUpdate::default());
            updater::spawn_startup_check(&handle);

            // Settings are loaded above, so the watch set is final. Missing user folders are
            // skipped (never created); only Hangline's own screenshot folder is created.
            app.manage(watchers::start_all(&handle));

            // "Open at login" mirrors the real registration: re-assert the persisted intent when
            // the login item is missing or stale (moved/unquoted exe), never override a Task
            // Manager disable, and leave a manual registration alone (the tray shows the real
            // state). The registry side is owned by `autostart` so the Run value is always quoted.
            autostart::reconcile(&handle);

            // Task 15 READY gating fallback: if the frontend never reports ready, the reveal
            // controller is unblocked after 3 s and shows the (click-through) panel anyway.
            spawn_ready_timeout(&handle);

            // Broadcast the initial full state; windows that load later call `get_app_state`. The
            // emit goes through the single serialized broadcast choke point (which also refreshes
            // the tray state).
            let _ = app_state::emit_state(&handle);

            // Age retention (keepForDays): one immediate sweep at startup, then hourly. Started
            // after the initial emit so the startup sweep cannot bump the revision first and make
            // that emit non-deterministic (the broadcast gate drops an equal-revision re-emit).
            // The thread is the only owner of the Recycle Bin cleanup for expired owned screenshots.
            retention::spawn(&handle);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Hangline");
    app.run(|handle, event| {
        // Task 15 graceful exit: stop the watcher/clipboard threads for exits that do not go
        // through the tray Quit action (logoff, an OS-requested close). `shutdown` is idempotent.
        if matches!(event, tauri::RunEvent::Exit) {
            log::info!("hangline: exit event received; stopping the watchers and clipboard listener");
            watchers::shutdown(handle);
            // P4-2: stop the full-screen poller so its thread cannot outlive the app.
            fullscreen::shutdown(handle);
            // W1.6: a make-up-press mouse capture must never outlive the process.
            capture::release_mouse_capture(handle, "app exit");
            // A transparent fullscreen overlay must never outlive the app (it would keep painting
            // over the desktop for the fraction of a second between window teardown and exit).
            fly::hide_on_exit(handle);
        }
    });
}

/// Spawns the 3 s READY-timeout fallback thread (once per process).
fn spawn_ready_timeout(app: &tauri::AppHandle) {
    let app = app.clone();
    let builder = std::thread::Builder::new().name("hangline-ready-timeout".to_string());
    if let Err(error) = builder.spawn(move || {
        std::thread::sleep(reveal::READY_TIMEOUT);
        reveal::on_ready_timeout(&app);
    }) {
        log::error!("panel: cannot spawn the READY timeout fallback thread: {error}");
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use app_state::AppState;
    use item::{Item, ItemKind};

    /// Scratch directory under `target/` (git-ignored); never touches `%LOCALAPPDATA%`.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("lib-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the lib test dir");
        dir
    }

    /// `after_drag_target` (the lookup behind the command) must ignore an id that already left the
    /// line: the command then returns before spawning a thread, so no prune and no revision bump.
    #[test]
    fn after_drag_target_ignores_an_unknown_id() {
        let dir = test_dir("after-drag-unknown");
        let state = AppState::load(dir.clone());
        let before = state.snapshot();

        assert!(after_drag_target(&state, "not-on-the-line").is_none());
        assert_eq!(
            state.snapshot(),
            before,
            "an unknown id must not schedule a mutation"
        );
        assert!(state.prune().expect("prune is a no-op").is_empty());
        assert_eq!(state.snapshot(), before);

        let _ = fs::remove_dir_all(dir);
    }

    /// The lookup used by the command resolves the dragged item by id and exposes its path.
    #[test]
    fn after_drag_target_finds_the_dragged_item_by_id() {
        let dir = test_dir("after-drag-known");
        let state = AppState::load(dir.clone());
        let path = dir.join("dragged.png");
        fs::write(&path, b"dragged bytes").expect("write the dragged file");
        let dragged = Item::new(path.clone(), ItemKind::Capture, 0, 10, 20);
        state.add_item(dragged.clone()).expect("add the dragged item");

        let target = after_drag_target(&state, &dragged.id).expect("known id");
        assert_eq!(target.id, dragged.id);
        assert_eq!(target.path, path);

        let _ = fs::remove_dir_all(dir);
    }

    /// The `drag-ended` effect string is parsed, and an unknown value falls back to `none`.
    #[test]
    fn after_drag_parses_known_effects_and_falls_back_to_none() {
        assert_eq!(parse_drag_effect("copy"), DragEffect::Copy);
        assert_eq!(parse_drag_effect("move"), DragEffect::Move);
        assert_eq!(parse_drag_effect("link"), DragEffect::Link);
        assert_eq!(parse_drag_effect("none"), DragEffect::None);
        assert_eq!(
            parse_drag_effect("bogus"),
            DragEffect::None,
            "an unknown effect must not become a prune signal"
        );
    }

    /// A `MOVE` whose file is still there must keep re-checking; once it disappears, prune.
    #[test]
    fn after_drag_retries_a_slow_move_until_the_file_disappears() {
        let mut probes = [Some(true), Some(true), Some(false)].into_iter();
        let outcome = wait_for_after_drag(
            DragEffect::Move,
            Duration::from_millis(1),
            5,
            || probes.next().expect("the wait must stop once the file is gone"),
        );
        assert_eq!(outcome, AfterDragOutcome::Prune);
    }

    /// Retry exhaustion keeps a still-readable file: the card stays and the outcome says so.
    #[test]
    fn after_drag_keeps_the_card_when_the_move_is_still_pending() {
        let mut probes = 0;
        let outcome = wait_for_after_drag(
            DragEffect::Move,
            Duration::from_millis(1),
            3,
            || {
                probes += 1;
                Some(true)
            },
        );
        assert_eq!(outcome, AfterDragOutcome::StillPending);
        assert_eq!(
            probes, 4,
            "one initial probe plus three retries, then the wait must stop"
        );
    }

    /// A `COPY` whose file exists is decided by the first probe; no retry budget is spent.
    #[test]
    fn after_drag_copy_is_decided_by_the_first_probe() {
        let mut probes = 0;
        let outcome = wait_for_after_drag(
            DragEffect::Copy,
            Duration::from_millis(1),
            5,
            || {
                probes += 1;
                Some(true)
            },
        );
        assert_eq!(outcome, AfterDragOutcome::Done);
        assert_eq!(probes, 1, "a copy is a single existence check");
    }

    /// A `MOVE` whose file is already gone prunes immediately (no pointless retries).
    #[test]
    fn after_drag_prunes_a_move_whose_file_is_already_gone() {
        let mut probes = 0;
        let outcome = wait_for_after_drag(
            DragEffect::Move,
            Duration::from_millis(1),
            5,
            || {
                probes += 1;
                Some(false)
            },
        );
        assert_eq!(outcome, AfterDragOutcome::Prune);
        assert_eq!(probes, 1);
    }

    /// A cancel (`none`) whose file is missing is the safety net: prune.
    #[test]
    fn after_drag_none_missing_prunes_as_a_safety_net() {
        let outcome = wait_for_after_drag(
            DragEffect::None,
            Duration::from_millis(1),
            5,
            || Some(false),
        );
        assert_eq!(outcome, AfterDragOutcome::Prune);
    }

    /// An item that left the line during the wait must stop it instead of logging/pruning for it.
    #[test]
    fn after_drag_stops_when_the_item_leaves_the_line() {
        let mut probes = [Some(true), None].into_iter();
        let outcome = wait_for_after_drag(
            DragEffect::Move,
            Duration::from_millis(1),
            5,
            || probes.next().expect("the wait must stop when the item is gone"),
        );
        assert_eq!(outcome, AfterDragOutcome::ItemGone);
    }

    /// Explorer receives `/select,"<path>"`: the path is quoted so spaces, commas and parentheses
    /// stay one argument, and the `/select,` prefix stays outside the quotes.
    #[test]
    fn explorer_select_argument_quotes_the_path() {
        assert_eq!(
            explorer_select_argument(Path::new(r"C:\shots\shot.png")),
            r#"/select,"C:\shots\shot.png""#
        );
        assert_eq!(
            explorer_select_argument(Path::new(r"C:\Users\A B\Pictures\my shot, (1).png")),
            r#"/select,"C:\Users\A B\Pictures\my shot, (1).png""#
        );
        // A Windows file name cannot contain `"`, so quoting needs no further escaping and the
        // prefix is never part of the quoted path.
        let argument = explorer_select_argument(Path::new(r"C:\shots\a.png"));
        assert!(argument.starts_with("/select,\""));
        assert!(argument.ends_with('"'));
    }

    /// A card whose file was deleted (or is not a file) gets a concrete error instead of an empty
    /// Explorer window; an existing file passes.
    #[test]
    fn validate_explorer_target_rejects_missing_files_and_directories() {
        let dir = test_dir("explorer-target");
        let missing = dir.join("gone.png");
        let error = validate_explorer_target(&missing).expect_err("a missing file is rejected");
        assert!(error.contains("not accessible"), "clear error: {error}");
        assert!(error.contains("gone.png"), "the path is in the error: {error}");
        let error = validate_explorer_target(&dir).expect_err("a directory is rejected");
        assert!(error.contains("non-file"), "clear error: {error}");

        let file = dir.join("here.png");
        fs::write(&file, b"png").expect("write the reveal target");
        assert!(validate_explorer_target(&file).is_ok());
        let _ = fs::remove_dir_all(dir);
    }
}
